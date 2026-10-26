//! Properties of loading and hashing (roadmap M4):
//!
//! - every spelling of a generated model (YAML block or flow style, JSON,
//!   any key order, quoted or plain text, any spelling of a quantity or
//!   number, chains or edges, spacing inside expressions) loads with the
//!   same hash, and its canonical form loads back to itself;
//! - an unknown key in any mapping is reported at exactly that path;
//! - no text makes the loader panic.

mod support;

use std::fmt::Write;

use proptest::prelude::*;
use support::{catalog, summary};
use taktsim_model::{Format, Loader};

/// A small deterministic generator for spelling choices, seeded by
/// proptest.
struct Choices(u64);

impl Choices {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn pick(&mut self, n: usize) -> usize {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next() % n).unwrap_or(0)
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.pick(i + 1));
        }
    }
}

/// A document tree with the text of every scalar as it will be written.
#[derive(Debug, Clone)]
enum Tree {
    Map(Vec<(String, Tree)>),
    List(Vec<Tree>),
    /// Text, written plain or quoted.
    Text(String),
    /// A number or boolean, written as is.
    Literal(String),
}

/// A generated model, independent of how it is spelled.
#[derive(Debug, Clone)]
struct Model {
    name: String,
    seed: Option<u64>,
    time_unit: Option<&'static str>,
    horizon: u64,
    precision: Option<u32>,
    components: Vec<Component>,
    observe_spacing: bool,
}

#[derive(Debug, Clone)]
enum Component {
    Source {
        id: String,
        mean: u64,
    },
    Machine {
        id: String,
        cycle: Dist,
        mtbf: Option<Dist>,
        capacity: Option<u64>,
    },
    Buffer {
        id: String,
        capacity: u64,
    },
    Sink {
        id: String,
    },
}

impl Component {
    fn id(&self) -> &str {
        match self {
            Self::Source { id, .. }
            | Self::Machine { id, .. }
            | Self::Buffer { id, .. }
            | Self::Sink { id } => id,
        }
    }
}

#[derive(Debug, Clone)]
enum Dist {
    Constant(u64),
    Exponential(u64),
    Normal(u64, u64),
    LogNormal(u64, f64),
    Uniform(u64, u64),
    Triangular(u64, u64, u64),
    Gamma(u64, f64),
    Erlang(u64, u32),
    Weibull(u64, f64),
}

/// Durations of whole milliseconds, so every spelling below is exact.
fn duration() -> impl Strategy<Value = u64> {
    (1_u64..=10_000_000).prop_map(|ms| ms * 1_000_000)
}

fn shape() -> impl Strategy<Value = f64> {
    prop_oneof![
        (1_u32..1000).prop_map(|n| f64::from(n) / 100.0),
        0.01_f64..20.0
    ]
}

fn dist() -> impl Strategy<Value = Dist> {
    prop_oneof![
        duration().prop_map(Dist::Constant),
        duration().prop_map(Dist::Exponential),
        (duration(), duration()).prop_map(|(m, s)| Dist::Normal(m, s)),
        (duration(), shape()).prop_map(|(m, s)| Dist::LogNormal(m, s)),
        (duration(), duration()).prop_map(|(a, b)| Dist::Uniform(a.min(b), a.max(b) + 1_000_000)),
        (duration(), duration(), duration()).prop_map(|(a, b, c)| {
            let mut v = [a, b, c];
            v.sort_unstable();
            Dist::Triangular(v[0], v[1], v[2] + 1_000_000)
        }),
        (duration(), shape()).prop_map(|(m, cv)| Dist::Gamma(m, cv)),
        (duration(), 1_u32..20).prop_map(|(m, k)| Dist::Erlang(m, k)),
        (duration(), shape()).prop_map(|(s, k)| Dist::Weibull(s, k)),
    ]
}

/// Ids of the form `<word><n>`: unique, valid, and never a YAML keyword.
fn model() -> impl Strategy<Value = Model> {
    let station = prop_oneof![
        (
            dist(),
            proptest::option::of(dist()),
            proptest::option::of(1_u64..5)
        )
            .prop_map(|(cycle, mtbf, capacity)| (0, cycle, mtbf, capacity)),
        (1_u64..100).prop_map(|c| (1, Dist::Constant(0), None, Some(c))),
    ];
    (
        "[a-z]{1,6}(-[a-z]{1,3})?",
        proptest::option::of(0_u64..=(1 << 53) - 1),
        proptest::option::of(proptest::sample::select(vec!["d", "h", "min", "s", "ms"])),
        duration(),
        proptest::option::of(1_u32..=1000),
        duration(),
        proptest::collection::vec(station, 1..6),
        "[a-z]{1,5}",
        any::<bool>(),
    )
        .prop_map(
            |(
                name,
                seed,
                time_unit,
                horizon,
                precision,
                source_mean,
                stations,
                prefix,
                observe_spacing,
            )| {
                let mut components = vec![Component::Source {
                    id: format!("{prefix}0"),
                    mean: source_mean,
                }];
                for (i, (kind, cycle, mtbf, capacity)) in stations.into_iter().enumerate() {
                    let id = format!("{prefix}{}", i + 1);
                    components.push(if kind == 0 {
                        Component::Machine {
                            id,
                            cycle,
                            mtbf,
                            capacity,
                        }
                    } else {
                        Component::Buffer {
                            id,
                            capacity: capacity.unwrap_or(1),
                        }
                    });
                }
                let id = format!("{prefix}{}", components.len());
                components.push(Component::Sink { id });
                Model {
                    name,
                    seed,
                    time_unit,
                    horizon,
                    precision,
                    components,
                    observe_spacing,
                }
            },
        )
}

/// One of several exact spellings of a duration of whole milliseconds.
fn spell_duration(ns: u64, c: &mut Choices) -> String {
    let ms = ns / 1_000_000;
    let canonical: taktsim_units::SimDuration = taktsim_units::SimDuration::from_nanos(ns);
    let space = if c.coin() { " " } else { "" };
    match c.pick(4) {
        0 => canonical.to_string(),
        1 => format!("{ms}{space}ms"),
        2 => format!("{}.{:09}{space}s", ns / 1_000_000_000, ns % 1_000_000_000),
        _ if ms.is_multiple_of(60_000) => format!("{}{space}min", ms / 60_000),
        _ => format!("{}.{:03}s", ms / 1000, ms % 1000),
    }
}

/// One of several spellings of the same double.
fn spell_number(x: f64, c: &mut Choices) -> Tree {
    let text = match c.pick(3) {
        0 => format!("{x}"),
        1 => format!("{x:e}"),
        _ => format!("{x:?}"),
    };
    // `{x}` of an integral value has no fraction: an integer, which a
    // number field reads as the same double.
    Tree::Literal(text)
}

fn spell_dist(dist: &Dist, c: &mut Choices) -> Tree {
    let dur = |ns: u64, c: &mut Choices| Tree::Text(spell_duration(ns, c));
    let mut entries: Vec<(String, Tree)> = match *dist {
        Dist::Constant(ns) => return dur(ns, c),
        Dist::Exponential(mean) => vec![("mean".into(), dur(mean, c))],
        Dist::Normal(mean, sd) => vec![("mean".into(), dur(mean, c)), ("sd".into(), dur(sd, c))],
        Dist::LogNormal(mean, sigma) => vec![
            ("mean".into(), dur(mean, c)),
            ("sigma".into(), spell_number(sigma, c)),
        ],
        Dist::Uniform(min, max) => vec![("min".into(), dur(min, c)), ("max".into(), dur(max, c))],
        Dist::Triangular(min, mode, max) => {
            vec![
                ("min".into(), dur(min, c)),
                ("mode".into(), dur(mode, c)),
                ("max".into(), dur(max, c)),
            ]
        }
        Dist::Gamma(mean, cv) => vec![
            ("mean".into(), dur(mean, c)),
            ("cv".into(), spell_number(cv, c)),
        ],
        Dist::Erlang(mean, k) => vec![
            ("mean".into(), dur(mean, c)),
            ("k".into(), Tree::Literal(k.to_string())),
        ],
        Dist::Weibull(scale, shape) => vec![
            ("scale".into(), dur(scale, c)),
            ("shape".into(), spell_number(shape, c)),
        ],
    };
    let name = match dist {
        Dist::Constant(_) => unreachable!(),
        Dist::Exponential(_) => "exponential",
        Dist::Normal(..) => "normal",
        Dist::LogNormal(..) => "lognormal",
        Dist::Uniform(..) => "uniform",
        Dist::Triangular(..) => "triangular",
        Dist::Gamma(..) => "gamma",
        Dist::Erlang(..) => "erlang",
        Dist::Weibull(..) => "weibull",
    };
    entries.push(("dist".into(), Tree::Text(name.into())));
    c.shuffle(&mut entries);
    Tree::Map(entries)
}

fn spell_count(n: u64, c: &mut Choices) -> Tree {
    Tree::Text(format!(
        "{n}{}{}",
        if c.coin() { " " } else { "" },
        if c.coin() { "parts" } else { "part" }
    ))
}

/// A call, with or without spaces inside: `f(a,b)` or `f( a ,  b )`.
fn call(function: &str, args: &[&str], spaced: bool) -> String {
    if spaced {
        format!("{function}( {} )", args.join(" ,  "))
    } else {
        format!("{function}({})", args.join(","))
    }
}

fn spell_component(comp: &Component, c: &mut Choices) -> Tree {
    let mut e = vec![("id".to_owned(), Tree::Text(comp.id().to_owned()))];
    match comp {
        Component::Source { mean, .. } => {
            e.push(("type".into(), Tree::Text("Source".into())));
            e.push((
                "interarrival".into(),
                spell_dist(&Dist::Exponential(*mean), c),
            ));
        }
        Component::Machine {
            cycle,
            mtbf,
            capacity,
            ..
        } => {
            e.push(("type".into(), Tree::Text("Machine".into())));
            e.push(("cycle_time".into(), spell_dist(cycle, c)));
            if let Some(mtbf) = mtbf {
                e.push(("mtbf".into(), spell_dist(mtbf, c)));
            }
            if let Some(n) = capacity {
                e.push(("capacity".into(), spell_count(*n, c)));
            }
        }
        Component::Buffer { capacity, .. } => {
            e.push(("type".into(), Tree::Text("Buffer".into())));
            e.push(("capacity".into(), spell_count(*capacity, c)));
        }
        Component::Sink { .. } => e.push(("type".into(), Tree::Text("Sink".into()))),
    }
    c.shuffle(&mut e);
    Tree::Map(e)
}

/// The chain through `ids`, cut into pieces; each piece a chain or single
/// edges, each end an id or a list of one.
fn spell_flow(ids: &[&str], c: &mut Choices) -> Vec<Tree> {
    let mut flow = Vec::new();
    let mut start = 0;
    while start + 1 < ids.len() {
        let end = (start + 1 + c.pick(ids.len() - start - 1)).min(ids.len() - 1);
        if c.coin() {
            flow.push(Tree::List(
                ids[start..=end]
                    .iter()
                    .map(|i| Tree::Text((*i).to_owned()))
                    .collect(),
            ));
        } else {
            for pair in ids[start..=end].windows(2) {
                let end_of = |id: &str, c: &mut Choices| {
                    if c.coin() {
                        Tree::Text(id.to_owned())
                    } else {
                        Tree::List(vec![Tree::Text(id.to_owned())])
                    }
                };
                let mut edge = vec![
                    ("from".to_owned(), end_of(pair[0], c)),
                    ("to".to_owned(), end_of(pair[1], c)),
                ];
                c.shuffle(&mut edge);
                flow.push(Tree::Map(edge));
            }
        }
        start = end;
    }
    flow
}

/// One spelling of the whole model as a tree.
fn spell(m: &Model, c: &mut Choices) -> Tree {
    let ids: Vec<&str> = m.components.iter().map(Component::id).collect();
    let sink = ids[ids.len() - 1];
    let mut model = vec![("name".to_owned(), Tree::Text(m.name.clone()))];
    if let Some(seed) = m.seed {
        model.push(("seed".into(), Tree::Literal(seed.to_string())));
    }
    if let Some(unit) = m.time_unit {
        model.push(("time_unit".into(), Tree::Text(unit.into())));
    }
    c.shuffle(&mut model);

    let mut experiment = vec![
        ("type".to_owned(), Tree::Text("terminating".into())),
        ("horizon".into(), Tree::Text(spell_duration(m.horizon, c))),
    ];
    if let Some(p) = m.precision {
        let ratio = format!("{}%", f64::from(p) / 10.0);
        let key = call("throughput", &[sink], c.coin());
        experiment.push((
            "precision".into(),
            Tree::Map(vec![(key, Tree::Text(ratio))]),
        ));
    }
    c.shuffle(&mut experiment);

    let components = m
        .components
        .iter()
        .map(|comp| spell_component(comp, c))
        .collect();
    let flow = spell_flow(&ids, c);

    let mut observe = vec![Tree::Text(call("throughput", &[sink], m.observe_spacing))];
    let machines: Vec<&str> = m
        .components
        .iter()
        .filter(|comp| matches!(comp, Component::Machine { .. }))
        .map(Component::id)
        .collect();
    if !machines.is_empty() {
        observe.push(Tree::Text(call(
            "state_shares",
            &machines,
            !m.observe_spacing,
        )));
    }
    for comp in &m.components {
        if let Component::Buffer { id, .. } = comp {
            observe.push(Tree::Text(call("wip", &[id], c.coin())));
        }
    }

    let mut top = vec![
        ("format".to_owned(), Tree::Text("1.0".into())),
        ("model".into(), Tree::Map(model)),
        ("experiment".into(), Tree::Map(experiment)),
        ("components".into(), Tree::List(components)),
        ("flow".into(), Tree::List(flow)),
        ("observe".into(), Tree::List(observe)),
    ];
    c.shuffle(&mut top);
    Tree::Map(top)
}

/// Text that YAML may write without quotes, in block or flow context.
fn plain_ok(s: &str, flow: bool) -> bool {
    let special = |ch: char| flow && matches!(ch, ',' | '[' | ']' | '{' | '}');
    // Words some YAML parser reads as a boolean or null.
    let keyword = ["y", "n", "yes", "no", "on", "off", "true", "false", "null"]
        .contains(&s.to_ascii_lowercase().as_str());
    !s.is_empty()
        && !keyword
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.starts_with(|ch: char| "-?:,[]{}#&*!|>'\"%@`".contains(ch) || ch == ' ')
        && !s.ends_with([' ', ':'])
        && !s.chars().any(special)
        // "1.0" would be read as a number.
        && !s.starts_with(|ch: char| ch.is_ascii_digit()) | s.ends_with(|ch: char| ch.is_ascii_alphabetic() || ch == '%')
}

fn yaml_scalar(s: &str, flow: bool, c: &mut Choices) -> String {
    match c.pick(3) {
        0 if plain_ok(s, flow) => s.to_owned(),
        1 => format!("'{}'", s.replace('\'', "''")),
        _ => quoted(s),
    }
}

/// `s` as a JSON (and YAML) double-quoted string.
fn quoted(s: &str) -> String {
    let mut out = String::from('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn yaml_flow(t: &Tree, c: &mut Choices) -> String {
    match t {
        Tree::Map(entries) => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", yaml_scalar(k, true, c), yaml_flow(v, c)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Tree::List(items) => {
            let parts: Vec<String> = items.iter().map(|v| yaml_flow(v, c)).collect();
            format!("[{}]", parts.join(", "))
        }
        Tree::Text(s) => yaml_scalar(s, true, c),
        Tree::Literal(s) => s.clone(),
    }
}

/// Writes `t` in block style at `indent`, choosing flow style for some
/// nested values.
fn yaml_block(t: &Tree, indent: usize, c: &mut Choices, out: &mut String) {
    let pad = " ".repeat(indent);
    match t {
        Tree::Map(entries) => {
            for (k, v) in entries {
                let key = yaml_scalar(k, false, c);
                match v {
                    Tree::Map(e) if !e.is_empty() && c.coin() => {
                        let _ = writeln!(out, "{pad}{key}:");
                        yaml_block(v, indent + 2, c, out);
                    }
                    Tree::List(items) if !items.is_empty() && c.coin() => {
                        let _ = writeln!(out, "{pad}{key}:");
                        yaml_block(v, indent + 2, c, out);
                    }
                    Tree::Map(_) | Tree::List(_) => {
                        let value = yaml_flow(v, c);
                        let _ = writeln!(out, "{pad}{key}: {value}");
                    }
                    Tree::Text(s) => {
                        let value = yaml_scalar(s, false, c);
                        let _ = writeln!(out, "{pad}{key}: {value}");
                    }
                    Tree::Literal(s) => {
                        let _ = writeln!(out, "{pad}{key}: {s}");
                    }
                }
            }
        }
        Tree::List(items) => {
            for item in items {
                match item {
                    Tree::Map(e) if !e.is_empty() && c.coin() => {
                        // `- ` then the mapping, indented under the dash.
                        let mut inner = String::new();
                        yaml_block(item, indent + 2, c, &mut inner);
                        let _ = write!(out, "{pad}- {}", &inner[indent + 2..]);
                    }
                    Tree::Map(_) | Tree::List(_) => {
                        let value = yaml_flow(item, c);
                        let _ = writeln!(out, "{pad}- {value}");
                    }
                    Tree::Text(s) => {
                        let value = yaml_scalar(s, false, c);
                        let _ = writeln!(out, "{pad}- {value}");
                    }
                    Tree::Literal(s) => {
                        let _ = writeln!(out, "{pad}- {s}");
                    }
                }
            }
        }
        Tree::Text(_) | Tree::Literal(_) => {
            let value = yaml_flow(t, c);
            let _ = writeln!(out, "{pad}{value}");
        }
    }
}

fn yaml(t: &Tree, c: &mut Choices) -> String {
    let mut out = String::new();
    if c.coin() {
        out.push_str("# a generated model\n");
    }
    yaml_block(t, 0, c, &mut out);
    out
}

fn json(t: &Tree, c: &mut Choices) -> String {
    let ws = |c: &mut Choices| [" ", "", "\n  ", "\t"][c.pick(4)].to_owned();
    match t {
        Tree::Map(entries) => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}:{}{}", quoted(k), ws(c), json(v, c)))
                .collect();
            format!("{{{}{}}}", ws(c), parts.join(&format!(",{}", ws(c))))
        }
        Tree::List(items) => {
            let parts: Vec<String> = items.iter().map(|v| json(v, c)).collect();
            format!("[{}]", parts.join(&format!(",{}", ws(c))))
        }
        Tree::Text(s) => quoted(s),
        Tree::Literal(s) => s.clone(),
    }
}

#[allow(clippy::panic)] // Test helper: a model that does not load fails the test.
fn load(text: &str, format: Format) -> taktsim_model::Model {
    let catalog = catalog();
    match Loader::new(&catalog).load_str(text, format) {
        Ok(model) => model,
        Err(errors) => panic!("{errors}\n---\n{text}"),
    }
}

/// Every mapping whose keys are names (not the precision targets),
/// as the field path and the steps to it.
fn maps(t: &Tree, path: &str, steps: &mut Vec<usize>, out: &mut Vec<(String, Vec<usize>)>) {
    match t {
        Tree::Map(entries) => {
            if path.ends_with("precision") {
                return;
            }
            out.push((path.to_owned(), steps.clone()));
            for (i, (k, v)) in entries.iter().enumerate() {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                steps.push(i);
                maps(v, &child, steps, out);
                steps.pop();
            }
        }
        Tree::List(items) => {
            for (i, v) in items.iter().enumerate() {
                steps.push(i);
                maps(v, &format!("{path}[{i}]"), steps, out);
                steps.pop();
            }
        }
        Tree::Text(_) | Tree::Literal(_) => {}
    }
}
fn at<'t>(t: &'t mut Tree, steps: &[usize]) -> &'t mut Tree {
    match (t, steps.split_first()) {
        (t, None) => t,
        (Tree::Map(entries), Some((i, rest))) => at(&mut entries[*i].1, rest),
        (Tree::List(items), Some((i, rest))) => at(&mut items[*i], rest),
        (Tree::Text(_) | Tree::Literal(_), Some(_)) => unreachable!(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn every_spelling_has_the_same_hash(model in model(), seeds in any::<[u64; 4]>()) {
        let mut spellings = Vec::new();
        for (i, seed) in seeds.iter().enumerate() {
            let mut c = Choices(seed | 1);
            let tree = spell(&model, &mut c);
            let (text, format) = if i % 2 == 0 { (yaml(&tree, &mut c), Format::Yaml) } else { (json(&tree, &mut c), Format::Json) };
            spellings.push(load(&text, format));
        }
        let first = &spellings[0];
        for other in &spellings[1..] {
            prop_assert_eq!(other.canonical_json(), first.canonical_json());
            prop_assert_eq!(other.hash(), first.hash());
        }
        // Normalizing is idempotent: the canonical form loads to itself.
        let again = load(first.canonical_json(), Format::Json);
        prop_assert_eq!(again.canonical_json(), first.canonical_json());
        prop_assert_eq!(again.hash(), first.hash());
    }

    #[test]
    fn an_unknown_key_is_reported_where_it_is(model in model(), seed in any::<u64>(), pick in any::<usize>()) {
        let mut c = Choices(seed | 1);
        let mut tree = spell(&model, &mut c);
        let mut found = Vec::new();
        maps(&tree, "", &mut Vec::new(), &mut found);
        let (path, steps) = found.swap_remove(pick % found.len());
        let Tree::Map(entries) = at(&mut tree, &steps) else { unreachable!() };
        entries.push(("zz_unknown".to_owned(), Tree::Literal("1".into())));
        let text = yaml(&tree, &mut c);
        let catalog = catalog();
        let errors = Loader::new(&catalog).load_str(&text, Format::Yaml).unwrap_err();
        let expected = if path.is_empty() { "zz_unknown".to_owned() } else { format!("{path}.zz_unknown") };
        let lines = summary(&errors);
        prop_assert_eq!(lines.len(), 1, "{:?}\n{}", lines, text);
        prop_assert!(lines[0].starts_with(&format!("{expected} @ ")), "{:?}\n{}", lines, text);
        prop_assert!(lines[0].ends_with(" UnknownField"), "{:?}", lines);
    }

    #[test]
    fn no_text_makes_the_loader_panic(
        text in prop_oneof![
            ".{0,200}",
            "[ \n\t:{}\\[\\],'\"a-z0-9#&*!|>%@`~?-]{0,200}",
            "[ \n{}\\[\\],:\"a-z0-9.eE+-]{0,200}",
        ]
    ) {
        let catalog = catalog();
        let loader = Loader::new(&catalog);
        let _ = loader.load_str(&text, Format::Yaml);
        let _ = loader.load_str(&text, Format::Json);
    }
}
