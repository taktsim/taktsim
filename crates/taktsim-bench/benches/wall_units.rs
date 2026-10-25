//! Wall-clock times for `taktsim_units` parsing and formatting (criterion;
//! ADR-0016), for local tuning:
//! `cargo bench -p taktsim-bench --bench wall_units`.
//! Not gated, and never evidence that a target is met (ADR-0010).

use std::error::Error;
use std::hint::black_box;

use criterion::{BenchmarkGroup, Criterion, Throughput, measurement::WallTime};
use taktsim_bench::units::{
    self, COUNT_TEXTS, COUNTS, DURATION_TEXTS, DURATIONS, RATE_TEXTS, RATIO_TEXTS,
};
use taktsim_units::{Count, Rate, Ratio, SimDuration};

/// Times `workload` as `name`, reporting a time per item. A workload that
/// fails ends early and would time less work, as in the gungraun
/// benchmarks' teardown, so any failed iteration fails the run afterwards.
fn bench<T, E>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    items: usize,
    mut workload: impl FnMut() -> Result<T, E>,
) -> Result<(), Box<dyn Error>> {
    group.throughput(Throughput::Elements(u64::try_from(items)?));
    let mut failures = 0_u64;
    group.bench_function(name, |b| {
        b.iter(|| {
            let result = workload();
            failures += u64::from(result.is_err());
            result
        });
    });
    if failures > 0 {
        return Err(format!("{name}: {failures} iterations failed").into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut c = Criterion::default().configure_from_args();

    let mut group = c.benchmark_group("parse");
    bench(&mut group, "duration", DURATION_TEXTS.len(), || {
        units::parse_all::<SimDuration>(black_box(&DURATION_TEXTS))
    })?;
    bench(&mut group, "count", COUNT_TEXTS.len(), || {
        units::parse_all::<Count>(black_box(&COUNT_TEXTS))
    })?;
    bench(&mut group, "rate", RATE_TEXTS.len(), || {
        units::parse_all::<Rate>(black_box(&RATE_TEXTS))
    })?;
    bench(&mut group, "ratio", RATIO_TEXTS.len(), || {
        units::parse_all::<Ratio>(black_box(&RATIO_TEXTS))
    })?;
    group.finish();

    let rates: Vec<Rate> = units::parsed(&RATE_TEXTS)?;
    let ratios: Vec<Ratio> = units::parsed(&RATIO_TEXTS)?;
    let mut buffer = String::with_capacity(256);
    let mut group = c.benchmark_group("format");
    bench(&mut group, "duration", DURATIONS.len(), || {
        units::format_all(black_box(&DURATIONS), &mut buffer)
    })?;
    bench(&mut group, "count", COUNTS.len(), || {
        units::format_all(black_box(&COUNTS), &mut buffer)
    })?;
    bench(&mut group, "rate", rates.len(), || {
        units::format_all(black_box(&rates), &mut buffer)
    })?;
    bench(&mut group, "ratio", ratios.len(), || {
        units::format_all(black_box(&ratios), &mut buffer)
    })?;
    group.finish();

    c.final_summary();
    Ok(())
}
