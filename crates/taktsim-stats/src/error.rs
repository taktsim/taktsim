//! The error type of this crate.

use taktsim_units::{Ratio, SimTime};

/// Why a statistic could not be computed.
///
/// Every variant names the parameter or input at fault, what was found and
/// what is allowed, and most carry one valid example, so a caller can turn
/// it into a field-path error with a fix hint (spec §5, Contracts). Nothing
/// in this crate panics on bad input; it returns one of these instead.
///
/// ```
/// use taktsim_stats::{ConfidenceLevel, StatsError, Summary};
///
/// let one = Summary::from_values(&[4.2])?;
/// let err = one.confidence_interval(ConfidenceLevel::DEFAULT).unwrap_err();
/// assert_eq!(err, StatsError::TooFewReplications { found: 1, required: 2 });
/// assert_eq!(
///     err.to_string(),
///     "a confidence interval needs at least 2 replications, found 1; \
///      run more replications, e.g. 10"
/// );
/// # Ok::<(), StatsError>(())
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum StatsError {
    /// A probability or confidence level outside the open interval (0, 1).
    #[error(
        "`{parameter}` must be a number strictly between 0 and 1, found {found}; example: {example}"
    )]
    NotAProbability {
        /// The parameter, e.g. `level`.
        parameter: &'static str,
        /// The value found.
        found: f64,
        /// One valid value.
        example: &'static str,
    },
    /// A whole-number parameter that must be at least 1 is 0.
    #[error("`{parameter}` must be a whole number >= 1, found 0; example: {example}")]
    Zero {
        /// The parameter, e.g. `degrees_of_freedom`.
        parameter: &'static str,
        /// One valid value.
        example: &'static str,
    },
    /// An input value is NaN or infinite.
    #[error("{what}: value {index} is {found}; only finite numbers are allowed")]
    NotFinite {
        /// The input, e.g. "replication results".
        what: &'static str,
        /// The position of the value in the input, from 0.
        index: usize,
        /// The value found.
        found: f64,
    },
    /// The inputs are finite, but a derived value (a sum of squares, a
    /// half-width) overflows `f64`.
    #[error("{what} overflows f64; rescale the values, e.g. use hours instead of nanoseconds")]
    Overflow {
        /// What overflowed, e.g. "sum of squared deviations".
        what: &'static str,
    },
    /// Fewer replications than the statistic needs.
    #[error(
        "a confidence interval needs at least {required} replications, found {found}; \
         run more replications, e.g. 10"
    )]
    TooFewReplications {
        /// The number of replications found.
        found: u32,
        /// The smallest number that works.
        required: u32,
    },
    /// More replications than a [`Summary`](crate::Summary) can count.
    #[error("at most {max} replications can be summarized")]
    TooManyReplications {
        /// The largest count.
        max: u32,
    },
    /// A series too short for warm-up detection.
    #[error(
        "{what} needs at least {required} observations, found {found}; \
         record a longer series, e.g. 1000 observations"
    )]
    SeriesTooShort {
        /// The method, e.g. "MSER-5".
        what: &'static str,
        /// The number of observations found.
        found: usize,
        /// The smallest number that works.
        required: usize,
    },
    /// A relative precision of zero, which no finite number of replications
    /// reaches.
    #[error("`precision` must be above 0%, found {found}; example: 2%")]
    PrecisionNotPositive {
        /// The precision found.
        found: Ratio,
    },
    /// The minimum number of replications is below 2 or above the budget.
    #[error(
        "`min_replications` must be between 2 and the budget `max_replications` = {max}, \
         found {found}; example: 10 with max_replications 100"
    )]
    MinReplicationsOutOfRange {
        /// The minimum found.
        found: u32,
        /// The budget.
        max: u32,
    },
    /// A replication result arrived after the budget was used up.
    #[error(
        "the budget of {max} replications is used up; \
         raise `max_replications` to run more, e.g. {}",
        max.saturating_mul(2)
    )]
    BudgetExhausted {
        /// The budget.
        max: u32,
    },
    /// An observation window that does not end after it starts.
    #[error(
        "the window must end after it starts, found start {start} and end {end}; \
         example: start 0s, end 8h"
    )]
    EmptyWindow {
        /// The start found.
        start: SimTime,
        /// The end found.
        end: SimTime,
    },
    /// A warm-up that ends before the window starts or at or after it ends.
    #[error(
        "the warm-up must end in [{start}, {end}), the window without its end, \
         found {found}; example: {start}"
    )]
    WarmUpOutsideWindow {
        /// The end of the warm-up found.
        found: SimTime,
        /// The start of the window.
        start: SimTime,
        /// The end of the window.
        end: SimTime,
    },
    /// Times that decrease.
    #[error(
        "{what}: times must not decrease, but value {index} at {found} follows one at {previous}"
    )]
    TimesOutOfOrder {
        /// The input, e.g. "signal changes".
        what: &'static str,
        /// The position of the value in the input, from 0.
        index: usize,
        /// The time of the value before it.
        previous: SimTime,
        /// Its time.
        found: SimTime,
    },
    /// A signal without a value at the start of the statistics.
    #[error(
        "the signal has no value at {start}, where the statistics start; its first change is \
         {}; record its value at or before {start}",
        match first { Some(t) => format!("at {t}"), None => "missing".to_owned() }
    )]
    NoValueAtStart {
        /// Where the statistics start: the end of the warm-up.
        start: SimTime,
        /// The time of the first change, if there is one.
        first: Option<SimTime>,
    },
}
