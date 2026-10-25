//! Shared by the gungraun benchmarks (`instructions_*.rs`).

use std::fmt::Debug;
use std::hint::black_box;

/// The value of a benchmark input or result that cannot be an error: every
/// input is a constant, and the crate's tests (`tests/workloads.rs`) run
/// every workload on it without error.
///
/// gungraun passes a setup function's return value straight to the
/// benchmark, so setup cannot return an error. Stopping the run is the only
/// correct outcome: a fallback input, or a loop that ended early, would
/// measure different work and could hide a regression. gungraun fails a
/// benchmark whose process does not exit successfully.
#[expect(
    clippy::expect_used,
    reason = "benchmark harness, not library code: a constant input that \
              fails is a bug in the benchmark and must stop the run"
)]
pub fn built<T, E: Debug>(input: Result<T, E>) -> T {
    input.expect("benchmark workload succeeds (checked by tests/workloads.rs)")
}

/// Teardown of a benchmark that returns a `Result`: stops the run on an
/// error (see [`built`]).
pub fn check<T, E: Debug>(result: Result<T, E>) {
    black_box(built(result));
}
