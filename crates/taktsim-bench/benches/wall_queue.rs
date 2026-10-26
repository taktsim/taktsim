//! Wall-clock times for the event queue, through the public engine API
//! (criterion; ADR-0016), for local tuning:
//! `cargo bench -p taktsim-bench --bench wall_queue`. Not gated, and never
//! evidence that a target is met (ADR-0010).
//!
//! The same workloads as `instructions_queue`. Each iteration builds and
//! starts a fresh engine outside the timed part (`iter_batched_ref`), then
//! times about 10 000 events.

use std::error::Error;
use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput};
use taktsim_bench::queue::{
    self, EVENTS, HOLD_POPULATIONS, MACHINE_COUNTS, TIED_POPULATIONS, WAVE_CASES,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut c = Criterion::default().configure_from_args();

    let mut group = c.benchmark_group("hold");
    group.throughput(Throughput::Elements(EVENTS));
    for n in HOLD_POPULATIONS {
        // A failed run stops early and would time less work, as in the
        // gungraun benchmarks' teardown: count failures, fail after.
        let mut failures = 0_u64;
        group.bench_function(format!("n{n}"), |b| {
            b.iter_batched_ref(
                || queue::hold_random(n),
                |engine| {
                    let ran = match engine {
                        Ok(engine) => queue::run_events(black_box(engine), EVENTS).is_ok(),
                        Err(_) => false,
                    };
                    failures += u64::from(!ran);
                },
                BatchSize::LargeInput,
            );
        });
        fail_on(failures, &format!("hold n{n}"))?;
    }
    group.finish();

    let mut group = c.benchmark_group("hold_tied");
    group.throughput(Throughput::Elements(EVENTS));
    for n in TIED_POPULATIONS {
        let mut failures = 0_u64;
        group.bench_function(format!("n{n}"), |b| {
            b.iter_batched_ref(
                || queue::hold_tied(n),
                |engine| {
                    let ran = match engine {
                        Ok(engine) => queue::run_events(black_box(engine), EVENTS).is_ok(),
                        Err(_) => false,
                    };
                    failures += u64::from(!ran);
                },
                BatchSize::LargeInput,
            );
        });
        fail_on(failures, &format!("hold_tied n{n}"))?;
    }
    group.finish();

    let mut group = c.benchmark_group("waves");
    for (lanes, rounds) in WAVE_CASES {
        let events = queue::wave_events(lanes, rounds);
        group.throughput(Throughput::Elements(events));
        let mut failures = 0_u64;
        group.bench_function(format!("n{lanes}"), |b| {
            b.iter_batched_ref(
                || queue::waves(lanes),
                |engine| {
                    let ran = match engine {
                        Ok(engine) => queue::run_events(black_box(engine), events).is_ok(),
                        Err(_) => false,
                    };
                    failures += u64::from(!ran);
                },
                BatchSize::LargeInput,
            );
        });
        fail_on(failures, &format!("waves n{lanes}"))?;
    }
    group.finish();

    let mut group = c.benchmark_group("cancel");
    group.throughput(Throughput::Elements(EVENTS));
    for machines in MACHINE_COUNTS {
        let mut failures = 0_u64;
        group.bench_function(format!("m{machines}"), |b| {
            b.iter_batched_ref(
                || queue::cancel_line(machines),
                |engine| {
                    let ran = match engine {
                        Ok(engine) => queue::run_events(black_box(engine), EVENTS).is_ok(),
                        Err(_) => false,
                    };
                    failures += u64::from(!ran);
                },
                BatchSize::LargeInput,
            );
        });
        fail_on(failures, &format!("cancel m{machines}"))?;
    }
    group.finish();

    c.final_summary();
    Ok(())
}

/// Fails the run if any iteration of `name` did not complete.
fn fail_on(failures: u64, name: &str) -> Result<(), Box<dyn Error>> {
    if failures > 0 {
        return Err(format!("{name}: {failures} iterations failed").into());
    }
    Ok(())
}
