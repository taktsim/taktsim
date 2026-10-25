//! Wall-clock times for `taktsim_core::random` (criterion; ADR-0016), for
//! local tuning: `cargo bench -p taktsim-bench --bench wall_random`.
//! Not gated, and never evidence that a target is met (ADR-0010).

use std::error::Error;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use taktsim_bench::random::{self, DRAWS, DistributionCase};

fn main() -> Result<(), Box<dyn Error>> {
    let mut c = Criterion::default().configure_from_args();

    let mut group = c.benchmark_group("stream");
    group.throughput(Throughput::Elements(u64::from(DRAWS)));
    let mut stream = random::stream();
    group.bench_function("next_u64", |b| {
        b.iter(|| random::next_u64_n(black_box(&mut stream), DRAWS));
    });
    group.bench_function("next_open01", |b| {
        b.iter(|| random::next_open01_n(black_box(&mut stream), DRAWS));
    });
    group.bench_function("next_standard_normal", |b| {
        b.iter(|| random::next_standard_normal_n(black_box(&mut stream), DRAWS));
    });
    group.finish();

    let mut group = c.benchmark_group("sample");
    group.throughput(Throughput::Elements(u64::from(DRAWS)));
    for case in DistributionCase::ALL {
        let distribution = case.distribution()?;
        let mut stream = random::stream();
        // A failed sample ends the loop early and would time less work, as
        // in the gungraun benchmarks' teardown: count failures, fail after.
        let mut failures = 0_u64;
        group.bench_function(case.name(), |b| {
            b.iter(|| {
                let total =
                    random::sample_n(black_box(&distribution), black_box(&mut stream), DRAWS);
                failures += u64::from(total.is_err());
                total
            });
        });
        if failures > 0 {
            return Err(format!("{}: {failures} iterations failed", case.name()).into());
        }
    }
    group.finish();

    c.final_summary();
    Ok(())
}
