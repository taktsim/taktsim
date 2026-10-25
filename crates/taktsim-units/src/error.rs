//! The error type of this crate.

/// Why a quantity could not be parsed or computed.
///
/// Every parse variant carries the input, what is allowed and one valid
/// example, so the model loader can turn it into a field-path error with a
/// fix hint (spec §5, Contracts).
///
/// ```
/// use taktsim_units::{SimDuration, UnitError};
///
/// let err = "42sec".parse::<SimDuration>().unwrap_err();
/// assert!(matches!(err, UnitError::UnknownUnit { .. }));
/// assert_eq!(
///     err.to_string(),
///     "unknown duration unit `sec` in `42sec`; allowed: d, h, min, s, ms; example: 42s"
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UnitError {
    /// The text does not start with a non-negative decimal number.
    #[error("expected a {quantity} such as `{example}`, found `{input}`")]
    InvalidNumber {
        /// The text that was parsed.
        input: String,
        /// What kind of quantity was expected, e.g. "duration".
        quantity: &'static str,
        /// One valid example.
        example: &'static str,
    },
    /// The number has no unit, or the unit is not one of the allowed ones.
    #[error(
        "{} {quantity} unit `{unit}` in `{input}`; allowed: {}; example: {example}",
        if unit.is_empty() { "missing" } else { "unknown" },
        allowed.join(", ")
    )]
    UnknownUnit {
        /// The text that was parsed.
        input: String,
        /// The unit found; empty if there was none.
        unit: String,
        /// What kind of quantity was expected.
        quantity: &'static str,
        /// The units this quantity accepts.
        allowed: &'static [&'static str],
        /// One valid example.
        example: &'static str,
    },
    /// The value is negative; quantities in this crate never are.
    #[error("negative {quantity} `{input}` is not allowed; example: {example}")]
    Negative {
        /// The text that was parsed.
        input: String,
        /// What kind of quantity was expected.
        quantity: &'static str,
        /// One valid example.
        example: &'static str,
    },
    /// A count with a fractional part, e.g. `2.5 parts`.
    #[error("`{input}` is not a whole number of {unit}; example: 20 parts")]
    NotWhole {
        /// The text that was parsed.
        input: String,
        /// The unit found.
        unit: String,
    },
    /// A duration finer than the 1 ns resolution of simulation time.
    #[error("`{input}` is finer than the 1 ns resolution of simulation time")]
    TooPrecise {
        /// The text that was parsed.
        input: String,
    },
    /// The result does not fit (simulation time ends after about 584 years).
    #[error("{what} overflows")]
    Overflow {
        /// What overflowed, e.g. "duration `600000y`" or "time 8h + 584y".
        what: String,
    },
    /// A subtraction would go below zero.
    #[error("{what} would be negative")]
    Underflow {
        /// What would have gone below zero.
        what: String,
    },
    /// A floating-point input is NaN or infinite.
    #[error("{what} is not a finite number")]
    NotFinite {
        /// What was not finite.
        what: String,
    },
    /// A rate over a zero-length duration.
    #[error("cannot compute a rate over a zero duration")]
    ZeroDuration,
}
