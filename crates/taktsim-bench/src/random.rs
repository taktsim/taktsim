//! Workloads for `taktsim_core::random`: raw stream draws, and one sample
//! loop per duration distribution.
//!
//! Every workload folds its draws into one value (a sum), so the compiler
//! cannot drop any of them, and a test can check the value against theory.
//!
//! ```
//! use taktsim_bench::random;
//!
//! let mut stream = random::stream();
//! let mean = random::next_open01_n(&mut stream, random::DRAWS) / f64::from(random::DRAWS);
//! assert!((mean - 0.5).abs() < 0.05);
//! ```

use taktsim_core::random::{DistributionError, DurationDistribution, Stream, StreamKey};
use taktsim_units::{SimDuration, UnitError};

/// The seed of every benchmark stream, so every run draws the same values.
///
/// ```
/// use taktsim_bench::random::{self, SEED};
/// use taktsim_core::random::{Stream, StreamKey};
///
/// let mut a = random::stream();
/// let mut b = Stream::new(SEED, StreamKey::new("bench", "draws"));
/// assert_eq!(a.next_u64(), b.next_u64());
/// ```
pub const SEED: u64 = 20_261_025;

/// Draws (or samples) per benchmark run: enough that the loop, not the
/// call around it, dominates an instruction count, and few enough that one
/// Callgrind run takes well under a second.
///
/// ```
/// assert_eq!(taktsim_bench::random::DRAWS, 1_000);
/// ```
pub const DRAWS: u32 = 1_000;

/// A new stream with [`SEED`] and the key `bench`/`draws`; the same stream
/// on every call.
///
/// ```
/// use taktsim_bench::random;
///
/// assert_eq!(random::stream().next_u64(), random::stream().next_u64());
/// ```
#[must_use]
pub fn stream() -> Stream {
    Stream::new(SEED, StreamKey::new("bench", "draws"))
}

/// The wrapping sum of `n` values of [`Stream::next_u64`].
///
/// ```
/// use taktsim_bench::random;
///
/// let mut by_hand = random::stream();
/// let expected = by_hand.next_u64().wrapping_add(by_hand.next_u64());
/// assert_eq!(random::next_u64_n(&mut random::stream(), 2), expected);
/// ```
pub fn next_u64_n(stream: &mut Stream, n: u32) -> u64 {
    (0..n).fold(0, |sum, _| sum.wrapping_add(stream.next_u64()))
}

/// The sum of `n` values of [`Stream::next_open01`].
///
/// ```
/// use taktsim_bench::random;
///
/// let sum = random::next_open01_n(&mut random::stream(), 3);
/// assert!(sum > 0.0 && sum < 3.0);
/// ```
pub fn next_open01_n(stream: &mut Stream, n: u32) -> f64 {
    (0..n).fold(0.0, |sum, _| sum + stream.next_open01())
}

/// The sum of `n` values of [`Stream::next_standard_normal`].
///
/// ```
/// use taktsim_bench::random;
///
/// let sum = random::next_standard_normal_n(&mut random::stream(), 2);
/// assert!(sum.is_finite());
/// ```
pub fn next_standard_normal_n(stream: &mut Stream, n: u32) -> f64 {
    (0..n).fold(0.0, |sum, _| sum + stream.next_standard_normal())
}

/// The total of `n` samples of `distribution`.
///
/// # Errors
///
/// The [`UnitError`] of the first sample, or of the first sum, that does
/// not fit in a [`SimDuration`]. Never for the [`DistributionCase`]s with
/// [`DRAWS`] samples (tested).
///
/// ```
/// use taktsim_bench::random::{self, DistributionCase};
///
/// let d = DistributionCase::Constant.distribution()?;
/// assert_eq!(random::sample_n(&d, &mut random::stream(), 4)?, "3min".parse()?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn sample_n(
    distribution: &DurationDistribution,
    stream: &mut Stream,
    n: u32,
) -> Result<SimDuration, UnitError> {
    (0..n).try_fold(SimDuration::ZERO, |total, _| {
        total.checked_add(distribution.sample(stream)?)
    })
}

/// One benchmarked distribution per kind in ADR-0003, plus a gamma with a
/// shape below 1, which is sampled on a second code path. Parameters are
/// the examples the constructors' errors suggest: typical cycle, repair and
/// failure times.
///
/// ```
/// use taktsim_bench::random::DistributionCase;
///
/// assert_eq!(DistributionCase::ALL.len(), 10);
/// assert_eq!(DistributionCase::Erlang.name(), "erlang");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionCase {
    /// Always 45 s.
    Constant,
    /// Exponential, mean 12 min.
    Exponential,
    /// Normal truncated at zero, mean 42 s, standard deviation 3 s.
    Normal,
    /// Lognormal, mean 42 s, sigma 0.08.
    LogNormal,
    /// Uniform between 40 s and 50 s.
    Uniform,
    /// Triangular between 40 s and 50 s, mode 42 s.
    Triangular,
    /// Gamma, mean 12 min, coefficient of variation 0.5 (shape 4).
    Gamma,
    /// Gamma, mean 12 min, coefficient of variation 2 (shape 0.25, below 1,
    /// so each sample also draws the boost factor).
    GammaLowShape,
    /// Erlang, mean 12 min, 3 phases.
    Erlang,
    /// Weibull, scale 6 h, shape 1.5.
    Weibull,
}

impl DistributionCase {
    /// Every case, in declaration order.
    ///
    /// ```
    /// use taktsim_bench::random::DistributionCase;
    ///
    /// assert_eq!(DistributionCase::ALL[0], DistributionCase::Constant);
    /// ```
    pub const ALL: [Self; 10] = [
        Self::Constant,
        Self::Exponential,
        Self::Normal,
        Self::LogNormal,
        Self::Uniform,
        Self::Triangular,
        Self::Gamma,
        Self::GammaLowShape,
        Self::Erlang,
        Self::Weibull,
    ];

    /// A short name for benchmark ids and reports.
    ///
    /// ```
    /// use taktsim_bench::random::DistributionCase;
    ///
    /// assert_eq!(DistributionCase::GammaLowShape.name(), "gamma_low_shape");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Constant => "constant",
            Self::Exponential => "exponential",
            Self::Normal => "normal",
            Self::LogNormal => "lognormal",
            Self::Uniform => "uniform",
            Self::Triangular => "triangular",
            Self::Gamma => "gamma",
            Self::GammaLowShape => "gamma_low_shape",
            Self::Erlang => "erlang",
            Self::Weibull => "weibull",
        }
    }

    /// The distribution of this case.
    ///
    /// # Errors
    ///
    /// The constructor's [`DistributionError`]; never for these parameters
    /// (tested).
    ///
    /// ```
    /// use taktsim_bench::random::DistributionCase;
    ///
    /// let d = DistributionCase::Weibull.distribution()?;
    /// assert_eq!(d.to_string(), "{dist: weibull, scale: 6h, shape: 1.5}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn distribution(self) -> Result<DurationDistribution, DistributionError> {
        match self {
            Self::Constant => Ok(DurationDistribution::constant(secs(45))),
            Self::Exponential => DurationDistribution::exponential(secs(720)),
            Self::Normal => DurationDistribution::normal(secs(42), secs(3)),
            Self::LogNormal => DurationDistribution::lognormal(secs(42), 0.08),
            Self::Uniform => DurationDistribution::uniform(secs(40), secs(50)),
            Self::Triangular => DurationDistribution::triangular(secs(40), secs(42), secs(50)),
            Self::Gamma => DurationDistribution::gamma(secs(720), 0.5),
            Self::GammaLowShape => DurationDistribution::gamma(secs(720), 2.0),
            Self::Erlang => DurationDistribution::erlang(secs(720), 3),
            Self::Weibull => DurationDistribution::weibull(secs(6 * 3_600), 1.5),
        }
    }
}

/// `n` whole seconds, for the parameters above (all far below the limit).
const fn secs(n: u64) -> SimDuration {
    SimDuration::from_nanos(n * 1_000_000_000)
}
