//! Instruction counts for the reference line, through the registry (gungraun,
//! Callgrind; ADR-0016, ADR-0022). Gated in CI: more than 10 % above the base
//! fails a pull request. Linux with Valgrind only:
//! `cargo bench -p taktsim-bench --bench instructions_line`.
//!
//! Each benchmark processes 10 000 events of an engine that setup built and
//! started: components, the protocol between them, notes and random draws.

mod support;

/// The benchmarks sit in a private module: see `instructions_queue.rs`.
mod benchmarks {
    use std::hint::black_box;

    use gungraun::{library_benchmark, library_benchmark_group};
    use taktsim_bench::line::{self, EVENTS};
    use taktsim_components::FlowMessage;
    use taktsim_core::{Engine, RunError, RunSummary};

    use crate::support;

    fn build(checks: bool) -> Engine<FlowMessage> {
        support::built(line::engine(checks))
    }

    #[library_benchmark(setup = build, teardown = support::check)]
    #[bench::checked(true)]
    #[bench::unchecked(false)]
    fn events(
        mut engine: Engine<FlowMessage>,
    ) -> (Result<RunSummary, RunError>, Engine<FlowMessage>) {
        let outcome = black_box(line::run_events(black_box(&mut engine), EVENTS));
        (outcome, engine)
    }

    #[library_benchmark(setup = build, teardown = support::check)]
    #[bench::checked(true)]
    fn events_traced(
        mut engine: Engine<FlowMessage>,
    ) -> (Result<u64, Box<dyn std::error::Error>>, Engine<FlowMessage>) {
        let outcome = black_box(line::run_traced(black_box(&mut engine), EVENTS));
        (outcome, engine)
    }

    library_benchmark_group!(name = reference_line, benchmarks = [events, events_traced]);
}

use benchmarks::reference_line;

gungraun::main!(library_benchmark_groups = reference_line);
