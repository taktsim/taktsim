//! Instruction counts for `taktsim_units` parsing and formatting (gungraun,
//! Callgrind; ADR-0016). Gated in CI: more than 10 % above the base fails a
//! pull request. Linux with Valgrind only:
//! `cargo bench -p taktsim-bench --bench instructions_units`.

mod support;

/// The benchmarks sit in a private module: gungraun's macros generate public
/// items without doc comments, which `missing_docs` rejects at the crate
/// root, and gungraun accepts no other attribute on a benchmark function.
/// Benchmark names do not change: gungraun builds them from the file, the
/// group and the function.
mod benchmarks {
    use std::fmt;
    use std::hint::black_box;
    use std::str::FromStr;

    use gungraun::{library_benchmark, library_benchmark_group};
    use taktsim_bench::units::{
        self, COUNT_TEXTS, COUNTS, DURATION_TEXTS, DURATIONS, RATE_TEXTS, RATIO_TEXTS,
    };
    use taktsim_units::{Count, Rate, Ratio, SimDuration, UnitError};

    use crate::support;

    /// The result, and the texts handed on to the teardown.
    type Parsed<const N: usize> = (Result<(), UnitError>, [&'static str; N]);

    #[library_benchmark(teardown = support::check)]
    #[bench::duration(DURATION_TEXTS)]
    fn parse_duration(texts: [&'static str; 8]) -> Parsed<8> {
        (
            black_box(units::parse_all::<SimDuration>(black_box(&texts))),
            texts,
        )
    }

    #[library_benchmark(teardown = support::check)]
    #[bench::count(COUNT_TEXTS)]
    fn parse_count(texts: [&'static str; 4]) -> Parsed<4> {
        (
            black_box(units::parse_all::<Count>(black_box(&texts))),
            texts,
        )
    }

    #[library_benchmark(teardown = support::check)]
    #[bench::rate(RATE_TEXTS)]
    fn parse_rate(texts: [&'static str; 4]) -> Parsed<4> {
        (
            black_box(units::parse_all::<Rate>(black_box(&texts))),
            texts,
        )
    }

    /// The values and a buffer with room for all of them, so formatting does
    /// not allocate inside the measurement.
    fn to_format<T>(values: Vec<T>) -> (Vec<T>, String) {
        (values, String::with_capacity(256))
    }

    /// The parsed texts, for the formats with no constant constructor.
    fn parsed_to_format<T: FromStr<Err = UnitError>>(texts: [&str; 4]) -> (Vec<T>, String) {
        to_format(support::built(units::parsed(&texts)))
    }

    /// The length written, and the values and buffer handed on to the
    /// teardown, so freeing them is not measured.
    type Formatted<T> = (Result<usize, fmt::Error>, (Vec<T>, String));

    /// The workload of every format benchmark.
    fn format_into<T: fmt::Display>(input: (Vec<T>, String)) -> Formatted<T> {
        let (values, mut buffer) = input;
        let length = black_box(units::format_all(
            black_box(&values),
            black_box(&mut buffer),
        ));
        (length, (values, buffer))
    }

    #[library_benchmark(teardown = support::check)]
    #[bench::duration(args = (DURATIONS.to_vec()), setup = to_format)]
    fn format_duration(input: (Vec<SimDuration>, String)) -> Formatted<SimDuration> {
        format_into(input)
    }

    #[library_benchmark(teardown = support::check)]
    #[bench::count(args = (COUNTS.to_vec()), setup = to_format)]
    fn format_count(input: (Vec<Count>, String)) -> Formatted<Count> {
        format_into(input)
    }

    #[library_benchmark(teardown = support::check)]
    #[bench::rate(args = (RATE_TEXTS), setup = parsed_to_format)]
    fn format_rate(input: (Vec<Rate>, String)) -> Formatted<Rate> {
        format_into(input)
    }

    #[library_benchmark(teardown = support::check)]
    #[bench::ratio(args = (RATIO_TEXTS), setup = parsed_to_format)]
    fn format_ratio(input: (Vec<Ratio>, String)) -> Formatted<Ratio> {
        format_into(input)
    }

    library_benchmark_group!(
        name = parse,
        benchmarks = [parse_duration, parse_count, parse_rate]
    );
    library_benchmark_group!(
        name = format,
        benchmarks = [format_duration, format_count, format_rate, format_ratio]
    );
}

use benchmarks::{format, parse};

gungraun::main!(library_benchmark_groups = parse, format);
