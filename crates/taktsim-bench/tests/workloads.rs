//! The benchmark workloads do the work they claim, on inputs that never
//! fail. The gungraun benchmarks rely on this: they stop the run if an input
//! fails (`benches/support/mod.rs`), and a failure found here first is
//! easier to read.
//!
//! Expected values come from the distributions' theory or from hand
//! arithmetic on the inputs, not from running the workloads.

use proptest::prelude::*;
use taktsim_bench::random::{self, DRAWS, DistributionCase, SEED};
use taktsim_bench::units::{
    self, COUNT_TEXTS, COUNTS, DURATION_TEXTS, DURATIONS, RATE_TEXTS, RATIO_TEXTS,
};
use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
use taktsim_units::{Count, Rate, Ratio, SimDuration, UnitError};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Theoretical mean and standard deviation of one sample, rounded to the
/// nanosecond (far finer than the 5-standard-error bounds below).
struct Theory {
    mean: SimDuration,
    sd: SimDuration,
}

/// The theory of each case.
///
/// Truncating the normal at zero changes nothing visible: zero is 14
/// standard deviations below the mean. Lognormal: sd = mean × √(e^σ² − 1).
/// Triangular: var = (a² + b² + c² − ab − ac − bc) / 18 = 84 / 18 s². Gamma
/// and Erlang: sd = cv × mean, cv = 1/√k for Erlang. Weibull with scale λ
/// and shape k: mean = λ Γ(1 + 1/k), var = λ² (Γ(1 + 2/k) − Γ(1 + 1/k)²),
/// with Γ(5/3) = 0.902745292950934 and Γ(7/3) = (4/3) Γ(4/3) =
/// (4/3) × 0.892979511569249 (Abramowitz and Stegun, table 6.1).
fn theory(case: DistributionCase) -> Result<Theory, UnitError> {
    let (mean, sd) = match case {
        DistributionCase::Constant => (45.0, 0.0),
        DistributionCase::Exponential => (720.0, 720.0),
        DistributionCase::Normal => (42.0, 3.0),
        DistributionCase::LogNormal => (42.0, 42.0 * (libm::exp(0.08 * 0.08) - 1.0).sqrt()),
        DistributionCase::Uniform => (45.0, 10.0 / 12.0_f64.sqrt()),
        DistributionCase::Triangular => (44.0, (84.0_f64 / 18.0).sqrt()),
        DistributionCase::Gamma => (720.0, 0.5 * 720.0),
        DistributionCase::GammaLowShape => (720.0, 2.0 * 720.0),
        DistributionCase::Erlang => (720.0, 720.0 / 3.0_f64.sqrt()),
        DistributionCase::Weibull => {
            let (lambda, g1): (f64, f64) = (21_600.0, 0.902_745_292_950_934);
            let g2 = 4.0 / 3.0 * 0.892_979_511_569_249;
            (lambda * g1, lambda * (g2 - g1 * g1).sqrt())
        }
    };
    Ok(Theory {
        mean: SimDuration::from_secs_f64(mean)?,
        sd: SimDuration::from_secs_f64(sd)?,
    })
}

#[test]
fn every_case_builds_with_the_parameters_its_docs_name() {
    let expected = [
        "45s",
        "{dist: exponential, mean: 12min}",
        "{dist: normal, mean: 42s, sd: 3s}",
        "{dist: lognormal, mean: 42s, sigma: 0.08}",
        "{dist: uniform, min: 40s, max: 50s}",
        "{dist: triangular, min: 40s, mode: 42s, max: 50s}",
        "{dist: gamma, mean: 12min, cv: 0.5}",
        "{dist: gamma, mean: 12min, cv: 2}",
        "{dist: erlang, mean: 12min, k: 3}",
        "{dist: weibull, scale: 6h, shape: 1.5}",
    ];
    for (case, text) in DistributionCase::ALL.into_iter().zip(expected) {
        assert_eq!(case.distribution().unwrap().to_string(), text, "{case:?}");
    }
}

/// `ALL` lists every variant once, in declaration order. The match has no
/// wildcard, so a new variant does not compile until it is added here, and
/// then this test fails until it is in `ALL`.
#[test]
fn all_lists_every_case() {
    let position = |case: DistributionCase| match case {
        DistributionCase::Constant => 0,
        DistributionCase::Exponential => 1,
        DistributionCase::Normal => 2,
        DistributionCase::LogNormal => 3,
        DistributionCase::Uniform => 4,
        DistributionCase::Triangular => 5,
        DistributionCase::Gamma => 6,
        DistributionCase::GammaLowShape => 7,
        DistributionCase::Erlang => 8,
        DistributionCase::Weibull => 9,
    };
    for (i, case) in DistributionCase::ALL.into_iter().enumerate() {
        assert_eq!(position(case), i, "{case:?}");
    }
}

/// Every case is a gated benchmark: the gungraun file has a
/// `#[bench::<name>(DistributionCase::<Variant>)]` line for each, so a
/// case cannot be added without its instruction count being gated.
#[test]
fn every_case_is_a_gated_benchmark() {
    let source = include_str!("../benches/instructions_random.rs");
    let ids: Vec<&str> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#[bench::"))
        .filter(|rest| rest.contains("(DistributionCase::"))
        .collect();
    for case in DistributionCase::ALL {
        let line = format!("{}(DistributionCase::{case:?})]", case.name());
        assert!(
            ids.contains(&line.as_str()),
            "no #[bench::{line} in instructions_random.rs"
        );
    }
    assert_eq!(ids.len(), DistributionCase::ALL.len(), "{ids:?}");
}

#[test]
fn case_names_are_unique_benchmark_ids() {
    let mut names: Vec<&str> = DistributionCase::ALL.iter().map(|c| c.name()).collect();
    assert!(
        names
            .iter()
            .all(|n| { !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_') })
    );
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), DistributionCase::ALL.len());
}

/// The benchmarks' workload, run exactly as a gungraun benchmark runs it
/// (a fresh stream, `DRAWS` samples): it succeeds, and its mean is within
/// 5 standard errors of the theoretical mean. For a normal sample mean,
/// that bound fails with probability 6e-7; the seed is fixed, so the
/// result never changes.
#[test]
fn sample_n_draws_from_each_distribution() {
    for case in DistributionCase::ALL {
        let distribution = case.distribution().unwrap();
        let total = random::sample_n(&distribution, &mut random::stream(), DRAWS).unwrap();
        let Theory { mean, sd } = theory(case).unwrap();
        let n = f64::from(DRAWS);
        let observed = SimDuration::from_secs_f64(total.as_secs_f64() / n).unwrap();
        let bound = SimDuration::from_secs_f64(5.0 * sd.as_secs_f64() / n.sqrt()).unwrap();
        let error = observed.max(mean).checked_sub(observed.min(mean)).unwrap();
        assert!(
            error <= bound,
            "{case:?}: sample mean {observed}, theory {mean} ± {bound}"
        );
    }
}

#[test]
fn constant_total_is_exact() {
    let d = DistributionCase::Constant.distribution().unwrap();
    let total = random::sample_n(&d, &mut random::stream(), DRAWS).unwrap();
    assert_eq!(total.as_nanos(), 45 * u64::from(DRAWS) * NANOS_PER_SEC);
}

#[test]
fn sample_n_reports_an_overflowing_total() {
    // 2^64 ns is about 584 years; 1,000 samples of a year cannot fit.
    let year = SimDuration::from_secs(365 * 86_400).unwrap();
    let d = DurationDistribution::constant(year);
    assert!(random::sample_n(&d, &mut random::stream(), 1_000).is_err());
    assert!(random::sample_n(&d, &mut random::stream(), 584).is_ok());
}

/// Means of `DRAWS` uniforms and normals within 5 standard errors:
/// sd(U(0, 1)) = 1/√12, sd(N(0, 1)) = 1.
#[test]
fn stream_workloads_draw_uniforms_and_normals() {
    let n = f64::from(DRAWS);
    let u = random::next_open01_n(&mut random::stream(), DRAWS) / n;
    assert!((u - 0.5).abs() <= 5.0 / 12.0_f64.sqrt() / n.sqrt(), "{u}");
    let z = random::next_standard_normal_n(&mut random::stream(), DRAWS) / n;
    assert!(z.abs() <= 5.0 / n.sqrt(), "{z}");
    // Top bit of each u64: a fair coin, mean 1/2, sd 1/2.
    let mut s = random::stream();
    let ones = (0..DRAWS).filter(|_| s.next_u64() >> 63 == 1).count();
    let share = f64::from(u32::try_from(ones).unwrap()) / n;
    assert!((share - 0.5).abs() <= 5.0 * 0.5 / n.sqrt(), "{share}");
}

#[test]
fn the_benchmark_stream_is_fixed() {
    let mut a = random::stream();
    let mut b = Stream::new(SEED, StreamKey::new("bench", "draws"));
    assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
}

proptest! {
    /// Each stream workload draws exactly `n` values: the stream is then at
    /// the same place as one that drew `n` by hand, and the folded value is
    /// the fold of those draws.
    #[test]
    fn stream_workloads_draw_exactly_n(n in 0_u32..200) {
        let mut by_hand = random::stream();
        let expected = (0..n).fold(0_u64, |s, _| s.wrapping_add(by_hand.next_u64()));
        let mut stream = random::stream();
        prop_assert_eq!(random::next_u64_n(&mut stream, n), expected);
        prop_assert_eq!(stream.next_u64(), by_hand.next_u64());

        // Same additions in the same order, so the sums agree bit for bit.
        let mut by_hand = random::stream();
        let expected = (0..n).fold(0.0, |s, _| s + by_hand.next_open01());
        let mut stream = random::stream();
        prop_assert_eq!(random::next_open01_n(&mut stream, n).to_bits(), expected.to_bits());
        prop_assert_eq!(stream.next_u64(), by_hand.next_u64());

        let mut by_hand = random::stream();
        let expected = (0..n).fold(0.0, |s, _| s + by_hand.next_standard_normal());
        let mut stream = random::stream();
        prop_assert_eq!(
            random::next_standard_normal_n(&mut stream, n).to_bits(),
            expected.to_bits()
        );
        prop_assert_eq!(
            stream.next_standard_normal().to_bits(),
            by_hand.next_standard_normal().to_bits()
        );
    }

    /// A constant's total is the constant times `n`, by integer arithmetic.
    #[test]
    fn constant_total_is_n_times_the_constant(nanos in 0_u64..1_000_000_000_000, n in 0_u32..1_000) {
        let d = DurationDistribution::constant(SimDuration::from_nanos(nanos));
        let total = random::sample_n(&d, &mut random::stream(), n).unwrap();
        prop_assert_eq!(total.as_nanos(), nanos * u64::from(n));
    }

    /// Formatting replaces the buffer's contents with the values' `Display`
    /// text, whatever the buffer held.
    #[test]
    fn format_all_writes_the_display_text(nanos in prop::collection::vec(0_u64..u64::MAX, 0..10), old in ".{0,20}") {
        let values: Vec<SimDuration> = nanos.into_iter().map(SimDuration::from_nanos).collect();
        let mut buffer = old;
        let length = units::format_all(&values, &mut buffer).unwrap();
        let expected: String = values.iter().map(ToString::to_string).collect();
        prop_assert_eq!(length, expected.len());
        prop_assert_eq!(buffer, expected);
    }
}

#[test]
fn every_text_parses_to_the_value_it_names() {
    let s = |n: u64| n * NANOS_PER_SEC;
    let durations: Vec<SimDuration> = units::parsed(&DURATION_TEXTS).unwrap();
    let nanos: Vec<u64> = durations.iter().map(|d| d.as_nanos()).collect();
    assert_eq!(
        nanos,
        [
            s(42),
            s(720),
            s(1_800),
            s(28_800),
            s(172_800),
            250_000_000,
            1_250_000_000,
            s(5_400)
        ]
    );

    let counts: Vec<Count> = units::parsed(&COUNT_TEXTS).unwrap();
    assert_eq!(
        counts.iter().map(|c| c.get()).collect::<Vec<_>>(),
        [20, 1, 500, 12]
    );

    let rates: Vec<Rate> = units::parsed(&RATE_TEXTS).unwrap();
    let per_hour: Vec<f64> = rates.iter().map(|r| r.as_per_hour()).collect();
    for (got, want) in per_hour.iter().zip([120.0, 120.0, 1_800.0, 1.0]) {
        assert!((got - want).abs() < 1e-9, "{got} parts/h, expected {want}");
    }

    let ratios: Vec<Ratio> = units::parsed(&RATIO_TEXTS).unwrap();
    let percent: Vec<f64> = ratios.iter().map(|r| r.as_percent()).collect();
    assert_eq!(percent, [2.0, 0.5, 95.0, 100.0]);

    assert!(units::parse_all::<SimDuration>(&DURATION_TEXTS).is_ok());
    assert!(units::parse_all::<Count>(&COUNT_TEXTS).is_ok());
    assert!(units::parse_all::<Rate>(&RATE_TEXTS).is_ok());
    assert!(units::parse_all::<Ratio>(&RATIO_TEXTS).is_ok());
}

#[test]
fn parse_all_stops_at_the_first_bad_text() {
    assert!(units::parse_all::<SimDuration>(&["42s", "42 parsecs", "1h"]).is_err());
    assert!(units::parsed::<Count>(&["1 part", "1.5 parts"]).is_err());
}

#[test]
fn formats_name_the_values_in_the_model_syntax() {
    let mut buffer = String::new();
    units::format_all(&DURATIONS, &mut buffer).unwrap();
    assert_eq!(buffer, "42s12min30min8h2d0.25s1.25s0.000000001s");
    units::format_all(&COUNTS, &mut buffer).unwrap();
    assert_eq!(buffer, "20 parts1 parts500 parts12 parts");
}

#[test]
fn formatted_rates_and_ratios_parse_back() {
    let mut buffer = String::new();
    for rate in units::parsed::<Rate>(&RATE_TEXTS).unwrap() {
        units::format_all(&[rate], &mut buffer).unwrap();
        assert_eq!(buffer.parse::<Rate>().unwrap(), rate);
    }
    for ratio in units::parsed::<Ratio>(&RATIO_TEXTS).unwrap() {
        units::format_all(&[ratio], &mut buffer).unwrap();
        assert_eq!(buffer.parse::<Ratio>().unwrap(), ratio);
    }
}

/// The event-queue workloads (`taktsim_bench::queue`). Expected values are
/// derived by hand from the models, as the comments of each test show.
mod queue {
    use taktsim_bench::queue::{
        self, HOLD_MAX, HOLD_POPULATIONS, MACHINE_COUNTS, MachineParams, Msg, TIED_POPULATIONS,
        Tally, WAVE_CASES, WAVE_INTERVAL,
    };
    use taktsim_core::random::DurationDistribution;
    use taktsim_core::{Engine, RunLimits, RunOutcome, TraceRecord, TraceSink};
    use taktsim_units::{SimDuration, SimTime};

    const SECOND: u64 = 1_000_000_000;

    fn secs(n: u64) -> SimDuration {
        SimDuration::from_nanos(n * SECOND)
    }

    fn at(n: u64) -> SimTime {
        SimTime::from_nanos(n * SECOND)
    }

    /// Keeps every delivered event's time and message, in order.
    #[derive(Default)]
    struct Order(Vec<(SimTime, Msg)>);

    impl TraceSink<Msg> for Order {
        fn record(&mut self, record: &TraceRecord<'_, Msg>) {
            if let TraceRecord::Event { time, message, .. } = record {
                self.0.push((*time, **message));
            }
        }
    }

    /// With a constant increment of 1 s and N pending events, the engine
    /// processes N events at each whole second: the first N at 1 s (their
    /// sequence numbers 0..N), and the event with sequence number s
    /// schedules the one with sequence number N + s, so events pop in
    /// sequence order, and the n-th (from 0) has sequence number n and time
    /// (n / N + 1) s. After m * N events, the time is m s. The checksum is
    /// that order folded by hand.
    #[test]
    fn tied_hold_pops_in_scheduling_order() {
        for n in [1_u64, 7, 100] {
            let rounds = 5;
            let mut engine = queue::hold_tied(u32::try_from(n).unwrap()).unwrap();
            let outcome = queue::run_events(&mut engine, rounds * n).unwrap();
            let mut expected = Tally::new();
            for i in 0..rounds * n {
                expected.absorb(i, at(i / n + 1));
            }
            assert_eq!(outcome.tally.checksum, expected.checksum, "N = {n}");
            assert_eq!(outcome.summary.events, rounds * n);
            assert_eq!(outcome.summary.outcome, RunOutcome::EventLimit);
            assert_eq!(outcome.summary.now, at(rounds));
            assert_eq!(outcome.tally.hops, rounds * n);
            assert_eq!(outcome.tally.events, rounds * n);
        }
    }

    /// The tied workload as benchmarked: 10 000 events, N = 100 and
    /// N = 10 000, so 100 rounds and 1 round, ending at 100 s and 1 s.
    #[test]
    fn tied_hold_at_the_benchmarked_sizes() {
        for n in TIED_POPULATIONS {
            let mut engine = queue::hold_tied(n).unwrap();
            let outcome = queue::run_events(&mut engine, queue::EVENTS).unwrap();
            assert_eq!(outcome.tally.hops, queue::EVENTS);
            assert_eq!(
                outcome.summary.now,
                at(queue::EVENTS / u64::from(n)),
                "N = {n}"
            );
        }
    }

    /// Runs up to a time instead of an event count: events at exactly the
    /// horizon are included, so 3 s with 2 pending events is 6 events.
    #[test]
    fn run_until_includes_the_horizon() {
        let mut engine = queue::hold_tied(2).unwrap();
        let outcome = queue::run_until(&mut engine, at(3)).unwrap();
        assert_eq!(outcome.summary.events, 6);
        assert_eq!(outcome.summary.outcome, RunOutcome::Horizon);
        assert_eq!(outcome.summary.now, at(3));
    }

    // Time of the K-th event of the hold model with uniform increments,
    // against renewal theory, with no allowance beyond a few standard
    // deviations.
    // In the code: K is `events`, N is `chains`, a is `scale`, x is `frac`.
    //
    // The increments are uniform on 1..=S ns, S = `HOLD_MAX` in ns. Take
    // them as uniform on (0, a] with a = S: each differs from a continuous
    // uniform by less than 1 ns, which moves the K-th event by less than
    // K ns in total (at most 1e4 ns, against standard deviations of 8e6 ns
    // and more below), so it is ignored. Their mean is mu = (S + 1) / 2 ns.
    // The population stays exactly N, so a run of K events never ends early
    // (the event count is exactly K), and the K-th event is the one at which
    // the N chains together have K events. Chain i is an ordinary renewal
    // process: its k-th event is the partial sum S_k of k increments, and
    // N_i(t) counts the S_k <= t.
    //
    // *Up to t = a* (K / N <= e - 1, so N = 10 000 at K = 10 000): S_k <= t
    // needs every one of k uniforms below t, and the partial sums of
    // uniforms on (0, 1] satisfy P(S_k <= x) = x^k / k! for x <= 1
    // (Irwin-Hall). With x = t / a:
    //
    // - m(t) = E N_i(t) = sum over k >= 1 of x^k / k! = e^x - 1;
    // - E N_i(t)^2 = sum of (2k - 1) P(S_k <= t) = 2 x e^x - (e^x - 1),
    //   so Var N_i(t) = 2 x e^x + e^x - e^(2x).
    //
    // The N chains are independent, so the total count has mean N m(t) and
    // variance N Var N_i(t). Setting N m(T) = K gives T = a ln(1 + K / N),
    // and since the count changes at rate N m'(T) = N e^x / a there, T has
    // standard deviation sqrt(N Var N_i(T)) a / (N e^x).
    //
    // *Beyond t = a* the renewal theorem gives the same quantities with an
    // error that, for the uniform distribution, falls exponentially in t:
    // m(t) = t / mu - 1/3 and Var N_i(t) = t sigma^2 / mu^3 + c with
    // sigma^2 = mu^2 / 3 and a constant c. The exact values were computed from
    // the Irwin-Hall distribution function (rational arithmetic, all terms
    // up to k = 6 t / a + 80) for N = 1 000, 500 and 100 (K = 10 000): at
    // N = 1 000, where T is 10.3 mu, the exact T and the asymptotic
    // T = (K / N + 1/3) mu differ by 1.5e-6 a (5e-5 standard deviations); at
    // N = 500 and N = 100 by less than 1e-9 a. The variance constant comes
    // out as c = 2/9 in all three, so Var N_i = (K / N + 1) / 3 and T has
    // standard deviation sqrt((K + N) / 3) mu / N. (Without c it would be
    // 5 % smaller at N = 1 000.) N = 1 and N = 10 are further out still.
    //
    // The bound is 5 standard deviations; the seed is fixed.
    #[test]
    #[allow(clippy::cast_precision_loss)] // counts far below 2^53
    fn random_hold_runs_at_the_rate_of_theory() {
        let events = queue::EVENTS as f64;
        let scale = HOLD_MAX.as_nanos() as f64;
        let mu = f64::midpoint(scale, 1.0);
        for population in HOLD_POPULATIONS {
            let mut engine = queue::hold_random(population).unwrap();
            let outcome = queue::run_events(&mut engine, queue::EVENTS).unwrap();
            assert_eq!(outcome.summary.events, queue::EVENTS, "N = {population}");
            assert_eq!(outcome.tally.hops, queue::EVENTS);
            let chains = f64::from(population);
            let per_chain = events / chains;
            let (expected, sd) = if per_chain <= std::f64::consts::E - 1.0 {
                let frac = libm::log(1.0 + per_chain);
                let exp_frac = libm::exp(frac);
                let var = 2.0 * frac * exp_frac + exp_frac - exp_frac * exp_frac;
                (
                    scale * frac,
                    (chains * var).sqrt() * scale / (chains * exp_frac),
                )
            } else {
                (
                    (per_chain + 1.0 / 3.0) * mu,
                    ((events + chains) / 3.0).sqrt() * mu / chains,
                )
            };
            let found = outcome.summary.now.as_nanos() as f64;
            let z = (found - expected) / sd;
            assert!(
                z.abs() <= 5.0,
                "N = {population}: ended at {found} ns, expected {expected} ns \
                 with sd {sd} ns (z = {z})"
            );
        }
    }

    /// A run continues where the previous call stopped: two calls of 120
    /// and 80 events end where one call of 200 events ends.
    #[test]
    fn runs_continue_where_they_stopped() {
        let mut one = queue::hold_random(10).unwrap();
        let mut two = queue::hold_random(10).unwrap();
        let whole = queue::run_events(&mut one, 200).unwrap();
        let first = queue::run_events(&mut two, 120).unwrap();
        let second = queue::run_events(&mut two, 80).unwrap();
        assert_eq!(first.tally.events + second.tally.events, 200);
        assert_eq!(second.summary.now, whole.summary.now);
    }

    /// Same engine, same seed, same events, byte for byte: every workload
    /// run twice gives the same outcome, checksum included.
    #[test]
    fn every_workload_is_deterministic() {
        type Build = Box<dyn Fn() -> Engine<Msg>>;
        let builds: Vec<(&str, u64, Build)> = vec![
            (
                "hold",
                queue::EVENTS,
                Box::new(|| queue::hold_random(100).unwrap()),
            ),
            (
                "tied",
                queue::EVENTS,
                Box::new(|| queue::hold_tied(100).unwrap()),
            ),
            (
                "waves",
                queue::wave_events(100, 20),
                Box::new(|| queue::waves(100).unwrap()),
            ),
            (
                "cancel",
                queue::EVENTS,
                Box::new(|| queue::cancel_line(20).unwrap()),
            ),
        ];
        for (name, events, build) in builds {
            let a = queue::run_events(&mut build(), events).unwrap();
            let b = queue::run_events(&mut build(), events).unwrap();
            assert_eq!(a, b, "{name}");
            assert_eq!(a.tally.events, events, "{name}");
        }
    }

    /// The ticks and waves of one round: the tick, then the waves by
    /// priority class (lane % 3), within a class by lane (the order they
    /// were sent). With 7 lanes: class 0 is lanes 0, 3, 6; class 1 is 1,
    /// 4; class 2 is 2, 5. Round r (from 0) is at (r + 1) s.
    #[test]
    fn waves_pop_by_class_then_by_lane() {
        let lanes = 7_u32;
        let rounds = 3_u32;
        let mut engine = queue::waves(lanes).unwrap();
        let mut order = Order::default();
        let events = queue::wave_events(lanes, rounds);
        assert_eq!(events, 24);
        let summary = engine
            .run(RunLimits::until(at(u64::from(rounds))), &mut order)
            .unwrap();
        assert_eq!(summary.events, events);
        assert_eq!(summary.now, at(u64::from(rounds)));

        let by_class = [0_u32, 3, 6, 1, 4, 2, 5];
        let mut expected = Vec::new();
        for round in 0..u64::from(rounds) {
            let time = at(round + 1);
            expected.push((time, Msg::Tick));
            for lane in by_class {
                let class = u8::try_from(lane % 3).unwrap();
                expected.push((time, Msg::Wave { lane, class }));
            }
        }
        assert_eq!(order.0, expected);
        assert_eq!(WAVE_INTERVAL, secs(1));
    }

    /// Checksum and counts of the same waves run, folded by hand from the
    /// sequence numbers: the tick of round r is r (L + 1) (the tick, then L
    /// waves, then the next tick), and lane i of round r is
    /// r (L + 1) + 1 + i.
    #[test]
    fn waves_checksum_matches_the_hand_derived_order() {
        let (lanes, rounds) = (7_u64, 3_u64);
        let mut engine = queue::waves(7).unwrap();
        let outcome = queue::run_events(&mut engine, queue::wave_events(7, 3)).unwrap();
        let mut expected = Tally::new();
        for round in 0..rounds {
            let first = round * (lanes + 1);
            expected.absorb(first, at(round + 1));
            for lane in [0_u64, 3, 6, 1, 4, 2, 5] {
                expected.absorb(first + 1 + lane, at(round + 1));
            }
        }
        assert_eq!(outcome.tally.checksum, expected.checksum);
        assert_eq!((outcome.tally.ticks, outcome.tally.waves), (3, 21));
    }

    /// The benchmarked sizes have about 10 000 events, and each round
    /// delivers its tick and waves in full.
    #[test]
    fn wave_cases_are_about_ten_thousand_events() {
        for (lanes, rounds) in WAVE_CASES {
            let events = queue::wave_events(lanes, rounds);
            assert!((9_000..=11_000).contains(&events), "{lanes} lanes");
            let mut engine = queue::waves(lanes).unwrap();
            let outcome = queue::run_events(&mut engine, events).unwrap();
            assert_eq!(outcome.tally.ticks, u64::from(rounds));
            assert_eq!(outcome.tally.waves, u64::from(lanes) * u64::from(rounds));
        }
    }

    fn constant_machines(cycle: u64, failure: u64, repair: u64) -> MachineParams {
        MachineParams {
            cycle: DurationDistribution::constant(secs(cycle)),
            time_to_failure: DurationDistribution::constant(secs(failure)),
            repair: DurationDistribution::constant(secs(repair)),
        }
    }

    /// Cycles of 10 s against failures due after 1 h: no failure ever
    /// fires. Each machine finishes a cycle at 10, 20, ..., 100 s: 10
    /// events, each cancelling a failure timer that is 1 h away.
    #[test]
    fn cycles_that_always_win_cancel_every_failure_timer() {
        let machines = 5;
        let mut engine = queue::cancel_heavy(machines, &constant_machines(10, 3_600, 5)).unwrap();
        let outcome = queue::run_until(&mut engine, at(100)).unwrap();
        let tally = outcome.tally;
        assert_eq!(tally.cycles, 10 * u64::from(machines));
        assert_eq!((tally.failures, tally.repairs), (0, 0));
        assert_eq!(tally.events, tally.cycles);
    }

    /// Failures due after 4 s against cycles of 10 s: every race ends in a
    /// failure that cancels the cycle timer. A machine fails at 4, repairs
    /// until 10 (6 s), fails again at 14, ...: failures at 4, 14, ..., 94
    /// (10) and repairs at 10, 20, ..., 100 (10, the last at the horizon).
    #[test]
    fn failures_that_always_win_cancel_every_cycle_timer() {
        let machines = 5;
        let mut engine = queue::cancel_heavy(machines, &constant_machines(10, 4, 6)).unwrap();
        let outcome = queue::run_until(&mut engine, at(100)).unwrap();
        let tally = outcome.tally;
        assert_eq!(tally.failures, 10 * u64::from(machines));
        assert_eq!(tally.repairs, 10 * u64::from(machines));
        assert_eq!(tally.cycles, 0);
    }

    /// A cycle and a failure due at the same instant: the failure has the
    /// lower priority number, so it fires first and cancels the cycle; the
    /// cycle never completes. With 10 s, 10 s and 5 s repairs, a machine
    /// fails at 10, repairs at 15, fails at 25, ...: failures at 10 + 15 j,
    /// up to 100 s (j = 0..=6: 7) and repairs at 15 + 15 j up to 90 s (6).
    #[test]
    fn a_failure_wins_a_tie_with_a_cycle_end() {
        let machines = 4;
        let mut engine = queue::cancel_heavy(machines, &constant_machines(10, 10, 5)).unwrap();
        let outcome = queue::run_until(&mut engine, at(100)).unwrap();
        let tally = outcome.tally;
        assert_eq!(tally.failures, 7 * u64::from(machines));
        assert_eq!(tally.repairs, 6 * u64::from(machines));
        assert_eq!(tally.cycles, 0);
    }

    /// The benchmarked machines (`MachineParams::line`, exponential times):
    /// a race between a cycle clock with rate 1 per minute and a failure
    /// clock with rate 1/10 per minute is won by the failure with
    /// probability (1/10) / (1/10 + 1) = 1/11, independently for each race.
    /// With R races (cycle ends plus failures) the failures are binomial:
    /// sd sqrt(R p (1 - p)); the bound is 6 of them, plus the number of
    /// machines for races cut off by the event limit. A machine has at most
    /// one repair pending, so repairs <= failures <= repairs + machines.
    /// Every race end cancels one timer.
    #[test]
    #[allow(clippy::cast_precision_loss)] // counts far below 2^53
    fn the_line_fails_one_race_in_eleven() {
        for machines in MACHINE_COUNTS {
            let mut engine = queue::cancel_line(machines).unwrap();
            let outcome = queue::run_events(&mut engine, 20 * queue::EVENTS).unwrap();
            let tally = outcome.tally;
            assert_eq!(tally.events, 20 * queue::EVENTS);
            assert_eq!(tally.events, tally.cycles + tally.failures + tally.repairs);
            assert!(tally.repairs <= tally.failures);
            assert!(tally.failures <= tally.repairs + u64::from(machines));
            let races = (tally.cycles + tally.failures) as f64;
            let p = 1.0 / 11.0;
            let bound = 6.0 * (races * p * (1.0 - p)).sqrt() + f64::from(machines);
            assert!(
                (tally.failures as f64 - races * p).abs() <= bound,
                "{machines} machines: {} failures in {races} races, expected {} ± {bound}",
                tally.failures,
                races * p
            );
        }
    }

    /// The cancellation workload leaves stale failure timers in the queue
    /// and still hands out events in time order: the times of delivered
    /// events never decrease.
    #[test]
    fn cancelled_timers_never_fire() {
        let mut engine = queue::cancel_line(50).unwrap();
        let mut order = Order::default();
        let limits = RunLimits::until(SimTime::from_nanos(u64::MAX)).max_events(5_000);
        engine.run(limits, &mut order).unwrap();
        assert_eq!(order.0.len(), 5_000);
        assert!(order.0.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    /// A tally counts each kind once: a cancellation workload has no hops,
    /// ticks or waves.
    #[test]
    fn tally_counts_by_kind() {
        let mut engine = queue::cancel_line(10).unwrap();
        let tally = queue::run_events(&mut engine, 1_000).unwrap().tally;
        assert_eq!(tally.hops + tally.ticks + tally.waves, 0);
        assert_eq!(tally.events, 1_000);
        assert_ne!(tally.checksum, Tally::new().checksum);
    }

    /// Every size in the constants is a gated benchmark: the gungraun file
    /// has an id for it, so a size cannot be added without its instruction
    /// count being gated.
    #[test]
    fn every_size_is_a_gated_benchmark() {
        let source = include_str!("../benches/instructions_queue.rs");
        let mut wanted = Vec::new();
        wanted.extend(HOLD_POPULATIONS.map(|n| format!("#[bench::n{n}({n})]")));
        wanted.extend(TIED_POPULATIONS.map(|n| format!("#[bench::n{n}({n})]")));
        wanted.extend(WAVE_CASES.map(|(l, r)| format!("#[bench::n{l}({l}, {r})]")));
        wanted.extend(MACHINE_COUNTS.map(|m| format!("#[bench::m{m}({m})]")));
        for line in &wanted {
            assert!(
                source.lines().any(|l| l.trim() == line),
                "no {line} in instructions_queue.rs"
            );
        }
        let ids = source
            .lines()
            .filter(|l| l.trim().starts_with("#[bench::"))
            .count();
        assert_eq!(ids, wanted.len());
    }
}
