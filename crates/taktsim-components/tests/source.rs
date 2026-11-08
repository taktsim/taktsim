//! Source: arrivals, batches, blocking, on demand (ADR-0022 §7).

mod support;

use support::{document, is_arrive, run, t};
use taktsim_components::{FlowMessage, SourceState};
use taktsim_trace::Record;

const SINK: &str = "  - {id: sink, type: Sink}";

#[test]
fn the_first_part_comes_after_the_first_interval() {
    let yaml = document(
        "1min",
        &format!("  - {{id: source, type: Source, interarrival: 20s}}\n{SINK}"),
        "  - [source, sink]",
    );
    let r = run(&yaml, 1);
    let times: Vec<_> = r.delivered().iter().map(|(time, _)| *time).collect();
    assert_eq!(times, [t("20s"), t("40s"), t("1min")]);
}

#[test]
fn next_interval_starts_when_the_batch_has_left() {
    // A batch of 3 every 1 min, into a machine of 10 s with no buffer: the
    // batch needs 30 s to leave, so the next interval starts then.
    let yaml = document(
        "3min",
        &format!(
            "  - {{id: source, type: Source, interarrival: 1min, batch: 3 parts}}\n  \
             - {{id: m, type: Machine, cycle_time: 10s}}\n{SINK}"
        ),
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    // The batch arrives at 1 min; its parts enter at 60, 70 and 80 s. The next
    // interval starts at 80 s, so the next batch comes at 140 s.
    assert_eq!(
        r.times("source", |m| matches!(m, FlowMessage::NextArrival)),
        [t("1min"), t("140s")]
    );
    assert_eq!(
        r.arrivals("m").iter().map(|e| e.time).collect::<Vec<_>>(),
        [
            t("60s"),
            t("70s"),
            t("80s"),
            t("140s"),
            t("150s"),
            t("160s")
        ]
    );
}

#[test]
fn blocked_source_holds_and_loses_nothing() {
    // Parts every 10 s into a machine of 25 s: the source holds its part
    // until the machine takes it, and creates no more meanwhile.
    let yaml = document(
        "2min",
        &format!(
            "  - {{id: source, type: Source, interarrival: 10s}}\n  \
             - {{id: m, type: Machine, cycle_time: 25s}}\n{SINK}"
        ),
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    let notes = r.source_notes("source");
    assert_eq!(notes[0].1.state, SourceState::Waiting);
    assert!(notes.iter().any(|(_, n)| n.state == SourceState::Blocked));
    assert!(notes.iter().all(|(_, n)| n.held <= 1));
    // 10: enters; next at 20, held until 35; next at 45, held until 60...
    assert_eq!(
        r.arrivals("m").iter().map(|e| e.time).collect::<Vec<_>>(),
        [t("10s"), t("35s"), t("60s"), t("85s"), t("110s")]
    );
    // Every created part is delivered, in the machine or still held.
    let counts = r.report.items;
    assert_eq!(
        counts.created.get(),
        counts.delivered.get() + counts.held.get()
    );
    assert_eq!(counts.in_transit.get(), 0);
}

#[test]
fn created_equals_emitted_plus_held() {
    let yaml = document(
        "1h",
        &format!(
            "  - {{id: source, type: Source, interarrival: {{dist: exponential, mean: 10s}}, batch: 4 parts}}\n  \
             - {{id: m, type: Machine, cycle_time: 20s}}\n{SINK}"
        ),
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 11);
    let id = r.id("source");
    let (mut created, mut emitted) = (0_u64, 0_u64);
    for record in &r.records {
        match record {
            Record::ItemCreated { by, .. } if *by == id => created += 1,
            Record::Event {
                from,
                message: FlowMessage::Arrive(_),
                ..
            } if *from == id => emitted += 1,
            Record::Note {
                by,
                message: FlowMessage::SourceState(note),
                ..
            } if *by == id => {
                // At a note, after the event that wrote it, in transit parts
                // have left the holder but have not been delivered yet, so
                // emitted can only lag.
                assert!(u64::from(note.held) <= 4);
                assert!(emitted + u64::from(note.held) <= created);
            }
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. }
            | Record::Decision { .. } => {}
        }
    }
    let last = r.source_notes("source").last().unwrap().1;
    assert_eq!(created, emitted + u64::from(last.held));
}

/// Line A of page 1 of the concepts: the numbers of its table.
#[test]
fn on_demand_source_reproduces_page_1() {
    let yaml = document(
        "8h",
        "  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 42s}\n  \
         - {id: buf1, type: Buffer, capacity: 20 parts}\n  \
         - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: sink, type: Sink}",
        "  - [source, cut, buf1, weld, sink]",
    );
    let r = run(&yaml, 42);
    let delivered = r.delivered();
    // First part at 87 s, then one every 45 s: 639 in the shift.
    assert_eq!(delivered.len(), 639);
    assert_eq!(delivered[0].0, t("87s"));
    assert_eq!(delivered[1].0, t("132s"));

    // On demand: created when taken, so ItemCreated and the first Arrive have
    // the same time, and the Source holds nothing.
    let arrivals = r.arrivals("cut");
    for record in &r.records {
        if let Record::ItemCreated { time, item, .. } = record {
            let first = arrivals.iter().find(|e| e.item == Some(*item)).unwrap();
            assert_eq!(first.time, *time);
        }
    }
    let notes = r.source_notes("source");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(
        (notes[0].1.state, notes[0].1.held),
        (SourceState::Waiting, 0)
    );

    // WIP from release to delivery reaches 22: 20 buffer places and a unit in
    // each station.
    let mut wip: i64 = 0;
    let mut most = 0;
    let (source, mut last_lead_time) = (r.id("source"), None);
    let mut released = std::collections::BTreeMap::new();
    for record in &r.records {
        match record {
            Record::Event {
                time,
                from,
                message: FlowMessage::Arrive(_),
                item: Some(item),
                ..
            } if *from == source => {
                wip += 1;
                most = most.max(wip);
                released.insert(*item, *time);
            }
            Record::ItemDisposed { time, item, .. } => {
                wip -= 1;
                let start = released[item];
                last_lead_time = Some(time.checked_duration_since(start).unwrap());
            }
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::Note { .. }
            | Record::Decision { .. } => {}
        }
    }
    assert_eq!(most, 22);
    // 22 parts at the bottleneck's 45 s: 16.5 min.
    assert_eq!(last_lead_time.unwrap(), support::d("16.5min"));
}

#[test]
fn attributes_travel_with_the_item() {
    let yaml = document(
        "10min",
        "  - {id: source, type: Source, interarrival: 30s, batch: 2 parts, variants: {basic: 50%, premium: 50%}, priority: 7}\n  \
         - {id: buf, type: Buffer, capacity: 9 parts}\n  \
         - {id: m, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}",
        "  - [source, buf, m, sink]",
    );
    let r = run(&yaml, 3);
    let at_sink = r.arrivals("sink");
    assert!(at_sink.len() >= 10);
    let attrs_of = |item, place: &str| {
        r.arrivals(place)
            .iter()
            .find(|e| e.item == Some(item))
            .map(|e| {
                let FlowMessage::Arrive(attrs) = e.message else {
                    panic!("{e:?}")
                };
                attrs
            })
    };
    let mut seen = std::collections::BTreeSet::new();
    for e in &at_sink {
        let FlowMessage::Arrive(attrs) = e.message else {
            panic!("{e:?}")
        };
        assert_eq!(attrs.priority, 7);
        assert_eq!(attrs.due, None);
        let item = e.item.unwrap();
        // The same at every hop.
        assert_eq!(attrs_of(item, "buf"), Some(attrs));
        assert_eq!(attrs_of(item, "m"), Some(attrs));
        seen.insert(r.variant_name(attrs.variant).unwrap());
    }
    // A trace names its variants on its own.
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        ["basic".to_owned(), "premium".to_owned()]
    );
    // One variant per batch: parts are created in pairs.
    let arrivals = r.arrivals("buf");
    for pair in arrivals.chunks(2).filter(|pair| pair.len() == 2) {
        assert_eq!(pair[0].message, pair[1].message);
    }
    assert!(r.events().iter().any(|e| is_arrive(&e.message)));
}

#[test]
fn a_source_without_variants_makes_the_default_variant() {
    let yaml = document(
        "1min",
        &format!("  - {{id: source, type: Source, interarrival: 30s}}\n{SINK}"),
        "  - [source, sink]",
    );
    let r = run(&yaml, 1);
    let e = &r.arrivals("sink")[0];
    let FlowMessage::Arrive(attrs) = e.message else {
        panic!("{e:?}")
    };
    assert_eq!(r.variant_name(attrs.variant).as_deref(), Some("default"));
}
