//! Independent tests of M6 (routing and control, ADR-0023), written from the
//! spec, the roadmap, ADR-0023 and the concept pages, without reading the
//! implementation. Every expected value is derived by hand in the comment beside
//! its test: event times and counts are counted on a timeline, long-run shares
//! are checked against the binomial distribution with a seed fixed when the test
//! was written.
//!
//! Conventions used in the derivations:
//! - A timed Source makes its first part one interval after the start, and the
//!   next interval counts from when the last part left (ADR-0022 §7, S2).
//! - An on-demand Source creates a part at the instant the next component takes
//!   it (ADR-0022 §7).
//! - A Machine holds a finished part in its place until the target takes it
//!   (blocking after service); a Sink takes everything at once.
//! - Horizons are chosen so that no event falls exactly on the horizon.

use std::collections::{BTreeMap, BTreeSet};

use taktsim_components::{
    DispatchDecision, FlowMessage, ItemAttrs, RouteDecision, RouteRule, RunOptions, RunReport,
    standard,
};
use taktsim_core::{TraceRecord, TraceSink};
use taktsim_model::{DispatchRule, Format, Loader};
use taktsim_trace::{TraceReader, replay};

/// A pick at a time in seconds: (from, key, since in seconds) per candidate.
type Pick = (u64, Vec<(usize, u64, u64)>);

/// One second in nanoseconds.
const S: u64 = 1_000_000_000;

/// A record of a run, owned.
#[derive(Debug, Clone)]
enum Rec {
    Event {
        time: u64,
        target: usize,
        from: usize,
        msg: FlowMessage,
        item: Option<u64>,
    },
    Created,
    Disposed,
    Note {
        time: u64,
        by: usize,
        msg: FlowMessage,
    },
    Decision {
        time: u64,
        by: usize,
        msg: FlowMessage,
    },
}

/// Collects every record and checks, at every record, that no item is disposed
/// of twice or before it exists: conservation as an outside observer sees it.
#[derive(Default)]
struct Collector {
    records: Vec<Rec>,
    live: BTreeSet<u64>,
    ever: BTreeSet<u64>,
    problems: Vec<String>,
}

impl TraceSink<FlowMessage> for Collector {
    fn record(&mut self, record: &TraceRecord<'_, FlowMessage>) {
        let rec = match record {
            TraceRecord::Event {
                time,
                target,
                from,
                message,
                item,
                ..
            } => {
                if let Some(item) = item
                    && !self.live.contains(&item.as_u64())
                {
                    self.problems
                        .push(format!("event carries {item} which is not live"));
                }
                Rec::Event {
                    time: time.as_nanos(),
                    target: target.index(),
                    from: from.index(),
                    msg: (*message).clone(),
                    item: item.map(taktsim_core::ItemId::as_u64),
                }
            }
            TraceRecord::ItemCreated { item, .. } => {
                if !self.ever.insert(item.as_u64()) {
                    self.problems.push(format!("{item} created twice"));
                }
                self.live.insert(item.as_u64());
                Rec::Created
            }
            TraceRecord::ItemDisposed { item, .. } => {
                if !self.live.remove(&item.as_u64()) {
                    self.problems
                        .push(format!("{item} disposed of while not live"));
                }
                Rec::Disposed
            }
            TraceRecord::Note { time, by, message } => Rec::Note {
                time: time.as_nanos(),
                by: by.index(),
                msg: (*message).clone(),
            },
            TraceRecord::Decision { time, by, message } => {
                if let FlowMessage::RouteDecision(d) = message
                    && !self.live.contains(&d.item.as_u64())
                {
                    self.problems
                        .push(format!("route decision names {} which is not live", d.item));
                }
                Rec::Decision {
                    time: time.as_nanos(),
                    by: by.index(),
                    msg: (*message).clone(),
                }
            }
        };
        self.records.push(rec);
    }
}

/// A finished run: component names in trace order, the records, the report.
struct Run {
    names: Vec<String>,
    recs: Vec<Rec>,
    report: RunReport,
}

/// A failure in a test helper fails the test that called it.
#[allow(clippy::panic, clippy::expect_used)] // Test helpers.
impl Run {
    fn idx(&self, name: &str) -> usize {
        self.names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no component {name}"))
    }

    fn name(&self, index: usize) -> &str {
        &self.names[index]
    }

    /// `(time, target, from, message, item)` of every delivered event.
    fn events(&self) -> impl Iterator<Item = (u64, usize, usize, &FlowMessage, Option<u64>)> {
        self.recs.iter().filter_map(|r| {
            if let Rec::Event {
                time,
                target,
                from,
                msg,
                item,
            } = r
            {
                Some((*time, *target, *from, msg, *item))
            } else {
                None
            }
        })
    }

    /// `(time, by, message)` of every decision.
    fn decisions(&self) -> impl Iterator<Item = (u64, usize, &FlowMessage)> {
        self.recs.iter().filter_map(|r| {
            if let Rec::Decision { time, by, msg } = r {
                Some((*time, *by, msg))
            } else {
                None
            }
        })
    }

    /// `(time, from, item, attrs)` of every `Arrive` delivered to `name`.
    fn arrivals(&self, name: &str) -> Vec<(u64, usize, u64, ItemAttrs)> {
        let at = self.idx(name);
        self.events()
            .filter_map(|(time, target, from, msg, item)| {
                if let FlowMessage::Arrive(attrs) = msg {
                    (target == at)
                        .then(|| (time, from, item.expect("arrive carries an item"), *attrs))
                } else {
                    None
                }
            })
            .collect()
    }

    /// `(time, item)` of every `Return` delivered to `name`.
    fn returns(&self, name: &str) -> Vec<(u64, u64)> {
        let at = self.idx(name);
        self.events()
            .filter(|e| e.1 == at && matches!(e.3, FlowMessage::Return(_)))
            .map(|e| (e.0, e.4.expect("return carries an item")))
            .collect()
    }

    /// `(time, item)` of every `CycleDone` timer delivered to `name`.
    fn cycles_done(&self, name: &str) -> Vec<(u64, u64)> {
        let at = self.idx(name);
        self.events()
            .filter_map(|(time, target, _, msg, _)| {
                if let FlowMessage::CycleDone { item } = msg {
                    (target == at).then(|| (time, item.as_u64()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Route decisions written by `name`, with their times.
    fn routes_by(&self, name: &str) -> Vec<(u64, RouteDecision)> {
        let at = self.idx(name);
        self.all_routes()
            .into_iter()
            .filter(|r| r.1 == at)
            .map(|r| (r.0, r.2))
            .collect()
    }

    /// Every route decision, by anyone.
    fn all_routes(&self) -> Vec<(u64, usize, RouteDecision)> {
        self.decisions()
            .filter_map(|(time, by, msg)| {
                if let FlowMessage::RouteDecision(d) = msg {
                    Some((time, by, (**d).clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Dispatch decisions written by `name`, with their times.
    fn dispatches_by(&self, name: &str) -> Vec<(u64, DispatchDecision)> {
        let at = self.idx(name);
        self.decisions()
            .filter_map(|(time, by, msg)| {
                if let FlowMessage::DispatchDecision(d) = msg {
                    (by == at).then(|| (time, (**d).clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// `(time, by, edge)` of every `EdgeCondition` note.
    fn edge_conditions(&self) -> Vec<(u64, usize, u32)> {
        self.recs
            .iter()
            .filter_map(|r| {
                if let Rec::Note {
                    time,
                    by,
                    msg: FlowMessage::EdgeCondition { edge, .. },
                } = r
                {
                    Some((*time, *by, *edge))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Variant id → name, from the `VariantName` notes.
    fn variant_names(&self) -> BTreeMap<u16, String> {
        self.recs
            .iter()
            .filter_map(|r| {
                if let Rec::Note {
                    msg: FlowMessage::VariantName { id, name },
                    ..
                } = r
                {
                    Some((id.get(), name.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Item → variant name, from every `Arrive`.
    fn variant_of_items(&self) -> BTreeMap<u64, String> {
        let names = self.variant_names();
        self.events()
            .filter_map(|(_, _, _, msg, item)| {
                if let (FlowMessage::Arrive(attrs), Some(i)) = (msg, item) {
                    Some((i, names[&attrs.variant.get()].clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// The component (by name) that received `item` from `holder` by `Arrive`
    /// or `Return`.
    fn receiver_of(&self, item: u64, holder: usize) -> Option<String> {
        self.events().find_map(|(_, target, from, msg, i)| {
            (i == Some(item)
                && from == holder
                && matches!(msg, FlowMessage::Arrive(_) | FlowMessage::Return(_)))
            .then(|| self.name(target).to_owned())
        })
    }

    /// Item → the component (by name) that received it from `holder` by
    /// `Arrive` or `Return`.
    fn receivers_from(&self, holder: usize) -> BTreeMap<u64, String> {
        self.events()
            .filter_map(|(_, target, from, msg, item)| {
                if let (FlowMessage::Arrive(_) | FlowMessage::Return(_), Some(i)) = (msg, item) {
                    (from == holder).then(|| (i, self.name(target).to_owned()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// How many `SetupDone` timers `name` received.
    fn setups_done(&self, name: &str) -> usize {
        let at = self.idx(name);
        self.events()
            .filter(|e| e.1 == at && matches!(e.3, FlowMessage::SetupDone))
            .count()
    }

    fn created(&self) -> u64 {
        self.report.items.created.get()
    }

    fn delivered(&self) -> u64 {
        self.report.items.delivered.get()
    }
}

#[allow(clippy::panic, clippy::expect_used)] // Test helper: a model that does not load fails the test.
fn load(yaml: &str) -> taktsim_model::Model {
    let registry = standard().expect("standard registry");
    Loader::new(registry.catalog())
        .load_str(yaml, Format::Yaml)
        .unwrap_or_else(|e| panic!("model should load:\n{e}"))
}

#[allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)] // Test helper: a failed run fails the test.
fn run(yaml: &str) -> Run {
    let registry = standard().expect("standard registry");
    let model = load(yaml);
    let prepared = registry
        .prepare(&model, RunOptions::new())
        .unwrap_or_else(|e| panic!("model should prepare: {e}"));
    let names = prepared.header().components.clone();
    let mut sink = Collector::default();
    let report = prepared
        .run(&mut sink)
        .unwrap_or_else(|e| panic!("run should succeed: {e}"));
    assert!(
        sink.problems.is_empty(),
        "conservation as observed: {:?}",
        sink.problems
    );
    // At the end: created = delivered + held + in transit (nothing is scrapped
    // or consumed in v0.1), and it agrees with an outside count of live items.
    let i = report.items;
    assert_eq!(
        i.created.get(),
        i.delivered.get() + i.held.get() + i.in_transit.get(),
        "conservation in the report"
    );
    assert_eq!(
        u64::try_from(sink.live.len()).unwrap(),
        i.held.get() + i.in_transit.get(),
        "live items counted from the trace"
    );
    let created = sink
        .records
        .iter()
        .filter(|r| matches!(r, Rec::Created))
        .count();
    let disposed = sink
        .records
        .iter()
        .filter(|r| matches!(r, Rec::Disposed))
        .count();
    assert_eq!(u64::try_from(created).unwrap(), i.created.get());
    assert_eq!(u64::try_from(disposed).unwrap(), i.delivered.get());
    Run {
        names,
        recs: sink.records,
        report,
    }
}

#[allow(clippy::expect_used)] // Test helper: a failed run fails the test.
fn traced_bytes(yaml: &str) -> Vec<u8> {
    let registry = standard().expect("standard registry");
    let model = load(yaml);
    let (bytes, _) = registry
        .prepare(&model, RunOptions::new())
        .expect("prepare")
        .run_traced(Vec::new())
        .expect("run");
    bytes
}

// ---------------------------------------------------------------------------
// Conditions (`when`)
// ---------------------------------------------------------------------------

const WHEN_BY_PRIORITY: &str = r#"
format: "1.0"
model: {name: when-priority, seed: 1}
experiment: {type: terminating, horizon: 1050s}
components:
  - {id: hi, type: Source, interarrival: 100s, priority: 1}
  - {id: lo, type: Source, interarrival: 100s}
  - {id: m, type: Machine, cycle_time: 10s, capacity: 2 parts}
  - {id: a, type: Machine, cycle_time: 10s}
  - {id: b, type: Machine, cycle_time: 10s}
  - {id: sink, type: Sink}
flow:
  - {from: [hi, lo], to: m}
  - {from: m, to: a, when: "priority >= 1"}
  - {from: m, to: b}
  - {from: [a, b], to: sink}
"#;

/// Derivation. At t = 100k s (k = 1..10) `hi` and `lo` each make one part and
/// offer it to `m`, which has two free places (the previous pair left at
/// 100(k-1)+10), so both enter at once and both finish at 100k+10. The first edge
/// matches `priority >= 1`: the `hi` part goes to `a`, the `lo` part takes the
/// unconditional second edge to `b`. Both are free (they finished at
/// 100(k-1)+20) and deliver at 100k+20 ≤ 1020 < 1050.
///
/// So: 10 arrivals at `a`, all priority 1; 10 at `b`, all priority 0; 20
/// delivered; 20 parts created. `m` writes one `when` record per part leaving it
/// (ADR-0023 §9), at 100k+10: for a `hi` part the list ends at edge 0, which
/// matched (key 1), chosen 0; for a `lo` part edge 0 did not match (key 0) and
/// edge 1 did (key 1), chosen 1. The sources have plain edges and write nothing.
/// One `EdgeCondition` note, by `m`, for edge 0.
#[test]
fn when_routes_by_priority_counted_by_hand() {
    let run = run(WHEN_BY_PRIORITY);
    let at_a = run.arrivals("a");
    let at_b = run.arrivals("b");
    assert_eq!(at_a.len(), 10);
    assert_eq!(at_b.len(), 10);
    assert!(at_a.iter().all(|a| a.3.priority == 1));
    assert!(at_b.iter().all(|a| a.3.priority == 0));
    assert_eq!(run.delivered(), 20);
    assert_eq!(run.created(), 20);

    let priority_of: BTreeMap<u64, u8> = run
        .arrivals("m")
        .into_iter()
        .map(|a| (a.2, a.3.priority))
        .collect();
    let records = run.routes_by("m");
    assert_eq!(records.len(), 20);
    for (time, d) in &records {
        assert_eq!(d.rule, RouteRule::When);
        assert_eq!(
            d.edge,
            d.candidates[usize::from(d.chosen)].id,
            "chosen edge is `edge`"
        );
        assert_eq!((time - 10 * S) % (100 * S), 0, "leaves m at 100k+10 s");
        match priority_of[&d.item.as_u64()] {
            1 => {
                assert_eq!(d.chosen, 0);
                assert_eq!(d.candidates.len(), 1);
                assert_eq!((d.candidates[0].id, d.candidates[0].key), (0, 1));
            }
            0 => {
                assert_eq!(d.chosen, 1);
                assert_eq!(d.candidates.len(), 2);
                assert_eq!((d.candidates[0].id, d.candidates[0].key), (0, 0));
                assert_eq!((d.candidates[1].id, d.candidates[1].key), (1, 1));
            }
            p => panic!("unexpected priority {p}"),
        }
    }
    assert!(run.routes_by("hi").is_empty() && run.routes_by("lo").is_empty());
    assert_eq!(run.edge_conditions(), vec![(0, run.idx("m"), 0)]);
}

const WHEN_FULL_BRANCH: &str = r#"
format: "1.0"
model: {name: when-full, seed: 1}
experiment: {type: terminating, horizon: 1000s}
components:
  - {id: s, type: Source, interarrival: 10s, priority: 1}
  - {id: a, type: Machine, cycle_time: 100s}
  - {id: b, type: Machine, cycle_time: 1s}
  - {id: sink, type: Sink}
flow:
  - {from: s, to: a, when: "priority >= 1"}
  - {from: s, to: b}
  - [a, sink]
  - [b, sink]
"#;

/// Derivation. Every part has priority 1, so every part's route is `a`
/// (decided once, ADR-0023 §3); `b` is idle and free throughout, and a part whose
/// target is full waits and never takes the other edge.
///
/// Part 1 is made at 10 s and enters `a` (10–110). Part 2 is made at 20 s and
/// waits at `s` until `a` frees at 110; the next interval counts from then, so
/// part 3 is made at 120 and waits until 210, and so on: `a` starts at 10, 110,
/// …, 910 (10 starts) and finishes at 110, …, 910 by 1000 s (9 deliveries).
/// Parts made: 10, 20, 120, 220, …, 920 → 11. At the end one part is in `a`
/// (started 910) and one is held at `s` (made 920).
///
/// `b` receives nothing. `s` writes one `when` record per part leaving it: 10,
/// at 10, 110, …, 910, each choosing edge 0.
#[test]
fn full_branch_waits_and_never_takes_another() {
    let run = run(WHEN_FULL_BRANCH);
    assert!(
        run.arrivals("b").is_empty(),
        "no part may fall through to b"
    );
    let starts: Vec<u64> = run.arrivals("a").iter().map(|a| a.0).collect();
    let expected: Vec<u64> = (0..10).map(|k| (10 + 100 * k) * S).collect();
    assert_eq!(starts, expected);
    assert_eq!(run.delivered(), 9);
    assert_eq!(run.created(), 11);
    assert_eq!(run.report.items.held.get(), 2);
    let records = run.routes_by("s");
    assert_eq!(records.iter().map(|r| r.0).collect::<Vec<_>>(), expected);
    assert!(
        records
            .iter()
            .all(|(_, d)| d.rule == RouteRule::When && d.chosen == 0)
    );
}

const WHEN_BY_VARIANT: &str = r#"
format: "1.0"
model: {name: when-variant, seed: 20261109}
experiment: {type: terminating, horizon: 3595s}
components:
  - {id: source, type: Source, variants: {premium: 30%, basic: 70%}}
  - {id: m, type: Machine, cycle_time: 10s}
  - {id: p, type: Machine, cycle_time: 5s}
  - {id: q, type: Machine, cycle_time: 5s}
  - {id: sink, type: Sink}
flow:
  - [source, m]
  - {from: m, to: p, when: "variant == 'premium'"}
  - {from: m, to: q}
  - [p, sink]
  - [q, sink]
"#;

/// Derivation. The on-demand source keeps `m` busy: `m` finishes at 10k s and
/// `p`, `q` (5 s) are always free by then, so a part leaves `m` at 10k for
/// k = 1..359 (3590 ≤ 3595 < 3600): 359 `when` records by `m`. Each premium part
/// goes to `p`, each basic one to `q`, whatever the seed.
///
/// The premium count is Binomial(359, 0.3): mean 107.7, standard deviation
/// √(359·0.3·0.7) = 8.68. Four standard deviations: 73 to 142.
#[test]
fn variant_condition_sends_each_variant_its_own_way() {
    let run = run(WHEN_BY_VARIANT);
    let names = run.variant_names();
    let at_p = run.arrivals("p");
    let at_q = run.arrivals("q");
    assert!(at_p.iter().all(|a| names[&a.3.variant.get()] == "premium"));
    assert!(at_q.iter().all(|a| names[&a.3.variant.get()] == "basic"));
    let records = run.routes_by("m");
    assert_eq!(records.len(), 359);
    assert_eq!(at_p.len() + at_q.len(), 359);
    assert!(
        (73..=142).contains(&at_p.len()),
        "premium share: {}",
        at_p.len()
    );
}

// ---------------------------------------------------------------------------
// Split
// ---------------------------------------------------------------------------

const SPLIT: &str = r#"
format: "1.0"
model: {name: split, seed: 7}
experiment: {type: terminating, horizon: 20001s}
components:
  - {id: source, type: Source}
  - {id: m, type: Machine, cycle_time: 2s}
  - {id: a, type: Machine, cycle_time: 1s}
  - {id: b, type: Machine, cycle_time: 1s}
  - {id: sink, type: Sink}
flow:
  - [source, m]
  - {from: m, split: {a: 25%, b: 75%}}
  - [a, sink]
  - [b, sink]
"#;

/// Recomputes a split record from its own fields (ADR-0023 §9): the draw in
/// `detail` against cumulative shares in list order; the last takes the rest.
#[allow(clippy::unwrap_used)] // Test helper: at most 1,024 candidates fit in u16.
fn recompute_split(d: &RouteDecision) -> u16 {
    let u = f64::from_bits(d.detail);
    let shares: Vec<f64> = d.candidates.iter().map(|c| f64::from_bits(c.key)).collect();
    let total: f64 = shares.iter().sum();
    let mut cumulative = 0.0;
    for (i, share) in shares.iter().enumerate() {
        cumulative += share;
        if u < cumulative / total {
            return u16::try_from(i).unwrap();
        }
    }
    u16::try_from(shares.len() - 1).unwrap()
}

/// Derivation. `m` finishes every 2 s and `a`, `b` (1 s) are free by then, so a
/// part leaves `m` at 2k s for k = 1..10000 (20000 ≤ 20001): 10000 split
/// records by `m`, one draw each.
///
/// Parts to `a`: Binomial(10000, 0.25): mean 2500, standard deviation
/// √(10000·0.25·0.75) = 43.3. Four standard deviations: 2327 to 2673.
///
/// Every record recomputes: the draw lies in (0, 1), the shares in the keys are
/// in the ratio 25:75, and the part arrives where the record says.
#[test]
fn split_converges_and_every_record_recomputes() {
    let run = run(SPLIT);
    let records = run.routes_by("m");
    assert_eq!(records.len(), 10_000);
    let (a, b) = (run.idx("a"), run.idx("b"));
    let receivers = run.receivers_from(run.idx("m"));
    let mut to_a = 0_u32;
    for (_, d) in &records {
        assert_eq!(d.rule, RouteRule::Split);
        assert_eq!(d.candidates.len(), 2);
        let u = f64::from_bits(d.detail);
        assert!(u > 0.0 && u < 1.0, "draw in (0, 1): {u}");
        assert_eq!(recompute_split(d), d.chosen, "record recomputes: {d:?}");
        let ka = d
            .candidates
            .iter()
            .find(|c| c.id as usize == a)
            .expect("a listed");
        let kb = d
            .candidates
            .iter()
            .find(|c| c.id as usize == b)
            .expect("b listed");
        let (sa, sb) = (f64::from_bits(ka.key), f64::from_bits(kb.key));
        assert!(
            (sb - 3.0 * sa).abs() <= 1e-12 * sb,
            "shares 25:75, got {sa}:{sb}"
        );
        let chosen = run
            .name(d.candidates[usize::from(d.chosen)].id as usize)
            .to_owned();
        assert_eq!(receivers.get(&d.item.as_u64()), Some(&chosen));
        if chosen == "a" {
            to_a += 1;
        }
    }
    assert!((2327..=2673).contains(&to_a), "parts to a: {to_a}");
    assert_eq!(run.arrivals("a").len() + run.arrivals("b").len(), 10_000);
}

/// Derivation. The normalized form sorts a mapping's keys and the draw runs
/// over sorted target ids (ADR-0023 §2, §4), so writing the split the other way
/// round is the same model: same hash, same draws, same trace, byte for byte.
#[test]
fn reordering_split_keys_changes_no_byte() {
    let written_other_way = SPLIT.replace("{a: 25%, b: 75%}", "{b: 75%, a: 25%}");
    assert_ne!(written_other_way, SPLIT);
    assert_eq!(traced_bytes(SPLIT), traced_bytes(&written_other_way));
}

/// Derivation. Streams are keyed by component id and purpose (ADR-0013), and a
/// split draws only from its holder's `route` stream (ADR-0023 §11). Adding an
/// unrelated line with its own split, listed first so every component number
/// moves, must leave `m`'s draws, times and choices unchanged: the same
/// sequence of (time, draw, chosen target name).
#[test]
fn unrelated_line_leaves_split_draws_unchanged() {
    let short = SPLIT.replace("horizon: 20001s", "horizon: 2001s");
    let with_other = short.replace(
        "components:\n",
        "components:\n  - {id: s2, type: Source}\n  - {id: m2, type: Machine, cycle_time: 3s}\n  - {id: c, type: Machine, cycle_time: 1s}\n  - {id: d, type: Machine, cycle_time: 1s}\n  - {id: sink2, type: Sink}\n",
    ).replace(
        "flow:\n",
        "flow:\n  - [s2, m2]\n  - {from: m2, split: {c: 40%, d: 60%}}\n  - [c, sink2]\n  - [d, sink2]\n",
    );
    let seen = |run: &Run| -> Vec<(u64, u64, String)> {
        run.routes_by("m")
            .into_iter()
            .map(|(t, d)| {
                let chosen = run
                    .name(d.candidates[usize::from(d.chosen)].id as usize)
                    .to_owned();
                (t, d.detail, chosen)
            })
            .collect()
    };
    let alone = run(&short);
    let beside = run(&with_other);
    assert_eq!(
        beside.routes_by("m2").len(),
        667,
        "the other line runs: 3k ≤ 2001"
    );
    assert_eq!(seen(&alone), seen(&beside));
    assert_eq!(seen(&alone).len(), 1000);
}

// ---------------------------------------------------------------------------
// Pools and choice rules
// ---------------------------------------------------------------------------

fn pool_model(choose: &str) -> String {
    format!(
        r#"
format: "1.0"
model: {{name: pool, seed: 3}}
experiment: {{type: terminating, horizon: 270s}}
components:
  - {{id: s, type: Source, interarrival: 60s}}
  - {{id: m1, type: Machine, cycle_time: 1000s, capacity: 2 parts}}
  - {{id: m2, type: Machine, cycle_time: 1000s, capacity: 2 parts}}
  - {{id: m3, type: Machine, cycle_time: 1000s, capacity: 2 parts}}
  - {{id: sink, type: Sink}}
flow:
  - {{from: s, to: [m1, m2, m3], choose: {choose}}}
  - {{from: [m1, m2, m3], to: sink}}
"#
    )
}

/// Runs the pool and returns, per part in order, (time, machine, record).
#[allow(clippy::expect_used)] // Test helper: a part that never arrives fails the test.
fn pool_run(choose: &str) -> (Run, Vec<(u64, String, RouteDecision)>) {
    let run = run(&pool_model(choose));
    let records = run.routes_by("s");
    let s = run.idx("s");
    let parts = records
        .iter()
        .map(|(t, d)| {
            let to = run
                .receiver_of(d.item.as_u64(), s)
                .expect("the part arrived");
            (*t, to, d.clone())
        })
        .collect();
    (run, parts)
}

#[allow(clippy::unwrap_used)] // Test helper: three components.
fn ids(run: &Run) -> [u32; 3] {
    ["m1", "m2", "m3"].map(|n| u32::try_from(run.idx(n)).unwrap())
}

/// Derivation, common to the pool tests. `s` makes parts at 60, 120, 180 and
/// 240 s; each leaves at once, since every machine always has a free place
/// somewhere (6 places, 4 parts), and the cycles (1000 s) never end before the
/// horizon (270 s). So four parts, four records by `s`, at 60k s, nothing
/// delivered.
///
/// `preferred` ranks (has room, position). p1: all have room → m1. p2: m1 holds 1
/// of 2 → room → m1. p3: m1 full → m2. p4: m2 holds 1 of 2 → m2.
/// Keys are the positions 0, 1, 2; room as `s` knows it: p1, p2 all yes; p3, p4
/// m1 no.
#[test]
fn preferred_fills_the_first_listed_first() {
    let (run, parts) = pool_run("preferred");
    let to: Vec<&str> = parts.iter().map(|p| p.1.as_str()).collect();
    assert_eq!(to, ["m1", "m1", "m2", "m2"]);
    let ids = ids(&run);
    let rooms = [
        [true, true, true],
        [true, true, true],
        [false, true, true],
        [false, true, true],
    ];
    for (k, (time, _, d)) in parts.iter().enumerate() {
        assert_eq!(*time, 60 * S * (u64::try_from(k).unwrap() + 1));
        assert_eq!(d.rule, RouteRule::Preferred);
        assert!(!d.crossed);
        assert_eq!(
            d.detail, *time,
            "detail is the time of the ranking that placed the offer"
        );
        assert_eq!(d.candidates.iter().map(|c| c.id).collect::<Vec<_>>(), ids);
        assert_eq!(
            d.candidates.iter().map(|c| c.key).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(
            d.candidates.iter().map(|c| c.room).collect::<Vec<_>>(),
            rooms[k].map(Some)
        );
    }
}

/// Derivation. `shortest_queue` ranks (has room, queue, position); a queue counts
/// units at the target. p1 at 60: queues (0, 0, 0) → m1. p2 at 120: (1, 0, 0) →
/// m2. p3 at 180: (1, 1, 0) → m3. p4 at 240: (1, 1, 1) → m1 by position. Every
/// machine has room at every ranking.
#[test]
fn shortest_queue_spreads_parts_by_count() {
    let (_, parts) = pool_run("shortest_queue");
    let to: Vec<&str> = parts.iter().map(|p| p.1.as_str()).collect();
    assert_eq!(to, ["m1", "m2", "m3", "m1"]);
    let keys = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [1, 1, 1]];
    for (k, (time, _, d)) in parts.iter().enumerate() {
        assert_eq!(d.rule, RouteRule::ShortestQueue);
        assert_eq!(d.detail, *time);
        assert_eq!(
            d.candidates.iter().map(|c| c.key).collect::<Vec<_>>(),
            keys[k]
        );
        assert!(d.candidates.iter().all(|c| c.room == Some(true)));
    }
}

/// Derivation. `lowest_utilization` ranks (has room, ns worked per place,
/// position), worked = units in cycle × elapsed, divided by capacity 2.
/// p1 at 60: (0, 0, 0) → m1.
/// p2 at 120: m1 = 1 unit × 60 s / 2 = 30 s; m2 = m3 = 0 → m2.
/// p3 at 180: m1 = 120/2 = 60 s; m2 = 60/2 = 30 s; m3 = 0 → m3.
/// p4 at 240: m1 = 180/2 = 90 s; m2 = 120/2 = 60 s; m3 = 60/2 = 30 s → m3.
/// So m1, m2, m3, m3, where `shortest_queue` gave m1 for p4.
#[test]
fn lowest_utilization_ranks_by_exact_work_per_place() {
    let (_, parts) = pool_run("lowest_utilization");
    let to: Vec<&str> = parts.iter().map(|p| p.1.as_str()).collect();
    assert_eq!(to, ["m1", "m2", "m3", "m3"]);
    let keys = [
        [0, 0, 0],
        [30 * S, 0, 0],
        [60 * S, 30 * S, 0],
        [90 * S, 60 * S, 30 * S],
    ];
    for (k, (time, _, d)) in parts.iter().enumerate() {
        assert_eq!(d.rule, RouteRule::LowestUtilization);
        assert_eq!(d.detail, *time);
        assert_eq!(
            d.candidates.iter().map(|c| c.key).collect::<Vec<_>>(),
            keys[k]
        );
    }
}

/// Derivation. `first_free` offers to every target and the first `Accept`
/// takes the part; the record states which target took it and no more: keys 0,
/// room unknown, detail 0 (ADR-0023 §9). Four parts, four records, and the
/// chosen candidate is where the part arrived.
#[test]
fn first_free_states_and_does_not_explain() {
    let (run, parts) = pool_run("first_free");
    assert_eq!(parts.len(), 4);
    for (_, to, d) in &parts {
        assert_eq!(d.rule, RouteRule::FirstFree);
        assert_eq!(d.detail, 0);
        assert!(d.candidates.iter().all(|c| c.key == 0 && c.room.is_none()));
        assert_eq!(
            run.name(d.candidates[usize::from(d.chosen)].id as usize),
            to
        );
    }
}

fn pool_over_time(choose: &str) -> String {
    format!(
        r#"
format: "1.0"
model: {{name: pool-time, seed: 3}}
experiment: {{type: terminating, horizon: 1255s}}
components:
  - {{id: s, type: Source, interarrival: 60s}}
  - {{id: m1, type: Machine, cycle_time: 50s}}
  - {{id: m2, type: Machine, cycle_time: 50s}}
  - {{id: sink, type: Sink}}
flow:
  - {{from: s, to: [m1, m2], choose: {choose}}}
  - {{from: [m1, m2], to: sink}}
"#
    )
}

/// Derivation. Parts at 60k s, k = 1..20 (1200 ≤ 1255 < 1260), each done 50 s
/// later, before the next arrives, so both machines are free at every ranking
/// and all 20 are delivered by 1250.
///
/// `preferred`: always m1 (both have room, m1 first): m1 20, m2 0.
/// `lowest_utilization`: at part k, every earlier part is done, so m1 has
/// worked 50 s × (parts it took) and m2 likewise. k odd: equal → m1; k even: m1
/// is one part ahead → m2. So m1 10, m2 10, alternating from m1.
#[test]
fn utilization_alternates_where_preferred_does_not() {
    let preferred = run(&pool_over_time("preferred"));
    assert_eq!(preferred.delivered(), 20);
    assert_eq!(preferred.arrivals("m1").len(), 20);
    assert_eq!(preferred.arrivals("m2").len(), 0);

    let util = run(&pool_over_time("lowest_utilization"));
    assert_eq!(util.delivered(), 20);
    let s = util.idx("s");
    let order: Vec<String> = util
        .routes_by("s")
        .iter()
        .map(|(_, d)| util.receiver_of(d.item.as_u64(), s).expect("arrived"))
        .collect();
    let expected: Vec<&str> = (0..20)
        .map(|k| if k % 2 == 0 { "m1" } else { "m2" })
        .collect();
    assert_eq!(order, expected);
}

// ---------------------------------------------------------------------------
// Dispatching rules on hand-built queues
// ---------------------------------------------------------------------------

const EDD: &str = r#"
format: "1.0"
model: {name: edd, seed: 5}
experiment: {type: terminating, horizon: 1020s}
components:
  - {id: s1, type: Source, interarrival: 50s, due_in: 3h}
  - {id: s2, type: Source, interarrival: 50s}
  - {id: s3, type: Source, interarrival: 50s, due_in: 1h}
  - {id: t, type: Machine, cycle_time: 100s}
  - {id: sink, type: Sink}
flow:
  - {from: [s1, s2, s3], to: t}
  - [t, sink]
control:
  dispatch: {t: earliest_due_date}
"#;

/// Derivation. All three sources make a part at 50 s. `t` is free and picks by
/// due date: s3's (50 + 3600 = 3650 s), then s1's (50 + 10800 = 10850 s), then
/// s2's (no due date: `u64::MAX`, after all others). `t` runs 50–150.
/// s3's next interval counts from 50, so it makes a part at 100 (due 3700), and
/// that part is waiting at 150 and wins again. From then on s3's part made at
/// 100k is waiting at the pick at 50 + 100k (k ≥ 1), with due 100k + 3600 s,
/// always earlier than s1's 10850 s: s1 and s2 never get `t`.
///
/// By 1020 s: picks at 50, 150, …, 950 → 10 arrivals at `t`, all from s3, each
/// with due = creation + 1 h; deliveries at 150, …, 950 → 9. Parts made: s1 1,
/// s2 1, s3 at 50, 100, 200, …, 1000 → 11; 13 in all. Every pick sees three
/// candidates, so 10 dispatch records by `t`, best first: s3, s1, s2.
#[test]
fn earliest_due_date_takes_the_earliest_and_no_date_last() {
    let run = run(EDD);
    let (s1, s2, s3) = (run.idx("s1"), run.idx("s2"), run.idx("s3"));
    let at_t = run.arrivals("t");
    assert_eq!(at_t.len(), 10);
    for (k, (time, from, _, attrs)) in at_t.iter().enumerate() {
        let k = u64::try_from(k).unwrap();
        assert_eq!(*time, (50 + 100 * k) * S);
        assert_eq!(*from, s3);
        let created = if k == 0 { 50 } else { 100 * k };
        assert_eq!(
            attrs.due.map(taktsim_units::SimTime::as_nanos),
            Some((created + 3600) * S)
        );
    }
    assert_eq!(run.delivered(), 9);
    assert_eq!(run.created(), 13);

    let records = run.dispatches_by("t");
    assert_eq!(records.len(), 10);
    for (k, (time, d)) in records.iter().enumerate() {
        let k = u64::try_from(k).unwrap();
        let created = if k == 0 { 50 } else { 100 * k };
        assert_eq!(*time, (50 + 100 * k) * S);
        assert_eq!(d.rule, DispatchRule::EarliestDueDate);
        assert_eq!(d.taker.index(), run.idx("t"));
        assert_eq!(d.total, 3);
        let seen: Vec<(usize, u64, u64)> = d
            .candidates
            .iter()
            .map(|c| (c.from.index(), c.key, c.since.as_nanos()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (s3, (created + 3600) * S, created * S),
                (s1, 10_850 * S, 50 * S),
                (s2, u64::MAX, 50 * S),
            ]
        );
        assert_eq!(
            d.candidates[0].item.map(taktsim_core::ItemId::as_u64),
            Some(at_t[usize::try_from(k).unwrap()].2)
        );
    }
}

/// Derivation. As for EDD, with priorities 0, 2 and 1 and keys 255 − priority:
/// 255, 253, 254. The priority-2 source wins every pick, the others never: 10
/// arrivals at `t`, all from `pb`, 9 delivered, 13 made, 10 records listing pb
/// (253), pc (254), pa (255).
#[test]
fn priority_class_takes_the_highest_class() {
    let model = EDD
        .replace(
            "{id: s1, type: Source, interarrival: 50s, due_in: 3h}",
            "{id: pa, type: Source, interarrival: 50s, priority: 0}",
        )
        .replace(
            "{id: s2, type: Source, interarrival: 50s}",
            "{id: pb, type: Source, interarrival: 50s, priority: 2}",
        )
        .replace(
            "{id: s3, type: Source, interarrival: 50s, due_in: 1h}",
            "{id: pc, type: Source, interarrival: 50s, priority: 1}",
        )
        .replace("[s1, s2, s3]", "[pa, pb, pc]")
        .replace("earliest_due_date", "priority_class");
    let run = run(&model);
    let (pa, pb, pc) = (run.idx("pa"), run.idx("pb"), run.idx("pc"));
    let at_t = run.arrivals("t");
    assert_eq!(at_t.len(), 10);
    assert!(at_t.iter().all(|a| a.1 == pb && a.3.priority == 2));
    assert_eq!(run.delivered(), 9);
    assert_eq!(run.created(), 13);
    let records = run.dispatches_by("t");
    assert_eq!(records.len(), 10);
    for (_, d) in &records {
        assert_eq!(d.rule, DispatchRule::PriorityClass);
        let seen: Vec<(usize, u64)> = d
            .candidates
            .iter()
            .map(|c| (c.from.index(), c.key))
            .collect();
        assert_eq!(seen, vec![(pb, 253), (pc, 254), (pa, 255)]);
    }
}

const SPT: &str = r#"
format: "1.0"
model: {name: spt, seed: 5}
experiment: {type: terminating, horizon: 990s}
components:
  - {id: sa, type: Source, interarrival: 20s, variants: {a: 50%, b: 50%}}
  - {id: sb, type: Source, interarrival: 20s, variants: {a: 50%, b: 50%}}
  - {id: t, type: Machine, cycle_time: 60s, cycle_time_by_variant: {b: 30s}}
  - {id: sink, type: Sink}
flow:
  - {from: [sa, sb], to: t}
  - [t, sink]
control:
  dispatch: {t: shortest_processing_time}
"#;

/// Derivation (variants are drawn, so this test reads them from the trace and
/// checks the rule's choice against them, pick by pick). The key of a unit is
/// its mean time on `t`: 60 s for `a`, 30 s for `b`. At every dispatch record,
/// the chosen unit has the smallest key, keys are 30 s or 60 s by the unit's
/// variant, the list is sorted by (key, since), and the unit that arrives is the
/// chosen one. Only one unit per source waits at a time, so a record has at
/// most two candidates.
///
/// Both sources make their first part at 20 s, so the first pick (at 20 s) has
/// two candidates and writes a record.
#[test]
fn shortest_processing_time_takes_the_shorter_variant() {
    let run = run(SPT);
    let variant_of = run.variant_of_items();
    let records = run.dispatches_by("t");
    assert_ne!(records.len(), 0);
    assert_eq!(records[0].0, 20 * S, "the first pick has two candidates");
    let arrivals = run.arrivals("t");
    for (time, d) in &records {
        assert_eq!(d.rule, DispatchRule::ShortestProcessingTime);
        assert!(d.candidates.len() <= 2 && d.total <= 2);
        for w in d.candidates.windows(2) {
            assert!(
                (w[0].key, w[0].since) <= (w[1].key, w[1].since),
                "best first: {d:?}"
            );
        }
        let chosen = &d.candidates[0];
        let arrived = arrivals
            .iter()
            .find(|a| a.0 == *time)
            .expect("an arrival at the pick");
        assert_eq!(
            chosen.item.map(taktsim_core::ItemId::as_u64),
            Some(arrived.2)
        );
        let expected_key = if variant_of[&arrived.2] == "b" {
            30 * S
        } else {
            60 * S
        };
        assert_eq!(chosen.key, expected_key);
    }
}

const SPT_FIXED: &str = r#"
format: "1.0"
model: {name: spt-fixed, seed: 5}
experiment: {type: terminating, horizon: 990s}
components:
  - {id: sa, type: Source, interarrival: 20s, variants: {a: 100%}}
  - {id: sb, type: Source, interarrival: 20s, variants: {b: 100%}}
  - {id: t, type: Machine, cycle_time: 60s, cycle_time_by_variant: {b: 30s}}
  - {id: sink, type: Sink}
flow:
  - {from: [sa, sb], to: t}
  - [t, sink]
control:
  dispatch: {t: shortest_processing_time}
"#;

/// Derivation, with one variant per source. Both make a part at 20 s; `t` takes
/// b (30 s < 60 s): 20–50. `sb`'s next part comes 20 s after its last left, so
/// a `b` part made at 20 + 30k − 10 is waiting at every pick at 20 + 30k, and
/// `a` never gets `t`. Picks at 20 + 30k ≤ 990 → k = 0..32: 33 picks, all `b`,
/// all with two candidates (sa's part made at 20 waits for ever) → 33 records,
/// b's key 30 s first, a's 60 s second. Deliveries at 50 + 30k ≤ 990 → 32.
#[test]
fn shortest_processing_time_counted_by_hand() {
    let run = run(SPT_FIXED);
    let (sa, sb) = (run.idx("sa"), run.idx("sb"));
    let at_t = run.arrivals("t");
    assert_eq!(at_t.len(), 33);
    assert!(at_t.iter().all(|a| a.1 == sb));
    assert_eq!(run.delivered(), 32);
    let records = run.dispatches_by("t");
    assert_eq!(records.len(), 33);
    for (k, (time, d)) in records.iter().enumerate() {
        let k = u64::try_from(k).unwrap();
        assert_eq!(*time, (20 + 30 * k) * S);
        let b_since = if k == 0 { 20 } else { 10 + 30 * k };
        let seen: Vec<(usize, u64, u64)> = d
            .candidates
            .iter()
            .map(|c| (c.from.index(), c.key, c.since.as_nanos()))
            .collect();
        assert_eq!(seen, vec![(sb, 30 * S, b_since * S), (sa, 60 * S, 20 * S)]);
    }
}

const MIN_SETUP: &str = r#"
format: "1.0"
model: {name: min-setup, seed: 5}
experiment: {type: terminating, horizon: 365s}
components:
  - {id: sa, type: Source, interarrival: 100s, variants: {a: 100%}}
  - {id: sb, type: Source, interarrival: 101s, variants: {b: 100%}}
  - {id: sc, type: Source, interarrival: 105s, variants: {c: 100%}}
  - id: t
    type: Machine
    cycle_time: 10s
    setup:
      per_variant: 100s
      matrix:
        - {from: a, to: c, time: 0s}
  - {id: sink, type: Sink}
flow:
  - {from: [sa, sb, sc], to: t}
  - [t, sink]
control:
  dispatch: {t: minimum_setup}
"#;

/// Derivation. Key = 2 × mean setup (ns) + 1 if the variant changes; ties by
/// `since`. `t` cycles 10 s; a change costs 100 s, except a → c, which is 0 s
/// (no changeover, key 1).
///
/// - 100: `a` made, only candidate, first unit costs nothing → t 100–110. sa's
///   next at 200.
/// - 101: `b` made, waits. 105: `c` made, waits.
/// - 110: follows a. b@101 key 200e9+1, c@105 key 1 → c (free change), 110–120.
///   sc's next at 215. Record [c(1, 105), b(200e9+1, 101)].
/// - 120: follows c. Only b@101 → b, no record. Setup 120–220, cycle 220–230.
///   sb's next at 221.
/// - 200 `a`, 215 `c`, 221 `b` made, all wait.
/// - 230: follows b. b@221 key 0, a@200 and c@215 key 200e9+1 → b, 230–240. sb's
///   next at 230 + 101 = 331. Record [b(0, 221), a(200e9+1, 200), c(200e9+1, 215)].
/// - 240: follows b. a@200, c@215, both 200e9+1 → tie, older a. Setup 240–340,
///   cycle 340–350. sa's next at 340. Record [a(…, 200), c(…, 215)].
/// - 331 `b` made, 340 `a` made.
/// - 350: follows a. a@340 key 0, c@215 key 1, b@331 key 200e9+1 → a, 350–360.
///   Record [a(0, 340), c(1, 215), b(…, 331)].
/// - 360: follows a. c@215 key 1, b@331 key 200e9+1 → c. Record [c(1, 215),
///   b(…, 331)].
///
/// (A first draft of this derivation had sb's part at 342 by an addition
/// error; 331 is right, and it changes no choice.)
///
/// Arrivals at t: a, c, b, b, a, a, c at 100, 110, 120, 230, 240, 350, 360.
/// Two changeovers finish (220, 340).
#[test]
fn minimum_setup_same_variant_then_cheapest_change() {
    let run = run(MIN_SETUP);
    let (sa, sb, sc) = (run.idx("sa"), run.idx("sb"), run.idx("sc"));
    let arrivals: Vec<(u64, usize)> = run.arrivals("t").iter().map(|a| (a.0 / S, a.1)).collect();
    assert_eq!(
        arrivals,
        vec![
            (100, sa),
            (110, sc),
            (120, sb),
            (230, sb),
            (240, sa),
            (350, sa),
            (360, sc)
        ]
    );
    assert_eq!(run.setups_done("t"), 2);

    let change = 2 * 100 * S + 1;
    let expected: Vec<Pick> = vec![
        (110, vec![(sc, 1, 105), (sb, change, 101)]),
        (
            230,
            vec![(sb, 0, 221), (sa, change, 200), (sc, change, 215)],
        ),
        (240, vec![(sa, change, 200), (sc, change, 215)]),
        (350, vec![(sa, 0, 340), (sc, 1, 215), (sb, change, 331)]),
        (360, vec![(sc, 1, 215), (sb, change, 331)]),
    ];
    let seen: Vec<Pick> = run
        .dispatches_by("t")
        .into_iter()
        .map(|(time, d)| {
            assert_eq!(d.rule, DispatchRule::MinimumSetup);
            (
                time / S,
                d.candidates
                    .iter()
                    .map(|c| (c.from.index(), c.key, c.since.as_nanos() / S))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

const FIFO_TWO_BUFFERS: &str = r#"
format: "1.0"
model: {name: fifo-two-buffers, seed: 5}
experiment: {type: terminating, horizon: 1050s}
components:
  - {id: s1, type: Source, interarrival: 30s}
  - {id: s2, type: Source, interarrival: 47s}
  - {id: b1, type: Buffer, capacity: 100 parts, discipline: lifo}
  - {id: b2, type: Buffer, capacity: 100 parts}
  - {id: t, type: Machine, cycle_time: 100s}
  - {id: sink, type: Sink}
flow:
  - [s1, b1]
  - [s2, b2]
  - {from: [b1, b2], to: t}
  - [t, sink]
control:
  dispatch: {t: fifo}
"#;

/// Derivation. Parts enter b1 at 30j s and b2 at 47j s (the buffers never fill,
/// so each source makes a part every interval). `fifo` is the oldest ready unit
/// across both buffers, and `t`'s rule replaces b1's `lifo` (ADR-0023 §6). The
/// arrivals in time order: 30 b1, 47 b2, 60 b1, 90 b1, 94 b2, 120 b1, 141 b2,
/// 150 b1, 180 b1, 188 b2, 210 b1, 235 b2, …. `t` picks at 30 + 100k and always
/// finds the next oldest already there, so it takes them in exactly that order.
///
/// Pick at 30: only b1 offers → no record. Picks k = 1..10 at 130 … 1030: both
/// buffers hold parts, each offer describes its oldest unit, so two candidates,
/// best first:
/// 130 [b2 47, b1 60]; 230 [b1 60, b2 94]; 330 [b1 90, b2 94]; 430 [b2 94, b1
/// 120]; 530 [b1 120, b2 141]; 630 [b2 141, b1 150]; 730 [b1 150, b2 188]; 830
/// [b1 180, b2 188]; 930 [b2 188, b1 210]; 1030 [b1 210, b2 235].
/// Keys equal `since` in ns. Deliveries at 130 … 1030 → 10.
#[test]
fn fifo_is_oldest_ready_across_two_buffers_and_overrides_lifo() {
    let run = run(FIFO_TWO_BUFFERS);
    let (b1, b2) = (run.idx("b1"), run.idx("b2"));
    let expected: Vec<(u64, Vec<(usize, u64)>)> = vec![
        (130, vec![(b2, 47), (b1, 60)]),
        (230, vec![(b1, 60), (b2, 94)]),
        (330, vec![(b1, 90), (b2, 94)]),
        (430, vec![(b2, 94), (b1, 120)]),
        (530, vec![(b1, 120), (b2, 141)]),
        (630, vec![(b2, 141), (b1, 150)]),
        (730, vec![(b1, 150), (b2, 188)]),
        (830, vec![(b1, 180), (b2, 188)]),
        (930, vec![(b2, 188), (b1, 210)]),
        (1030, vec![(b1, 210), (b2, 235)]),
    ];
    let seen: Vec<(u64, Vec<(usize, u64)>)> = run
        .dispatches_by("t")
        .into_iter()
        .map(|(time, d)| {
            assert_eq!(d.rule, DispatchRule::Fifo);
            assert_eq!(d.total, 2);
            for c in &d.candidates {
                assert_eq!(c.key, c.since.as_nanos(), "fifo's key is since");
            }
            (
                time / S,
                d.candidates
                    .iter()
                    .map(|c| (c.from.index(), c.since.as_nanos() / S))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(seen, expected);
    let from: Vec<usize> = run.arrivals("t").iter().map(|a| a.1).collect();
    assert_eq!(from, vec![b1, b2, b1, b1, b2, b1, b2, b1, b1, b2, b1]);
    assert_eq!(run.delivered(), 10);
}

const EDD_STAGGERED: &str = r#"
format: "1.0"
model: {name: edd-staggered, seed: 5}
experiment: {type: terminating, horizon: 1020s}
components:
  - {id: s1, type: Source, interarrival: 50s, due_in: 3h}
  - {id: s2, type: Source, interarrival: 51s}
  - {id: s3, type: Source, interarrival: 52s, due_in: 1h}
  - {id: t, type: Machine, cycle_time: 100s}
  - {id: sink, type: Sink}
flow:
  - {from: [s1, s2, s3], to: t}
  - [t, sink]
control:
  dispatch: {t: earliest_due_date}
"#;

/// Derivation. The same three sources, with their first parts at different
/// instants, so that no two offers reach a free `t` at one instant.
///
/// - 50: s1's part (due 50 + 10800 s) is alone → `t`, 50–150, no record. s1's
///   next part at 100 (due 10900 s), which then waits for ever.
/// - 51: s2's part (no due date) waits. 52: s3's part (due 3652 s) waits.
/// - 150: s1@100 (10900 s), s2@51 (`u64::MAX`), s3@52 (3652 s) → s3. s3's next
///   at 150 + 52 = 202 (due 3802 s).
/// - From then on, at each pick 150 + 100k, s3's part made at 102 + 100k
///   (k ≥ 1) waits with due 3702 + 100k s, earlier than s1's: s3 again.
///
/// By 1020 s: arrivals at `t` at 50 (s1) and 150 … 950 (s3, 9); 9 records, at
/// 150 … 950, each [s3, s1 (10900 s, since 100), s2 (`u64::MAX`, since 51)];
/// 9 deliveries (150 … 950); parts made: s1 2, s2 1, s3 at 52, 202, …, 1002 →
/// 10; 13 in all.
#[test]
fn earliest_due_date_counted_by_hand_with_staggered_arrivals() {
    let run = run(EDD_STAGGERED);
    let (s1, s2, s3) = (run.idx("s1"), run.idx("s2"), run.idx("s3"));
    let arrivals: Vec<(u64, usize)> = run.arrivals("t").iter().map(|a| (a.0 / S, a.1)).collect();
    let mut expected = vec![(50, s1)];
    expected.extend((0..9).map(|k| (150 + 100 * k, s3)));
    assert_eq!(arrivals, expected);
    assert_eq!(run.delivered(), 9);
    assert_eq!(run.created(), 13);
    let records = run.dispatches_by("t");
    assert_eq!(records.len(), 9);
    for (k, (time, d)) in records.iter().enumerate() {
        let k = u64::try_from(k).unwrap();
        let made = if k == 0 { 52 } else { 102 + 100 * k };
        assert_eq!(*time, (150 + 100 * k) * S);
        assert_eq!(d.rule, DispatchRule::EarliestDueDate);
        assert_eq!(d.total, 3);
        let seen: Vec<(usize, u64, u64)> = d
            .candidates
            .iter()
            .map(|c| (c.from.index(), c.key, c.since.as_nanos()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (s3, (made + 3600) * S, made * S),
                (s1, 10_900 * S, 100 * S),
                (s2, u64::MAX, 51 * S),
            ]
        );
    }
}

/// Derivation. As the staggered EDD case, with priorities 0 (pa, 50 s), 2 (pb,
/// 52 s) and 1 (pc, 51 s); keys 255 − priority. 50: pa alone → `t`. From 150
/// on, pb's part (key 253) is waiting at every pick and wins over pc (254) and
/// pa (255). pb's parts are made at 52, then 102 + 100k. 9 records, each
/// [pb (253), pc (254, since 51), pa (255, since 100)].
#[test]
fn priority_class_counted_by_hand_with_staggered_arrivals() {
    let model = EDD_STAGGERED
        .replace(
            "{id: s1, type: Source, interarrival: 50s, due_in: 3h}",
            "{id: pa, type: Source, interarrival: 50s, priority: 0}",
        )
        .replace(
            "{id: s2, type: Source, interarrival: 51s}",
            "{id: pc, type: Source, interarrival: 51s, priority: 1}",
        )
        .replace(
            "{id: s3, type: Source, interarrival: 52s, due_in: 1h}",
            "{id: pb, type: Source, interarrival: 52s, priority: 2}",
        )
        .replace("[s1, s2, s3]", "[pa, pc, pb]")
        .replace("earliest_due_date", "priority_class");
    let run = run(&model);
    let (pa, pb, pc) = (run.idx("pa"), run.idx("pb"), run.idx("pc"));
    let arrivals: Vec<(u64, usize)> = run.arrivals("t").iter().map(|a| (a.0 / S, a.1)).collect();
    let mut expected = vec![(50, pa)];
    expected.extend((0..9).map(|k| (150 + 100 * k, pb)));
    assert_eq!(arrivals, expected);
    let records = run.dispatches_by("t");
    assert_eq!(records.len(), 9);
    for (k, (_, d)) in records.iter().enumerate() {
        let k = u64::try_from(k).unwrap();
        let made = if k == 0 { 52 } else { 102 + 100 * k };
        assert_eq!(d.rule, DispatchRule::PriorityClass);
        let seen: Vec<(usize, u64, u64)> = d
            .candidates
            .iter()
            .map(|c| (c.from.index(), c.key, c.since.as_nanos() / S))
            .collect();
        assert_eq!(seen, vec![(pb, 253, made), (pc, 254, 51), (pa, 255, 100)]);
    }
}

/// Derivation. One variant per source, first parts at different instants:
/// sa (a, 60 s on `t`) every 20 s, sb (b, 30 s) every 21 s.
///
/// - 20: sa's `a` alone → `t`, 20–80. sa's next at 40, which then waits for ever.
/// - 21: sb's `b` waits.
/// - 80: a@40 (60 s), b@21 (30 s) → b, 80–110. sb's next at 101.
/// - From then on a `b` made at 71 + 30k waits at every pick 80 + 30k (k ≥ 1).
///
/// Picks of `b` at 80 + 30k ≤ 990 → k = 0..30: 31 records, each [b (30 s), a
/// (60 s, since 40)]. Arrivals at `t`: 1 + 31 = 32. Deliveries: `a` at 80, `b`
/// at 110 + 30k ≤ 990 → k = 0..29: 31 in all.
#[test]
fn shortest_processing_time_counted_by_hand_with_staggered_arrivals() {
    let model = SPT_FIXED.replace(
        "{id: sb, type: Source, interarrival: 20s, variants: {b: 100%}}",
        "{id: sb, type: Source, interarrival: 21s, variants: {b: 100%}}",
    );
    let run = run(&model);
    let (sa, sb) = (run.idx("sa"), run.idx("sb"));
    let arrivals: Vec<(u64, usize)> = run.arrivals("t").iter().map(|a| (a.0 / S, a.1)).collect();
    let mut expected = vec![(20, sa)];
    expected.extend((0..31).map(|k| (80 + 30 * k, sb)));
    assert_eq!(arrivals, expected);
    assert_eq!(run.delivered(), 31);
    let records = run.dispatches_by("t");
    assert_eq!(records.len(), 31);
    for (k, (time, d)) in records.iter().enumerate() {
        let k = u64::try_from(k).unwrap();
        assert_eq!(*time, (80 + 30 * k) * S);
        assert_eq!(d.rule, DispatchRule::ShortestProcessingTime);
        let b_made = if k == 0 { 21 } else { 71 + 30 * k };
        let seen: Vec<(usize, u64, u64)> = d
            .candidates
            .iter()
            .map(|c| (c.from.index(), c.key, c.since.as_nanos() / S))
            .collect();
        assert_eq!(seen, vec![(sb, 30 * S, b_made), (sa, 60 * S, 40)]);
    }
}

/// Derivation. A declared rule's result must not depend on the order in which
/// components are listed (the spec: the rule decides which waiting unit a
/// resource takes). Listing s3 before s1 in the same-instant EDD model changes
/// only component numbers, so the sequence of sources (by name) that reach `t`
/// must be the same.
#[test]
fn dispatch_result_does_not_depend_on_listing_order() {
    let reordered = EDD
        .replace(
            "  - {id: s1, type: Source, interarrival: 50s, due_in: 3h}\n  - {id: s2, type: Source, interarrival: 50s}\n  - {id: s3, type: Source, interarrival: 50s, due_in: 1h}\n",
            "  - {id: s3, type: Source, interarrival: 50s, due_in: 1h}\n  - {id: s2, type: Source, interarrival: 50s}\n  - {id: s1, type: Source, interarrival: 50s, due_in: 3h}\n",
        )
        .replace("[s1, s2, s3]", "[s3, s2, s1]");
    assert_ne!(reordered, EDD);
    let by_name = |run: &Run| -> Vec<(u64, String)> {
        run.arrivals("t")
            .iter()
            .map(|a| (a.0, run.name(a.1).to_owned()))
            .collect()
    };
    assert_eq!(by_name(&run(EDD)), by_name(&run(&reordered)));
}

// ---------------------------------------------------------------------------
// Loops and the return lane
// ---------------------------------------------------------------------------

const REWORK: &str = r#"
format: "1.0"
model: {name: rework, seed: 42}
experiment: {type: terminating, horizon: 28790s}
components:
  - {id: source, type: Source}
  - {id: weld, type: Machine, cycle_time: 45s}
  - {id: qc, type: Machine, cycle_time: 40s}
  - {id: sink, type: Sink}
flow:
  - [source, weld, qc]
  - {from: qc, split: {weld: 5%, sink: 95%}}
"#;

/// Derivation. `weld` is never starved (its lane, else the on-demand source) and
/// never blocked: a part it finishes at 45k goes to `qc`, which took the
/// previous one at 45(k−1) and finished it at 45(k−1)+40 < 45k. A part sent
/// back at 45k+40 enters weld's lane at once (a return never waits) and starts
/// at 45(k+1), ahead of a new part. So weld completes exactly every 45 s:
/// at 45k, k = 1..639 (28755 ≤ 28790), and starts at 45k, k = 0..639: 640
/// starts. `qc` decides at 45k+40 for k = 1..638 (28750 ≤ 28790 < 28795): 638
/// split records. Every return (decided by k = 638) starts by 45·639 = 28755.
///
/// So with R parts sent back: created = 640 − R, delivered = 638 − R, and at the
/// end one part is in weld and one in qc. Each returned part finishes at weld
/// exactly 50 s after its `Return` (5 s in the lane, 45 s cycle). R is
/// Binomial(638, 0.05): mean 31.9, standard deviation 5.5; four standard
/// deviations: 10 to 53. The lane never holds more than one part, far below
/// C = 2.
#[test]
fn rework_loop_counted_by_hand() {
    let run = run(REWORK);
    let done = run.cycles_done("weld");
    let expected: Vec<u64> = (1..=639).map(|k| 45 * k * S).collect();
    assert_eq!(
        done.iter().map(|d| d.0).collect::<Vec<_>>(),
        expected,
        "weld never waits"
    );
    let records = run.routes_by("qc");
    assert_eq!(records.len(), 638);
    let weld = run.idx("weld");
    let mut back = 0_u64;
    for (time, d) in &records {
        assert_eq!(d.rule, RouteRule::Split);
        assert_eq!((time - 40 * S) % (45 * S), 0);
        assert_eq!(recompute_split(d), d.chosen);
        if d.candidates[usize::from(d.chosen)].id as usize == weld {
            back += 1;
        }
    }
    let returns = run.returns("weld");
    assert_eq!(
        u64::try_from(returns.len()).unwrap(),
        back,
        "one Return per part sent back"
    );
    for (time, item) in &returns {
        let finished = done
            .iter()
            .find(|d| d.1 == *item && d.0 > *time)
            .expect("rework finished");
        assert_eq!(
            finished.0,
            time + 50 * S,
            "the lane is served first: {item}"
        );
    }
    assert!((10..=53).contains(&back), "parts sent back: {back}");
    assert_eq!(run.created(), 640 - back);
    assert_eq!(run.delivered(), 638 - back);
    assert_eq!(run.report.items.held.get(), 2);
    assert_eq!(
        u64::try_from(run.arrivals("weld").len()).unwrap(),
        640 - back,
        "new parts arrive"
    );
}

// ---------------------------------------------------------------------------
// The spec's example, completed (ADR-0023 disagreement 6): records, determinism
// and replay
// ---------------------------------------------------------------------------

const SPEC_EXAMPLE: &str = r#"
format: "1.0"
model: {name: spec-m6, seed: 42}
experiment: {type: terminating, horizon: 8h}
components:
  - id: source
    type: Source
    interarrival: {dist: exponential, mean: 40s}
    variants: {basic: 80%, premium: 20%}
    due_in: {dist: uniform, min: 2h, max: 8h}
  - {id: cut, type: Machine, cycle_time: 42s}
  - {id: weld-1, type: Machine, cycle_time: 70s}
  - {id: weld-2, type: Machine, cycle_time: 70s}
  - {id: qc, type: Machine, cycle_time: 20s}
  - {id: paint, type: Machine, cycle_time: 30s}
  - {id: pack, type: Machine, cycle_time: 15s}
  - {id: sink, type: Sink}
flow:
  - {from: source, to: cut}
  - {from: cut, to: [weld-1, weld-2], choose: shortest_queue}
  - {from: [weld-1, weld-2], to: qc}
  - {from: qc, to: paint, when: "variant == 'premium'"}
  - {from: qc, to: pack}
  - [paint, pack, sink]
control:
  dispatch: {weld-1: earliest_due_date, weld-2: earliest_due_date}
"#;

/// Derivation. Every routing choice has a record (roadmap M6, ADR-0023 §9):
/// - each part leaving `cut` crosses a `shortest_queue` edge → one record by
///   `cut` per part that arrived at weld-1 or weld-2 from `cut`;
/// - each part leaving `qc` crosses a `when` → one record by `qc` per part that
///   arrived at `paint` or `pack` from `qc`;
/// - plain edges (source → cut, welds → qc, paint → pack → sink) write none.
///
/// Every `shortest_queue` record recomputes: the chosen candidate is the best
/// by (has room, queue, position), unless `crossed`. Each premium part goes to
/// paint and each basic part to pack. Same model, same seed: the same trace
/// byte for byte, and replay re-executes it.
#[test]
fn spec_example_has_a_record_for_every_choice_and_reruns_identically() {
    let run = run(SPEC_EXAMPLE);
    let (cut, qc) = (run.idx("cut"), run.idx("qc"));
    let from_cut = run
        .arrivals("weld-1")
        .iter()
        .chain(run.arrivals("weld-2").iter())
        .filter(|a| a.1 == cut)
        .count();
    let from_qc = run
        .arrivals("paint")
        .iter()
        .chain(run.arrivals("pack").iter())
        .filter(|a| a.1 == qc)
        .count();
    assert!(from_cut > 300, "the line runs: {from_cut}");
    assert_eq!(run.routes_by("cut").len(), from_cut);
    assert_eq!(run.routes_by("qc").len(), from_qc);
    let routers: BTreeSet<String> = run
        .all_routes()
        .iter()
        .map(|r| run.name(r.1).to_owned())
        .collect();
    assert_eq!(routers, BTreeSet::from(["cut".to_owned(), "qc".to_owned()]));

    for (_, d) in run.routes_by("cut") {
        assert_eq!(d.rule, RouteRule::ShortestQueue);
        if d.crossed {
            continue;
        }
        let best = d
            .candidates
            .iter()
            .enumerate()
            .min_by_key(|(i, c)| (c.room != Some(true), c.key, *i))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(usize::from(d.chosen), best, "record recomputes: {d:?}");
    }
    let names = run.variant_names();
    assert!(
        run.arrivals("paint")
            .iter()
            .all(|a| names[&a.3.variant.get()] == "premium")
    );
    assert!(
        run.arrivals("pack")
            .iter()
            .filter(|a| a.1 == qc)
            .all(|a| names[&a.3.variant.get()] == "basic")
    );

    let first = traced_bytes(SPEC_EXAMPLE);
    assert_eq!(first, traced_bytes(SPEC_EXAMPLE), "byte-identical rerun");
    let registry = standard().unwrap();
    let model = load(SPEC_EXAMPLE);
    let mut engine = registry
        .build(model.document(), taktsim_core::EngineConfig::new(42))
        .unwrap();
    replay(TraceReader::new(first.as_slice()).unwrap(), &mut engine).expect("replay matches");
}

/// Derivation. Dispatch records in the spec example follow §9: by a weld,
/// rule EDD, at least two candidates (one candidate writes nothing, except a
/// re-pick after a `Decline`), best first by (key, since), and the unit that
/// arrives at that instant is the first listed. With `due_in` from 2 h to 8 h,
/// every key is a date: creation + draw, so never `u64::MAX` within 8 h.
#[test]
fn spec_example_dispatch_records_rank_best_first() {
    let run = run(SPEC_EXAMPLE);
    for weld in ["weld-1", "weld-2"] {
        let arrivals = run.arrivals(weld);
        for (time, d) in run.dispatches_by(weld) {
            assert_eq!(d.rule, DispatchRule::EarliestDueDate);
            assert_ne!(d.candidates.len(), 0);
            for w in d.candidates.windows(2) {
                assert!(
                    (w[0].key, w[0].since) <= (w[1].key, w[1].since),
                    "best first: {d:?}"
                );
            }
            assert!(d.candidates.iter().all(|c| c.key != u64::MAX));
            let arrived = arrivals
                .iter()
                .find(|a| a.0 == time)
                .expect("an arrival at the record");
            assert_eq!(
                d.candidates[0].item.map(taktsim_core::ItemId::as_u64),
                Some(arrived.2)
            );
            assert_eq!(
                arrived.3.due.map(taktsim_units::SimTime::as_nanos),
                Some(d.candidates[0].key)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Malformed inputs: rejected at load with a field path and an example
// ---------------------------------------------------------------------------

const BASE_HEAD: &str = r#"
format: "1.0"
model: {name: bad, seed: 1}
experiment: {type: terminating, horizon: 1h}
"#;

/// Loads `body` after the common head and returns every error as
/// (path, has an example, text).
#[allow(clippy::panic, clippy::expect_used)] // Test helper: a model that loads fails the test.
fn errors_of(body: &str) -> Vec<(String, bool, String)> {
    let registry = standard().expect("standard registry");
    let text = format!("{BASE_HEAD}{body}");
    match Loader::new(registry.catalog()).load_str(&text, Format::Yaml) {
        Ok(_) => panic!("model should be rejected:\n{text}"),
        Err(errors) => errors
            .iter()
            .map(|e| {
                (
                    e.path().to_string(),
                    e.kind().example().is_some(),
                    e.to_string(),
                )
            })
            .collect(),
    }
}

fn assert_rejected_at(body: &str, prefix: &str) {
    let errors = errors_of(body);
    let hit = errors.iter().find(|e| e.0.starts_with(prefix));
    assert!(
        hit.is_some(),
        "expected an error at `{prefix}`, got {errors:#?}"
    );
    assert!(
        errors
            .iter()
            .filter(|e| e.0.starts_with(prefix))
            .any(|e| e.1),
        "the error at `{prefix}` gives an example: {errors:#?}"
    );
}

/// A valid flow for `LINE`, for the cases whose fault is elsewhere.
const FLOW_OK: &str =
    "flow:\n  - [source, m]\n  - {from: m, to: [a, b]}\n  - [a, sink]\n  - [b, sink]\n";

const LINE: &str = r"
components:
  - {id: source, type: Source, variants: {basic: 80%, premium: 20%}}
  - {id: m, type: Machine, cycle_time: 10s}
  - {id: a, type: Machine, cycle_time: 10s}
  - {id: b, type: Machine, cycle_time: 10s}
  - {id: sink, type: Sink}
";

/// Each case states the rule of ADR-0023 §1 it breaks and the path where the
/// error belongs: the edge (`flow[i]`), the key on it, the component, or the
/// `control` entry.
#[test]
fn malformed_routing_is_rejected_with_a_path() {
    // Split shares sum to 90 %, not 100 %.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, split: {{a: 40%, b: 50%}}}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1]",
    );
    // An edge after an unconditional one from the same component can never be used.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a}}\n  - {{from: m, to: b, when: \"priority >= 1\"}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[2]",
    );
    // The last edge from a component has a `when`: some parts have no way out.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a, when: \"priority >= 1\"}}\n  - {{from: m, to: b, when: \"priority < 1\"}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[2]",
    );
    // `when` is not a boolean.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a, when: \"priority + 1\"}}\n  - {{from: m, to: b}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1].when",
    );
    // `when` names a variant nobody declares.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a, when: \"variant == 'gold'\"}}\n  - {{from: m, to: b}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1].when",
    );
    // `when` reads something that is not a unit attribute.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a, when: \"quality == 'ok'\"}}\n  - {{from: m, to: b}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1].when",
    );
    // `choose` needs at least two targets.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: [a], choose: shortest_queue}}\n  - [a, sink]\n  - [b, sink]\n  - [source, b]\n"
        ),
        "flow[1]",
    );
    // An unknown choice rule.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: [a, b], choose: random}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1].choose",
    );
    // `split` together with `to`.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a, split: {{a: 50%, b: 50%}}}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1]",
    );
    // A split share of 0 %.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, split: {{a: 0%, b: 100%}}}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1].split",
    );
    // A split to an unknown id.
    assert_rejected_at(
        &format!(
            "{LINE}flow:\n  - [source, m]\n  - {{from: m, split: {{a: 50%, z: 50%}}}}\n  - [a, sink]\n  - [b, sink]\n"
        ),
        "flow[1].split",
    );
    // `control.dispatch` names an unknown station.
    assert_rejected_at(
        &format!("{LINE}{FLOW_OK}control:\n  dispatch: {{weld: fifo}}\n"),
        "control.dispatch",
    );
    // An unknown dispatching rule.
    assert_rejected_at(
        &format!("{LINE}{FLOW_OK}control:\n  dispatch: {{m: lifo}}\n"),
        "control.dispatch",
    );
    // `release` stays an unknown key until v0.2.
    assert_rejected_at(
        &format!("{LINE}{FLOW_OK}control:\n  release: {{policy: conwip}}\n"),
        "control",
    );
}

/// Source and Machine keys of M6 (ADR-0023 §1, §7).
#[test]
fn malformed_source_and_machine_keys_are_rejected_with_a_path() {
    // `due_in` on an on-demand Source.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source, due_in: 1h}\n  - {id: m, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}\nflow:\n  - [source, m, sink]\n",
        "components[0]",
    );
    // `priority` and `priority_mix` together.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source, interarrival: 1min, priority: 1, priority_mix: [{priority: 1, share: 50%}, {priority: 0, share: 50%}]}\n  - {id: sink, type: Sink}\nflow:\n  - [source, sink]\n",
        "components[0]",
    );
    // `priority_mix` shares sum to 90 %.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source, interarrival: 1min, priority_mix: [{priority: 1, share: 40%}, {priority: 0, share: 50%}]}\n  - {id: sink, type: Sink}\nflow:\n  - [source, sink]\n",
        "components[0].priority_mix",
    );
    // `priority_mix` with a priority twice.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source, interarrival: 1min, priority_mix: [{priority: 1, share: 50%}, {priority: 1, share: 50%}]}\n  - {id: sink, type: Sink}\nflow:\n  - [source, sink]\n",
        "components[0].priority_mix",
    );
    // `cycle_time_by_variant` names a variant nobody declares.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source, variants: {a: 50%, b: 50%}}\n  - {id: m, type: Machine, cycle_time: 10s, cycle_time_by_variant: {gold: 5s}}\n  - {id: sink, type: Sink}\nflow:\n  - [source, m, sink]\n",
        "components[1].cycle_time_by_variant",
    );
    // `cycle_time_by_variant` with a constant zero.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source, variants: {a: 50%, b: 50%}}\n  - {id: m, type: Machine, cycle_time: 10s, cycle_time_by_variant: {a: 0s}}\n  - {id: sink, type: Sink}\nflow:\n  - [source, m, sink]\n",
        "components[1].cycle_time_by_variant",
    );
}

/// Loops (ADR-0023 §8).
#[test]
fn malformed_loops_are_rejected_with_a_path() {
    let parts = "components:\n  - {id: source, type: Source}\n  - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: qc, type: Machine, cycle_time: 40s}\n  - {id: sink, type: Sink}\n";
    // A condition alone is no exit: weld → qc → weld with `when` has no gate.
    assert_rejected_at(
        &format!(
            "{parts}flow:\n  - [source, weld, qc]\n  - {{from: qc, to: weld, when: \"priority >= 1\"}}\n  - {{from: qc, to: sink}}\n"
        ),
        "flow",
    );
    // A first-free edge back into the loop is no exit either.
    assert_rejected_at(
        &format!("{parts}flow:\n  - [source, weld, qc]\n  - {{from: qc, to: [weld, sink]}}\n"),
        "flow",
    );
    // A Buffer as return target.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source}\n  - {id: buf, type: Buffer, capacity: 5 parts}\n  - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: qc, type: Machine, cycle_time: 40s}\n  - {id: sink, type: Sink}\nflow:\n  - [source, buf, weld, qc]\n  - {from: qc, split: {buf: 10%, sink: 90%}}\n",
        "flow",
    );
    // New parts entering the loop away from its return target.
    assert_rejected_at(
        "components:\n  - {id: source, type: Source}\n  - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: qc, type: Machine, cycle_time: 40s}\n  - {id: sink, type: Sink}\nflow:\n  - [source, qc]\n  - [weld, qc]\n  - {from: qc, split: {weld: 10%, sink: 90%}}\n",
        "flow",
    );
}

/// The good counterparts load: the rework loop of page 12, a first-match list
/// with a repeated target after a `when` (ADR-0023 §1), and a gate with a `when`
/// before its split.
#[test]
fn good_routing_forms_load() {
    load(REWORK);
    load(&format!(
        "{BASE_HEAD}{LINE}flow:\n  - [source, m]\n  - {{from: m, to: a, when: \"variant == 'premium'\"}}\n  - {{from: m, to: b, when: \"priority >= 1\"}}\n  - {{from: m, to: a}}\n  - [a, sink]\n  - [b, sink]\n"
    ));
    load(&format!(
        "{BASE_HEAD}components:\n  - {{id: source, type: Source, variants: {{basic: 80%, premium: 20%}}}}\n  - {{id: weld, type: Machine, cycle_time: 45s}}\n  - {{id: qc, type: Machine, cycle_time: 40s}}\n  - {{id: paint, type: Machine, cycle_time: 30s}}\n  - {{id: sink, type: Sink}}\nflow:\n  - [source, weld, qc]\n  - {{from: qc, to: paint, when: \"variant == 'premium'\"}}\n  - {{from: qc, split: {{weld: 5%, sink: 95%}}}}\n  - [paint, sink]\n"
    ));
}
