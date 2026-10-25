//! Distributions of durations (ADR-0003), sampled with platform-independent
//! math (ADR-0013).

use std::fmt;

use libm::{exp, log, pow};
use taktsim_units::{SimDuration, UnitError};

use super::Stream;

/// Why the parameters of a distribution are invalid. Each variant names
/// the distribution and parameter, carries the value found and one valid
/// example (spec §5), so a loader can build a field-path error from it.
///
/// ```
/// use taktsim_core::random::{DistributionError, DurationDistribution};
///
/// let err = DurationDistribution::lognormal("42s".parse()?, -0.5).unwrap_err();
/// assert!(matches!(
///     err,
///     DistributionError::NotPositiveNumber { parameter: "sigma", .. }
/// ));
/// assert_eq!(
///     err.to_string(),
///     "lognormal: `sigma` must be a finite number > 0, found -0.5; \
///      example: {dist: lognormal, mean: 42s, sigma: 0.08}"
/// );
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum DistributionError {
    /// A duration parameter is zero.
    #[error(
        "{distribution}: `{parameter}` must be longer than 0s, found {found}; example: {example}"
    )]
    NotPositive {
        /// The distribution, e.g. `exponential`.
        distribution: &'static str,
        /// The parameter, e.g. `mean`.
        parameter: &'static str,
        /// The value found.
        found: SimDuration,
        /// One valid distribution of this kind.
        example: &'static str,
    },
    /// A dimensionless parameter is not a finite number above zero.
    #[error(
        "{distribution}: `{parameter}` must be a finite number > 0, found {found}; example: {example}"
    )]
    NotPositiveNumber {
        /// The distribution, e.g. `lognormal`.
        distribution: &'static str,
        /// The parameter, e.g. `sigma`.
        parameter: &'static str,
        /// The value found.
        found: f64,
        /// One valid distribution of this kind.
        example: &'static str,
    },
    /// A bound is on the wrong side of another, e.g. `max <= min`.
    #[error("{distribution}: `{parameter}` must be {must_be}, found {found}; example: {example}")]
    OutOfOrder {
        /// The distribution, e.g. `uniform`.
        distribution: &'static str,
        /// The parameter, e.g. `max`.
        parameter: &'static str,
        /// The value found.
        found: SimDuration,
        /// The rule, e.g. "longer than `min`".
        must_be: &'static str,
        /// One valid distribution of this kind.
        example: &'static str,
    },
    /// An Erlang distribution with zero phases.
    #[error("erlang: `k` must be a whole number ≥ 1, found 0; example: {example}")]
    NoPhases {
        /// One valid Erlang distribution.
        example: &'static str,
    },
    /// A parameter so extreme that the values derived from it overflow or
    /// underflow `f64`, so the distribution cannot be sampled. Shown in
    /// scientific notation, e.g. `1e-160`.
    #[error(
        "{distribution}: `{parameter}` = {found:e} is too extreme to sample \
         (derived values overflow or vanish); example: {example}"
    )]
    TooExtreme {
        /// The distribution, e.g. `gamma`.
        distribution: &'static str,
        /// The parameter, e.g. `cv`.
        parameter: &'static str,
        /// The value found.
        found: f64,
        /// One valid distribution of this kind.
        example: &'static str,
    },
}

/// A distribution of durations, e.g. cycle times or repair times.
///
/// Build one with a constructor, which checks the parameters, then draw from
/// it with [`sample`](Self::sample). Every sample is rounded to the nearest
/// nanosecond and is never negative. Samples depend only on the stream, so
/// the same seed gives the same durations on every platform.
///
/// ```
/// use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
///
/// let cycle = DurationDistribution::lognormal("42s".parse()?, 0.08)?;
/// let mut stream = Stream::new(42, StreamKey::new("cut", "cycle_time"));
/// let first = cycle.sample(&mut stream)?;
/// assert!(first > "30s".parse()? && first < "60s".parse()?);
/// assert_eq!(cycle.to_string(), "{dist: lognormal, mean: 42s, sigma: 0.08}");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct DurationDistribution(Kind);

/// The parameters as given, plus values derived from them once, in seconds.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Constant(SimDuration),
    Exponential {
        mean: SimDuration,
        mean_s: f64,
    },
    Normal {
        mean: SimDuration,
        sd: SimDuration,
        mean_s: f64,
        sd_s: f64,
    },
    LogNormal {
        mean: SimDuration,
        sigma: f64,
        mu: f64,
    },
    Uniform {
        min: SimDuration,
        max: SimDuration,
        min_s: f64,
        width_s: f64,
    },
    Triangular {
        min: SimDuration,
        mode: SimDuration,
        max: SimDuration,
        min_s: f64,
        max_s: f64,
        /// The CDF at the mode.
        split: f64,
        /// (max - min) × (mode - min) and (max - min) × (max - mode).
        left: f64,
        right: f64,
    },
    Gamma {
        mean: SimDuration,
        cv: f64,
        sampler: GammaSampler,
    },
    /// Erlang(k) is exactly gamma with integer shape k, so it is sampled
    /// the same way, at the same cost for any k.
    Erlang {
        mean: SimDuration,
        k: u32,
        sampler: GammaSampler,
    },
    Weibull {
        scale: SimDuration,
        shape: f64,
        scale_s: f64,
        inv_shape: f64,
    },
}

/// Draws gamma(shape, scale) values (Marsaglia and Tsang, ACM TOMS 26(3),
/// 2000).
#[derive(Debug, Clone, PartialEq)]
struct GammaSampler {
    scale_s: f64,
    /// Marsaglia–Tsang constants for the shape (or shape + 1 when below 1).
    d: f64,
    c: f64,
    /// 1 / shape when the shape is below 1, for the boost u^(1/shape).
    boost: Option<f64>,
}

impl GammaSampler {
    /// The sampler for `shape` and `scale_s`; both must be finite and above
    /// zero, or sampling may never finish (inf - inf is NaN, which never
    /// passes the acceptance test). The constructors check this.
    fn new(shape: f64, scale_s: f64) -> Self {
        let (d, boost) = if shape < 1.0 {
            // Draw with shape + 1, then multiply by u^(1/shape).
            (shape + 2.0 / 3.0, Some(1.0 / shape))
        } else {
            (shape - 1.0 / 3.0, None)
        };
        Self {
            scale_s,
            d,
            c: 1.0 / (9.0 * d).sqrt(),
            boost,
        }
    }

    fn sample(&self, stream: &mut Stream) -> f64 {
        let (d, c) = (self.d, self.c);
        let g = loop {
            let z = stream.next_standard_normal();
            let v = 1.0 + c * z;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            if log(stream.next_open01()) < 0.5 * z * z + d - d * v + d * log(v) {
                break d * v;
            }
        };
        match self.boost {
            Some(inv_shape) => g * pow(stream.next_open01(), inv_shape) * self.scale_s,
            None => g * self.scale_s,
        }
    }
}

const EXPONENTIAL: &str = "{dist: exponential, mean: 12min}";
const NORMAL: &str = "{dist: normal, mean: 42s, sd: 3s}";
const LOGNORMAL: &str = "{dist: lognormal, mean: 42s, sigma: 0.08}";
const UNIFORM: &str = "{dist: uniform, min: 40s, max: 50s}";
const TRIANGULAR: &str = "{dist: triangular, min: 40s, mode: 42s, max: 50s}";
const GAMMA: &str = "{dist: gamma, mean: 12min, cv: 0.5}";
const ERLANG: &str = "{dist: erlang, mean: 12min, k: 3}";
const WEIBULL: &str = "{dist: weibull, scale: 6h, shape: 1.5}";

/// Checks that a duration parameter is longer than zero.
fn positive(
    distribution: &'static str,
    parameter: &'static str,
    value: SimDuration,
    example: &'static str,
) -> Result<SimDuration, DistributionError> {
    if value == SimDuration::ZERO {
        return Err(DistributionError::NotPositive {
            distribution,
            parameter,
            found: value,
            example,
        });
    }
    Ok(value)
}

/// Checks that a dimensionless parameter is finite and above zero.
fn positive_number(
    distribution: &'static str,
    parameter: &'static str,
    value: f64,
    example: &'static str,
) -> Result<f64, DistributionError> {
    if value.is_finite() && value > 0.0 {
        return Ok(value);
    }
    Err(DistributionError::NotPositiveNumber {
        distribution,
        parameter,
        found: value,
        example,
    })
}

/// Checks that values derived from `parameter` are all finite.
fn derived_finite(
    distribution: &'static str,
    parameter: &'static str,
    value: f64,
    derived: &[f64],
    example: &'static str,
) -> Result<(), DistributionError> {
    if derived.iter().all(|x| x.is_finite()) {
        return Ok(());
    }
    Err(DistributionError::TooExtreme {
        distribution,
        parameter,
        found: value,
        example,
    })
}

impl DurationDistribution {
    /// Always `duration`; what a bare quantity in a model means.
    ///
    /// ```
    /// use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
    ///
    /// let d = DurationDistribution::constant("45s".parse()?);
    /// let mut s = Stream::new(1, StreamKey::new("weld", "cycle_time"));
    /// assert_eq!(d.sample(&mut s)?, "45s".parse()?);
    /// assert_eq!(d.to_string(), "45s");
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn constant(duration: SimDuration) -> Self {
        Self(Kind::Constant(duration))
    }

    /// Exponential with the given `mean`, e.g. times between failures.
    ///
    /// # Errors
    ///
    /// [`DistributionError::NotPositive`] if `mean` is zero.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let mttr = DurationDistribution::exponential("12min".parse()?)?;
    /// assert_eq!(mttr.to_string(), "{dist: exponential, mean: 12min}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn exponential(mean: SimDuration) -> Result<Self, DistributionError> {
        let mean = positive("exponential", "mean", mean, EXPONENTIAL)?;
        Ok(Self(Kind::Exponential {
            mean,
            mean_s: mean.as_secs_f64(),
        }))
    }

    /// Normal with `mean` and standard deviation `sd`, truncated at zero:
    /// negative draws are rejected and drawn again, so the result follows
    /// the normal distribution conditioned on being non-negative (ADR-0013).
    ///
    /// # Errors
    ///
    /// [`DistributionError::NotPositive`] if `mean` or `sd` is zero. A
    /// positive mean keeps at least half of all draws, so sampling stays
    /// fast.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::normal("42s".parse()?, "3s".parse()?)?;
    /// assert_eq!(d.to_string(), "{dist: normal, mean: 42s, sd: 3s}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn normal(mean: SimDuration, sd: SimDuration) -> Result<Self, DistributionError> {
        let mean = positive("normal", "mean", mean, NORMAL)?;
        let sd = positive("normal", "sd", sd, NORMAL)?;
        Ok(Self(Kind::Normal {
            mean,
            sd,
            mean_s: mean.as_secs_f64(),
            sd_s: sd.as_secs_f64(),
        }))
    }

    /// Lognormal with the given `mean` (of the distribution itself, not of
    /// its logarithm) and shape `sigma` (the standard deviation of the
    /// logarithm). The logarithm has mean μ = ln(mean) − σ²/2.
    ///
    /// # Errors
    ///
    /// [`DistributionError::NotPositive`] if `mean` is zero,
    /// [`DistributionError::NotPositiveNumber`] unless `sigma` is a finite
    /// number above zero, [`DistributionError::TooExtreme`] if `sigma` is so
    /// large that μ overflows.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::lognormal("42s".parse()?, 0.08)?;
    /// assert_eq!(d.to_string(), "{dist: lognormal, mean: 42s, sigma: 0.08}");
    /// assert!(DurationDistribution::lognormal("42s".parse()?, 1e200).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn lognormal(mean: SimDuration, sigma: f64) -> Result<Self, DistributionError> {
        let mean = positive("lognormal", "mean", mean, LOGNORMAL)?;
        let sigma = positive_number("lognormal", "sigma", sigma, LOGNORMAL)?;
        let mu = log(mean.as_secs_f64()) - sigma * sigma / 2.0;
        derived_finite("lognormal", "sigma", sigma, &[mu], LOGNORMAL)?;
        Ok(Self(Kind::LogNormal { mean, sigma, mu }))
    }

    /// Uniform between `min` and `max`.
    ///
    /// # Errors
    ///
    /// [`DistributionError::OutOfOrder`] unless `min < max`.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::uniform("40s".parse()?, "50s".parse()?)?;
    /// assert_eq!(d.to_string(), "{dist: uniform, min: 40s, max: 50s}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn uniform(min: SimDuration, max: SimDuration) -> Result<Self, DistributionError> {
        if max <= min {
            return Err(DistributionError::OutOfOrder {
                distribution: "uniform",
                parameter: "max",
                found: max,
                must_be: "longer than `min`",
                example: UNIFORM,
            });
        }
        let min_s = min.as_secs_f64();
        Ok(Self(Kind::Uniform {
            min,
            max,
            min_s,
            width_s: max.as_secs_f64() - min_s,
        }))
    }

    /// Triangular between `min` and `max` with its peak at `mode`.
    ///
    /// # Errors
    ///
    /// [`DistributionError::OutOfOrder`] unless `min ≤ mode ≤ max` and
    /// `min < max`.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::triangular(
    ///     "40s".parse()?,
    ///     "42s".parse()?,
    ///     "50s".parse()?,
    /// )?;
    /// assert_eq!(d.to_string(), "{dist: triangular, min: 40s, mode: 42s, max: 50s}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn triangular(
        min: SimDuration,
        mode: SimDuration,
        max: SimDuration,
    ) -> Result<Self, DistributionError> {
        if mode < min {
            return Err(DistributionError::OutOfOrder {
                distribution: "triangular",
                parameter: "mode",
                found: mode,
                must_be: "at least `min`",
                example: TRIANGULAR,
            });
        }
        if max < mode || max == min {
            return Err(DistributionError::OutOfOrder {
                distribution: "triangular",
                parameter: "max",
                found: max,
                must_be: "at least `mode` and longer than `min`",
                example: TRIANGULAR,
            });
        }
        let (min_s, mode_s, max_s) = (min.as_secs_f64(), mode.as_secs_f64(), max.as_secs_f64());
        let range = max_s - min_s;
        Ok(Self(Kind::Triangular {
            min,
            mode,
            max,
            min_s,
            max_s,
            split: (mode_s - min_s) / range,
            left: range * (mode_s - min_s),
            right: range * (max_s - mode_s),
        }))
    }

    /// Gamma with the given `mean` and coefficient of variation `cv`
    /// (standard deviation / mean): shape k = 1/cv², scale = mean × cv².
    ///
    /// # Errors
    ///
    /// [`DistributionError::NotPositive`] if `mean` is zero,
    /// [`DistributionError::NotPositiveNumber`] unless `cv` is a finite
    /// number above zero, [`DistributionError::TooExtreme`] if `cv` is so
    /// small or large that the shape or scale overflows (beyond roughly
    /// 10⁻¹⁵⁴ or 10¹⁵⁴).
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::gamma("12min".parse()?, 0.5)?;
    /// assert_eq!(d.to_string(), "{dist: gamma, mean: 12min, cv: 0.5}");
    /// assert!(DurationDistribution::gamma("12min".parse()?, 1e-160).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn gamma(mean: SimDuration, cv: f64) -> Result<Self, DistributionError> {
        let mean = positive("gamma", "mean", mean, GAMMA)?;
        let cv = positive_number("gamma", "cv", cv, GAMMA)?;
        let shape = 1.0 / (cv * cv);
        let scale_s = mean.as_secs_f64() * cv * cv;
        // Shape, scale and 1/shape must all be finite and above zero.
        let usable = [shape, scale_s, 1.0 / shape]
            .iter()
            .all(|x| x.is_finite() && *x > 0.0);
        if !usable {
            return Err(DistributionError::TooExtreme {
                distribution: "gamma",
                parameter: "cv",
                found: cv,
                example: GAMMA,
            });
        }
        Ok(Self(Kind::Gamma {
            mean,
            cv,
            sampler: GammaSampler::new(shape, scale_s),
        }))
    }

    /// Erlang with the given `mean` and `k` phases: the sum of `k`
    /// exponential phases with mean `mean / k`, e.g. a repair made of
    /// several steps. Its coefficient of variation is 1/√k.
    ///
    /// Erlang(k) is exactly gamma with integer shape k, and is sampled that
    /// way, so a sample costs the same for any `k`.
    ///
    /// # Errors
    ///
    /// [`DistributionError::NotPositive`] if `mean` is zero,
    /// [`DistributionError::NoPhases`] if `k` is zero.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::erlang("12min".parse()?, 3)?;
    /// assert_eq!(d.to_string(), "{dist: erlang, mean: 12min, k: 3}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn erlang(mean: SimDuration, k: u32) -> Result<Self, DistributionError> {
        let mean = positive("erlang", "mean", mean, ERLANG)?;
        if k == 0 {
            return Err(DistributionError::NoPhases { example: ERLANG });
        }
        // k ≤ 2^32 is exact in f64, and mean / k stays above zero for any
        // mean of at least 1 ns.
        let shape = f64::from(k);
        Ok(Self(Kind::Erlang {
            mean,
            k,
            sampler: GammaSampler::new(shape, mean.as_secs_f64() / shape),
        }))
    }

    /// Weibull with the given `scale` and `shape`, e.g. wear-out failures
    /// (shape > 1).
    ///
    /// # Errors
    ///
    /// [`DistributionError::NotPositive`] if `scale` is zero,
    /// [`DistributionError::NotPositiveNumber`] unless `shape` is a finite
    /// number above zero, [`DistributionError::TooExtreme`] if `shape` is so
    /// small that 1/shape overflows.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::weibull("6h".parse()?, 1.5)?;
    /// assert_eq!(d.to_string(), "{dist: weibull, scale: 6h, shape: 1.5}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn weibull(scale: SimDuration, shape: f64) -> Result<Self, DistributionError> {
        let scale = positive("weibull", "scale", scale, WEIBULL)?;
        let shape = positive_number("weibull", "shape", shape, WEIBULL)?;
        let inv_shape = 1.0 / shape;
        derived_finite("weibull", "shape", shape, &[inv_shape], WEIBULL)?;
        Ok(Self(Kind::Weibull {
            scale,
            shape,
            scale_s: scale.as_secs_f64(),
            inv_shape,
        }))
    }

    /// One sample, drawn from `stream` and rounded to the nearest
    /// nanosecond. Uniform and triangular samples always lie in
    /// `[min, max]`, even where `f64` seconds are coarser than a nanosecond
    /// (beyond about 97 days).
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] or [`UnitError::NotFinite`] if a sample is
    /// longer than simulation time can hold (about 584 years), e.g. from an
    /// exponential with a mean of a century, or a Weibull with a very small
    /// shape.
    ///
    /// ```
    /// use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
    ///
    /// let d = DurationDistribution::uniform("40s".parse()?, "50s".parse()?)?;
    /// let mut s = Stream::new(3, StreamKey::new("asm", "cycle_time"));
    /// let x = d.sample(&mut s)?;
    /// assert!(x >= "40s".parse()? && x <= "50s".parse()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn sample(&self, stream: &mut Stream) -> Result<SimDuration, UnitError> {
        let secs = match &self.0 {
            Kind::Constant(duration) => return Ok(*duration),
            Kind::Exponential { mean_s, .. } => -mean_s * log(stream.next_open01()),
            Kind::Normal { mean_s, sd_s, .. } => loop {
                let x = mean_s + sd_s * stream.next_standard_normal();
                if x >= 0.0 {
                    break x;
                }
            },
            Kind::LogNormal { sigma, mu, .. } => exp(mu + sigma * stream.next_standard_normal()),
            Kind::Uniform {
                min,
                max,
                min_s,
                width_s,
            } => {
                let x = SimDuration::from_secs_f64(min_s + width_s * stream.next_open01())?;
                return Ok(x.clamp(*min, *max));
            }
            Kind::Triangular {
                min,
                max,
                min_s,
                max_s,
                split,
                left,
                right,
                ..
            } => {
                let u = stream.next_open01();
                let secs = if u < *split {
                    min_s + (u * left).sqrt()
                } else {
                    max_s - ((1.0 - u) * right).sqrt()
                };
                return Ok(SimDuration::from_secs_f64(secs)?.clamp(*min, *max));
            }
            Kind::Gamma { sampler, .. } | Kind::Erlang { sampler, .. } => sampler.sample(stream),
            Kind::Weibull {
                scale_s, inv_shape, ..
            } => scale_s * pow(-log(stream.next_open01()), *inv_shape),
        };
        SimDuration::from_secs_f64(secs)
    }
}

impl fmt::Display for DurationDistribution {
    /// The model-file form (ADR-0003): a bare duration for a constant,
    /// otherwise `{dist: …, …}` with the parameters as given.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Constant(d) => write!(f, "{d}"),
            Kind::Exponential { mean, .. } => write!(f, "{{dist: exponential, mean: {mean}}}"),
            Kind::Normal { mean, sd, .. } => {
                write!(f, "{{dist: normal, mean: {mean}, sd: {sd}}}")
            }
            Kind::LogNormal { mean, sigma, .. } => {
                write!(f, "{{dist: lognormal, mean: {mean}, sigma: {sigma}}}")
            }
            Kind::Uniform { min, max, .. } => {
                write!(f, "{{dist: uniform, min: {min}, max: {max}}}")
            }
            Kind::Triangular { min, mode, max, .. } => write!(
                f,
                "{{dist: triangular, min: {min}, mode: {mode}, max: {max}}}"
            ),
            Kind::Gamma { mean, cv, .. } => write!(f, "{{dist: gamma, mean: {mean}, cv: {cv}}}"),
            Kind::Erlang { mean, k, .. } => write!(f, "{{dist: erlang, mean: {mean}, k: {k}}}"),
            Kind::Weibull { scale, shape, .. } => {
                write!(f, "{{dist: weibull, scale: {scale}, shape: {shape}}}")
            }
        }
    }
}
