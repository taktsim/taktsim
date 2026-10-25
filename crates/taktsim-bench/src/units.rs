//! Workloads for `taktsim_units`: parsing quantities as they appear in
//! model files, and formatting them back.
//!
//! ```
//! use taktsim_bench::units::{self, DURATION_TEXTS};
//! use taktsim_units::SimDuration;
//!
//! let durations: Vec<SimDuration> = units::parsed(&DURATION_TEXTS)?;
//! let mut text = String::new();
//! units::format_all(&durations, &mut text)?;
//! assert!(text.starts_with("42s12min"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt::{self, Display, Write as _};
use std::hint::black_box;
use std::str::FromStr;

use taktsim_units::{Count, SimDuration};

/// Durations as written in models: every unit, whole and decimal numbers,
/// with and without a space.
///
/// ```
/// use taktsim_bench::units::DURATION_TEXTS;
///
/// assert!(DURATION_TEXTS.contains(&"0.5h"));
/// ```
pub const DURATION_TEXTS: [&str; 8] = [
    "42s", "12min", "0.5h", "8h", "2d", "250ms", "1.25 s", "90 min",
];

/// Counts as written in models.
///
/// ```
/// use taktsim_bench::units::COUNT_TEXTS;
///
/// assert!(COUNT_TEXTS.contains(&"20 parts"));
/// ```
pub const COUNT_TEXTS: [&str; 4] = ["20 parts", "1 part", "500parts", "12 parts"];

/// Rates as written in models, one per time unit.
///
/// ```
/// use taktsim_bench::units::RATE_TEXTS;
///
/// assert!(RATE_TEXTS.contains(&"120 parts/h"));
/// ```
pub const RATE_TEXTS: [&str; 4] = ["120 parts/h", "2 parts/min", "0.5 parts/s", "1 part/h"];

/// Percentages as written in models.
///
/// ```
/// use taktsim_bench::units::RATIO_TEXTS;
///
/// assert!(RATIO_TEXTS.contains(&"2%"));
/// ```
pub const RATIO_TEXTS: [&str; 4] = ["2%", "0.5%", "95 %", "100%"];

/// Durations to format: each display unit, a fraction of a second and the
/// smallest step, 1 ns.
///
/// ```
/// use taktsim_bench::units::DURATIONS;
///
/// assert_eq!(DURATIONS[0].to_string(), "42s");
/// ```
pub const DURATIONS: [SimDuration; 8] = [
    SimDuration::from_nanos(42_000_000_000),
    SimDuration::from_nanos(720_000_000_000),
    SimDuration::from_nanos(1_800_000_000_000),
    SimDuration::from_nanos(28_800_000_000_000),
    SimDuration::from_nanos(172_800_000_000_000),
    SimDuration::from_nanos(250_000_000),
    SimDuration::from_nanos(1_250_000_000),
    SimDuration::from_nanos(1),
];

/// Counts to format.
///
/// ```
/// use taktsim_bench::units::COUNTS;
///
/// assert_eq!(COUNTS[0].to_string(), "20 parts");
/// ```
pub const COUNTS: [Count; 4] = [
    Count::new(20),
    Count::new(1),
    Count::new(500),
    Count::new(12),
];

/// Parses every text as a `T`, keeping nothing; the workload of the parse
/// benchmarks.
///
/// # Errors
///
/// The error of the first text that does not parse; never for the texts
/// above (tested).
///
/// ```
/// use taktsim_bench::units::{self, COUNT_TEXTS};
/// use taktsim_units::{Count, SimDuration};
///
/// assert!(units::parse_all::<Count>(&COUNT_TEXTS).is_ok());
/// assert!(units::parse_all::<SimDuration>(&COUNT_TEXTS).is_err());
/// ```
pub fn parse_all<T: FromStr>(texts: &[&str]) -> Result<(), T::Err> {
    for text in texts {
        black_box(text.parse::<T>()?);
    }
    Ok(())
}

/// Parses every text as a `T`, for benchmark setup.
///
/// # Errors
///
/// The error of the first text that does not parse.
///
/// ```
/// use taktsim_bench::units::{self, RATIO_TEXTS};
/// use taktsim_units::Ratio;
///
/// let ratios: Vec<Ratio> = units::parsed(&RATIO_TEXTS)?;
/// assert_eq!(ratios[0].as_percent(), 2.0);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
pub fn parsed<T: FromStr>(texts: &[&str]) -> Result<Vec<T>, T::Err> {
    texts.iter().map(|text| text.parse()).collect()
}

/// Writes every value into `buffer`, replacing its contents, and returns
/// the length written; the workload of the format benchmarks. A buffer with
/// enough capacity is reused without allocating.
///
/// # Errors
///
/// [`fmt::Error`] if a value's `Display` fails; the taktsim units never do.
///
/// ```
/// use taktsim_bench::units::{self, COUNTS};
///
/// let mut buffer = String::new();
/// assert_eq!(units::format_all(&COUNTS, &mut buffer)?, buffer.len());
/// assert_eq!(buffer, "20 parts1 parts500 parts12 parts");
/// # Ok::<(), std::fmt::Error>(())
/// ```
pub fn format_all<T: Display>(values: &[T], buffer: &mut String) -> Result<usize, fmt::Error> {
    buffer.clear();
    for value in values {
        write!(buffer, "{value}")?;
    }
    Ok(buffer.len())
}
