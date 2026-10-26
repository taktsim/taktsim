//! Instruction counts for the event queue, through the public engine API
//! (gungraun, Callgrind; ADR-0001, ADR-0016). Gated in CI: more than 10 %
//! above the base fails a pull request. Linux with Valgrind only:
//! `cargo bench -p taktsim-bench --bench instructions_queue`.
//!
//! Each benchmark processes about 10 000 events of an engine that setup
//! built and started, so the count is the event loop: queue operations, the
//! engine's bookkeeping and a one-line component. Compare sizes within a
//! group (the same events at 1 and at 10 000 pending) to see what the
//! queue's size costs per event.

mod support;

/// The benchmarks sit in a private module: gungraun's macros generate public
/// items without doc comments, which `missing_docs` rejects at the crate
/// root, and gungraun accepts no other attribute on a benchmark function.
/// Benchmark names do not change: gungraun builds them from the file, the
/// group and the function.
mod benchmarks {
    use std::hint::black_box;

    use gungraun::{library_benchmark, library_benchmark_group};
    use taktsim_bench::queue::{self, EVENTS, Msg, Outcome};
    use taktsim_core::{Engine, RunError};

    use crate::support;

    /// The outcome, and the engine handed on to the teardown, which drops
    /// it outside the measurement.
    type Ran = (Result<Outcome, RunError>, Engine<Msg>);

    fn build_hold_random(population: u32) -> Engine<Msg> {
        support::built(queue::hold_random(population))
    }

    fn build_hold_tied(population: u32) -> Engine<Msg> {
        support::built(queue::hold_tied(population))
    }

    /// An engine with `lanes` lanes, and the number of events of `rounds`
    /// rounds.
    fn build_waves(lanes: u32, rounds: u32) -> (Engine<Msg>, u64) {
        (
            support::built(queue::waves(lanes)),
            queue::wave_events(lanes, rounds),
        )
    }

    fn build_cancel_line(machines: u32) -> Engine<Msg> {
        support::built(queue::cancel_line(machines))
    }

    // One id per size of `HOLD_POPULATIONS`, `TIED_POPULATIONS`,
    // `WAVE_CASES` and `MACHINE_COUNTS`; tests/workloads.rs fails if a
    // size is missing here, or an id has no size.
    #[library_benchmark(setup = build_hold_random, teardown = support::check)]
    #[bench::n1(1)]
    #[bench::n10(10)]
    #[bench::n100(100)]
    #[bench::n1000(1000)]
    #[bench::n10000(10000)]
    fn hold(mut engine: Engine<Msg>) -> Ran {
        let outcome = black_box(queue::run_events(black_box(&mut engine), EVENTS));
        (outcome, engine)
    }

    #[library_benchmark(setup = build_hold_tied, teardown = support::check)]
    #[bench::n100(100)]
    #[bench::n10000(10000)]
    fn hold_tied(mut engine: Engine<Msg>) -> Ran {
        let outcome = black_box(queue::run_events(black_box(&mut engine), EVENTS));
        (outcome, engine)
    }

    #[library_benchmark(setup = build_waves, teardown = support::check)]
    #[bench::n100(100, 99)]
    #[bench::n1000(1000, 10)]
    #[bench::n10000(10000, 1)]
    fn waves(input: (Engine<Msg>, u64)) -> Ran {
        let (mut engine, events) = input;
        let outcome = black_box(queue::run_events(black_box(&mut engine), events));
        (outcome, engine)
    }

    #[library_benchmark(setup = build_cancel_line, teardown = support::check)]
    #[bench::m20(20)]
    #[bench::m1000(1000)]
    fn cancel(mut engine: Engine<Msg>) -> Ran {
        let outcome = black_box(queue::run_events(black_box(&mut engine), EVENTS));
        (outcome, engine)
    }

    library_benchmark_group!(name = hold_model, benchmarks = [hold, hold_tied]);
    library_benchmark_group!(name = fan_out, benchmarks = [waves]);
    library_benchmark_group!(name = cancellation, benchmarks = [cancel]);
}

use benchmarks::{cancellation, fan_out, hold_model};

gungraun::main!(library_benchmark_groups = hold_model, fan_out, cancellation);
