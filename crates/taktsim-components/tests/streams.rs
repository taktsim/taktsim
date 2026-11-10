//! Random streams: their names are part of the trace contract, and purposes are
//! kept apart (ADR-0013, ADR-0022 §8).

mod support;

use support::{d, document, run};
use taktsim_components::FlowMessage;
use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
use taktsim_units::SimDuration;

fn stream(seed: u64, component: &str, purpose: &str) -> Stream {
    Stream::new(seed, StreamKey::new(component, purpose))
}

#[allow(clippy::unwrap_used)] // Test helper.
fn exponential(mean: &str) -> DurationDistribution {
    DurationDistribution::exponential(d(mean)).unwrap()
}

#[test]
fn stream_purposes_are_pinned() {
    let seed = 2026;
    let yaml = document(
        "4h",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 3min}, variants: {red: 50%, blue: 50%}}\n  \
         - {id: m, type: Machine, cycle_time: {dist: exponential, mean: 1min}, mtbf: {dist: exponential, mean: 1h}, mttr: {dist: exponential, mean: 5min}, setup: {per_variant: {dist: exponential, mean: 30s}}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let r = run(&yaml, seed);
    let events = r.events();

    // `interarrival`: the first part at the stream's first draw.
    let mut interarrival = stream(seed, "source", "interarrival");
    let first = exponential("3min").sample(&mut interarrival).unwrap();
    assert_eq!(
        r.times("source", |m| matches!(m, FlowMessage::NextArrival))[0],
        taktsim_units::SimTime::START.checked_add(first).unwrap()
    );

    // `variant`: once per batch, against the sorted names (blue, red).
    let mut variant = stream(seed, "source", "variant");
    let arrivals = r.arrivals("m");
    for e in arrivals.iter().take(20) {
        let FlowMessage::Arrive(attrs) = e.message else {
            panic!("{e:?}")
        };
        let expected = if variant.next_open01() < 0.5 {
            "blue"
        } else {
            "red"
        };
        assert_eq!(r.variant_name(attrs.variant).as_deref(), Some(expected));
    }

    // `failures`: the first failure at the stream's first draw.
    let mut failures = stream(seed, "m", "failures");
    let up = exponential("1h").sample(&mut failures).unwrap();
    let failed = r.times("m", |m| matches!(m, FlowMessage::Failure))[0];
    assert_eq!(failed.as_nanos(), up.as_nanos());

    // `repairs`: the first repair after the stream's first draw.
    let mut repairs = stream(seed, "m", "repairs");
    let down = exponential("5min").sample(&mut repairs).unwrap();
    let repaired = r.times("m", |m| matches!(m, FlowMessage::Repaired))[0];
    assert_eq!(repaired, failed.checked_add(down).unwrap());

    // `cycle_time`: the first unit's cycle.
    let mut cycle = stream(seed, "m", "cycle_time");
    let first_cycle = exponential("1min").sample(&mut cycle).unwrap();
    let started = arrivals[0].time;
    let done = r.times("m", |m| matches!(m, FlowMessage::CycleDone { .. }))[0];
    if done < failed {
        assert_eq!(done, started.checked_add(first_cycle).unwrap());
    }

    // `setup`: the first changeover. The unit that needs it is the first whose
    // variant differs from the one before it.
    let mut setup = stream(seed, "m", "setup");
    let change = exponential("30s").sample(&mut setup).unwrap();
    let notes = r.machine_notes("m");
    let setups: Vec<usize> = notes
        .iter()
        .enumerate()
        .filter(|(_, (_, n))| n.state == taktsim_components::MachineState::Setup)
        .map(|(i, _)| i)
        .collect();
    let begun = notes[setups[0]].0;
    let ended = r.times("m", |m| matches!(m, FlowMessage::SetupDone))[0];
    // No failure in the first changeover at this seed.
    assert_eq!(ended.checked_duration_since(begun).unwrap(), change);
    assert_ne!(events.len(), 0);
}

#[test]
fn a_source_with_one_variant_takes_no_variant_stream() {
    // One name: no draw. So `variants: {red: 100%}` and a second name's
    // absence shift nothing in the machine's draws, and the run needs no
    // `variant` key: taking it would conflict with nothing, but the table says
    // it is taken with two or more.
    let plain = document(
        "1h",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let named = plain.replace("mean: 1min}}", "mean: 1min}, variants: {red: 100%}}");
    assert_eq!(run(&plain, 3).delivered(), run(&named, 3).delivered());
}

#[test]
fn adding_a_line_shifts_nothing() {
    let line = "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 30s, sigma: 0.3}, mtbf: {dist: exponential, mean: 30min}, mttr: 2min}\n  \
         - {id: sink, type: Sink}";
    let other = "  - {id: source2, type: Source, interarrival: {dist: exponential, mean: 20s}}\n  \
         - {id: cut2, type: Machine, cycle_time: {dist: exponential, mean: 10s}}\n  \
         - {id: sink2, type: Sink}";
    let alone = run(&document("8h", line, "  - [source, cut, sink]"), 9);
    let both = run(
        &document(
            "8h",
            &format!("{line}\n{other}"),
            "  - [source, cut, sink]\n  - [source2, cut2, sink2]",
        ),
        9,
    );
    let times =
        |r: &support::Run| -> Vec<_> { r.arrivals("sink").iter().map(|e| e.time).collect() };
    assert!(times(&alone).len() > 100);
    assert_eq!(times(&alone), times(&both));
    assert_eq!(
        alone.times("cut", |m| matches!(m, FlowMessage::Failure)),
        both.times("cut", |m| matches!(m, FlowMessage::Failure))
    );
}

/// The work of each unit at `m`: the time from its start to its cycle's end, less
/// the time `m` was down, for a saturated machine with no setup.
fn work(r: &support::Run) -> Vec<SimDuration> {
    let failures = r.times("m", |m| matches!(m, FlowMessage::Failure));
    let repairs = r.times("m", |m| matches!(m, FlowMessage::Repaired));
    let started = r.arrivals("m");
    let done = r.times("m", |m| matches!(m, FlowMessage::CycleDone { .. }));
    started
        .iter()
        .zip(done)
        .map(|(start, end)| {
            let mut nanos = end.as_nanos() - start.time.as_nanos();
            for (from, to) in failures.iter().zip(&repairs) {
                if *from >= start.time && *to <= end {
                    nanos -= to.as_nanos() - from.as_nanos();
                }
            }
            SimDuration::from_nanos(nanos)
        })
        .collect()
}

#[test]
fn changing_mtbf_keeps_cycle_draws() {
    let line = |mtbf: &str| {
        document(
            "10h",
            &format!(
                "  - {{id: source, type: Source}}\n  \
                 - {{id: m, type: Machine, cycle_time: {{dist: exponential, mean: 1min}}, mtbf: {{dist: exponential, mean: {mtbf}}}, mttr: 3min}}\n  \
                 - {{id: sink, type: Sink}}"
            ),
            "  - [source, m, sink]",
        )
    };
    let a = work(&run(&line("30min"), 21));
    let b = work(&run(&line("2h"), 21));
    let n = a.len().min(b.len());
    assert!(n > 200, "{n}");
    assert_eq!(a[..n], b[..n]);
    // And they are the stream's draws.
    let mut cycle = stream(21, "m", "cycle_time");
    for got in &a[..n] {
        let draw = exponential("1min").sample(&mut cycle).unwrap();
        assert_eq!(*got, draw);
    }
}
