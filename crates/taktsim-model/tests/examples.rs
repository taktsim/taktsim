//! Every documented example loads (roadmap M4), and every spelling of the
//! same model has the same hash.
//!
//! The examples of spec §4 and the README are read from the documents
//! themselves, so a change there is tested. Every example presented as
//! v0.1 loads; one labelled v0.2, or v0.1 from a later milestone, is
//! rejected for exactly the sections that wait for it.

mod support;

use std::path::Path;

use support::{catalog, data, summary};
use taktsim_model::{Distribution, Format, Loader, Model, decode_str};

#[allow(clippy::panic)] // Test helper: a model that does not load fails the test.
fn load(path: &Path) -> Model {
    let catalog = catalog();
    match Loader::new(&catalog).load_file(path) {
        Ok(model) => model,
        Err(errors) => panic!("{}:\n{errors}", path.display()),
    }
}

/// The YAML blocks of a Markdown file.
fn yaml_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        match (&mut current, line.trim_start()) {
            (None, "```yaml") => current = Some(String::new()),
            (Some(block), "```") => {
                blocks.push(std::mem::take(block));
                current = None;
            }
            (Some(block), _) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
}

#[allow(clippy::unwrap_used)] // Test helper: the documents are in the repository.
fn repo_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[allow(clippy::unwrap_used)] // Test helper: called only for models that do not load.
fn errors_of(yaml: &str) -> Vec<String> {
    let catalog = catalog();
    summary(
        &Loader::new(&catalog)
            .load_str(yaml, Format::Yaml)
            .unwrap_err(),
    )
}

#[test]
fn every_valid_fixture_loads() {
    let dir = data("valid");
    let mut loaded = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let model = load(&path);
            // The canonical form is itself a model with the same hash.
            let catalog = catalog();
            let again = Loader::new(&catalog)
                .load_str(model.canonical_json(), Format::Json)
                .unwrap();
            assert_eq!(again.hash(), model.hash(), "{}", path.display());
            assert_eq!(again.canonical_json(), model.canonical_json());
            loaded += 1;
        }
    }
    assert_eq!(loaded, 4);
}

#[test]
fn yaml_json_and_included_spellings_have_one_hash() {
    let yaml = load(&data("valid").join("spec-line-a.yaml"));
    let json = load(&data("valid").join("spec-line-a.json"));
    let included = load(&data("valid").join("with-include.yaml"));
    assert_eq!(json.canonical_json(), yaml.canonical_json());
    assert_eq!(json.hash(), yaml.hash());
    assert_eq!(included.hash(), yaml.hash());
    let files: Vec<(&str, &str)> = included
        .includes()
        .iter()
        .map(|i| (i.path(), i.sha256()))
        .collect();
    assert_eq!(files[0].0, "parts/stations.yaml");
    assert_eq!(
        files[1],
        (
            "parts/ends.json",
            "7a4bada22af401f3ddb7c1416ebf85875bf5192061775e00038aedca29870a33"
        )
    );
}

/// The canonical form and hash of the spec's example, fixed: the same on
/// every platform (CI runs this on every tier-1 target), and changed only
/// with an intended change of the normalized form.
#[test]
fn the_canonical_form_and_hash_are_fixed() {
    let model = load(&data("valid").join("spec-line-a.yaml"));
    let expected = std::fs::read_to_string(data("spec-line-a.canonical.json")).unwrap();
    assert_eq!(model.canonical_json(), expected.trim_end());
    assert_eq!(
        model.hash().to_string(),
        "56d63bb349f4eef8da1e437c48f4e01206eaee6315c97c5913637f56ff462dae"
    );
}

/// The spec's YAML block that starts with `first_line`.
#[allow(clippy::panic)] // Test helper: a missing example fails the test.
fn spec_block(first_line: &str) -> String {
    let spec = repo_file("docs/spec.md");
    let blocks: Vec<String> = yaml_blocks(&spec)
        .into_iter()
        .filter(|b| b.starts_with(first_line))
        .collect();
    match blocks.as_slice() {
        [block] => block.clone(),
        _ => panic!("{} spec examples start with {first_line:?}", blocks.len()),
    }
}

/// Spec §4's v0.1 model document loads, with the spec's placeholder header
/// read as `format:` (ADR-0008), and is the model whose hash is pinned.
#[test]
fn the_spec_v0_1_model_document_loads() {
    let example = spec_block("simcode: \"1.0\"").replacen("simcode:", "format:", 1);
    let catalog = catalog();
    let model = match Loader::new(&catalog).load_str(&example, Format::Yaml) {
        Ok(model) => model,
        Err(errors) => panic!("{errors}"),
    };
    assert_eq!(
        model.hash().to_string(),
        "56d63bb349f4eef8da1e437c48f4e01206eaee6315c97c5913637f56ff462dae"
    );
}

#[test]
fn the_readme_model_loads() {
    let readme = repo_file("README.md");
    let example = &yaml_blocks(&readme)[0];
    let catalog = catalog();
    if let Err(errors) = Loader::new(&catalog).load_str(example, Format::Yaml) {
        panic!("{errors}");
    }
}

/// Spec §4's v0.2 shift calendars, added to the v0.1 model: rejected
/// until v0.2 defines them.
#[test]
fn the_spec_v0_2_shifts_are_not_v0_1() {
    let model = spec_block("simcode: \"1.0\"").replacen("simcode:", "format:", 1);
    let shifts = spec_block("shifts:");
    assert_eq!(
        errors_of(&format!("{model}{shifts}")),
        ["shifts @ 34:1 UnknownField"]
    );
}

/// The routing header the two routing examples are added to.
const ROUTING_HEAD: &str = "format: \"1.0\"\nmodel: {name: routing}\nexperiment: {type: terminating, horizon: 8h}\n\
    components:\n  - {id: source, type: Source, interarrival: 1min}\n  - {id: cut, type: Machine, cycle_time: 1min}\n\
    \x20 - {id: weld-1, type: Machine, cycle_time: 1min}\n  - {id: weld-2, type: Machine, cycle_time: 1min}\n\
    \x20 - {id: qc, type: Machine, cycle_time: 1min}\n  - {id: paint, type: Machine, cycle_time: 1min}\n\
    \x20 - {id: pack, type: Sink}\n";

/// Spec §4's routing example is v0.1 from milestone M6: until then its
/// conditions, choice rules and dispatching are rejected, and the rest
/// loads.
#[test]
fn the_spec_routing_example_waits_for_m6() {
    let routing = spec_block("flow:\n  - { from: source, to: cut }");
    let document = format!("{ROUTING_HEAD}{routing}");
    assert_eq!(
        errors_of(&document),
        [
            "flow[1].choose @ 14:40 UnknownField",
            "flow[3].when @ 16:28 UnknownField",
            "control @ 18:1 UnknownField",
        ]
    );
    let without = document
        .replace(", choose: shortest_queue", "")
        .replace(", when: \"variant == 'premium'\"", "");
    let without = without.split("control:").next().unwrap();
    let catalog = catalog();
    let model = Loader::new(&catalog)
        .load_str(without, Format::Yaml)
        .unwrap();
    assert_eq!(model.document().flow().len(), 5);
}

/// Spec §4's v0.2 examples (release control and order lists; materials and
/// BOMs) are rejected for exactly their v0.2 sections.
#[test]
fn the_spec_v0_2_examples_are_not_v0_1() {
    let release = spec_block("control:\n  release:");
    let document = format!("{ROUTING_HEAD}flow: [[source, cut, pack]]\n{release}");
    assert_eq!(
        errors_of(&document),
        ["control @ 13:1 UnknownField", "orders @ 15:1 UnknownField"]
    );

    let materials = spec_block("materials:");
    let document = format!(
        "format: \"1.0\"\nmodel: {{name: asm}}\nexperiment: {{type: terminating, horizon: 8h}}\nflow: [[asm, qc]]\n{materials}"
    );
    // Materials, products and Assembly come with v0.2; the Inspection's
    // shares are read (with their unit, ADR-0020 answer 10).
    assert_eq!(
        errors_of(&document),
        [
            "materials @ 5:1 UnknownField",
            "products @ 9:1 UnknownField",
            "components[0].type @ 17:11 UnknownValue",
        ]
    );
}

/// Every distribution ADR-0003 and the sampler's error messages show.
#[test]
fn every_distribution_example_loads() {
    for text in [
        "45s",
        "{dist: exponential, mean: 12min}",
        "{dist: normal, mean: 42s, sd: 3s}",
        "{dist: lognormal, mean: 42s, sigma: 0.08}",
        "{dist: uniform, min: 40s, max: 50s}",
        "{dist: triangular, min: 40s, mode: 42s, max: 50s}",
        "{dist: gamma, mean: 12min, cv: 0.5}",
        "{dist: erlang, mean: 12min, k: 3}",
        "{dist: weibull, scale: 6h, shape: 1.5}",
    ] {
        let d: Distribution = decode_str(text, Format::Yaml).unwrap();
        assert_eq!(d.to_string(), text);
    }
}

/// The examples the errors give are valid values themselves.
#[test]
fn error_examples_are_valid() {
    use taktsim_model::{ComponentRef, Decode};
    use taktsim_units::{Count, Rate, Ratio, SimDuration};

    fn check<T: Decode>() {
        assert!(
            decode_str::<T>(T::EXAMPLE, Format::Yaml).is_ok(),
            "{}",
            T::EXAMPLE
        );
    }
    check::<SimDuration>();
    check::<Count>();
    check::<Rate>();
    check::<Ratio>();
    check::<Distribution>();
    check::<ComponentRef>();
    check::<String>();
    check::<bool>();
    check::<f64>();
    check::<u32>();
    check::<u64>();
}
