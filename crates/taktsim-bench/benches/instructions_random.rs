//! Instruction counts for `taktsim_core::random` (gungraun, Callgrind;
//! ADR-0016). Gated in CI: more than 10 % above the base fails a pull
//! request. Linux with Valgrind only:
//! `cargo bench -p taktsim-bench --bench instructions_random`.

mod support;

/// The benchmarks sit in a private module: gungraun's macros generate public
/// items without doc comments, which `missing_docs` rejects at the crate
/// root, and gungraun accepts no other attribute on a benchmark function.
/// Benchmark names do not change: gungraun builds them from the file, the
/// group and the function.
mod benchmarks {
    use std::hint::black_box;

    use gungraun::{library_benchmark, library_benchmark_group};
    use taktsim_bench::random::{self, DRAWS, DistributionCase};
    use taktsim_core::random::{DurationDistribution, Stream};
    use taktsim_units::{SimDuration, UnitError};

    use crate::support;

    #[library_benchmark]
    #[bench::draws(setup = random::stream)]
    fn next_u64(mut stream: Stream) -> u64 {
        black_box(random::next_u64_n(black_box(&mut stream), DRAWS))
    }

    #[library_benchmark]
    #[bench::draws(setup = random::stream)]
    fn next_open01(mut stream: Stream) -> f64 {
        black_box(random::next_open01_n(black_box(&mut stream), DRAWS))
    }

    #[library_benchmark]
    #[bench::draws(setup = random::stream)]
    fn next_standard_normal(mut stream: Stream) -> f64 {
        black_box(random::next_standard_normal_n(
            black_box(&mut stream),
            DRAWS,
        ))
    }

    /// The distribution of `case` and a fresh benchmark stream.
    fn sampler(case: DistributionCase) -> (DurationDistribution, Stream) {
        (support::built(case.distribution()), random::stream())
    }

    /// The total, and the inputs handed on to the teardown.
    type Sampled = (
        Result<SimDuration, UnitError>,
        (DurationDistribution, Stream),
    );

    // One id per case of `DistributionCase::ALL`, named by `name()`;
    // tests/workloads.rs fails if a case is missing here.
    #[library_benchmark(setup = sampler, teardown = support::check)]
    #[bench::constant(DistributionCase::Constant)]
    #[bench::exponential(DistributionCase::Exponential)]
    #[bench::normal(DistributionCase::Normal)]
    #[bench::lognormal(DistributionCase::LogNormal)]
    #[bench::uniform(DistributionCase::Uniform)]
    #[bench::triangular(DistributionCase::Triangular)]
    #[bench::gamma(DistributionCase::Gamma)]
    #[bench::gamma_low_shape(DistributionCase::GammaLowShape)]
    #[bench::erlang(DistributionCase::Erlang)]
    #[bench::weibull(DistributionCase::Weibull)]
    fn sample(input: (DurationDistribution, Stream)) -> Sampled {
        let (distribution, mut stream) = input;
        let total = black_box(random::sample_n(
            black_box(&distribution),
            black_box(&mut stream),
            DRAWS,
        ));
        (total, (distribution, stream))
    }

    library_benchmark_group!(
        name = stream,
        benchmarks = [next_u64, next_open01, next_standard_normal]
    );
    library_benchmark_group!(name = distributions, benchmarks = [sample]);
}

use benchmarks::{distributions, stream};

gungraun::main!(library_benchmark_groups = stream, distributions);
