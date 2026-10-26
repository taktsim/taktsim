//! Distributions of durations as written in a model (ADR-0003).

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use taktsim_core::random::{DistributionError, DurationDistribution};
use taktsim_units::SimDuration;

use crate::decode::{Decode, Decoder};
use crate::error::{LoadErrorKind, Segment};
use crate::syntax::{Node, Value};

/// A stochastic duration parameter: a bare duration is a constant; any
/// other distribution is an object named by `dist`, with named,
/// unit-checked parameters (ADR-0003, D1).
///
/// The loader checks the parameters with the constructors that sample them
/// ([`DurationDistribution`]), so a distribution that loads can be
/// sampled. Its normalized form (and so the model hash) writes durations in
/// their canonical text: `{dist: exponential, mean: 0.2h}` normalizes to
/// `{"dist":"exponential","mean":"12min"}`, and a constant stays a bare
/// duration.
///
/// ```
/// use taktsim_model::{Distribution, Format, decode_str};
///
/// let d: Distribution = decode_str("{dist: lognormal, mean: 42s, sigma: 0.08}", Format::Yaml)?;
/// assert_eq!(d.to_string(), "{dist: lognormal, mean: 42s, sigma: 0.08}");
/// assert_eq!(serde_json::to_string(&d)?, r#"{"dist":"lognormal","mean":"42s","sigma":0.08}"#);
///
/// let constant: Distribution = decode_str("45s", Format::Yaml)?;
/// assert_eq!(serde_json::to_string(&constant)?, r#""45s""#);
///
/// let err = decode_str::<Distribution>("{dist: uniform, min: 50s, max: 40s}", Format::Yaml).unwrap_err();
/// assert_eq!(err.first().path().to_string(), "max");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Distribution {
    /// Always this duration: a bare quantity, e.g. `cycle_time: 45s`.
    Constant(SimDuration),
    /// `{dist: exponential, mean}`.
    Exponential {
        /// The mean.
        mean: SimDuration,
    },
    /// `{dist: normal, mean, sd}`, truncated at zero by rejection
    /// (ADR-0013).
    Normal {
        /// The mean of the untruncated normal.
        mean: SimDuration,
        /// The standard deviation of the untruncated normal.
        sd: SimDuration,
    },
    /// `{dist: lognormal, mean, sigma}`: `mean` is the distribution's mean,
    /// `sigma` the standard deviation of its logarithm.
    LogNormal {
        /// The mean.
        mean: SimDuration,
        /// The shape: the standard deviation of ln(x).
        sigma: f64,
    },
    /// `{dist: uniform, min, max}`.
    Uniform {
        /// The smallest value.
        min: SimDuration,
        /// The largest value.
        max: SimDuration,
    },
    /// `{dist: triangular, min, mode, max}`.
    Triangular {
        /// The smallest value.
        min: SimDuration,
        /// The most likely value.
        mode: SimDuration,
        /// The largest value.
        max: SimDuration,
    },
    /// `{dist: gamma, mean, cv}`.
    Gamma {
        /// The mean.
        mean: SimDuration,
        /// The coefficient of variation: standard deviation / mean.
        cv: f64,
    },
    /// `{dist: erlang, mean, k}`: the sum of `k` exponential phases.
    Erlang {
        /// The mean.
        mean: SimDuration,
        /// The number of phases, at least 1.
        k: u32,
    },
    /// `{dist: weibull, scale, shape}`.
    Weibull {
        /// The scale.
        scale: SimDuration,
        /// The shape.
        shape: f64,
    },
}

/// The names `dist` takes, in the order of ADR-0003.
pub const DISTRIBUTIONS: [&str; 8] = [
    "exponential",
    "normal",
    "lognormal",
    "uniform",
    "triangular",
    "gamma",
    "erlang",
    "weibull",
];

impl Distribution {
    /// The `dist` name; `None` for a constant.
    ///
    /// ```
    /// use taktsim_model::Distribution;
    ///
    /// let d = Distribution::Exponential { mean: "12min".parse()? };
    /// assert_eq!(d.name(), Some("exponential"));
    /// assert_eq!(Distribution::Constant("45s".parse()?).name(), None);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn name(&self) -> Option<&'static str> {
        Some(match self {
            Self::Constant(_) => return None,
            Self::Exponential { .. } => "exponential",
            Self::Normal { .. } => "normal",
            Self::LogNormal { .. } => "lognormal",
            Self::Uniform { .. } => "uniform",
            Self::Triangular { .. } => "triangular",
            Self::Gamma { .. } => "gamma",
            Self::Erlang { .. } => "erlang",
            Self::Weibull { .. } => "weibull",
        })
    }

    /// The sampler for this distribution.
    ///
    /// # Errors
    ///
    /// A [`DistributionError`] if the parameters break a rule of ADR-0003.
    /// A distribution read by the loader never does: the loader builds the
    /// sampler to check it.
    ///
    /// ```
    /// use taktsim_core::random::{Stream, StreamKey};
    /// use taktsim_model::Distribution;
    ///
    /// let d = Distribution::Uniform { min: "40s".parse()?, max: "50s".parse()? };
    /// let mut stream = Stream::new(42, StreamKey::new("cut", "cycle_time"));
    /// let x = d.sampler()?.sample(&mut stream)?;
    /// assert!(x >= "40s".parse()? && x <= "50s".parse()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn sampler(&self) -> Result<DurationDistribution, DistributionError> {
        match *self {
            Self::Constant(d) => Ok(DurationDistribution::constant(d)),
            Self::Exponential { mean } => DurationDistribution::exponential(mean),
            Self::Normal { mean, sd } => DurationDistribution::normal(mean, sd),
            Self::LogNormal { mean, sigma } => DurationDistribution::lognormal(mean, sigma),
            Self::Uniform { min, max } => DurationDistribution::uniform(min, max),
            Self::Triangular { min, mode, max } => DurationDistribution::triangular(min, mode, max),
            Self::Gamma { mean, cv } => DurationDistribution::gamma(mean, cv),
            Self::Erlang { mean, k } => DurationDistribution::erlang(mean, k),
            Self::Weibull { scale, shape } => DurationDistribution::weibull(scale, shape),
        }
    }
}

impl std::fmt::Display for Distribution {
    /// As written in YAML: `45s` or `{dist: lognormal, mean: 42s, sigma:
    /// 0.08}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.name().unwrap_or_default();
        match self {
            Self::Constant(d) => write!(f, "{d}"),
            Self::Exponential { mean } => write!(f, "{{dist: {name}, mean: {mean}}}"),
            Self::Normal { mean, sd } => write!(f, "{{dist: {name}, mean: {mean}, sd: {sd}}}"),
            Self::LogNormal { mean, sigma } => {
                write!(f, "{{dist: {name}, mean: {mean}, sigma: {sigma}}}")
            }
            Self::Uniform { min, max } => write!(f, "{{dist: {name}, min: {min}, max: {max}}}"),
            Self::Triangular { min, mode, max } => {
                write!(f, "{{dist: {name}, min: {min}, mode: {mode}, max: {max}}}")
            }
            Self::Gamma { mean, cv } => write!(f, "{{dist: {name}, mean: {mean}, cv: {cv}}}"),
            Self::Erlang { mean, k } => write!(f, "{{dist: {name}, mean: {mean}, k: {k}}}"),
            Self::Weibull { scale, shape } => {
                write!(f, "{{dist: {name}, scale: {scale}, shape: {shape}}}")
            }
        }
    }
}

impl Serialize for Distribution {
    /// The normalized form: a constant as its duration text, any other
    /// distribution as a mapping with `dist` and its parameters.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = self.name().unwrap_or_default();
        let mut map = match self {
            Self::Constant(d) => return d.serialize(serializer),
            Self::Exponential { .. } => serializer.serialize_map(Some(2))?,
            Self::Normal { .. }
            | Self::LogNormal { .. }
            | Self::Uniform { .. }
            | Self::Gamma { .. }
            | Self::Erlang { .. }
            | Self::Weibull { .. } => serializer.serialize_map(Some(3))?,
            Self::Triangular { .. } => serializer.serialize_map(Some(4))?,
        };
        map.serialize_entry("dist", name)?;
        match self {
            Self::Constant(_) => {}
            Self::Exponential { mean } => map.serialize_entry("mean", mean)?,
            Self::Normal { mean, sd } => {
                map.serialize_entry("mean", mean)?;
                map.serialize_entry("sd", sd)?;
            }
            Self::LogNormal { mean, sigma } => {
                map.serialize_entry("mean", mean)?;
                map.serialize_entry("sigma", sigma)?;
            }
            Self::Uniform { min, max } => {
                map.serialize_entry("min", min)?;
                map.serialize_entry("max", max)?;
            }
            Self::Triangular { min, mode, max } => {
                map.serialize_entry("min", min)?;
                map.serialize_entry("mode", mode)?;
                map.serialize_entry("max", max)?;
            }
            Self::Gamma { mean, cv } => {
                map.serialize_entry("mean", mean)?;
                map.serialize_entry("cv", cv)?;
            }
            Self::Erlang { mean, k } => {
                map.serialize_entry("mean", mean)?;
                map.serialize_entry("k", k)?;
            }
            Self::Weibull { scale, shape } => {
                map.serialize_entry("scale", scale)?;
                map.serialize_entry("shape", shape)?;
            }
        }
        map.end()
    }
}

/// The `dist` value.
struct Name(&'static str);

impl Decode for Name {
    const EXPECTED: &'static str = "a distribution name";
    const EXAMPLE: &'static str = "exponential";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let text = String::decode(node, cx)?;
        if let Some(name) = DISTRIBUTIONS.iter().find(|&&n| n == text) {
            return Some(Self(name));
        }
        let kind = LoadErrorKind::UnknownValue {
            what: "distribution",
            suggestion: crate::error::suggest(&text, DISTRIBUTIONS),
            found: text,
            allowed: DISTRIBUTIONS.iter().map(|&n| n.to_owned()).collect(),
        };
        cx.error(node.start(), kind);
        None
    }
}

/// The parameter a distribution error is about.
fn parameter(error: &DistributionError) -> Option<&'static str> {
    if let DistributionError::NotPositive { parameter, .. }
    | DistributionError::NotPositiveNumber { parameter, .. }
    | DistributionError::OutOfOrder { parameter, .. }
    | DistributionError::TooExtreme { parameter, .. } = error
    {
        Some(parameter)
    } else if let DistributionError::NoPhases { .. } = error {
        Some("k")
    } else {
        None
    }
}

impl Decode for Distribution {
    const EXPECTED: &'static str =
        "a duration (a constant), or a distribution such as `{dist: exponential, mean: 12min}`";
    const EXAMPLE: &'static str = "{dist: lognormal, mean: 42s, sigma: 0.08}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        match node.value() {
            Value::Str(text) if text.contains('(') => {
                // The compact form of ADR-0003's option D2 is not accepted.
                cx.not_allowed(
                    node,
                    text.clone(),
                    "a duration, or a distribution written as a mapping (ADR-0003)",
                    Self::EXAMPLE,
                );
                None
            }
            Value::Str(_) => SimDuration::decode(node, cx).map(Self::Constant),
            Value::Map(entries) => {
                let mut fields = cx.fields::<Self>(node)?;
                let Some(Name(name)) = fields.required::<Name>("dist") else {
                    fields.ignore_rest();
                    return None;
                };
                let distribution = read(name, &mut fields);
                fields.finish();
                let distribution = distribution?;
                if let Err(error) = distribution.sampler() {
                    let param = parameter(&error);
                    let at = param
                        .and_then(|p| entries.iter().find(|e| e.key() == p))
                        .map_or(node.start(), |e| e.value().start());
                    let kind = LoadErrorKind::Distribution(error);
                    match param {
                        Some(p) => cx.within(Segment::Key(p.to_owned()), |cx| cx.error(at, kind)),
                        None => cx.error(at, kind),
                    }
                    return None;
                }
                Some(distribution)
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::List(_) => {
                cx.wrong_type::<Self>(node);
                None
            }
        }
    }
}

/// Reads the parameters of the distribution `name`.
fn read(name: &str, f: &mut crate::decode::Fields<'_, '_>) -> Option<Distribution> {
    Some(match name {
        "exponential" => Distribution::Exponential {
            mean: f.required("mean")?,
        },
        "normal" => {
            let (mean, sd) = (f.required("mean"), f.required("sd"));
            Distribution::Normal {
                mean: mean?,
                sd: sd?,
            }
        }
        "lognormal" => {
            let (mean, sigma) = (f.required("mean"), f.required("sigma"));
            Distribution::LogNormal {
                mean: mean?,
                sigma: sigma?,
            }
        }
        "uniform" => {
            let (min, max) = (f.required("min"), f.required("max"));
            Distribution::Uniform {
                min: min?,
                max: max?,
            }
        }
        "triangular" => {
            let (min, mode, max) = (f.required("min"), f.required("mode"), f.required("max"));
            Distribution::Triangular {
                min: min?,
                mode: mode?,
                max: max?,
            }
        }
        "gamma" => {
            let (mean, cv) = (f.required("mean"), f.required("cv"));
            Distribution::Gamma {
                mean: mean?,
                cv: cv?,
            }
        }
        "erlang" => {
            let (mean, k) = (f.required("mean"), f.required("k"));
            Distribution::Erlang { mean: mean?, k: k? }
        }
        "weibull" => {
            let (scale, shape) = (f.required("scale"), f.required("shape"));
            Distribution::Weibull {
                scale: scale?,
                shape: shape?,
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Found;
    use crate::{Format, decode_str};

    fn load(yaml: &str) -> Result<Distribution, Vec<(String, String, LoadErrorKind)>> {
        decode_str(yaml, Format::Yaml).map_err(|errors| {
            errors
                .iter()
                .map(|e| {
                    (
                        e.location().map(|l| l.to_string()).unwrap_or_default(),
                        e.path().to_string(),
                        e.kind().clone(),
                    )
                })
                .collect()
        })
    }

    fn d(text: &str) -> SimDuration {
        text.parse().unwrap()
    }

    #[test]
    fn every_distribution_of_adr_0003_loads_and_prints_back() {
        let cases = [
            ("45s", Distribution::Constant(d("45s"))),
            (
                "{dist: exponential, mean: 12min}",
                Distribution::Exponential { mean: d("12min") },
            ),
            (
                "{dist: normal, mean: 42s, sd: 3s}",
                Distribution::Normal {
                    mean: d("42s"),
                    sd: d("3s"),
                },
            ),
            (
                "{dist: lognormal, mean: 42s, sigma: 0.08}",
                Distribution::LogNormal {
                    mean: d("42s"),
                    sigma: 0.08,
                },
            ),
            (
                "{dist: uniform, min: 40s, max: 50s}",
                Distribution::Uniform {
                    min: d("40s"),
                    max: d("50s"),
                },
            ),
            (
                "{dist: triangular, min: 40s, mode: 42s, max: 50s}",
                Distribution::Triangular {
                    min: d("40s"),
                    mode: d("42s"),
                    max: d("50s"),
                },
            ),
            (
                "{dist: gamma, mean: 12min, cv: 0.5}",
                Distribution::Gamma {
                    mean: d("12min"),
                    cv: 0.5,
                },
            ),
            (
                "{dist: erlang, mean: 12min, k: 3}",
                Distribution::Erlang {
                    mean: d("12min"),
                    k: 3,
                },
            ),
            (
                "{dist: weibull, scale: 6h, shape: 1.5}",
                Distribution::Weibull {
                    scale: d("6h"),
                    shape: 1.5,
                },
            ),
        ];
        for (yaml, expected) in cases {
            assert_eq!(load(yaml), Ok(expected), "{yaml}");
            assert_eq!(expected.to_string(), yaml);
            // The sampler prints the same text: the two cannot drift.
            assert_eq!(expected.sampler().unwrap().to_string(), yaml);
        }
    }

    #[test]
    fn parameters_are_checked_where_they_are_written() {
        let err = load("{dist: uniform, min: 50s, max: 40s}").unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!((err[0].0.as_str(), err[0].1.as_str()), ("1:32", "max"));
        assert!(matches!(
            err[0].2,
            LoadErrorKind::Distribution(DistributionError::OutOfOrder { .. })
        ));

        let err = load("{dist: lognormal, mean: 42s, sigma: -0.5}").unwrap_err();
        assert_eq!(err[0].1, "sigma");
        let err = load("{dist: erlang, mean: 1min, k: 0}").unwrap_err();
        assert_eq!(err[0].1, "k");
        let err = load("{dist: exponential, mean: 0s}").unwrap_err();
        assert_eq!(err[0].1, "mean");
        let err = load("{dist: gamma, mean: 1min, cv: 1e-160}").unwrap_err();
        assert!(matches!(
            err[0].2,
            LoadErrorKind::Distribution(DistributionError::TooExtreme { .. })
        ));
    }

    #[test]
    fn unknown_names_and_keys_are_errors_with_suggestions() {
        let err = load("{dist: lognrmal, mean: 42s, sigma: 0.08}").unwrap_err();
        assert_eq!(err.len(), 1, "{err:?}");
        assert!(matches!(
            &err[0].2,
            LoadErrorKind::UnknownValue { suggestion: Some(s), .. } if s == "lognormal"
        ));
        let err = load("{dist: exponential, mean: 1min, sd: 3s}").unwrap_err();
        assert!(matches!(&err[0].2, LoadErrorKind::UnknownField { field, .. } if field == "sd"));
        let err = load("{mean: 1min}").unwrap_err();
        assert!(matches!(
            err[0].2,
            LoadErrorKind::MissingField { field: "dist", .. }
        ));
        let err = load("{dist: normal}").unwrap_err();
        let missing: Vec<_> = err.iter().map(|e| e.1.as_str()).collect();
        assert_eq!(missing, ["mean", "sd"]);
    }

    #[test]
    fn wrong_kinds_of_values() {
        let err = load("42").unwrap_err();
        assert!(matches!(
            err[0].2,
            LoadErrorKind::WrongType {
                found: Found::Int(42),
                ..
            }
        ));
        let err = load("lognormal(42s, 0.08)").unwrap_err();
        assert!(matches!(err[0].2, LoadErrorKind::NotAllowed { .. }));
        let err = load("{dist: lognormal, mean: 42, sigma: 0.08}").unwrap_err();
        assert_eq!(err[0].1, "mean");
        let err = load("{dist: erlang, mean: 1min, k: 2.5}").unwrap_err();
        assert_eq!(err[0].1, "k");
    }

    #[test]
    fn the_normalized_form_uses_canonical_durations() {
        let d = load("{dist: exponential, mean: 0.2h}").unwrap();
        assert_eq!(
            serde_json::to_string(&d).unwrap(),
            r#"{"dist":"exponential","mean":"12min"}"#
        );
        let d = load("{dist: triangular, min: 1s, mode: 2s, max: 3s}").unwrap();
        assert_eq!(
            serde_json::to_string(&d).unwrap(),
            r#"{"dist":"triangular","min":"1s","mode":"2s","max":"3s"}"#
        );
        for (yaml, json) in [
            (
                "{dist: normal, mean: 2s, sd: 1s}",
                r#"{"dist":"normal","mean":"2s","sd":"1s"}"#,
            ),
            (
                "{dist: uniform, min: 1s, max: 2s}",
                r#"{"dist":"uniform","min":"1s","max":"2s"}"#,
            ),
            (
                "{dist: gamma, mean: 2s, cv: 0.5}",
                r#"{"dist":"gamma","mean":"2s","cv":0.5}"#,
            ),
            (
                "{dist: erlang, mean: 2s, k: 2}",
                r#"{"dist":"erlang","mean":"2s","k":2}"#,
            ),
            (
                "{dist: weibull, scale: 2s, shape: 2}",
                r#"{"dist":"weibull","scale":"2s","shape":2.0}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&load(yaml).unwrap()).unwrap(), json);
        }
    }
}
