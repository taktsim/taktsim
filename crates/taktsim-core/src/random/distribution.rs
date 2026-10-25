//! Distributions of durations (ADR-0003), sampled with platform-independent
//! math (ADR-0013).

use std::f64::consts::TAU;
use std::fmt;

use libm::{cos, exp, log};
use taktsim_units::{SimDuration, UnitError};

use super::Stream;

/// The largest phase count of an Erlang distribution. Each sample draws `k`
/// values, so the bound keeps one sample cheap; for larger `k` a gamma or a
/// normal distribution is the better model anyway (ADR-0013).
pub const MAX_ERLANG_PHASES: u32 = 1000;

/// Why the parameters of a distribution are invalid: the parameter, what was
/// found, what is allowed and a valid example (spec §5).
///
/// ```
/// use taktsim_core::random::DurationDistribution;
///
/// let err = DurationDistribution::lognormal("42s".parse()?, -0.5).unwrap_err();
/// assert_eq!(
///     err.to_string(),
///     "lognormal: `sigma` must be a finite number > 0, found -0.5; \
///      example: {dist: lognormal, mean: 42s, sigma: 0.08}"
/// );
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{distribution}: `{parameter}` must be {allowed}, found {found}; example: {example}")]
pub struct DistributionError {
    /// The distribution, e.g. `lognormal`.
    pub distribution: &'static str,
    /// The parameter, e.g. `sigma`.
    pub parameter: &'static str,
    /// The value found.
    pub found: String,
    /// What is allowed, e.g. "a finite number > 0".
    pub allowed: &'static str,
    /// One valid distribution of this kind.
    pub example: &'static str,
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
        scale_s: f64,
        /// Marsaglia–Tsang constants for shape k (or k + 1 when k < 1).
        d: f64,
        c: f64,
        /// 1 / k when k < 1, for the boost u^(1/k).
        boost: Option<f64>,
    },
    Erlang {
        mean: SimDuration,
        k: u32,
        phase_mean_s: f64,
    },
    Weibull {
        scale: SimDuration,
        shape: f64,
        scale_s: f64,
        inv_shape: f64,
    },
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
) -> Result<f64, DistributionError> {
    if value == SimDuration::ZERO {
        return Err(DistributionError {
            distribution,
            parameter,
            found: value.to_string(),
            allowed: "longer than 0s",
            example,
        });
    }
    Ok(value.as_secs_f64())
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
    Err(DistributionError {
        distribution,
        parameter,
        found: value.to_string(),
        allowed: "a finite number > 0",
        example,
    })
}

/// An error for two parameters in the wrong order.
fn out_of_order(
    distribution: &'static str,
    parameter: &'static str,
    found: SimDuration,
    allowed: &'static str,
    example: &'static str,
) -> DistributionError {
    DistributionError {
        distribution,
        parameter,
        found: found.to_string(),
        allowed,
        example,
    }
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
    /// [`DistributionError`] if `mean` is zero.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let mttr = DurationDistribution::exponential("12min".parse()?)?;
    /// assert_eq!(mttr.to_string(), "{dist: exponential, mean: 12min}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn exponential(mean: SimDuration) -> Result<Self, DistributionError> {
        let mean_s = positive("exponential", "mean", mean, EXPONENTIAL)?;
        Ok(Self(Kind::Exponential { mean, mean_s }))
    }

    /// Normal with `mean` and standard deviation `sd`, truncated at zero:
    /// negative draws are rejected and drawn again, so the result follows
    /// the normal distribution conditioned on being non-negative.
    ///
    /// # Errors
    ///
    /// [`DistributionError`] if `mean` or `sd` is zero. A positive mean
    /// keeps at least half of all draws, so sampling stays fast.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::normal("42s".parse()?, "3s".parse()?)?;
    /// assert_eq!(d.to_string(), "{dist: normal, mean: 42s, sd: 3s}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn normal(mean: SimDuration, sd: SimDuration) -> Result<Self, DistributionError> {
        let mean_s = positive("normal", "mean", mean, NORMAL)?;
        let sd_s = positive("normal", "sd", sd, NORMAL)?;
        Ok(Self(Kind::Normal {
            mean,
            sd,
            mean_s,
            sd_s,
        }))
    }

    /// Lognormal with the given `mean` (of the distribution itself, not of
    /// its logarithm) and shape `sigma` (the standard deviation of the
    /// logarithm). The logarithm has mean μ = ln(mean) − σ²/2.
    ///
    /// # Errors
    ///
    /// [`DistributionError`] if `mean` is zero or `sigma` is not a finite
    /// number above zero.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::lognormal("42s".parse()?, 0.08)?;
    /// assert_eq!(d.to_string(), "{dist: lognormal, mean: 42s, sigma: 0.08}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn lognormal(mean: SimDuration, sigma: f64) -> Result<Self, DistributionError> {
        let mean_s = positive("lognormal", "mean", mean, LOGNORMAL)?;
        let sigma = positive_number("lognormal", "sigma", sigma, LOGNORMAL)?;
        Ok(Self(Kind::LogNormal {
            mean,
            sigma,
            mu: log(mean_s) - sigma * sigma / 2.0,
        }))
    }

    /// Uniform between `min` and `max`.
    ///
    /// # Errors
    ///
    /// [`DistributionError`] unless `min < max`.
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
            return Err(out_of_order(
                "uniform",
                "max",
                max,
                "longer than `min`",
                UNIFORM,
            ));
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
    /// [`DistributionError`] unless `min ≤ mode ≤ max` and `min < max`.
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
            return Err(out_of_order(
                "triangular",
                "mode",
                mode,
                "at least `min`",
                TRIANGULAR,
            ));
        }
        if max < mode || max == min {
            return Err(out_of_order(
                "triangular",
                "max",
                max,
                "at least `mode` and longer than `min`",
                TRIANGULAR,
            ));
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
    /// [`DistributionError`] if `mean` is zero or `cv` is not a finite
    /// number above zero.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::gamma("12min".parse()?, 0.5)?;
    /// assert_eq!(d.to_string(), "{dist: gamma, mean: 12min, cv: 0.5}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn gamma(mean: SimDuration, cv: f64) -> Result<Self, DistributionError> {
        let mean_s = positive("gamma", "mean", mean, GAMMA)?;
        let cv = positive_number("gamma", "cv", cv, GAMMA)?;
        let shape = 1.0 / (cv * cv);
        let (d, boost) = if shape < 1.0 {
            // Draw with shape k + 1, then multiply by u^(1/k).
            (shape + 2.0 / 3.0, Some(1.0 / shape))
        } else {
            (shape - 1.0 / 3.0, None)
        };
        Ok(Self(Kind::Gamma {
            mean,
            cv,
            scale_s: mean_s * cv * cv,
            d,
            c: 1.0 / (9.0 * d).sqrt(),
            boost,
        }))
    }

    /// Erlang with the given `mean` and `k` phases: the sum of `k`
    /// exponential phases with mean `mean / k`.
    ///
    /// # Errors
    ///
    /// [`DistributionError`] if `mean` is zero or `k` is outside
    /// 1..=[`MAX_ERLANG_PHASES`].
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::erlang("12min".parse()?, 3)?;
    /// assert_eq!(d.to_string(), "{dist: erlang, mean: 12min, k: 3}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn erlang(mean: SimDuration, k: u32) -> Result<Self, DistributionError> {
        let mean_s = positive("erlang", "mean", mean, ERLANG)?;
        if !(1..=MAX_ERLANG_PHASES).contains(&k) {
            return Err(DistributionError {
                distribution: "erlang",
                parameter: "k",
                found: k.to_string(),
                allowed: "a whole number from 1 to 1000",
                example: ERLANG,
            });
        }
        Ok(Self(Kind::Erlang {
            mean,
            k,
            phase_mean_s: mean_s / f64::from(k),
        }))
    }

    /// Weibull with the given `scale` and `shape`, e.g. wear-out failures
    /// (shape > 1).
    ///
    /// # Errors
    ///
    /// [`DistributionError`] if `scale` is zero or `shape` is not a finite
    /// number above zero.
    ///
    /// ```
    /// use taktsim_core::random::DurationDistribution;
    ///
    /// let d = DurationDistribution::weibull("6h".parse()?, 1.5)?;
    /// assert_eq!(d.to_string(), "{dist: weibull, scale: 6h, shape: 1.5}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn weibull(scale: SimDuration, shape: f64) -> Result<Self, DistributionError> {
        let scale_s = positive("weibull", "scale", scale, WEIBULL)?;
        let shape = positive_number("weibull", "shape", shape, WEIBULL)?;
        Ok(Self(Kind::Weibull {
            scale,
            shape,
            scale_s,
            inv_shape: 1.0 / shape,
        }))
    }

    /// One sample, drawn from `stream` and rounded to the nearest nanosecond.
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] or [`UnitError::NotFinite`] if a sample is
    /// longer than simulation time can hold (about 584 years), e.g. from a
    /// lognormal with a very large `sigma`.
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
                let x = mean_s + sd_s * standard_normal(stream);
                if x >= 0.0 {
                    break x;
                }
            },
            Kind::LogNormal { sigma, mu, .. } => exp(mu + sigma * standard_normal(stream)),
            Kind::Uniform { min_s, width_s, .. } => min_s + width_s * stream.next_open01(),
            Kind::Triangular {
                min_s,
                max_s,
                split,
                left,
                right,
                ..
            } => {
                let u = stream.next_open01();
                if u < *split {
                    min_s + (u * left).sqrt()
                } else {
                    max_s - ((1.0 - u) * right).sqrt()
                }
            }
            Kind::Gamma {
                scale_s,
                d,
                c,
                boost,
                ..
            } => {
                let g = marsaglia_tsang(stream, *d, *c);
                let g = match boost {
                    Some(inv_shape) => g * exp(log(stream.next_open01()) * inv_shape),
                    None => g,
                };
                g * scale_s
            }
            Kind::Erlang {
                k, phase_mean_s, ..
            } => {
                let log_sum: f64 = (0..*k).map(|_| log(stream.next_open01())).sum();
                -phase_mean_s * log_sum
            }
            Kind::Weibull {
                scale_s, inv_shape, ..
            } => scale_s * exp(log(-log(stream.next_open01())) * inv_shape),
        };
        SimDuration::from_secs_f64(secs)
    }
}

/// A standard normal value (Box–Muller; the second value of the pair is not
/// used, so every sample consumes exactly two uniforms).
fn standard_normal(stream: &mut Stream) -> f64 {
    let radius = (-2.0 * log(stream.next_open01())).sqrt();
    radius * cos(TAU * stream.next_open01())
}

/// A gamma(d + 1/3, 1) value (Marsaglia and Tsang, ACM TOMS 26(3), 2000).
fn marsaglia_tsang(stream: &mut Stream, d: f64, c: f64) -> f64 {
    loop {
        let z = standard_normal(stream);
        let v = 1.0 + c * z;
        if v <= 0.0 {
            continue;
        }
        let v = v * v * v;
        if log(stream.next_open01()) < 0.5 * z * z + d - d * v + d * log(v) {
            return d * v;
        }
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
