//! Wall-clock times for `taktsim_units` parsing and formatting (criterion;
//! ADR-0016), for local tuning:
//! `cargo bench -p taktsim-bench --bench wall_units`.
//! Not gated, and never evidence that a target is met (ADR-0010).

use std::error::Error;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use taktsim_bench::units::{
    self, COUNT_TEXTS, COUNTS, DURATION_TEXTS, DURATIONS, RATE_TEXTS, RATIO_TEXTS,
};
use taktsim_units::{Count, Rate, Ratio, SimDuration};

/// Elements per iteration, for a time per item in the report.
fn items<T>(values: &[T]) -> Result<Throughput, Box<dyn Error>> {
    Ok(Throughput::Elements(u64::try_from(values.len())?))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut c = Criterion::default().configure_from_args();

    let mut group = c.benchmark_group("parse");
    group.throughput(items(&DURATION_TEXTS)?);
    group.bench_function("duration", |b| {
        b.iter(|| units::parse_all::<SimDuration>(black_box(&DURATION_TEXTS)));
    });
    group.throughput(items(&COUNT_TEXTS)?);
    group.bench_function("count", |b| {
        b.iter(|| units::parse_all::<Count>(black_box(&COUNT_TEXTS)));
    });
    group.throughput(items(&RATE_TEXTS)?);
    group.bench_function("rate", |b| {
        b.iter(|| units::parse_all::<Rate>(black_box(&RATE_TEXTS)));
    });
    group.throughput(items(&RATIO_TEXTS)?);
    group.bench_function("ratio", |b| {
        b.iter(|| units::parse_all::<Ratio>(black_box(&RATIO_TEXTS)));
    });
    group.finish();

    let rates: Vec<Rate> = units::parsed(&RATE_TEXTS)?;
    let ratios: Vec<Ratio> = units::parsed(&RATIO_TEXTS)?;
    let mut buffer = String::with_capacity(256);
    let mut group = c.benchmark_group("format");
    group.throughput(items(&DURATIONS)?);
    group.bench_function("duration", |b| {
        b.iter(|| units::format_all(black_box(&DURATIONS), &mut buffer));
    });
    group.throughput(items(&COUNTS)?);
    group.bench_function("count", |b| {
        b.iter(|| units::format_all(black_box(&COUNTS), &mut buffer));
    });
    group.throughput(items(&rates)?);
    group.bench_function("rate", |b| {
        b.iter(|| units::format_all(black_box(&rates), &mut buffer));
    });
    group.throughput(items(&ratios)?);
    group.bench_function("ratio", |b| {
        b.iter(|| units::format_all(black_box(&ratios), &mut buffer));
    });
    group.finish();

    c.final_summary();
    Ok(())
}
