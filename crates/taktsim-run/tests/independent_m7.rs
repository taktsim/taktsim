//! Independent tests of M7's library (`taktsim_run`), written from `docs/spec.md`,
//! ADR-0024, `docs/concepts` and the public signatures and doc comments only, without
//! reading the implementation. Every expected value is derived on paper; the
//! derivation stands in a comment by the test. Seeds were fixed when written.

// Helpers outside `#[test]` functions: a failed expectation is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};
use taktsim_components::{
    BuildError, ComponentType, Domain, FlowMessage, MachineState, Ports, Registry, Roles, Wiring,
    standard,
};
use taktsim_core::{Component, ComponentError, Context, Delivery, Disposal, ItemId};
use taktsim_model::{
    ComponentParams, Document, Encode, Fields, Findings, Format, Json, decode_str, parse,
};
use taktsim_run::{
    AccuracyKind, Failure, FailureClass, ModelSource, Over, ResultDocument, RunEntry, RunId,
    RunRequest, RunResponse, Scalar, Store, replay_stored, run, validate,
};
use taktsim_trace::{Header, Record, Summary, TraceReader};
use taktsim_units::SimDuration;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fresh directory for one test, below Cargo's temporary directory.
fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("independent_m7_run")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fresh_store(name: &str) -> (PathBuf, Store) {
    let root = fresh(name).join("store");
    let store = Store::open(&root).unwrap();
    (root, store)
}

/// A whole model from its parts.
fn model(seed: Option<u64>, horizon: &str, components: &[&str], flow: &str) -> String {
    let seed = seed.map_or(String::new(), |s| format!(", seed: {s}"));
    let list: String = components.iter().fold(String::new(), |mut all, c| {
        all.push_str("  - ");
        all.push_str(c);
        all.push('\n');
        all
    });
    format!(
        "format: \"1.0\"\nmodel: {{name: m{seed}}}\nexperiment: {{type: terminating, horizon: {horizon}}}\n\
         components:\n{list}flow:\n{flow}\n"
    )
}

fn go(text: &str, store: &Store, request: &RunRequest<'_>) -> Result<RunResponse, Failure> {
    let registry = standard().unwrap();
    run(
        &registry,
        ModelSource::Text(text, Format::Yaml),
        request,
        store,
    )
}

fn ok(text: &str, store: &Store) -> ResultDocument {
    go(text, store, &RunRequest::new()).unwrap().result
}

fn metric(doc: &ResultDocument, name: &str) -> u64 {
    let found = doc
        .metrics
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no metric {name} in {:?}", names(doc)));
    assert_eq!(found.values.len(), 1, "{name} is a scalar");
    let Scalar::Integer(n) = found.values[0].value else {
        panic!("{name} is a count");
    };
    n
}

fn names(doc: &ResultDocument) -> Vec<String> {
    doc.metrics.iter().map(|m| m.name.clone()).collect()
}

struct Loaded {
    header: Header,
    records: Vec<Record<FlowMessage>>,
    summary: Summary,
}

fn stored_file(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, q| p.join(q))
}

fn load(root: &Path, entry: &RunEntry) -> Loaded {
    let trace = entry.trace.as_ref().expect("a kept run has a trace");
    let file = fs::File::open(stored_file(root, &trace.path)).unwrap();
    let mut reader = TraceReader::<_, FlowMessage>::new(BufReader::new(file)).unwrap();
    let header = reader.header().clone();
    let mut records = Vec::new();
    while let Some(record) = reader.next_record().unwrap() {
        records.push(record);
    }
    let summary = reader.summary().expect("read to the end");
    Loaded {
        header,
        records,
        summary,
    }
}

fn index_of(header: &Header, id: &str) -> usize {
    header.components.iter().position(|c| c == id).unwrap()
}

/// Times in nanoseconds of every creation by `id`.
fn created_times(loaded: &Loaded, id: &str) -> Vec<u64> {
    let me = index_of(&loaded.header, id);
    loaded
        .records
        .iter()
        .filter_map(|r| {
            if let Record::ItemCreated { time, by, .. } = r {
                (by.index() == me).then_some(time.as_nanos())
            } else {
                None
            }
        })
        .collect()
}

/// Times in nanoseconds of every delivery by `id`.
fn delivered_times(loaded: &Loaded, id: &str) -> Vec<u64> {
    let me = index_of(&loaded.header, id);
    loaded
        .records
        .iter()
        .filter_map(|r| {
            if let Record::ItemDisposed { time, by, how, .. } = r {
                (by.index() == me && *how == Disposal::Delivered).then_some(time.as_nanos())
            } else {
                None
            }
        })
        .collect()
}

const NS: u64 = 1_000_000_000;

fn seconds(list: &[u64]) -> Vec<u64> {
    list.iter().map(|&s| s * NS).collect()
}

/// Nanoseconds a machine spent in each state until `end`, from its notes; the time
/// before its first note counts as `initial`.
fn state_time(
    loaded: &Loaded,
    id: &str,
    initial: MachineState,
    end: u64,
) -> BTreeMap<MachineState, u64> {
    let me = index_of(&loaded.header, id);
    let mut times = BTreeMap::new();
    let mut state = initial;
    let mut since = 0;
    for record in &loaded.records {
        if let Record::Note { time, by, message } = record
            && by.index() == me
            && let FlowMessage::MachineState(note) = message
        {
            *times.entry(state).or_insert(0) += time.as_nanos() - since;
            state = note.state;
            since = time.as_nanos();
        }
    }
    *times.entry(state).or_insert(0) += end - since;
    times
}

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .fold(String::new(), |mut text, byte| {
            write!(text, "{byte:02x}").unwrap();
            text
        })
}

fn duration(text: &str) -> SimDuration {
    text.parse().unwrap()
}

fn runs_in(root: &Path) -> usize {
    let runs = root.join("runs");
    if runs.exists() {
        fs::read_dir(runs).unwrap().count()
    } else {
        0
    }
}

fn tmp_entries(root: &Path) -> usize {
    let tmp = root.join("tmp");
    if tmp.exists() {
        fs::read_dir(tmp).unwrap().count()
    } else {
        0
    }
}

const FLOW_LINE: &str = "  - [source, cut, sink]";

// ---------------------------------------------------------------------------
// Deterministic lines, counted by hand
// ---------------------------------------------------------------------------

/// Source on demand, cut 42 s, a buffer of 5, weld 45 s, 8 h = 28 800 s.
///
/// Weld is the bottleneck. It starts its k-th part at 42 + 45 (k-1) and finishes it at
/// 87 + 45 (k-1). Deliveries by 28 800: k - 1 <= (28 800 - 87) / 45 = 638.06, so 639,
/// the last at 87 + 45 * 638 = 28 797.
///
/// Who holds what at 28 800: the buffer gains one part per 630 s of the 42 s against 45 s
/// race, so it is long full. At 28 797 weld takes part 640 from the buffer, cut's finished
/// part moves into the buffer, and cut takes a new part, made there and then because a
/// Source on demand holds nothing (`SourceState::Waiting`). Cut would finish it at
/// 28 839. So alive = weld 1 + buffer 5 + cut 1 = 7, and created = 639 + 7 = 646.
#[test]
fn line_of_two_machines_and_a_buffer_counts_by_hand() {
    let (root, store) = fresh_store("line_639");
    let text = model(
        None,
        "8h",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 42s}",
            "{id: buf, type: Buffer, capacity: 5 parts}",
            "{id: weld, type: Machine, cycle_time: 45s}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, cut, buf, weld, sink]",
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "delivered(sink)"), 639);
    assert_eq!(metric(&doc, "created(source)"), 646);
    assert_eq!(metric(&doc, "items_alive"), 7);
    assert_eq!(doc.runs[0].end_time, duration("8h"));

    let trace = load(&root, &doc.runs[0]);
    let times = delivered_times(&trace, "sink");
    assert_eq!(times.len(), 639);
    for (k, t) in (0u64..).zip(&times) {
        assert_eq!(*t, (87 + 45 * k) * NS, "delivery {k}");
    }
}

/// Source on demand, cut 10 s, a buffer of 1, weld 30 s, 125 s. Cut is faster, so it blocks.
///
/// t=0: part 1 made, cut starts. t=10: part 1 through the empty buffer into weld (starved
/// until now); part 2 made. t=20: part 2 into the buffer; part 3 made. t=30: part 3 done,
/// the buffer is full: cut blocked. t=40: weld delivers 1, takes 2; 3 moves to the buffer;
/// part 4 made. t=50: 4 done, blocked until t=70, when weld delivers 2, takes 3, 4 moves,
/// part 5 is made. Then the same every 30 s. Created at 0, 10, 20, 40, 70, 100 and
/// delivered at 40, 70, 100. At 125: weld holds 4, the buffer 5, cut 6: 3 alive.
///
/// Cut: busy 0-30, blocked 30-40, then per 30 s busy 10 and blocked 20, cut off at 125:
/// busy 40-50, 70-80, 100-110; blocked 50-70, 80-100, 110-125. Busy 30 + 30 = 60;
/// blocked 10 + 20 + 20 + 15 = 65; together 125.
/// Weld: starved 0-10, busy 10-125: 10 and 115.
#[test]
fn blocking_timeline_by_hand() {
    let (root, store) = fresh_store("blocking");
    let text = model(
        None,
        "125s",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 10s}",
            "{id: buf, type: Buffer, capacity: 1 parts}",
            "{id: weld, type: Machine, cycle_time: 30s}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, cut, buf, weld, sink]",
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(
        created_times(&trace, "source"),
        seconds(&[0, 10, 20, 40, 70, 100])
    );
    assert_eq!(delivered_times(&trace, "sink"), seconds(&[40, 70, 100]));
    assert_eq!(metric(&doc, "items_alive"), 3);

    let cut = state_time(&trace, "cut", MachineState::Busy, 125 * NS);
    assert_eq!(cut[&MachineState::Busy], 60 * NS);
    assert_eq!(cut[&MachineState::Blocked], 65 * NS);
    let weld = state_time(&trace, "weld", MachineState::Starved, 125 * NS);
    assert_eq!(weld[&MachineState::Starved], 10 * NS);
    assert_eq!(weld[&MachineState::Busy], 115 * NS);
}

/// Source on demand, cut 30 s, weld 10 s, no buffer, 95 s. Weld is faster, so it starves.
///
/// Cut finishes at 30, 60, 90 and makes a new part at 0, 30, 60, 90: created 4. Weld works
/// 30-40, 60-70, 90-100: delivered at 40 and 70, and part 3 is still in weld at 95, part
/// 4 in cut: alive 2. Weld starved 0-30, 40-60, 70-90 = 70 s; busy 10 + 10 + 5 = 25 s.
#[test]
fn starving_timeline_by_hand() {
    let (root, store) = fresh_store("starving");
    let text = model(
        None,
        "95s",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 30s}",
            "{id: weld, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, cut, weld, sink]",
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(created_times(&trace, "source"), seconds(&[0, 30, 60, 90]));
    assert_eq!(delivered_times(&trace, "sink"), seconds(&[40, 70]));
    assert_eq!(metric(&doc, "items_alive"), 2);
    let weld = state_time(&trace, "weld", MachineState::Starved, 95 * NS);
    assert_eq!(weld[&MachineState::Starved], 70 * NS);
    assert_eq!(weld[&MachineState::Busy], 25 * NS);
}

/// Source every 60 s, machine 45 s, 3 630 s. The machine outruns the source and is idle
/// (no demand; glossary), not starved: only a Source feeds it.
///
/// The interval counts from the last part leaving, and each part leaves at once: created at
/// 60 k for 60 k <= 3 630, so 60 parts. Delivered at 60 k + 45 <= 3 630, so k <= 59:
/// 59 parts. Part 60 is in the machine at the end. Busy: 59 * 45 + 30 = 2 685 s; idle 945 s.
#[test]
fn idle_first_station_timeline_by_hand() {
    let (root, store) = fresh_store("idle");
    let text = model(
        None,
        "3630s",
        &[
            "{id: source, type: Source, interarrival: 60s}",
            "{id: cut, type: Machine, cycle_time: 45s}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "created(source)"), 60);
    assert_eq!(metric(&doc, "delivered(sink)"), 59);
    assert_eq!(metric(&doc, "items_alive"), 1);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(created_times(&trace, "source")[0], 60 * NS);
    assert_eq!(delivered_times(&trace, "sink")[0], 105 * NS);
    let cut = state_time(&trace, "cut", MachineState::Idle, 3630 * NS);
    assert_eq!(cut[&MachineState::Busy], 2685 * NS);
    assert_eq!(cut[&MachineState::Idle], 945 * NS);
}

/// Source every 30 s, machine 100 s, 1 000 s: the Source is held up (concepts page 5: it
/// holds its part and starts its next interval only when the part has left).
///
/// Part 1 at 30, leaves at once, machine busy until 130. Part 2 at 60, held until 130. Its
/// leaving starts the next interval: part 3 at 160, held until 230 and so on: created at
/// 30, 60, 160, 260, ..., 960: 2 + 9 = 11. Delivered at 130, 230, ..., 930: 9. At 1 000: the
/// machine holds part 10 (until 1 030) and the Source part 11: 2 alive, which ADR-0024 §3
/// says `items_alive` counts.
#[test]
fn held_source_timeline_by_hand() {
    let (root, store) = fresh_store("held");
    let text = model(
        None,
        "1000s",
        &[
            "{id: source, type: Source, interarrival: 30s}",
            "{id: cut, type: Machine, cycle_time: 100s}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(
        created_times(&trace, "source"),
        seconds(&[30, 60, 160, 260, 360, 460, 560, 660, 760, 860, 960])
    );
    assert_eq!(
        delivered_times(&trace, "sink"),
        seconds(&[130, 230, 330, 430, 530, 630, 730, 830, 930])
    );
    assert_eq!(metric(&doc, "items_alive"), 2);
}

/// A batch of 3 every 100 s straight into a Sink, 1 030 s. Every part leaves at once, so
/// batches at 100, 200, ..., 1 000: 10 batches, 30 parts, none alive.
#[test]
fn batches_count_by_hand() {
    let (root, store) = fresh_store("batch");
    let text = model(
        None,
        "1030s",
        &[
            "{id: source, type: Source, interarrival: 100s, batch: 3 parts}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, sink]",
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "created(source)"), 30);
    assert_eq!(metric(&doc, "delivered(sink)"), 30);
    assert_eq!(metric(&doc, "items_alive"), 0);
    let trace = load(&root, &doc.runs[0]);
    let want: Vec<u64> = (1..=10u64).flat_map(|k| [k * 100 * NS; 3]).collect();
    assert_eq!(created_times(&trace, "source"), want);
    assert_eq!(delivered_times(&trace, "sink"), want);
}

/// A machine of capacity 2, cycle 10 s, 95 s: it takes 2 parts at once. Rounds end at 10,
/// 20, ..., 90: 9 rounds, 18 delivered. Created 2 at 0 and 2 at each round's end: 20.
/// 2 alive at 95.
#[test]
fn machine_capacity_counts_by_hand() {
    let (_, store) = fresh_store("capacity");
    let text = model(
        None,
        "95s",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 10s, capacity: 2 parts}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "delivered(sink)"), 18);
    assert_eq!(metric(&doc, "created(source)"), 20);
    assert_eq!(metric(&doc, "items_alive"), 2);
}

/// A horizon of 30 min with a cycle of 1 h: nothing reaches the Sink, and that is stated
/// (ADR-0024 §3: "`delivered(sink) = 0` is stated, not missing").
/// A Source of 1 h with a horizon of 30 min creates nothing; `created(source) = 0` likewise.
/// Each with the unit `items`, and rows for neither a Machine nor a Buffer.
#[test]
fn zero_rows_are_stated_and_only_the_documented_rows_exist() {
    let (_, store) = fresh_store("zero_rows");
    let text = model(
        None,
        "30min",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 1h}",
            "{id: buf, type: Buffer, capacity: 1 parts}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, cut, buf, sink]",
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "delivered(sink)"), 0);
    assert_eq!(metric(&doc, "created(source)"), 1);
    assert_eq!(metric(&doc, "items_alive"), 1);
    assert_eq!(
        names(&doc),
        ["created(source)", "delivered(sink)", "items_alive"],
        "no row for a Machine or Buffer, no scrapped or consumed row at zero"
    );
    assert!(doc.metrics.iter().all(|m| m.unit == "items"));

    let late = model(
        None,
        "30min",
        &[
            "{id: source, type: Source, interarrival: 1h}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, sink]",
    );
    let doc = ok(&late, &store);
    assert_eq!(metric(&doc, "created(source)"), 0);
    assert_eq!(metric(&doc, "delivered(sink)"), 0);
    assert_eq!(metric(&doc, "items_alive"), 0);
}

// ---------------------------------------------------------------------------
// Failures and repairs with fixed durations
// ---------------------------------------------------------------------------

/// Source on demand, machine 70 s, `mtbf: 1h`, `mttr: 10min`, 28 000 s.
///
/// Concepts page 8: `mtbf` is the up time from the end of a repair (or time zero) to the next
/// failure, and an interrupted cycle resumes with the time it had left. So failures at
/// 3 600 + 4 200 j and repairs ending 600 s later. In [0, 28 000] that is failures at 3 600,
/// 7 800, 12 000, 16 200, 20 400, 24 600, each repaired before 28 000; the 7th is at 28 800.
/// Up time 28 000 - 6 * 600 = 24 400 s, so floor(24 400 / 70) = 348 parts delivered.
///
/// The k-th part finishes after 70 k of up time. The clock then shows 70 k + 600 for each
/// failure before: n = floor(70 k / 3 600). No 70 k up to 24 360 is a multiple of 3 600, so
/// no failure ties with a cycle's end.
///
/// It draws nothing: seeds empty, `no_random_draws`.
#[test]
fn fixed_failures_timeline_by_hand() {
    let (root, store) = fresh_store("failures");
    let text = model(
        None,
        "28000s",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 70s, mtbf: 1h, mttr: 10min}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "delivered(sink)"), 348);
    assert_eq!(metric(&doc, "items_alive"), 1);
    assert_eq!(doc.provenance.seeds, [] as [u64; 0]);

    let trace = load(&root, &doc.runs[0]);
    let times = delivered_times(&trace, "sink");
    assert_eq!(times.len(), 348);
    for (k, t) in (1u64..).zip(&times) {
        let up = 70 * k;
        assert_eq!(*t, (up + 600 * (up / 3600)) * NS, "delivery {k}");
    }

    let failures: Vec<u64> = trace
        .records
        .iter()
        .filter_map(|r| {
            if let Record::Event {
                time,
                message: FlowMessage::Failure,
                ..
            } = r
            {
                Some(time.as_nanos())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        failures,
        seconds(&[3600, 7800, 12_000, 16_200, 20_400, 24_600])
    );

    let cut = state_time(&trace, "cut", MachineState::Busy, 28_000 * NS);
    assert_eq!(cut[&MachineState::Down], 3600 * NS);
    assert_eq!(cut[&MachineState::Busy], 24_400 * NS);
}

/// Cut 10 s, a buffer of 2, weld 30 s with `mtbf: 115s`, `mttr: 50s`, 600 s.
///
/// Weld starts at 10 and always finds a part: cut is three times as fast. Its clock of up
/// time: delivered at 40, 70, 100. Failure at 115, 15 s into a cycle: 15 s left. Repaired at
/// 165; delivered at 180, 210, 240, 270. Next failure 165 + 115 = 280: 10 s into a cycle, 20
/// left. Repaired at 330; delivered at 350, 380, 410, 440. Next failure 330 + 115 = 445: 5 s
/// into a cycle, 25 left. Repaired at 495; delivered at 520, 550, 580, the next at 610 is
/// past the horizon. So 14 delivered. While it is down nothing moves, and upstream fills.
///
/// At 600: weld holds 1, the buffer 2 and cut 1: 4 alive, so 18 created.
#[test]
fn failure_blocks_upstream_and_resumes_by_hand() {
    let (root, store) = fresh_store("failure_blocks");
    let text = model(
        None,
        "600s",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 10s}",
            "{id: buf, type: Buffer, capacity: 2 parts}",
            "{id: weld, type: Machine, cycle_time: 30s, mtbf: 115s, mttr: 50s}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, cut, buf, weld, sink]",
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(
        delivered_times(&trace, "sink"),
        seconds(&[
            40, 70, 100, 180, 210, 240, 270, 350, 380, 410, 440, 520, 550, 580
        ])
    );
    assert_eq!(metric(&doc, "items_alive"), 4);
    assert_eq!(metric(&doc, "created(source)"), 18);
    let weld = state_time(&trace, "weld", MachineState::Starved, 600 * NS);
    assert_eq!(weld[&MachineState::Down], 150 * NS);
    assert_eq!(weld[&MachineState::Starved], 10 * NS);
    assert_eq!(weld[&MachineState::Busy], 440 * NS);
}

// ---------------------------------------------------------------------------
// Setups: counted by hand from the variants the trace shows
// ---------------------------------------------------------------------------

/// The variants of the parts arriving at `id`, in order.
fn variants_into(loaded: &Loaded, id: &str) -> Vec<u16> {
    let me = index_of(&loaded.header, id);
    loaded
        .records
        .iter()
        .filter_map(|r| {
            if let Record::Event {
                target,
                message: FlowMessage::Arrive(attrs),
                ..
            } = r
            {
                (target.index() == me).then_some(attrs.variant.get())
            } else {
                None
            }
        })
        .collect()
}

fn setups_done(loaded: &Loaded) -> usize {
    loaded
        .records
        .iter()
        .filter(|r| {
            matches!(
                r,
                Record::Event {
                    message: FlowMessage::SetupDone,
                    ..
                }
            )
        })
        .count()
}

/// Source on demand with two variants, machine 10 s, `setup: {per_variant: 7s}`, 2 000 s.
///
/// The first part costs no setup; a part of another variant costs 7 s (concepts page 11).
/// One place on the machine, so part k+1 arrives when part k has left. Part k leaves at
/// 10 k + 7 C(k), where C(k) counts the changes of variant among the first k parts. The
/// variants come from the trace; the arithmetic does not.
#[test]
fn per_variant_setups_count_by_hand() {
    let (root, store) = fresh_store("setups");
    let text = model(
        Some(11),
        "2000s",
        &[
            "{id: source, type: Source, variants: {a: 50%, b: 50%}}",
            "{id: cut, type: Machine, cycle_time: 10s, setup: {per_variant: 7s}}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    assert_eq!(doc.provenance.seeds, [11]);
    let trace = load(&root, &doc.runs[0]);
    let variants = variants_into(&trace, "cut");
    let times = delivered_times(&trace, "sink");
    assert!(variants.contains(&0) && variants.contains(&1));

    let mut changes = 0;
    for (k, t) in (1u64..).zip(&times) {
        let i = usize::try_from(k).unwrap() - 1;
        if i > 0 && variants[i] != variants[i - 1] {
            changes += 1;
        }
        assert_eq!(*t, (10 * k + 7 * changes) * NS, "part {k}");
    }
    // The part after the last delivered one is on the machine: its setup, if any, ended
    // by 2 000 only if there is time for it.
    let leave = 10 * times.len() as u64 + 7 * changes;
    let next_changes = variants.get(times.len()).is_some_and(|v| {
        times
            .last()
            .is_some_and(|_| *v != variants[times.len() - 1])
            && leave + 7 <= 2000
    });
    assert_eq!(
        setups_done(&trace) as u64,
        changes + u64::from(next_changes)
    );
}

/// `matrix` beats `per_variant`, and a `0s` entry is no changeover (the doc comment of
/// `Setup`). a to b costs 6 s, b to a costs nothing and there is no `per_variant`.
/// Part k leaves at 10 k + 6 A(k), where A(k) counts the a-to-b changes among the first k.
/// Sorted variant names give a id 0 and b id 1.
#[test]
fn matrix_setups_count_by_hand() {
    let (root, store) = fresh_store("matrix");
    let text = model(
        Some(5),
        "2000s",
        &[
            "{id: source, type: Source, variants: {a: 50%, b: 50%}}",
            "{id: cut, type: Machine, cycle_time: 10s, setup: {matrix: [{from: a, to: b, time: 6s}, {from: b, to: a, time: 0s}]}}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    let variants = variants_into(&trace, "cut");
    let times = delivered_times(&trace, "sink");
    let mut changes = 0;
    for (k, t) in (1u64..).zip(&times) {
        let i = usize::try_from(k).unwrap() - 1;
        if i > 0 && variants[i - 1] == 0 && variants[i] == 1 {
            changes += 1;
        }
        assert_eq!(*t, (10 * k + 6 * changes) * NS, "part {k}");
    }
    assert!(changes > 0);
}

/// One variant and a setup: no change ever, so `floor(1 000 / 10) = 100` delivered and no
/// setup. A single variant draws nothing (ADR-0024 §5), so there is no seed either.
#[test]
fn one_variant_has_no_setup_and_no_seed() {
    let (root, store) = fresh_store("one_variant");
    let text = model(
        None,
        "1000s",
        &[
            "{id: source, type: Source, variants: {a: 100%}}",
            "{id: cut, type: Machine, cycle_time: 10s, setup: {per_variant: 7s}}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    assert_eq!(metric(&doc, "delivered(sink)"), 100);
    assert_eq!(doc.provenance.seeds, [] as [u64; 0]);
    assert_eq!(setups_done(&load(&root, &doc.runs[0])), 0);
}

// ---------------------------------------------------------------------------
// Conservation and bounds at every record of a run with everything random
// ---------------------------------------------------------------------------

/// Spec §4: units created = in process + delivered at every event; `0 <= fill <= capacity`;
/// never more than `capacity` units in a machine. So walk every record and keep where each
/// item is: created by a Source, moved only by an `Arrive` from where it is, delivered once.
/// A FIFO buffer gives its parts back in the order they entered. The sums at the end equal
/// the metrics, and the counters equal the result's.
#[test]
fn conservation_and_bounds_hold_at_every_record() {
    for seed in [1_u64, 2, 3] {
        let (root, store) = fresh_store(&format!("conservation_{seed}"));
        let text = model(
            Some(seed),
            "4h",
            &[
                "{id: source, type: Source, interarrival: {dist: exponential, mean: 20s}, batch: 2 parts, variants: {a: 70%, b: 30%}}",
                "{id: inbuf, type: Buffer, capacity: 4 parts}",
                "{id: cut, type: Machine, cycle_time: {dist: exponential, mean: 15s}, mtbf: {dist: exponential, mean: 30min}, mttr: {dist: exponential, mean: 3min}}",
                "{id: buf, type: Buffer, capacity: 3 parts}",
                "{id: weld, type: Machine, cycle_time: {dist: lognormal, mean: 20s, sigma: 0.5}, capacity: 2 parts, setup: {per_variant: {dist: exponential, mean: 5s}}}",
                "{id: sink, type: Sink}",
            ],
            "  - [source, inbuf, cut, buf, weld, sink]",
        );
        let doc = ok(&text, &store);
        let trace = load(&root, &doc.runs[0]);
        let capacity: BTreeMap<usize, u64> = [("inbuf", 4), ("cut", 1), ("buf", 3), ("weld", 2)]
            .into_iter()
            .map(|(id, c)| (index_of(&trace.header, id), c))
            .collect();
        let source = index_of(&trace.header, "source");
        let sink = index_of(&trace.header, "sink");

        let mut place: BTreeMap<u64, usize> = BTreeMap::new();
        let mut fill: BTreeMap<usize, u64> = BTreeMap::new();
        let mut queue: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
        let (mut created, mut delivered, mut events) = (0_u64, 0_u64, 0_u64);
        let mut now = 0;
        for record in &trace.records {
            let time = record.time().as_nanos();
            assert!(time >= now, "the clock never goes back");
            now = time;
            match record {
                Record::ItemCreated { item, by, .. } => {
                    assert_eq!(by.index(), source);
                    assert!(place.insert(item.as_u64(), source).is_none());
                    created += 1;
                }
                Record::ItemDisposed { item, by, how, .. } => {
                    assert_eq!(by.index(), sink);
                    assert_eq!(*how, Disposal::Delivered);
                    assert_eq!(place.remove(&item.as_u64()), Some(sink));
                    delivered += 1;
                }
                Record::Event {
                    target,
                    from,
                    message,
                    item,
                    ..
                } => {
                    events += 1;
                    if let (FlowMessage::Arrive(_), Some(item)) = (message, item) {
                        let id = item.as_u64();
                        assert_eq!(place.insert(id, target.index()), Some(from.index()));
                        if from.index() != source {
                            *fill.get_mut(&from.index()).unwrap() -= 1;
                        }
                        if let Some(limit) = capacity.get(&target.index()) {
                            let now_in = fill.entry(target.index()).or_insert(0);
                            *now_in += 1;
                            assert!(*now_in <= *limit, "over capacity at {time} ns");
                        }
                        if from.index() == index_of(&trace.header, "buf")
                            || from.index() == index_of(&trace.header, "inbuf")
                        {
                            let list = queue.get_mut(&from.index()).unwrap();
                            assert_eq!(list.remove(0), id, "FIFO leaves in entry order");
                        }
                        if target.index() == index_of(&trace.header, "buf")
                            || target.index() == index_of(&trace.header, "inbuf")
                        {
                            queue.entry(target.index()).or_default().push(id);
                        }
                    }
                }
                Record::Note { .. } => {}
            }
            assert_eq!(
                created - delivered,
                place.len() as u64,
                "conservation at {time} ns"
            );
        }
        assert_eq!(metric(&doc, "created(source)"), created);
        assert_eq!(metric(&doc, "delivered(sink)"), delivered);
        assert_eq!(metric(&doc, "items_alive"), created - delivered);
        assert_eq!(doc.runs[0].events, events);
        assert_eq!(doc.runs[0].records, trace.records.len() as u64);
        assert_eq!(trace.summary.events, events);
        assert!(delivered > 100, "the run did something");
    }
}

// ---------------------------------------------------------------------------
// Long-run rates against closed form, with fixed seeds
// ---------------------------------------------------------------------------

fn within(what: &str, actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} is not within {tolerance} of {expected}"
    );
}

/// A saturated machine with exponential cycles delivers a Poisson process: over 100 h with a
/// mean of 10 s, N has mean and variance 36 000. Four sigmas: 4 * sqrt(36 000) = 759.
#[test]
fn saturated_exponential_machine_rate() {
    let (_, store) = fresh_store("poisson");
    let text = model(
        Some(42),
        "100h",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    #[allow(clippy::cast_precision_loss)] // A count far below 2^53.
    let delivered = metric(&doc, "delivered(sink)") as f64;
    within("delivered", delivered, 36_000.0, 759.0);
}

/// Availability (concepts page 8): A = MTBF / (MTBF + MTTR) = 60 / 66, with up and down times
/// exponential, means a = 3 600 s and b = 360 s. A fixed cycle of 10 s on a saturated machine
/// delivers one part per 10 s of up time, because a cycle resumes where it stopped. Over
/// T = 200 h = 720 000 s: mean A T / 10 = 65 454.5.
/// The up time of a two-state Markov chain has variance 2 a^2 b^2 / (a + b)^3 per unit of
/// time: 54.1 * T, so sigma = 6 241 s, or 624 parts. Four sigmas: 2 497.
#[test]
fn availability_rate() {
    let (_, store) = fresh_store("availability");
    let text = model(
        Some(42),
        "200h",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 10s, mtbf: {dist: exponential, mean: 1h}, mttr: {dist: exponential, mean: 6min}}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    #[allow(clippy::cast_precision_loss)] // A count far below 2^53.
    let delivered = metric(&doc, "delivered(sink)") as f64;
    within("delivered", delivered, 65_454.5, 2_497.0);
}

/// Two saturated stations, equal exponential rates mu, b places between them. The number of
/// parts past the first station's input is a birth-death chain on 0..=b+2 with equal rates, so
/// it is uniform on its b + 3 states. The second station is busy unless the chain is at 0:
/// X = mu (b + 2) / (b + 3). No buffer: 2/3 mu. Three places: 5/6 mu.
/// Over 100 h with a mean of 10 s, mu T = 36 000. The count is at most Poisson in its
/// dispersion: four sigmas are at most 4 sqrt(mean).
#[test]
fn tandem_with_blocking_rates() {
    for (name, buffer, expected) in [("none", 0_u32, 24_000.0_f64), ("three", 3, 30_000.0)] {
        let (_, store) = fresh_store(&format!("tandem_{name}"));
        let station = "cycle_time: {dist: exponential, mean: 10s}";
        let first = format!("{{id: a, type: Machine, {station}}}");
        let second = format!("{{id: b, type: Machine, {station}}}");
        let place = format!("{{id: buf, type: Buffer, capacity: {buffer} parts}}");
        let mut list = vec!["{id: source, type: Source}", first.as_str()];
        if buffer > 0 {
            list.push(place.as_str());
        }
        list.extend([second.as_str(), "{id: sink, type: Sink}"]);
        let flow = if buffer > 0 {
            "  - [source, a, buf, b, sink]"
        } else {
            "  - [source, a, b, sink]"
        };
        let doc = ok(&model(Some(42), "100h", &list, flow), &store);
        #[allow(clippy::cast_precision_loss)] // A count far below 2^53.
        let delivered = metric(&doc, "delivered(sink)") as f64;
        within(name, delivered, expected, 4.0 * expected.sqrt());
    }
}

/// M/D/1 with lambda = 1/40 s, service 30 s, rho = 0.75: Pollaczek-Khinchine gives
/// L = rho + rho^2 / (2 (1 - rho)) = 0.75 + 0.5625 / 0.5 = 1.875 parts in the system.
/// `items_alive` over time is L, since a large buffer never holds up the Source. Over T = 300 h
/// there are 27 000 arrivals. The M/M/1 asymptotic variance of the time average,
/// 2 rho (1 + rho) / (1 - rho)^4 per service time, bounds M/D/1's: 672 over T / 30 s = 36 000
/// services is sigma <= 0.137. Four sigmas: 0.55.
/// Created is Poisson with mean 27 000: four sigmas 657.
#[test]
fn md1_time_average_in_system() {
    let (root, store) = fresh_store("md1");
    let text = model(
        Some(42),
        "300h",
        &[
            "{id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}",
            "{id: buf, type: Buffer, capacity: 100000 parts}",
            "{id: cut, type: Machine, cycle_time: 30s}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, buf, cut, sink]",
    );
    let doc = ok(&text, &store);
    #[allow(clippy::cast_precision_loss)] // A count far below 2^53.
    let created = metric(&doc, "created(source)") as f64;
    within("created", created, 27_000.0, 657.0);

    let trace = load(&root, &doc.runs[0]);
    let horizon = duration("300h").as_nanos();
    let (mut area, mut alive, mut since) = (0_u128, 0_u128, 0_u64);
    for record in &trace.records {
        let t = record.time().as_nanos();
        if matches!(
            record,
            Record::ItemCreated { .. } | Record::ItemDisposed { .. }
        ) {
            area += alive * u128::from(t - since);
            since = t;
            alive = if matches!(record, Record::ItemCreated { .. }) {
                alive + 1
            } else {
                alive - 1
            };
        }
    }
    area += alive * u128::from(horizon - since);
    #[allow(clippy::cast_precision_loss)] // The ratio only needs a few digits.
    let mean = area as f64 / f64::from(1_080_000_u32) / 1e9;
    within("L", mean, 1.875, 0.55);
}

// ---------------------------------------------------------------------------
// Determinism, replay and streams
// ---------------------------------------------------------------------------

const RANDOM: &[&str] = &[
    "{id: source, type: Source, interarrival: {dist: exponential, mean: 40s}, variants: {a: 50%, b: 50%}}",
    "{id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 30s, sigma: 0.2}, mtbf: {dist: exponential, mean: 1h}, mttr: {dist: exponential, mean: 5min}, setup: {per_variant: {dist: exponential, mean: 10s}}}",
    "{id: sink, type: Sink}",
];

/// Same model, same seed: same bytes (principle 3, ADR-0024 §3). In two stores, so nothing is
/// reused; the three stored files are compared byte for byte.
#[test]
fn same_seed_gives_identical_files_in_two_stores() {
    let (a_root, a) = fresh_store("bytes_a");
    let (b_root, b) = fresh_store("bytes_b");
    let text = model(Some(42), "2h", RANDOM, FLOW_LINE);
    let doc_a = ok(&text, &a);
    let doc_b = ok(&text, &b);
    assert_eq!(doc_a, doc_b);
    let id = doc_a.runs[0].run_id.as_ref().unwrap().as_str().to_owned();
    for file in ["model.json", "result.json", "trace.takt"] {
        let bytes_a = fs::read(a_root.join("runs").join(&id).join(file)).unwrap();
        let bytes_b = fs::read(b_root.join("runs").join(&id).join(file)).unwrap();
        assert_eq!(bytes_a, bytes_b, "{file}");
    }
}

/// A seed reaches the stream: two seeds, two traces, two ids, one model hash and one system
/// hash. The seed that ran is in `seeds`, in `runs[].seed` and in the header.
#[test]
fn seed_changes_the_stream_and_the_id_and_nothing_else() {
    let (root, store) = fresh_store("two_seeds");
    let text = model(None, "2h", RANDOM, FLOW_LINE);
    let mut one = RunRequest::new();
    one.seeds = &[1];
    let mut two = RunRequest::new();
    two.seeds = &[2];
    let a = go(&text, &store, &one).unwrap().result;
    let b = go(&text, &store, &two).unwrap().result;
    assert_ne!(a.runs[0].run_id, b.runs[0].run_id);
    assert_ne!(
        a.runs[0].trace.as_ref().unwrap().stream_hash,
        b.runs[0].trace.as_ref().unwrap().stream_hash
    );
    assert_eq!(a.provenance.model_hash, b.provenance.model_hash);
    assert_eq!(a.provenance.system_hash, b.provenance.system_hash);
    assert_eq!(a.provenance.seeds, [1]);
    assert_eq!(a.runs[0].seed, Some(1));
    assert_eq!(load(&root, &a.runs[0]).header.seed, Some(1));
    assert_eq!(runs_in(&root), 2);
}

/// K2 (ADR-0024 §5): `--seed 2` over `seed: 1`. The seed that ran is recorded everywhere, the
/// stored model keeps what the file said, and the stream is the one of a file saying
/// `seed: 2`, since the seed alone decides it. The two files are different models.
#[test]
fn seed_option_over_model_seed_is_recorded_not_written_into_the_model() {
    let (root, store) = fresh_store("k2");
    let says_one = model(Some(1), "2h", RANDOM, FLOW_LINE);
    let says_two = model(Some(2), "2h", RANDOM, FLOW_LINE);
    let mut request = RunRequest::new();
    request.seeds = &[2];
    let over = go(&says_one, &store, &request).unwrap().result;
    let own = ok(&says_two, &store);
    assert_eq!(over.provenance.seeds, [2]);
    assert_eq!(over.runs[0].seed, Some(2));
    assert_eq!(load(&root, &over.runs[0]).header.seed, Some(2));
    assert_eq!(
        over.runs[0].trace.as_ref().unwrap().stream_hash,
        own.runs[0].trace.as_ref().unwrap().stream_hash
    );
    assert_ne!(over.provenance.model_hash, own.provenance.model_hash);
    assert_ne!(over.runs[0].run_id, own.runs[0].run_id);

    let stored = fs::read_to_string(
        root.join("runs")
            .join(over.runs[0].run_id.as_ref().unwrap().as_str())
            .join("model.json"),
    )
    .unwrap();
    let json = parse(&stored, Format::Json).unwrap().to_json();
    assert_eq!(json["model"]["seed"], Json::from(1));
}

/// Streams belong to a component and a purpose (CLAUDE.md, principle 3), so a second chain
/// changes neither the times nor the counts of the first, wherever it stands in the
/// document: before it, which moves every component number, or after it.
#[test]
fn an_unrelated_chain_leaves_the_other_streams_unchanged() {
    let s1 = "{id: s1, type: Source, interarrival: {dist: exponential, mean: 40s}, variants: {a: 50%, b: 50%}}";
    let m1 = "{id: m1, type: Machine, cycle_time: {dist: exponential, mean: 30s}, mtbf: {dist: exponential, mean: 30min}, mttr: {dist: exponential, mean: 2min}}";
    let k1 = "{id: k1, type: Sink}";
    let s2 = "{id: s2, type: Source, interarrival: {dist: exponential, mean: 11s}}";
    let m2 = "{id: m2, type: Machine, cycle_time: {dist: lognormal, mean: 9s, sigma: 0.4}}";
    let k2 = "{id: k2, type: Sink}";
    let base = model(Some(42), "4h", &[s1, m1, k1], "  - [s1, m1, k1]");
    let after = model(
        Some(42),
        "4h",
        &[s1, m1, k1, s2, m2, k2],
        "  - [s1, m1, k1]\n  - [s2, m2, k2]",
    );
    let before = model(
        Some(42),
        "4h",
        &[s2, m2, k2, s1, m1, k1],
        "  - [s2, m2, k2]\n  - [s1, m1, k1]",
    );

    let (base_root, base_store) = fresh_store("streams_base");
    let doc = ok(&base, &base_store);
    let base_trace = load(&base_root, &doc.runs[0]);
    let want_created = created_times(&base_trace, "s1");
    let want_delivered = delivered_times(&base_trace, "k1");
    assert!(want_delivered.len() > 100);

    for (name, text) in [("after", after), ("before", before)] {
        let (root, store) = fresh_store(&format!("streams_{name}"));
        let doc = ok(&text, &store);
        let trace = load(&root, &doc.runs[0]);
        assert_eq!(created_times(&trace, "s1"), want_created, "{name}");
        assert_eq!(delivered_times(&trace, "k1"), want_delivered, "{name}");
        assert_ne!(delivered_times(&trace, "k2"), [] as [u64; 0]);
        // Rows per component, sorted or not, for both chains.
        for row in [
            "created(s1)",
            "created(s2)",
            "delivered(k1)",
            "delivered(k2)",
        ] {
            assert!(metric(&doc, row) > 0, "{row}");
        }
    }
}

/// Replay of a stored run compares every record: it reports the run's own counts and hash, for
/// a run with a seed and for one with none, whose header says so.
#[test]
fn replay_of_stored_runs_reports_the_run() {
    let (root, store) = fresh_store("replay");
    let registry = standard().unwrap();
    for text in [
        model(Some(42), "1h", RANDOM, FLOW_LINE),
        model(
            None,
            "1h",
            &[
                "{id: source, type: Source}",
                "{id: cut, type: Machine, cycle_time: 42s, mtbf: 10min, mttr: 1min}",
                "{id: sink, type: Sink}",
            ],
            FLOW_LINE,
        ),
    ] {
        let doc = ok(&text, &store);
        let entry = &doc.runs[0];
        let id = entry.run_id.as_ref().unwrap();
        let summary = replay_stored(&registry, &store, id).unwrap();
        assert_eq!(summary.records, entry.records);
        assert_eq!(summary.events, entry.events);
        assert_eq!(
            summary.stream_hash,
            entry.trace.as_ref().unwrap().stream_hash
        );
        let header = load(&root, entry).header;
        assert_eq!(header.seed, entry.seed);
    }
}

// ---------------------------------------------------------------------------
// What a damaged store gives: replay names it, never a pass
// ---------------------------------------------------------------------------

fn stored_run(name: &str) -> (PathBuf, Store, RunId) {
    let (root, store) = fresh_store(name);
    let doc = ok(&model(Some(42), "10min", RANDOM, FLOW_LINE), &store);
    let id = doc.runs[0].run_id.clone().unwrap();
    (root, store, id)
}

/// A changed `model.json` no longer hashes to the header's hash: "the stored files disagree",
/// `RunFailed` (doc comment of `replay_stored`).
#[test]
fn replay_refuses_a_changed_stored_model() {
    let (root, store, id) = stored_run("replay_model");
    let file = root.join("runs").join(id.as_str()).join("model.json");
    let text = fs::read_to_string(&file).unwrap().replace("40s", "41s");
    fs::write(&file, text).unwrap();
    let failure = replay_stored(&standard().unwrap(), &store, &id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
}

/// One flipped byte in the records of a trace: never a pass. The block's CRC catches it, so the
/// trace cannot be read: "`Io` if a stored file cannot be read" (doc comment of `replay_stored`),
/// as in the check run, and not a replay that diverged.
#[test]
fn replay_refuses_a_damaged_trace() {
    let (root, store, id) = stored_run("replay_trace");
    let file = root.join("runs").join(id.as_str()).join("trace.takt");
    let mut bytes = fs::read(&file).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x40;
    fs::write(&file, bytes).unwrap();
    let failure = replay_stored(&standard().unwrap(), &store, &id).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io, "{failure:?}");
    assert_eq!(failure.diagnostics[0].type_name, "store.read");
    assert!(
        failure.diagnostics[0].message.contains("CRC"),
        "{}",
        failure.diagnostics[0].message
    );
}

/// The running package differs from the stored `provenance`: `RunFailed`.
#[test]
fn replay_refuses_another_package_version() {
    let (root, store, id) = stored_run("replay_package");
    let file = root.join("runs").join(id.as_str()).join("result.json");
    let text = fs::read_to_string(&file).unwrap();
    let changed = text.replace(
        &format!("\"package_version\": \"{}\"", env!("CARGO_PKG_VERSION")),
        "\"package_version\": \"999.0.0\"",
    );
    assert_ne!(text, changed);
    fs::write(&file, changed).unwrap();
    let failure = replay_stored(&standard().unwrap(), &store, &id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
}

/// A file that is gone cannot be read: `Io`.
#[test]
fn replay_of_a_missing_trace_is_an_io_failure() {
    let (root, store, id) = stored_run("replay_missing");
    fs::remove_file(root.join("runs").join(id.as_str()).join("trace.takt")).unwrap();
    let failure = replay_stored(&standard().unwrap(), &store, &id).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
}

/// An id that is not in the store is `Invalid`: exit 1 ("unknown run id" in `describe`).
#[test]
fn replay_of_an_unknown_id_is_invalid() {
    let (_, store) = fresh_store("replay_unknown");
    let id: RunId = format!("taktsim-core-{}", "0".repeat(64)).parse().unwrap();
    let failure = replay_stored(&standard().unwrap(), &store, &id).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
}

/// A run id becomes a directory below the store (ADR-0024 §8), and later commands take it from
/// outside. So one that climbs out of `runs/` cannot be a `RunId`.
#[test]
fn a_run_id_cannot_leave_the_store() {
    let digits = "0".repeat(64);
    for bad in [
        format!("../../x-{digits}"),
        format!("..-{digits}"),
        format!("a/b-{digits}"),
        format!("a\\b-{digits}"),
        format!("-{digits}"),
    ] {
        assert!(bad.parse::<RunId>().is_err(), "{bad} was accepted");
    }
}

// ---------------------------------------------------------------------------
// Ids and hashes from their definitions
// ---------------------------------------------------------------------------

/// ADR-0024 §6: the id is the engine's name, a dash, and the hex SHA-256 of the JCS of nine
/// keys, in JCS's sorted order. The experiment id is `exp-` and the hex SHA-256 of
/// `{"experiment_id_format":1,"runs":[...]}`. Written out here by hand, with a seed and with
/// `null` for none.
#[test]
fn ids_follow_their_definition() {
    let (_, store) = fresh_store("ids");
    let drawn = ok(&model(Some(42), "10min", RANDOM, FLOW_LINE), &store);
    let exact = ok(
        &model(
            None,
            "10min",
            &[
                "{id: source, type: Source}",
                "{id: cut, type: Machine, cycle_time: 42s}",
                "{id: sink, type: Sink}",
            ],
            FLOW_LINE,
        ),
        &store,
    );
    for (doc, seed) in [(&drawn, "42"), (&exact, "null")] {
        let p = &doc.provenance;
        let entry = &doc.runs[0];
        let trace = entry.trace.as_ref().unwrap();
        let canonical = format!(
            r#"{{"engine":"{}","engine_version":"{}","message_format":"{}","model_hash":"{}","package":"{}","package_version":"{}","record_format":"{}","run_id_format":1,"seed":{seed}}}"#,
            p.engine,
            p.engine_version,
            trace.message_format,
            p.model_hash,
            p.package,
            p.package_version,
            trace.record_format,
        );
        let run_id = format!("{}-{}", p.engine, sha256_hex(&canonical));
        assert_eq!(entry.run_id.as_ref().unwrap().as_str(), run_id, "{seed}");
        let experiment = format!(r#"{{"experiment_id_format":1,"runs":["{run_id}"]}}"#);
        assert_eq!(
            doc.experiment_id.as_ref().unwrap().as_str(),
            format!("exp-{}", sha256_hex(&experiment)),
            "{seed}"
        );
    }
    assert_eq!(drawn.provenance.engine, "taktsim-core");
    assert_eq!(drawn.provenance.package, "taktsim-components");
    let trace = drawn.runs[0].trace.as_ref().unwrap();
    assert!(trace.record_format.starts_with("taktsim-core/"));
    assert!(trace.message_format.starts_with("taktsim-components/"));
}

/// ADR-0024 §8: `model.json` is the canonical normalized model: its SHA-256 is the model hash.
/// The system hash (doc comment of `SystemHash`) is the SHA-256 of the canonical `format`,
/// `components` and `flow` sections, so it can be recomputed from `model.json`.
#[test]
fn hashes_follow_their_definition_from_the_stored_model() {
    let (root, store) = fresh_store("hashes");
    let doc = ok(&model(Some(42), "10min", RANDOM, FLOW_LINE), &store);
    let id = doc.runs[0].run_id.as_ref().unwrap().as_str();
    let bytes = fs::read(root.join("runs").join(id).join("model.json")).unwrap();
    assert_eq!(doc.provenance.model_hash.to_string(), sha256_hex(&bytes));

    let text = String::from_utf8(bytes).unwrap();
    let json = parse(&text, Format::Json).unwrap().to_json();
    let system = Json::object([
        ("components", json["components"].clone()),
        ("flow", json["flow"].clone()),
        ("format", json["format"].clone()),
    ]);
    assert_eq!(
        doc.provenance.system_hash.to_string(),
        sha256_hex(taktsim_model::jcs::canonicalize(&system).unwrap())
    );
}

/// The horizon is in the model hash, not the system hash (ADR-0024 §3, §6). So another horizon
/// is another run, and the stored model says it. Giving it by option or in the file gives the
/// same model, so the same id, and the option's text form cannot matter: `1h` and `3600s` are
/// one `SimDuration`.
#[test]
fn horizon_option_and_horizon_in_the_file_are_one_model() {
    let (root, store) = fresh_store("horizon");
    let eight = model(Some(42), "8h", RANDOM, FLOW_LINE);
    let one = model(Some(42), "1h", RANDOM, FLOW_LINE);
    let mut request = RunRequest::new();
    request.horizon = Some(duration("3600s"));
    let by_option = go(&eight, &store, &request).unwrap();
    assert!(!by_option.reused);
    let in_file = go(&one, &store, &RunRequest::new()).unwrap();
    assert!(in_file.reused, "the same model and seed is the same run");
    assert_eq!(by_option.result, in_file.result);
    assert_eq!(by_option.result.runs[0].end_time, duration("1h"));
    let bare = validate(
        &standard().unwrap(),
        ModelSource::Text(&eight, Format::Yaml),
    );
    assert_eq!(
        bare.system_hash.as_ref().unwrap().to_string(),
        by_option.result.provenance.system_hash.to_string()
    );
    assert_ne!(
        bare.model_hash.as_ref().unwrap().to_string(),
        by_option.result.provenance.model_hash.to_string()
    );

    let id = by_option.result.runs[0].run_id.as_ref().unwrap().as_str();
    let stored = fs::read_to_string(root.join("runs").join(id).join("model.json")).unwrap();
    let horizon = parse(&stored, Format::Json).unwrap().to_json()["experiment"]["horizon"].clone();
    assert_eq!(horizon.as_str().map(duration), Some(duration("1h")));
}

/// A model file is text under version control; what counts is its content. Comments, the order
/// of keys, CRLF line ends (Windows, ADR-0010) and JSON instead of YAML give one model hash.
#[test]
fn the_same_model_in_other_clothes_has_the_same_hash() {
    let registry = standard().unwrap();
    let plain = "format: \"1.0\"\nmodel: {name: m, seed: 1}\nexperiment: {type: terminating, horizon: 1h}\n\
                 components:\n  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 42s}\n  - {id: sink, type: Sink}\n\
                 flow:\n  - [source, cut, sink]\n";
    let want = validate(&registry, ModelSource::Text(plain, Format::Yaml));
    assert!(want.valid);

    let comments = format!("# A comment.\n{}", plain.replace("flow:", "# More.\nflow:"));
    let crlf = plain.replace('\n', "\r\n");
    let reordered = "flow:\n  - [source, cut, sink]\n\
                     components:\n  - {type: Source, id: source}\n  - {cycle_time: 42s, id: cut, type: Machine}\n  - {type: Sink, id: sink}\n\
                     experiment: {horizon: 1h, type: terminating}\nmodel: {seed: 1, name: m}\nformat: \"1.0\"\n";
    let json = r#"{"format":"1.0","model":{"name":"m","seed":1},"experiment":{"type":"terminating","horizon":"1h"},
        "components":[{"id":"source","type":"Source"},{"id":"cut","type":"Machine","cycle_time":"42s"},{"id":"sink","type":"Sink"}],
        "flow":[["source","cut","sink"]]}"#;
    for (name, source) in [
        ("comments", ModelSource::Text(&comments, Format::Yaml)),
        ("crlf", ModelSource::Text(&crlf, Format::Yaml)),
        ("reordered", ModelSource::Text(reordered, Format::Yaml)),
        ("json", ModelSource::Text(json, Format::Json)),
    ] {
        let report = validate(&registry, source);
        assert!(report.valid, "{name}: {:?}", report.errors);
        assert_eq!(report.model_hash, want.model_hash, "{name}");
        assert_eq!(report.system_hash, want.system_hash, "{name}");
    }
}

/// "A run can always be reproduced from a model hash" (spec §4). So the stored `model.json`,
/// with the seed of the result, run in another store, gives the same id and the same trace, also
/// for a model that used `include:`. Loading a canonical model must not change it.
#[test]
fn the_stored_model_reproduces_the_run() {
    let dir = fresh("reproduce");
    fs::write(
        dir.join("cells.yaml"),
        "components:\n  - {id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}\n  - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 30s}}\n",
    )
    .unwrap();
    fs::write(
        dir.join("line.yaml"),
        "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\ninclude:\n  - cells.yaml\ncomponents:\n  - {id: sink, type: Sink}\nflow:\n  - [source, cut, sink]\n",
    )
    .unwrap();
    let registry = standard().unwrap();
    let first_root = dir.join("first");
    let first_store = Store::open(&first_root).unwrap();
    let mut request = RunRequest::new();
    request.seeds = &[9];
    let first = run(
        &registry,
        ModelSource::Path(&dir.join("line.yaml")),
        &request,
        &first_store,
    )
    .unwrap()
    .result;
    let id = first.runs[0].run_id.as_ref().unwrap().as_str();
    let stored = first_root.join("runs").join(id).join("model.json");
    let text = fs::read_to_string(&stored).unwrap();

    let second_root = dir.join("second");
    let second_store = Store::open(&second_root).unwrap();
    let second = run(
        &registry,
        ModelSource::Text(&text, Format::Json),
        &request,
        &second_store,
    )
    .unwrap()
    .result;
    assert_eq!(second.provenance.model_hash, first.provenance.model_hash);
    assert_eq!(second.runs[0].run_id, first.runs[0].run_id);
    assert_eq!(
        fs::read(second_root.join("runs").join(id).join("trace.takt")).unwrap(),
        fs::read(first_root.join("runs").join(id).join("trace.takt")).unwrap()
    );
    // The file of the first run's own model, as a file, loads to the same too.
    let path_run = run(
        &registry,
        ModelSource::Path(&stored),
        &request,
        &second_store,
    )
    .unwrap();
    assert!(path_run.reused);
}

// ---------------------------------------------------------------------------
// Which seed; what accuracy
// ---------------------------------------------------------------------------

/// ADR-0024 §5: a table of what draws. `Source`: a non-constant `interarrival` or two or more
/// variants. `Machine`: any non-constant time. `Buffer`, `Sink`: never. A model that draws has
/// the given seed in `seeds`; one that does not has none, and a seed is unused with a warning.
#[test]
fn what_draws_follows_the_table() {
    let exp = "{dist: exponential, mean: 1min}";
    let source = "{id: source, type: Source}".to_owned();
    let two = "{id: source, type: Source, variants: {a: 1%, b: 99%}}".to_owned();
    let cut = "{id: cut, type: Machine, cycle_time: 1min}".to_owned();
    let cases: Vec<(&str, String, String, bool)> = vec![
        ("constant everything", source.clone(), cut.clone(), false),
        (
            "constant interarrival",
            "{id: source, type: Source, interarrival: 1min, batch: 2 parts, priority: 3}"
                .to_owned(),
            cut.clone(),
            false,
        ),
        (
            "one variant",
            "{id: source, type: Source, variants: {a: 100%}}".to_owned(),
            cut.clone(),
            false,
        ),
        ("two variants", two.clone(), cut.clone(), true),
        (
            "drawn interarrival",
            format!("{{id: source, type: Source, interarrival: {exp}}}"),
            cut.clone(),
            true,
        ),
        (
            "constant failures",
            source.clone(),
            "{id: cut, type: Machine, cycle_time: 1min, mtbf: 1h, mttr: 1min}".to_owned(),
            false,
        ),
        (
            "constant setup",
            source.clone(),
            "{id: cut, type: Machine, cycle_time: 1min, setup: {per_variant: 1min}}".to_owned(),
            false,
        ),
        (
            "drawn cycle",
            source.clone(),
            format!("{{id: cut, type: Machine, cycle_time: {exp}}}"),
            true,
        ),
        (
            "drawn mtbf",
            source.clone(),
            format!("{{id: cut, type: Machine, cycle_time: 1min, mtbf: {exp}, mttr: 1min}}"),
            true,
        ),
        (
            "drawn mttr",
            source.clone(),
            format!("{{id: cut, type: Machine, cycle_time: 1min, mtbf: 1h, mttr: {exp}}}"),
            true,
        ),
        // A drawn time counts even if one variant means it is never drawn: "any non-constant
        // time".
        (
            "drawn per_variant",
            source.clone(),
            format!("{{id: cut, type: Machine, cycle_time: 1min, setup: {{per_variant: {exp}}}}}"),
            true,
        ),
    ];
    for (name, first, second, draws) in cases {
        let (_, store) = fresh_store(&format!("draws_{}", name.replace(' ', "_")));
        let text = model(
            None,
            "10min",
            &[&first, &second, "{id: sink, type: Sink}"],
            FLOW_LINE,
        );
        let mut request = RunRequest::new();
        request.seeds = &[7];
        let response = go(&text, &store, &request).unwrap();
        if draws {
            assert_eq!(response.result.provenance.seeds, [7], "{name}");
            assert_eq!(response.result.runs[0].seed, Some(7), "{name}");
            assert!(response.warnings.is_empty(), "{name}");
            let refused = go(&text, &store, &RunRequest::new()).unwrap_err();
            assert_eq!(refused.class, FailureClass::Invalid, "{name}");
        } else {
            assert!(response.result.provenance.seeds.is_empty(), "{name}");
            assert_eq!(response.result.runs[0].seed, None, "{name}");
            assert!(
                response.warnings.iter().any(
                    |w| w.type_name == "run.seed_unused" && w.path.as_deref() == Some("--seed")
                ),
                "{name}: {:?}",
                response.warnings
            );
            assert!(go(&text, &store, &RunRequest::new()).is_ok(), "{name}");
        }
    }
}

/// ADR-0024 §5 step 3: refused, exit 1, type `run.no_seed`, path `model.seed`, example
/// `--seed 42`. Never entropy. Nothing is stored.
#[test]
fn no_seed_for_a_model_that_draws_is_refused_with_the_fix() {
    let (root, store) = fresh_store("no_seed");
    let failure = go(
        &model(None, "1h", RANDOM, FLOW_LINE),
        &store,
        &RunRequest::new(),
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    let line = &failure.diagnostics[0];
    assert_eq!(line.type_name, "run.no_seed");
    assert_eq!(line.path.as_deref(), Some("model.seed"));
    assert_eq!(line.example.as_deref(), Some("--seed 42"));
    assert!(line.found.is_some() && line.allowed.is_some());
    assert_eq!(runs_in(&root), 0);
}

/// ADR-0024 §5: `--seed` takes at most 2^53 - 1, as `model.seed` does. 2^53 - 1 runs, 2^53 is
/// `Invalid`, path `--seed`.
#[test]
fn the_seed_has_a_largest_value() {
    let (_, store) = fresh_store("seed_limit");
    let text = model(None, "1min", RANDOM, FLOW_LINE);
    let mut request = RunRequest::new();
    request.seeds = &[9_007_199_254_740_991];
    let largest = go(&text, &store, &request).unwrap();
    assert_eq!(largest.result.provenance.seeds, [9_007_199_254_740_991_u64]);
    request.seeds = &[9_007_199_254_740_992];
    let failure = go(&text, &store, &request).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].path.as_deref(), Some("--seed"));
}

/// A given seed on a model that draws nothing is unused, even when it is the model's own.
/// Two different seeds name one run: "it keeps one run from looking like two". The warning
/// names the seed, also when the run is reused: what the caller asked for is still unused.
#[test]
fn a_model_that_draws_nothing_has_one_run_whatever_the_seed() {
    let (root, store) = fresh_store("unused");
    let list = &[
        "{id: source, type: Source}",
        "{id: cut, type: Machine, cycle_time: 1min}",
        "{id: sink, type: Sink}",
    ];
    let text = model(None, "1h", list, FLOW_LINE);
    let mut seven = RunRequest::new();
    seven.seeds = &[7];
    let mut eight = RunRequest::new();
    eight.seeds = &[8];
    let a = go(&text, &store, &seven).unwrap();
    let b = go(&text, &store, &eight).unwrap();
    assert!(!a.reused && b.reused);
    assert_eq!(a.result, b.result);
    assert_eq!(runs_in(&root), 1);
    for response in [&a, &b] {
        assert_eq!(response.warnings.len(), 1);
        assert_eq!(response.warnings[0].type_name, "run.seed_unused");
    }
    assert!(
        b.warnings[0]
            .found
            .as_deref()
            .is_some_and(|f| f.contains('8'))
    );

    assert_eq!(load(&root, &a.result.runs[0]).header.seed, None);

    let says = model(Some(5), "1h", list, FLOW_LINE);
    let c = go(&says, &store, &RunRequest::new()).unwrap();
    assert_eq!(c.result.provenance.seeds, [] as [u64; 0]);
    assert_eq!(c.warnings[0].path.as_deref(), Some("model.seed"));
}

/// ADR-0024 §4: every value has at least one accuracy statement. In M7, `single_replication`
/// for a run with a seed and `no_random_draws` for one with none; neither is a sampling
/// interval, and every number says so.
#[test]
fn every_value_states_what_can_be_said_of_its_error() {
    let (_, store) = fresh_store("accuracy");
    let drawn = ok(&model(Some(1), "10min", RANDOM, FLOW_LINE), &store);
    let exact = ok(
        &model(
            None,
            "10min",
            &[
                "{id: source, type: Source}",
                "{id: cut, type: Machine, cycle_time: 1min}",
                "{id: sink, type: Sink}",
            ],
            FLOW_LINE,
        ),
        &store,
    );
    for (doc, want) in [(&drawn, "single_replication"), (&exact, "no_random_draws")] {
        assert_ne!(doc.metrics, [] as [taktsim_run::Metric; 0]);
        for metric in &doc.metrics {
            for value in &metric.values {
                assert_eq!(value.accuracy.len(), 1);
                let AccuracyKind::None { reason, detail } = &value.accuracy[0].kind else {
                    panic!("{}: not `none`", metric.name);
                };
                assert_eq!(reason, want, "{}", metric.name);
                assert_ne!(detail, "");
            }
        }
    }
}

/// ADR-0024 §3: every declared observable is either among the `observables` of some
/// metric or in `unreported`, once. The text is the declared text.
///
/// M7 computed none, so all four were `unreported`. M8 answers each (ADR-0025 §2), and a
/// precision target runs until it is reached, which needs both budgets (ADR-0019 item 4,
/// ADR-0025 §4); the refusal without them is pinned in `tests/experiments.rs`. Each
/// declared text is on exactly the metric of its name, and on no other.
#[test]
fn declared_observables_are_answered_or_unreported() {
    let (_, store) = fresh_store("unreported");
    let text = "format: \"1.0\"\nmodel: {name: m, seed: 1}\n\
                experiment: {type: terminating, horizon: 1h, precision: {throughput(sink): 2%, lead_time(sink): 5%}}\n\
                components:\n  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  - {id: cut, type: Machine, cycle_time: 42s}\n  - {id: buf, type: Buffer, capacity: 5 parts}\n  - {id: sink, type: Sink}\n\
                flow:\n  - [source, cut, buf, sink]\n\
                observe:\n  - throughput(sink)\n  - wip(buf)\n  - state_shares(cut)\n";
    let request = RunRequest {
        budget: taktsim_run::Budget {
            max_runs: std::num::NonZeroU32::new(20),
            max_seconds: std::num::NonZeroU64::new(600),
        },
        ..RunRequest::new()
    };
    let doc = go(text, &store, &request).unwrap().result;
    assert_eq!(doc.unreported.len(), 0, "{:?}", doc.unreported);
    let tagged: Vec<(&str, Vec<&str>)> = doc
        .metrics
        .iter()
        .filter(|m| !m.observables.is_empty())
        .map(|m| {
            (
                m.name.as_str(),
                m.observables.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        tagged,
        [
            ("lead_time(sink)", vec!["lead_time(sink)"]),
            ("state_shares(cut)", vec!["state_shares(cut)"]),
            ("throughput(sink)", vec!["throughput(sink)"]),
            ("wip(buf)", vec!["wip(buf)"]),
        ]
    );
}

/// The windows: `0s` to the end for a count, `{at: end}` for `items_alive`.
#[test]
fn counts_cover_the_run_and_items_alive_is_a_reading() {
    let (_, store) = fresh_store("over");
    let doc = ok(&model(Some(1), "90min", RANDOM, FLOW_LINE), &store);
    for metric in &doc.metrics {
        if metric.name == "items_alive" {
            assert_eq!(
                metric.over,
                Over::At {
                    at: duration("90min")
                }
            );
        } else {
            assert_eq!(
                metric.over,
                Over::Window {
                    from: duration("0s"),
                    to: duration("90min")
                }
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Partial runs
// ---------------------------------------------------------------------------

/// A run stopped after N events is a prefix of the whole run: same seed, same streams, and
/// nothing about the limit reaches a component. So its counts are the whole trace's counts up
/// to the record before event N + 1, its records are that many, and its end is event N's time.
/// It is partial, with no id and no trace, and nothing is kept.
#[test]
fn a_limited_run_is_a_prefix_of_the_whole_run() {
    let (root, store) = fresh_store("prefix");
    let text = model(Some(42), "1h", RANDOM, FLOW_LINE);
    let whole = ok(&text, &store);
    let trace = load(&root, &whole.runs[0]);
    let limit = 25;
    assert!(whole.runs[0].events > limit);

    let event_positions: Vec<usize> = trace
        .records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, Record::Event { .. }).then_some(i))
        .collect();
    let cut = event_positions[usize::try_from(limit).unwrap()];
    let last = trace.records[event_positions[usize::try_from(limit).unwrap() - 1]].time();
    let prefix = &trace.records[..cut];
    let count = |p: fn(&Record<FlowMessage>) -> bool| prefix.iter().filter(|r| p(r)).count() as u64;
    let created = count(|r| matches!(r, Record::ItemCreated { .. }));
    let delivered = count(|r| matches!(r, Record::ItemDisposed { .. }));

    let (other_root, other) = fresh_store("prefix_partial");
    let mut request = RunRequest::new();
    request.max_events = Some(limit);
    let partial = go(&text, &other, &request).unwrap().result;

    assert!(!partial.complete);
    assert_eq!(partial.stop_reason, "event_limit");
    assert_eq!(partial.experiment_id, None);
    let entry = &partial.runs[0];
    assert!(!entry.complete);
    assert_eq!(entry.stop_reason, "event_limit");
    assert_eq!(entry.run_id, None);
    assert_eq!(entry.trace, None);
    assert_eq!(entry.seed, Some(42));
    assert_eq!(partial.provenance.seeds, [42]);
    assert_eq!(entry.events, limit);
    assert_eq!(entry.records, cut as u64);
    assert_eq!(entry.end_time.as_nanos(), last.as_nanos());
    assert_eq!(metric(&partial, "created(source)"), created);
    assert_eq!(metric(&partial, "delivered(sink)"), delivered);
    assert_eq!(metric(&partial, "items_alive"), created - delivered);
    for row in &partial.metrics {
        let want_end = entry.end_time;
        match row.over {
            Over::Window { from, to } => {
                assert_eq!(from, duration("0s"));
                assert_eq!(to, want_end);
            }
            Over::At { at } => assert_eq!(at, want_end),
        }
    }
    assert_eq!(runs_in(&other_root), 0);
    assert_eq!(tmp_entries(&other_root), 0);
}

/// `max_events`: "the most events the run processes"; core stops with `EventLimit` when "`max_events`
/// events were processed and another one is due". With a limit of exactly the events the run
/// has, it did all it was asked and no other is due before the horizon: it is complete, and it
/// is the same run. One fewer is partial.
#[test]
fn a_limit_equal_to_the_events_of_the_run_is_no_limit() {
    let text = model(Some(42), "10min", RANDOM, FLOW_LINE);
    let (_, full_store) = fresh_store("limit_full");
    let whole = ok(&text, &full_store);
    let events = whole.runs[0].events;

    let (root, store) = fresh_store("limit_equal");
    let mut request = RunRequest::new();
    request.max_events = Some(events - 1);
    let short = go(&text, &store, &request).unwrap().result;
    assert!(!short.complete);
    assert_eq!(short.runs[0].events, events - 1);
    assert_eq!(runs_in(&root), 0);

    request.max_events = Some(events);
    let exact = go(&text, &store, &request).unwrap().result;
    assert!(exact.complete, "{}", exact.stop_reason);
    assert_eq!(exact.stop_reason, "horizon");
    assert_eq!(exact.runs[0].end_time, duration("10min"));
    assert_eq!(exact.runs[0].run_id, whole.runs[0].run_id);
    assert_eq!(exact, whole);
}

/// A limit of zero asks for no event. The run is stopped before its first event, or refused as a
/// bad value; a stored id or a panic it may not give.
#[test]
fn a_limit_of_zero_is_partial_or_refused() {
    let (root, store) = fresh_store("limit_zero");
    let mut request = RunRequest::new();
    request.max_events = Some(0);
    let text = model(Some(42), "10min", RANDOM, FLOW_LINE);
    match go(&text, &store, &request) {
        Ok(response) => {
            assert!(!response.result.complete);
            assert_eq!(response.result.runs[0].events, 0);
            assert_eq!(response.result.experiment_id, None);
        }
        Err(failure) => assert_eq!(failure.class, FailureClass::Invalid),
    }
    assert_eq!(runs_in(&root), 0);
}

/// ADR-0024 §3: `Cancelled` gives `cancelled`, `complete: false`, with no id, and is not kept. A
/// flag already set stops the run before anything else.
#[test]
fn a_cancelled_run_is_partial_and_not_kept() {
    let (root, store) = fresh_store("cancel");
    let flag = AtomicBool::new(true);
    let mut request = RunRequest::new();
    request.cancel = Some(&flag);
    let doc = go(
        &model(Some(42), "10min", RANDOM, FLOW_LINE),
        &store,
        &request,
    )
    .unwrap()
    .result;
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "cancelled");
    assert_eq!(doc.runs[0].stop_reason, "cancelled");
    assert_eq!(doc.experiment_id, None);
    assert_eq!(doc.runs[0].run_id, None);
    assert_eq!(runs_in(&root), 0);
    assert_eq!(tmp_entries(&root), 0);
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// ADR-0024 §8's layout, pinned from the ADR's text.
#[test]
fn store_layout_follows_the_adr() {
    let (root, store) = fresh_store("layout");
    let doc = ok(&model(Some(1), "10min", RANDOM, FLOW_LINE), &store);
    let entry = &doc.runs[0];
    let id = entry.run_id.as_ref().unwrap().as_str();
    assert_eq!(
        fs::read_to_string(root.join("store.json")).unwrap().trim(),
        r#"{"store_format": 1}"#
    );
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap().trim(),
        "*"
    );
    let trace = entry.trace.as_ref().unwrap();
    assert_eq!(trace.path, format!("runs/{id}/trace.takt"));
    let file = stored_file(&root, &trace.path);
    let bytes = fs::read(&file).unwrap();
    assert_eq!(trace.bytes, bytes.len() as u64);
    assert_eq!(trace.trace_format, u32::from(taktsim_trace::FORMAT_VERSION));

    let result = fs::read_to_string(root.join("runs").join(id).join("result.json")).unwrap();
    assert_eq!(result, doc.to_text());
    assert!(result.ends_with("}\n") && !result.contains('\r'));

    let mut names: Vec<String> = fs::read_dir(root.join("runs").join(id))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["model.json", "result.json", "trace.takt"]);
    assert_eq!(tmp_entries(&root), 0);

    // The trace states the same thing as the result.
    let loaded = load(&root, entry);
    assert_eq!(loaded.summary.records, entry.records);
    assert_eq!(loaded.summary.events, entry.events);
    assert_eq!(loaded.summary.hash, trace.stream_hash);
    assert_eq!(loaded.summary.bytes, trace.bytes);
    assert_eq!(
        loaded.header.model_hash.to_string(),
        doc.provenance.model_hash.to_string()
    );
    assert_eq!(
        loaded.header.system_hash.to_string(),
        doc.provenance.system_hash.to_string()
    );
    let end = loaded.summary.end.expect("a stored trace states its end");
    assert_eq!(end.time.as_nanos(), entry.end_time.as_nanos());
    assert_eq!(loaded.header.engine_version, doc.provenance.engine_version);
    assert_eq!(loaded.header.record_format, trace.record_format);
    assert_eq!(loaded.header.message_format, trace.message_format);
}

/// The same request reuses without running, adds nothing, and a limit does not matter then:
/// "nothing runs, so no limit is reached". Without reuse it runs and compares, and adds nothing.
#[test]
fn reuse_and_no_reuse() {
    let (root, store) = fresh_store("reuse");
    let text = model(Some(1), "10min", RANDOM, FLOW_LINE);
    let first = go(&text, &store, &RunRequest::new()).unwrap();
    assert!(!first.reused);

    let second = go(&text, &store, &RunRequest::new()).unwrap();
    assert!(second.reused);
    assert_eq!(second.result, first.result);

    let mut limited = RunRequest::new();
    limited.max_events = Some(1);
    let third = go(&text, &store, &limited).unwrap();
    assert!(third.reused);
    assert!(third.result.complete);

    let mut again = RunRequest::new();
    again.reuse = false;
    let fourth = go(&text, &store, &again).unwrap();
    assert!(!fourth.reused);
    assert_eq!(fourth.result, first.result);
    assert_eq!(runs_in(&root), 1);
    assert_eq!(tmp_entries(&root), 0);
}

/// Change what the store says of the stream, and `--no-reuse` finds the difference: failed,
/// `run.store_differs` as `describe` lists it, and the stored run stays as it is. A run stopped
/// by a limit compares nothing and is not refused: "a shorter stream says nothing".
#[test]
fn no_reuse_detects_a_stored_difference_and_never_overwrites() {
    let (root, store) = fresh_store("no_reuse");
    let text = model(Some(1), "10min", RANDOM, FLOW_LINE);
    let doc = ok(&text, &store);
    let id = doc.runs[0].run_id.as_ref().unwrap().as_str();
    let file = root.join("runs").join(id).join("result.json");
    let hash = doc.runs[0].trace.as_ref().unwrap().stream_hash.to_string();
    let forged = fs::read_to_string(&file)
        .unwrap()
        .replace(&hash, &"0".repeat(64));
    fs::write(&file, &forged).unwrap();
    let trace_before = fs::read(root.join("runs").join(id).join("trace.takt")).unwrap();

    let mut again = RunRequest::new();
    again.reuse = false;
    let failure = go(&text, &store, &again).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.store_differs");
    assert_eq!(fs::read_to_string(&file).unwrap(), forged);
    assert_eq!(
        fs::read(root.join("runs").join(id).join("trace.takt")).unwrap(),
        trace_before
    );
    assert_eq!(tmp_entries(&root), 0);

    again.max_events = Some(1);
    let partial = go(&text, &store, &again).unwrap();
    assert!(!partial.result.complete);
    assert_eq!(fs::read_to_string(&file).unwrap(), forged);
}

/// A store whose layout this build does not know is refused (`Io`, exit 7), by `open`.
/// A store's root that is a file cannot be written: `Io` too.
#[test]
fn stores_this_build_cannot_use_are_io_failures() {
    let dir = fresh("store_refused");
    let future = dir.join("future");
    fs::create_dir_all(&future).unwrap();
    fs::write(future.join("store.json"), r#"{"store_format": 2}"#).unwrap();
    assert_eq!(Store::open(&future).unwrap_err().class, FailureClass::Io);

    let garbage = dir.join("garbage");
    fs::create_dir_all(&garbage).unwrap();
    fs::write(garbage.join("store.json"), "not json {").unwrap();
    assert_eq!(Store::open(&garbage).unwrap_err().class, FailureClass::Io);

    let file = dir.join("a_file");
    fs::write(&file, "x").unwrap();
    let text = model(Some(1), "1min", RANDOM, FLOW_LINE);
    let failure = match Store::open(&file) {
        Ok(store) => go(&text, &store, &RunRequest::new()).unwrap_err(),
        Err(failure) => failure,
    };
    assert_eq!(failure.class, FailureClass::Io);
}

/// `Store::discover`: nearest `.taktsim/` upward, like `.git`. The nearest wins over a farther
/// one.
#[test]
fn discovery_takes_the_nearest_store() {
    let top = fresh("discover");
    let middle = top.join("a");
    let below = middle.join("b");
    fs::create_dir_all(&below).unwrap();
    fs::create_dir_all(top.join(".taktsim")).unwrap();
    fs::create_dir_all(middle.join(".taktsim")).unwrap();
    assert_eq!(
        Store::discover(&below).unwrap().root(),
        middle.join(".taktsim")
    );
    assert_eq!(Store::discover(&top).unwrap().root(), top.join(".taktsim"));
}

// ---------------------------------------------------------------------------
// The result document
// ---------------------------------------------------------------------------

/// What is written reads back as the same value, and the text is stable under that.
#[test]
fn the_document_round_trips() {
    let (_, store) = fresh_store("round_trip");
    let doc = ok(&model(Some(1), "10min", RANDOM, FLOW_LINE), &store);
    let text = doc.to_text();
    let back: ResultDocument = decode_str(&text, Format::Json).unwrap();
    assert_eq!(back, doc);
    assert_eq!(back.to_text(), text);
}

/// Reader rules (ADR-0024 §3, §4): unknown keys are ignored everywhere in the document, and an
/// unknown accuracy kind reads as no statement, never as an error.
#[test]
fn a_later_document_can_be_read() {
    let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let text = format!(
        r#"{{"format": "taktsim-result/1", "complete": true, "stop_reason": "precision_reached", "later": 1,
          "provenance": {{"engine": "e", "engine_version": "1", "package": "p", "package_version": "1",
             "model_hash": "{hash}", "system_hash": "{hash}", "platform": "x", "seeds": [], "later": []}},
          "runs": [{{"complete": true, "stop_reason": "a_new_word", "end_time": "1h", "events": 0, "records": 0, "later": null}}],
          "metrics": [{{"name": "future(x)", "unit": "kg", "later": 1, "over": {{"from": "1h", "to": "2h", "later": 1}},
             "values": [{{"value": 0.5, "later": 1, "labels": {{"state": "busy"}},
                 "accuracy": [{{"kind": "discretization", "bound": 3}}]}}, {{"value": true, "accuracy": [{{"kind": "none", "reason": "r", "detail": "d", "later": 1}}]}}]}}],
          "unreported": [{{"observable": "x", "reason": "y", "later": 1}}]}}"#
    );
    let doc: ResultDocument = decode_str(&text, Format::Json).unwrap();
    assert_eq!(doc.stop_reason, "precision_reached");
    assert!(matches!(
        doc.metrics[0].values[0].accuracy[0].kind,
        AccuracyKind::Unknown { .. }
    ));
    assert_eq!(doc.metrics[0].values[1].value, Scalar::Flag(true));
    assert_eq!(doc.runs[0].stop_reason, "a_new_word");
}

/// A value's `accuracy` is required with at least one entry and `runs` needs at least one entry
/// (ADR-0024 §3, §4). A document without either is refused with a path. So are the wrong format
/// word and a missing `complete`, the field a reader decides on.
#[test]
fn a_document_breaking_the_rules_is_refused() {
    let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let good_run = r#"{"complete": true, "stop_reason": "horizon", "end_time": "1h", "events": 0, "records": 0}"#;
    let good_metric = r#"{"name": "n", "unit": "items", "over": {"at": "1h"}, "values": [{"value": 1, "accuracy": [{"kind": "none", "reason": "r", "detail": "d"}]}]}"#;
    let doc = |format: &str, complete: &str, runs: &str, metrics: &str| {
        format!(
            r#"{{"format": "{format}", {complete} "stop_reason": "horizon",
              "provenance": {{"engine": "e", "engine_version": "1", "package": "p", "package_version": "1",
                 "model_hash": "{hash}", "system_hash": "{hash}", "platform": "x", "seeds": []}},
              "runs": {runs}, "metrics": {metrics}, "unreported": []}}"#
        )
    };
    let good = doc(
        "taktsim-result/1",
        "\"complete\": true,",
        &format!("[{good_run}]"),
        &format!("[{good_metric}]"),
    );
    assert!(decode_str::<ResultDocument>(&good, Format::Json).is_ok());

    let no_accuracy = good_metric.replace(
        r#""accuracy": [{"kind": "none", "reason": "r", "detail": "d"}]"#,
        r#""accuracy": []"#,
    );
    let without = good_metric.replace(
        r#", "accuracy": [{"kind": "none", "reason": "r", "detail": "d"}]"#,
        "",
    );
    let cases = [
        (
            "metrics[0].values[0].accuracy",
            doc(
                "taktsim-result/1",
                "\"complete\": true,",
                &format!("[{good_run}]"),
                &format!("[{no_accuracy}]"),
            ),
        ),
        (
            "metrics[0].values[0].accuracy",
            doc(
                "taktsim-result/1",
                "\"complete\": true,",
                &format!("[{good_run}]"),
                &format!("[{without}]"),
            ),
        ),
        (
            "runs",
            doc(
                "taktsim-result/1",
                "\"complete\": true,",
                "[]",
                &format!("[{good_metric}]"),
            ),
        ),
        (
            "format",
            doc(
                "taktsim-result/2",
                "\"complete\": true,",
                &format!("[{good_run}]"),
                &format!("[{good_metric}]"),
            ),
        ),
        (
            "complete",
            doc(
                "taktsim-result/1",
                "",
                &format!("[{good_run}]"),
                &format!("[{good_metric}]"),
            ),
        ),
    ];
    for (path, text) in cases {
        let errors = decode_str::<ResultDocument>(&text, Format::Json)
            .err()
            .unwrap_or_else(|| panic!("{path} was accepted"));
        let paths: Vec<String> = errors.iter().map(|e| e.path().to_string()).collect();
        assert!(paths.iter().any(|p| p == path), "{path}: {paths:?}");
    }
}

/// `validate` reads only, and its hashes are those of the run (ADR-0024 §9). Invalid: errors with
/// a path and no hashes.
#[test]
fn validate_agrees_with_run_and_writes_nothing() {
    let (root, store) = fresh_store("validate");
    let registry = standard().unwrap();
    let text = model(Some(1), "10min", RANDOM, FLOW_LINE);
    let report = validate(&registry, ModelSource::Text(&text, Format::Yaml));
    assert!(report.valid);
    assert_eq!(report.errors, [] as [taktsim_run::Diagnostic; 0]);
    assert!(!root.exists(), "validate touched the store");
    let doc = ok(&text, &store);
    assert_eq!(
        report.model_hash.as_ref().unwrap().to_string(),
        doc.provenance.model_hash.to_string()
    );
    assert_eq!(
        report.system_hash.as_ref().unwrap().to_string(),
        doc.provenance.system_hash.to_string()
    );

    let bad = text.replace("mean: 40s", "mean: 40");
    let report = validate(&registry, ModelSource::Text(&bad, Format::Yaml));
    assert!(!report.valid);
    assert_eq!(report.model_hash, None);
    assert_eq!(report.system_hash, None);
    assert_eq!(
        report.errors[0].path.as_deref(),
        Some("components[0].interarrival.mean")
    );
}

/// A file that cannot be read is an error report as well: an I/O error, with no example, since
/// there is no valid value of a missing file (ADR-0024 §9).
#[test]
fn validate_of_a_missing_file_says_it_cannot_be_read() {
    let dir = fresh("validate_missing");
    let report = validate(
        &standard().unwrap(),
        ModelSource::Path(&dir.join("nothing.yaml")),
    );
    assert!(!report.valid);
    assert_eq!(report.errors[0].type_name, "model.io");
    assert_eq!(report.errors[0].example, None);
}

// ---------------------------------------------------------------------------
// What the standard catalog cannot do: types added from outside
// ---------------------------------------------------------------------------

/// A domain with no rules of its own, so a test can have components with no Source.
struct Lax;

impl Domain for Lax {
    type Message = FlowMessage;
    type Shared = ();
    const PACKAGE: &'static str = "lax";
    const PACKAGE_VERSION: &'static str = "1.0.0";

    fn check(_: &Document, _: &Roles, _: &mut Findings) {}

    fn read_message(_: &FlowMessage, _: &mut dyn FnMut(taktsim_components::MessageFact<'_>)) {}

    fn prepare(_: &Document, _: &Roles) -> Result<(), BuildError> {
        Ok(())
    }

    fn draws_random(_: &Document) -> bool {
        false
    }
}

macro_rules! no_params {
    ($name:ident, $word:literal) => {
        #[derive(Debug)]
        struct $name;

        impl Encode for $name {
            fn encode(&self) -> Json {
                Json::object([])
            }
        }

        impl ComponentParams for $name {
            const TYPE: &'static str = $word;
            const EXAMPLE: &'static str = "{id: x}";

            fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
                Some(Self)
            }
        }
    };
}

/// At 5 s creates an item, which it keeps. No other event: a stall.
#[derive(Default)]
struct HoardBehaviour {
    held: Vec<ItemId>,
}

impl Component<FlowMessage> for HoardBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        ctx.schedule(SimDuration::from_secs(5)?, FlowMessage::NextArrival)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        self.held.push(ctx.create_item());
        Ok(())
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        self.held.iter().copied().for_each(visit);
    }
}

no_params!(Hoard, "Hoard");

impl ComponentType<Lax> for Hoard {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::InSystem;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(HoardBehaviour::default()))
    }
}

/// At 5 s creates three items and disposes of them in the three ways. No other event: it drains.
struct SpendBehaviour;

impl Component<FlowMessage> for SpendBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        ctx.schedule(SimDuration::from_secs(5)?, FlowMessage::NextArrival)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        for how in [Disposal::Delivered, Disposal::Scrapped, Disposal::Consumed] {
            let item = ctx.create_item();
            ctx.dispose(item, how)?;
        }
        Ok(())
    }
}

no_params!(Spend, "Spend");

impl ComponentType<Lax> for Spend {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::InSystem;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(SpendBehaviour))
    }
}

/// Says it draws nothing, and draws its timers.
struct LiarBehaviour {
    stream: Option<taktsim_core::random::Stream>,
}

impl LiarBehaviour {
    fn arm(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(());
        };
        let nanos = 1_000_000_000 + (stream.next_u64() % 1_000_000_000);
        ctx.schedule(SimDuration::from_nanos(nanos), FlowMessage::NextArrival)?;
        Ok(())
    }
}

impl Component<FlowMessage> for LiarBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.stream = Some(ctx.stream("lie")?);
        self.arm(ctx)
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        self.arm(ctx)
    }
}

no_params!(Liar, "Liar");

impl ComponentType<Lax> for Liar {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(LiarBehaviour { stream: None }))
    }
}

/// Does nothing, and says it draws nothing: a neighbour, so a model has its one flow entry.
struct QuietBehaviour;

impl Component<FlowMessage> for QuietBehaviour {
    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

no_params!(Quiet, "Quiet");

impl ComponentType<Lax> for Quiet {
    const PORTS: Ports = Ports::Both;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(QuietBehaviour))
    }
}

/// Answers nothing about its draws: the default says it draws.
struct SilentBehaviour;

impl Component<FlowMessage> for SilentBehaviour {
    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

no_params!(Silent, "Silent");

impl ComponentType<Lax> for Silent {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(SilentBehaviour))
    }
}

/// A domain that draws for what no component states, like a split on an edge (ADR-0024 §5): its
/// `draws_random` answers yes whatever the components say.
struct Routed;

impl Domain for Routed {
    type Message = FlowMessage;
    type Shared = ();
    const PACKAGE: &'static str = "routed";
    const PACKAGE_VERSION: &'static str = "1.0.0";

    fn check(_: &Document, _: &Roles, _: &mut Findings) {}

    fn read_message(_: &FlowMessage, _: &mut dyn FnMut(taktsim_components::MessageFact<'_>)) {}

    fn prepare(_: &Document, _: &Roles) -> Result<(), BuildError> {
        Ok(())
    }

    fn draws_random(_: &Document) -> bool {
        true
    }
}

impl ComponentType<Routed> for Quiet {
    const PORTS: Ports = Ports::Both;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(QuietBehaviour))
    }
}

/// The domain's half of the no-draw claim: a model whose components all draw nothing still draws when
/// its domain does, so it needs a seed and is refused without one at `model.seed`, at load, never
/// labelled exact.
#[test]
fn a_domain_that_draws_makes_a_model_of_constant_components_draw() {
    let registry = Registry::<Routed>::new().with::<Quiet>().unwrap();
    let text = |head: &str| {
        format!(
            "format: \"1.0\"\nmodel: {{name: m{head}}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
             components:\n  - {{id: a, type: Quiet}}\n  - {{id: b, type: Quiet}}\nflow: [[a, b]]\n"
        )
    };
    let model = taktsim_model::Loader::new(registry.catalog())
        .load_str(&text(""), Format::Yaml)
        .unwrap();
    assert!(registry.draws_random(model.document()));

    let refused = registry.prepare(&model, taktsim_components::RunOptions::new().no_seed());
    let Err(taktsim_components::RunFailure::DrawsRandom { path }) = refused else {
        panic!("a domain that draws ran with no seed: {refused:?}");
    };
    assert_eq!(path.to_string(), "model.seed");
    assert!(
        registry
            .prepare(&model, taktsim_components::RunOptions::new().seed(1))
            .is_ok()
    );

    let (_, store) = fresh_store("domain_draws");
    let failure = run(
        &registry,
        ModelSource::Text(&text(""), Format::Yaml),
        &RunRequest::new(),
        &store,
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].type_name, "run.no_seed");
    assert_eq!(failure.diagnostics[0].path.as_deref(), Some("model.seed"));
    let seeded = run(
        &registry,
        ModelSource::Text(&text(", seed: 1"), Format::Yaml),
        &RunRequest::new(),
        &store,
    )
    .unwrap();
    assert_eq!(seeded.result.provenance.seeds, [1]);
    assert_eq!(seeded.result.provenance.package, "routed");
}

fn lax() -> Registry<Lax> {
    Registry::<Lax>::new()
        .with::<Hoard>()
        .unwrap()
        .with::<Spend>()
        .unwrap()
        .with::<Liar>()
        .unwrap()
        .with::<Silent>()
        .unwrap()
        .with::<Quiet>()
        .unwrap()
}

fn one_of(kind: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: m}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n  - {{id: x, type: {kind}}}\n  - {{id: a, type: Quiet}}\n  - {{id: b, type: Quiet}}\nflow: [[a, b]]\n"
    )
}

fn lax_run(kind: &str, store: &Store) -> Result<RunResponse, Failure> {
    let text = one_of(kind);
    run(
        &lax(),
        ModelSource::Text(&text, Format::Yaml),
        &RunRequest::new(),
        store,
    )
}

/// ADR-0024 §3's table: `Idle` with no item alive is `drained`, complete, exit 0. The clock jumps to
/// the horizon (`RunOutcome::Idle`), so the numbers hold to 1 h.
///
/// Spend makes three items at 5 s, one disposed of each way. So `created(x)` = 3, one each of
/// `delivered(x)`, `scrapped(x)`, `consumed(x)`, none alive: "keyed on each record's `by`", and
/// `Ports` "never decides who may create". Provenance names the domain, not `taktsim-components`.
#[test]
fn drained_is_complete_and_every_disposal_has_its_row() {
    let (root, store) = fresh_store("drained");
    let doc = lax_run("Spend", &store).unwrap().result;
    assert!(doc.complete);
    assert_eq!(doc.stop_reason, "drained");
    assert_eq!(doc.runs[0].stop_reason, "drained");
    assert_eq!(doc.runs[0].end_time, duration("1h"));
    assert_eq!(metric(&doc, "created(x)"), 3);
    assert_eq!(metric(&doc, "delivered(x)"), 1);
    assert_eq!(metric(&doc, "scrapped(x)"), 1);
    assert_eq!(metric(&doc, "consumed(x)"), 1);
    assert_eq!(metric(&doc, "items_alive"), 0);
    assert_eq!(doc.provenance.package, "lax");
    assert_eq!(doc.provenance.package_version, "1.0.0");
    assert_eq!(doc.runs[0].events, 1);
    assert_eq!(runs_in(&root), 1);
}

/// The package is in the run id (ADR-0024 §6): the Spend model through `Lax` gives an id that
/// follows the same definition, with `"package":"lax"`.
#[test]
fn the_domain_package_is_in_the_run_id() {
    let (_, store) = fresh_store("package_id");
    let doc = lax_run("Spend", &store).unwrap().result;
    let p = &doc.provenance;
    let trace = doc.runs[0].trace.as_ref().unwrap();
    let canonical = format!(
        r#"{{"engine":"taktsim-core","engine_version":"{}","message_format":"{}","model_hash":"{}","package":"lax","package_version":"1.0.0","record_format":"{}","run_id_format":1,"seed":null}}"#,
        p.engine_version, trace.message_format, p.model_hash, trace.record_format,
    );
    assert_eq!(
        doc.runs[0].run_id.as_ref().unwrap().as_str(),
        format!("taktsim-core-{}", sha256_hex(canonical))
    );
}

/// ADR-0024 §3's table: `Idle` with items alive is no result but error `run.stalled`, exit 2, and
/// nothing is kept. "The error gives the time of the last event, `items_alive` and `recent`,
/// read from the temporary trace." The last event is at 5 s, though the clock is at the horizon;
/// one item is alive; and the last lines show what happened last. No example: exit 2 has none.
#[test]
fn a_stall_is_a_failure_with_when_and_what() {
    let (root, store) = fresh_store("stall");
    let failure = lax_run("Hoard", &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let line = &failure.diagnostics[0];
    assert_eq!(line.type_name, "run.stalled");
    assert_eq!(line.time, Some(duration("5s")));
    assert_eq!(line.items_alive, Some(1));
    assert_eq!(line.example, None);
    assert!(
        line.recent.iter().any(|r| r.contains("5s")),
        "{:?}",
        line.recent
    );
    assert_eq!(runs_in(&root), 0);
    assert_eq!(tmp_entries(&root), 0);
}

/// ADR-0024 §5: a run claiming to draw nothing is replayed before it is stored, with another
/// internal seed. A type whose claim is false diverges: exit 2, `run.no_draw_claim_false`, nothing
/// stored.
#[test]
fn a_false_claim_of_drawing_nothing_is_caught_and_not_kept() {
    let (root, store) = fresh_store("liar");
    let failure = lax_run("Liar", &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.no_draw_claim_false");
    assert_eq!(failure.diagnostics[0].example, None);
    assert_eq!(runs_in(&root), 0);
    assert_eq!(tmp_entries(&root), 0);
}

/// A type that does not say is taken to draw (ADR-0024 §5: "default `true`, so an unknown type
/// must declare otherwise"): with no seed it is refused.
#[test]
fn a_type_that_does_not_answer_is_taken_to_draw() {
    let (_, store) = fresh_store("silent");
    let failure = lax_run("Silent", &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].type_name, "run.no_seed");
}

/// A run with nothing created and nothing happening at all: `Idle` with no item. Drained, no
/// items, the rows of no component, only `items_alive`.
#[test]
fn a_run_with_no_event_at_all_is_drained() {
    let (_, store) = fresh_store("nothing");
    let text = one_of("Silent");
    let mut request = RunRequest::new();
    request.seeds = &[1];
    let doc = run(
        &lax(),
        ModelSource::Text(&text, Format::Yaml),
        &request,
        &store,
    )
    .unwrap()
    .result;
    assert!(doc.complete);
    assert_eq!(doc.stop_reason, "drained");
    assert_eq!(doc.runs[0].events, 0);
    assert_eq!(names(&doc), ["items_alive"]);
    assert_eq!(metric(&doc, "items_alive"), 0);
}

/// Ten replays of the drained run keep agreeing with the stored one, through a domain that is
/// not `Flow`, and a mismatch in the domain's version is seen by `replay_stored`.
#[test]
fn replay_works_for_any_domain() {
    let (_, store) = fresh_store("replay_lax");
    let doc = lax_run("Spend", &store).unwrap().result;
    let id = doc.runs[0].run_id.as_ref().unwrap();
    let summary = replay_stored(&lax(), &store, id).unwrap();
    assert_eq!(summary.events, 1);
    let wrong = replay_stored(&standard().unwrap(), &store, id).unwrap_err();
    assert_ne!(wrong.class, FailureClass::Invalid);
}

/// The helper is checked against SHA-256's published test vector, so a mistake in it cannot hide
/// behind its own use.
#[test]
fn the_helper_hash_agrees_with_a_known_value() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

// ---------------------------------------------------------------------------
// Failed runs: exit 2, with when and what came last
// ---------------------------------------------------------------------------

/// At 5 s, after a first event at 3 s, returns an error from its handler.
struct BreaksBehaviour {
    events: u32,
}

impl Component<FlowMessage> for BreaksBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        ctx.schedule(SimDuration::from_secs(3)?, FlowMessage::NextArrival)?;
        ctx.schedule(SimDuration::from_secs(5)?, FlowMessage::NextArrival)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        self.events += 1;
        if self.events == 2 {
            return Err(ComponentError::Unexpected {
                state: "broken",
                event: "NextArrival".into(),
            });
        }
        Ok(())
    }
}

no_params!(Breaks, "Breaks");

impl ComponentType<Lax> for Breaks {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(BreaksBehaviour { events: 0 }))
    }
}

/// Breaks its own invariant at its event at 5 s.
struct ViolatesBehaviour {
    broken: bool,
}

impl Component<FlowMessage> for ViolatesBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        ctx.schedule(SimDuration::from_secs(5)?, FlowMessage::NextArrival)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        self.broken = true;
        Ok(())
    }

    fn check_invariants(&self) -> Result<(), taktsim_core::Violation> {
        if self.broken {
            return Err(taktsim_core::Violation {
                invariant: "stays whole",
                detail: "it broke".into(),
            });
        }
        Ok(())
    }
}

no_params!(Violates, "Violates");

impl ComponentType<Lax> for Violates {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(ViolatesBehaviour { broken: false }))
    }
}

/// Makes an item at 5 s and does not say it holds it: conservation breaks (spec §4).
struct LosesBehaviour;

impl Component<FlowMessage> for LosesBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        ctx.schedule(SimDuration::from_secs(5)?, FlowMessage::NextArrival)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let _lost = ctx.create_item();
        Ok(())
    }
}

no_params!(Loses, "Loses");

impl ComponentType<Lax> for Loses {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::InSystem;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(LosesBehaviour))
    }
}

/// Cannot be built: exit 1, since nothing ran.
#[derive(Debug)]
struct Unbuildable;

impl Encode for Unbuildable {
    fn encode(&self) -> Json {
        Json::object([])
    }
}

impl ComponentParams for Unbuildable {
    const TYPE: &'static str = "Unbuildable";
    const EXAMPLE: &'static str = "{id: x, type: Unbuildable}";

    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

impl ComponentType<Lax> for Unbuildable {
    const PORTS: Ports = Ports::None;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        wiring: &Wiring<'_, ()>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Err(BuildError::UnknownType {
            path: wiring.path().clone(),
            type_name: "Unbuildable".into(),
            registered: Vec::new(),
        })
    }
}

/// Has outlets and does nothing: a zero row all the same.
struct IdleBehaviour;

impl Component<FlowMessage> for IdleBehaviour {
    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

no_params!(Out, "Out");
no_params!(In, "In");

impl ComponentType<Lax> for Out {
    const PORTS: Ports = Ports::Outlet;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(IdleBehaviour))
    }
}

impl ComponentType<Lax> for In {
    const PORTS: Ports = Ports::Inlet;
    const STATES: &'static [&'static str] = &[];
    const CREATION: taktsim_components::Creation = taktsim_components::Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn draws_random(&self) -> bool {
        false
    }

    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(IdleBehaviour))
    }
}

fn lax_more() -> Registry<Lax> {
    lax()
        .with::<Breaks>()
        .unwrap()
        .with::<Violates>()
        .unwrap()
        .with::<Loses>()
        .unwrap()
        .with::<Unbuildable>()
        .unwrap()
        .with::<Out>()
        .unwrap()
        .with::<In>()
        .unwrap()
}

/// ADR-0024 §9: exit 2 carries `path` of `components[i]` where a component is known, the simulation
/// `time`, `recent`, the last events, and no `example`. §8: a failed run is not kept. `x` is the first
/// entry, so `components[0]`. Each of these fails at its event at 5 s.
#[test]
fn a_failed_run_says_where_when_and_what_came_last() {
    for (kind, word) in [
        ("Breaks", "run.component_failed"),
        ("Violates", "run.invariant_violated"),
        ("Loses", "run.conservation_violated"),
    ] {
        let (root, store) = fresh_store(&format!("failed_{kind}"));
        let text = one_of(kind);
        let failure = run(
            &lax_more(),
            ModelSource::Text(&text, Format::Yaml),
            &RunRequest::new(),
            &store,
        )
        .unwrap_err();
        assert_eq!(failure.class, FailureClass::RunFailed, "{kind}");
        let line = &failure.diagnostics[0];
        assert_eq!(line.type_name, word, "{kind}");
        assert_eq!(line.time, Some(duration("5s")), "{kind}: {line:?}");
        assert_eq!(
            line.path.as_deref(),
            Some("components[0]"),
            "{kind}: {line:?}"
        );
        assert!(!line.recent.is_empty(), "{kind}: {line:?}");
        assert!(
            line.recent.last().is_some_and(|l| l.contains("5s")),
            "{kind}: the last event is at 5 s: {:?}",
            line.recent
        );
        assert_eq!(line.example, None, "{kind}");
        assert_eq!(runs_in(&root), 0, "{kind}");
        assert_eq!(tmp_entries(&root), 0, "{kind}");
    }
}

/// A component that cannot be built is `Invalid`, not a failed run: nothing ran (`describe` lists
/// `run.build` at exit 1). It says which entry.
#[test]
fn a_component_that_cannot_be_built_is_invalid_at_its_path() {
    let (root, store) = fresh_store("unbuildable");
    let text = one_of("Unbuildable");
    let failure = run(
        &lax_more(),
        ModelSource::Text(&text, Format::Yaml),
        &RunRequest::new(),
        &store,
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].type_name, "run.build");
    assert_eq!(
        failure.diagnostics[0].path.as_deref(),
        Some("components[0]")
    );
    assert_eq!(runs_in(&root), 0);
}

/// ADR-0024 §3: "always one with `Ports::Outlet`", "always one with `Ports::Inlet`": a type with those
/// ports that does nothing has its zero row, whatever its name; `Ports::None` and `Ports::Both` have
/// none (`a`, `p`, `q`).
#[test]
fn ports_decide_which_zero_rows_are_shown() {
    let (_, store) = fresh_store("ports_rows");
    let text = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                components:\n  - {id: o, type: Out}\n  - {id: a, type: Quiet}\n  - {id: i, type: In}\n  - {id: p, type: Quiet}\n  - {id: q, type: Quiet}\n\
                flow: [[o, a, i], [p, q]]\n";
    let doc = run(
        &lax_more(),
        ModelSource::Text(text, Format::Yaml),
        &RunRequest::new(),
        &store,
    )
    .unwrap()
    .result;
    assert_eq!(metric(&doc, "created(o)"), 0);
    assert_eq!(metric(&doc, "delivered(i)"), 0);
    let mut all = names(&doc);
    all.sort();
    assert_eq!(all, ["created(o)", "delivered(i)", "items_alive"]);
}

/// A limit of exactly what the drained run has: it drained, and that is complete. One event, limit 1.
#[test]
fn a_limit_equal_to_the_events_of_a_drained_run_is_no_limit() {
    let (_, store) = fresh_store("limit_drained");
    let text = one_of("Spend");
    let mut request = RunRequest::new();
    request.max_events = Some(1);
    let doc = run(
        &lax(),
        ModelSource::Text(&text, Format::Yaml),
        &request,
        &store,
    )
    .unwrap()
    .result;
    assert!(doc.complete, "{}", doc.stop_reason);
    assert_eq!(doc.stop_reason, "drained");
}

// ---------------------------------------------------------------------------
// More of the store's rules
// ---------------------------------------------------------------------------

/// `replay_stored`: "`RunFailed` if the stored files disagree". The result names what the trace is, so
/// a result whose hash, seed or model hash is not the trace's disagrees, even when the trace itself is
/// sound and replays.
#[test]
fn replay_refuses_a_result_that_disagrees_with_its_trace() {
    let (root, store, id) = stored_run("replay_disagree");
    let file = root.join("runs").join(id.as_str()).join("result.json");
    let text = fs::read_to_string(&file).unwrap();
    let doc: ResultDocument = decode_str(&text, Format::Json).unwrap();
    let stream = doc.runs[0].trace.as_ref().unwrap().stream_hash.to_string();
    let hash = doc.provenance.model_hash.to_string();
    let system = doc.provenance.system_hash.to_string();
    let cases = [
        ("stream hash", text.replace(&stream, &"0".repeat(64))),
        ("model hash", text.replace(&hash, &"0".repeat(64))),
        ("system hash", text.replace(&system, &"0".repeat(64))),
        (
            "seeds",
            text.replace("\"seed\": 42", "\"seed\": 43").replace(
                "\"seeds\": [\n      42\n    ]",
                "\"seeds\": [\n      43\n    ]",
            ),
        ),
    ];
    let mut passed = Vec::new();
    for (name, forged) in cases {
        assert_ne!(forged, text, "{name}");
        fs::write(&file, &forged).unwrap();
        let outcome = replay_stored(&standard().unwrap(), &store, &id);
        fs::write(&file, &text).unwrap();
        match outcome {
            Ok(_) => passed.push(name),
            Err(failure) => assert_eq!(failure.class, FailureClass::RunFailed, "{name}"),
        }
    }
    assert!(replay_stored(&standard().unwrap(), &store, &id).is_ok());
    assert!(passed.is_empty(), "replay passed with a forged {passed:?}");
}

/// An id is "content-addressed" (spec §5). A stored run filed under another run's id disagrees with
/// itself: the id can be recomputed from its result, so replay refuses it.
#[test]
fn replay_refuses_a_run_filed_under_another_id() {
    let (root, store, id) = stored_run("replay_wrong_id");
    let other: RunId = format!("taktsim-core-{}", "a".repeat(64)).parse().unwrap();
    fs::rename(
        root.join("runs").join(id.as_str()),
        root.join("runs").join(other.as_str()),
    )
    .unwrap();
    let failure = replay_stored(&standard().unwrap(), &store, &other).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
}

/// K2's table: "Same file, same flag: same id". A flag saying what the file says is the same run.
#[test]
fn a_seed_option_equal_to_the_model_seed_is_the_same_run() {
    let (_, store) = fresh_store("same_seed");
    let text = model(Some(42), "10min", RANDOM, FLOW_LINE);
    let plain = go(&text, &store, &RunRequest::new()).unwrap();
    let mut request = RunRequest::new();
    request.seeds = &[42];
    let flagged = go(&text, &store, &request).unwrap();
    assert!(flagged.reused);
    assert_eq!(plain.result, flagged.result);
}

/// Seed 0 is a seed like any other: it runs, and it is not "no seed": the header and `seeds` say 0.
/// Its id differs from a run without one, whose JCS has `null` (ADR-0024 §7: "seeds 0 and 7 would look
/// like two runs" is what H1 was rejected for).
#[test]
fn seed_zero_is_a_seed() {
    let (root, store) = fresh_store("seed_zero");
    let text = model(Some(0), "10min", RANDOM, FLOW_LINE);
    let doc = ok(&text, &store);
    assert_eq!(doc.provenance.seeds, [0]);
    assert_eq!(doc.runs[0].seed, Some(0));
    assert_eq!(load(&root, &doc.runs[0]).header.seed, Some(0));
    let p = &doc.provenance;
    let trace = doc.runs[0].trace.as_ref().unwrap();
    let canonical = format!(
        r#"{{"engine":"{}","engine_version":"{}","message_format":"{}","model_hash":"{}","package":"{}","package_version":"{}","record_format":"{}","run_id_format":1,"seed":0}}"#,
        p.engine,
        p.engine_version,
        trace.message_format,
        p.model_hash,
        p.package,
        p.package_version,
        trace.record_format,
    );
    assert_eq!(
        doc.runs[0].run_id.as_ref().unwrap().as_str(),
        format!("taktsim-core-{}", sha256_hex(canonical))
    );
}

/// The system hash stays when only what is asked of the system changes: the model's name, its seed, the
/// horizon, `observe`. The model hash moves with each. A change to a component moves both.
#[test]
fn the_system_hash_ignores_what_is_asked_of_the_system() {
    let registry = standard().unwrap();
    let base = model(Some(1), "1h", RANDOM, FLOW_LINE);
    let hashes = |text: &str| {
        let report = validate(&registry, ModelSource::Text(text, Format::Yaml));
        assert!(report.valid, "{:?}", report.errors);
        (
            report.model_hash.unwrap().to_string(),
            report.system_hash.unwrap().to_string(),
        )
    };
    let (model_hash, system_hash) = hashes(&base);
    for asked in [
        base.replace("name: m", "name: other"),
        base.replace("seed: 1", "seed: 2"),
        base.replace("horizon: 1h", "horizon: 2h"),
        format!("{base}observe:\n  - throughput(sink)\n"),
    ] {
        let (a, b) = hashes(&asked);
        assert_ne!(a, model_hash, "{asked}");
        assert_eq!(b, system_hash, "{asked}");
    }
    let (a, b) = hashes(&base.replace("mean: 40s", "mean: 41s"));
    assert_ne!(a, model_hash);
    assert_ne!(b, system_hash);
}

/// §8: the stored id's path "is 93 characters below the store": `runs/` 5, the id 13 + 64 = 77,
/// `/trace.takt` 11. Windows' limit depends on it.
#[test]
fn the_trace_path_is_93_characters() {
    let (_, store) = fresh_store("path_length");
    let doc = ok(&model(Some(1), "1min", RANDOM, FLOW_LINE), &store);
    assert_eq!(doc.runs[0].trace.as_ref().unwrap().path.len(), 93);
}

/// An experiment id is over the sorted ids: two orders, one id, and the definition by hand.
#[test]
fn an_experiment_id_is_over_sorted_ids() {
    let a: RunId = format!("taktsim-core-{}", "a".repeat(64)).parse().unwrap();
    let b: RunId = format!("taktsim-core-{}", "b".repeat(64)).parse().unwrap();
    let one = taktsim_run::ExperimentId::over([a.clone(), b.clone()]).unwrap();
    let two = taktsim_run::ExperimentId::over([b.clone(), a.clone()]).unwrap();
    assert_eq!(one, two);
    let canonical = format!(r#"{{"experiment_id_format":1,"runs":["{a}","{b}"]}}"#);
    assert_eq!(one.as_str(), format!("exp-{}", sha256_hex(canonical)));
}

/// An event exactly at the horizon happens (the doc comment of `Registry`: cycle 1 min, 1 h: 60
/// delivered). The 60th part leaves at 3 600, the machine takes the 61st, made then: 61 created, one
/// alive at the end.
#[test]
fn an_event_at_the_horizon_counts() {
    let (_, store) = fresh_store("at_horizon");
    let doc = ok(
        &model(
            None,
            "1h",
            &[
                "{id: source, type: Source}",
                "{id: cut, type: Machine, cycle_time: 1min}",
                "{id: sink, type: Sink}",
            ],
            FLOW_LINE,
        ),
        &store,
    );
    assert_eq!(metric(&doc, "delivered(sink)"), 60);
    assert_eq!(metric(&doc, "created(source)"), 61);
    assert_eq!(metric(&doc, "items_alive"), 1);
}

/// A horizon with a fraction, or in other units, comes back as the same time in a result that reads
/// back: the text in `end_time` and `over` must parse, and be that time.
#[test]
fn horizons_of_every_form_come_back_exact() {
    // The horizon, the time between parts, the horizon in ns and the parts by hand: each part
    // leaves at once, so floor(horizon / interval).
    for (name, text, every, nanos, parts) in [
        ("half", "0.5h", "1min", 1_800 * NS, 30),
        ("fraction", "90.5s", "10s", 90 * NS + NS / 2, 9),
        ("ms", "1500ms", "100ms", 1_500_000_000, 15),
        ("day", "1d", "1h", 86_400 * NS, 24),
        ("space", "\"2 min\"", "7s", 120 * NS, 17),
    ] {
        let (_, store) = fresh_store(&format!("horizon_{name}"));
        let source = format!("{{id: source, type: Source, interarrival: {every}}}");
        let doc = ok(
            &model(
                None,
                text,
                &[source.as_str(), "{id: sink, type: Sink}"],
                "  - [source, sink]",
            ),
            &store,
        );
        assert_eq!(doc.runs[0].end_time.as_nanos(), nanos, "{name}");
        let Over::At { at } = doc
            .metrics
            .iter()
            .find(|m| m.name == "items_alive")
            .unwrap()
            .over
        else {
            panic!("items_alive is a reading");
        };
        assert_eq!(at.as_nanos(), nanos, "{name}");
        assert_eq!(metric(&doc, "delivered(sink)"), parts, "{name}");
        let back: ResultDocument = decode_str(&doc.to_text(), Format::Json).unwrap();
        assert_eq!(back, doc, "{name}");
    }
}

/// ADR-0024 §3, §5: a cut-off run that claims to draw nothing has had no check run, and says what any
/// partial says; a limit stops it wherever it is.
#[test]
fn a_limited_run_with_no_seed_is_partial_like_any_other() {
    let (root, store) = fresh_store("limit_no_seed");
    let mut request = RunRequest::new();
    request.max_events = Some(4);
    let doc = go(
        &model(
            None,
            "1h",
            &[
                "{id: source, type: Source}",
                "{id: cut, type: Machine, cycle_time: 1min}",
                "{id: sink, type: Sink}",
            ],
            FLOW_LINE,
        ),
        &store,
        &request,
    )
    .unwrap()
    .result;
    assert!(!doc.complete);
    assert_eq!(doc.runs[0].seed, None);
    assert_eq!(doc.provenance.seeds, [] as [u64; 0]);
    assert_eq!(doc.runs[0].events, 4);
    assert_eq!(doc.experiment_id, None);
    assert_eq!(runs_in(&root), 0);
}

/// The bytes of trace written so far by a run in progress below `root/tmp/`.
fn trace_bytes_in_progress(root: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(root.join("tmp")) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| fs::metadata(entry.path().join("trace.takt")).ok())
        .map(|meta| meta.len())
        .max()
        .unwrap_or(0)
}

/// Cancelling while it runs: the flag stops a long run, which would otherwise take far longer than this
/// test does. It is partial, not kept, and the temporary files are gone. The flag is set once the run
/// has written events to its trace, not after a fixed sleep: on a slow or loaded machine the work before
/// the first event can take longer than any sleep, and a flag set before then stops the run at 0 events.
#[test]
fn cancelling_from_outside_stops_a_running_run() {
    let (root, store) = fresh_store("cancel_running");
    let text = model(
        Some(1),
        "100000h",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: {dist: exponential, mean: 1s}}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let flag = AtomicBool::new(false);
    let doc = std::thread::scope(|scope| {
        scope.spawn(|| {
            // Far more than a header: buffered records have reached the file. A run that never
            // writes is stopped after a minute, and fails below.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while trace_bytes_in_progress(&root) < 64 * 1024 && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let mut request = RunRequest::new();
        request.cancel = Some(&flag);
        go(&text, &store, &request).unwrap().result
    });
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "cancelled");
    assert!(doc.runs[0].events > 0);
    assert_eq!(runs_in(&root), 0);
    assert_eq!(tmp_entries(&root), 0);
}

/// A model held in memory has no directory to include from: refused with a path and never a panic, and
/// the file a model names is not read from the current directory (`ModelSource::Text`).
#[test]
fn text_with_an_include_has_nothing_to_read_from() {
    let (_, store) = fresh_store("text_include");
    let text = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                include:\n  - Cargo.toml\ncomponents:\n  - {id: source, type: Source}\n  - {id: sink, type: Sink}\nflow:\n  - [source, sink]\n";
    let failure = go(text, &store, &RunRequest::new()).unwrap_err();
    assert_ne!(failure.class, FailureClass::RunFailed);
    assert!(
        failure.diagnostics[0]
            .path
            .as_deref()
            .is_some_and(|p| p.starts_with("include[0]")),
        "{:?}",
        failure.diagnostics[0]
    );
}

/// `chain_failures.yaml`, ADR-0024 §11: "with failures and a capacity-limited buffer that does fill". The
/// buffer, counted from the trace's `Arrive`s, reaches its capacity and never exceeds it; the machine goes
/// down at least once. `chain.yaml` is Source, Machine, Buffer, Machine, Sink. Replay passes.
#[test]
fn the_reference_models_are_what_the_adr_says() {
    let models = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("benchmarks")
        .join("models");
    let registry = standard().unwrap();

    let (chain_root, chain_store) = fresh_store("reference_chain");
    let chain = run(
        &registry,
        ModelSource::Path(&models.join("chain.yaml")),
        &RunRequest::new(),
        &chain_store,
    )
    .unwrap()
    .result;
    let trace = load(&chain_root, &chain.runs[0]);
    assert_eq!(trace.header.components.len(), 5);
    assert!(metric(&chain, "delivered(sink)") > 0);

    let (root, store) = fresh_store("reference_failures");
    let doc = run(
        &registry,
        ModelSource::Path(&models.join("chain_failures.yaml")),
        &RunRequest::new(),
        &store,
    )
    .unwrap()
    .result;
    let trace = load(&root, &doc.runs[0]);
    let id = doc.runs[0].run_id.as_ref().unwrap();
    assert!(replay_stored(&registry, &store, id).is_ok());

    let text = fs::read_to_string(root.join("runs").join(id.as_str()).join("model.json")).unwrap();
    let json = parse(&text, Format::Json).unwrap().to_json();
    let components = json["components"].as_array().unwrap();
    let buffer = components.iter().find(|c| c["type"] == "Buffer").unwrap();
    let name = buffer["id"].as_str().unwrap();
    let capacity: u64 = buffer["capacity"]
        .as_str()
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let me = index_of(&trace.header, name);
    let (mut fill, mut most) = (0_u64, 0_u64);
    let mut failures = 0;
    for record in &trace.records {
        if let Record::Event {
            target,
            from,
            message,
            ..
        } = record
        {
            if matches!(message, FlowMessage::Failure) {
                failures += 1;
            }
            if matches!(message, FlowMessage::Arrive(_)) {
                if target.index() == me {
                    fill += 1;
                    most = most.max(fill);
                }
                if from.index() == me {
                    fill -= 1;
                }
            }
        }
    }
    assert_eq!(most, capacity, "the buffer fills, and no further");
    assert!(failures > 0);
}

/// A LIFO buffer of 3 between cut at 11 s and weld at 30 s, 170 s; items numbered from 0 here, by
/// creation (`ItemId` follows creation).
///
/// Item 0 leaves cut at 11 into weld, until 41. Item 1 enters the buffer at 22, item 2 at 33. At 41 weld
/// delivers 0 and takes the last in: 2, until 71. Item 3 (made at 33) enters at 44, item 4 at 55, so the
/// buffer holds 1, 3, 4. Item 5 (made at 55) is done at 66 and finds it full. At 71 weld delivers 2 and takes
/// 4; item 5 enters; item 6 is made, done at 82, blocked. At 101 weld delivers 4 and takes 5; 6 enters. At
/// 131 it delivers 5 and takes 6. At 161 it delivers 6. Delivered 0, 2, 4, 5, 6 at 41, 71, 101, 131, 161.
/// Items 1 and 3 never leave: that is what LIFO does.
#[test]
fn lifo_buffer_timeline_by_hand() {
    let (root, store) = fresh_store("lifo");
    let text = model(
        None,
        "170s",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 11s}",
            "{id: buf, type: Buffer, capacity: 3 parts, discipline: lifo}",
            "{id: weld, type: Machine, cycle_time: 30s}",
            "{id: sink, type: Sink}",
        ],
        "  - [source, cut, buf, weld, sink]",
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(
        delivered_times(&trace, "sink"),
        seconds(&[41, 71, 101, 131, 161])
    );
    let items: Vec<u64> = trace
        .records
        .iter()
        .filter_map(|r| {
            if let Record::ItemDisposed { item, .. } = r {
                Some(item.as_u64())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(items, [0, 2, 4, 5, 6]);
}

/// A batch of 3 every 10 s into a machine at 100 s, 500 s: the Source holds its batch and starts the next
/// interval when the last part has left (doc comment of `Source`).
///
/// Batch 1 made at 10; part 0 leaves at 10, part 1 at 110, part 2 at 210. Next interval from 210: batch 2 at
/// 220. Machine busy with part 2 until 310: part 3 leaves at 310, part 4 at 410, part 5 at 510. Delivered at
/// 110, 210, 310, 410, and at 500 the machine holds part 4, the Source part 5: 2 alive.
#[test]
fn a_held_batch_timeline_by_hand() {
    let (root, store) = fresh_store("held_batch");
    let text = model(
        None,
        "500s",
        &[
            "{id: source, type: Source, interarrival: 10s, batch: 3 parts}",
            "{id: cut, type: Machine, cycle_time: 100s}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(
        created_times(&trace, "source"),
        seconds(&[10, 10, 10, 220, 220, 220])
    );
    assert_eq!(
        delivered_times(&trace, "sink"),
        seconds(&[110, 210, 310, 410])
    );
    assert_eq!(metric(&doc, "items_alive"), 2);
}

/// Concepts page 8: "the clock to the next failure runs whether cutting is working or waiting, and pauses
/// only while it is down". A part every 100 s (from the last one leaving), machine 10 s, `mtbf` 150 s, `mttr`
/// 20 s, 700 s.
///
/// Failures at 150 (idle), repaired 170; 320 (idle), repaired 340; 490 (idle), repaired 510; 660, repaired
/// 680. Parts at 100, 200, 300, 400 are done 10 s later. The part of 500 finds the machine down: "nothing moves
/// in or out", so it is held until 510 and done at 520; its leaving at 510 starts the interval, so the next
/// part is at 610, done at 620, the next at 710 is past the end.
#[test]
fn the_failure_clock_runs_while_idle_and_a_down_machine_takes_nothing() {
    let (root, store) = fresh_store("idle_failures");
    let text = model(
        None,
        "700s",
        &[
            "{id: source, type: Source, interarrival: 100s}",
            "{id: cut, type: Machine, cycle_time: 10s, mtbf: 150s, mttr: 20s}",
            "{id: sink, type: Sink}",
        ],
        FLOW_LINE,
    );
    let doc = ok(&text, &store);
    let trace = load(&root, &doc.runs[0]);
    assert_eq!(
        delivered_times(&trace, "sink"),
        seconds(&[110, 210, 310, 410, 520, 620])
    );
    assert_eq!(
        created_times(&trace, "source"),
        seconds(&[100, 200, 300, 400, 500, 610])
    );
    let failures: Vec<u64> = trace
        .records
        .iter()
        .filter_map(|r| {
            if let Record::Event {
                time,
                message: FlowMessage::Failure,
                ..
            } = r
            {
                Some(time.as_nanos())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(failures, seconds(&[150, 320, 490, 660]));
    assert_eq!(doc.provenance.seeds, [] as [u64; 0]);
}

/// The largest seed in the id's canonical form: JCS writes it as digits, not an exponent (ADR-0024 §5:
/// at most 2^53 - 1 "so the run id has a canonical form").
#[test]
fn the_largest_seed_has_a_canonical_id() {
    let (_, store) = fresh_store("largest_seed_id");
    let mut request = RunRequest::new();
    request.seeds = &[9_007_199_254_740_991];
    let doc = go(&model(None, "1min", RANDOM, FLOW_LINE), &store, &request)
        .unwrap()
        .result;
    let p = &doc.provenance;
    let trace = doc.runs[0].trace.as_ref().unwrap();
    let canonical = format!(
        r#"{{"engine":"{}","engine_version":"{}","message_format":"{}","model_hash":"{}","package":"{}","package_version":"{}","record_format":"{}","run_id_format":1,"seed":9007199254740991}}"#,
        p.engine,
        p.engine_version,
        trace.message_format,
        p.model_hash,
        p.package,
        p.package_version,
        trace.record_format,
    );
    assert_eq!(
        doc.runs[0].run_id.as_ref().unwrap().as_str(),
        format!("taktsim-core-{}", sha256_hex(canonical))
    );
}
