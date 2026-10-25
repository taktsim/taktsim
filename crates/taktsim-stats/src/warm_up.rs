//! Warm-up detection with MSER-5 (spec §6 default; ADR-0014).
//!
//! MSER (White 1997) chooses the truncation point d that minimizes
//!
//! ```text
//! MSER(d) = 1/(k − d)² · Σ_{j = d+1..k} (Z_j − Z̄(d))²
//! ```
//!
//! where Z̄(d) is the mean of the observations after the first d. This is the
//! squared half-width of a confidence interval for the truncated mean, up to
//! a constant (Franklin & White 2008), so MSER keeps the part of the series
//! whose mean is best determined. MSER-5 (White, Cobb & Spratt 2000) applies
//! it to the means `Z_j` of consecutive batches of 5 observations. The search
//! covers only the first half of the batches: near the end of the series
//! the statistic rests on a few batches and is unreliable, and a
//! truncation beyond the half means that the series is too short.

use taktsim_units::SimTime;

use crate::StatsError;

/// The batch size of MSER-5.
///
/// ```
/// assert_eq!(taktsim_stats::MSER_BATCH_SIZE, 5);
/// ```
pub const MSER_BATCH_SIZE: usize = 5;

/// The fewest batches MSER-5 works with: the largest truncation searched
/// must leave at least 2 batches, so k − ⌊k/2⌋ ≥ 2.
const MIN_BATCHES: usize = 3;

/// [`MSER_BATCH_SIZE`] as `f64`, the divisor of the batch means.
const BATCH_SIZE_F64: f64 = 5.0;
const _: () = assert!(
    MSER_BATCH_SIZE == 5,
    "BATCH_SIZE_F64 must equal MSER_BATCH_SIZE"
);

/// The truncation point MSER-5 found.
///
/// ```
/// use taktsim_stats::mser5;
///
/// // 50 observations at 10, after a transient of 20 at 30.
/// let series: Vec<f64> = (0..70)
///     .map(|i| if i < 20 { 30.0 } else { 10.0 + if i % 2 == 0 { 0.5 } else { -0.5 } })
///     .collect();
/// let cut = mser5(&series)?;
/// assert_eq!(cut.deleted_observations(), 20);
/// assert_eq!(cut.batches(), 14);
/// assert!(!cut.reached_search_limit());
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MserTruncation {
    deleted_batches: usize,
    batches: usize,
    statistic: f64,
}

impl MserTruncation {
    /// The number of leading observations to delete: 5 × the batches deleted.
    ///
    /// ```
    /// use taktsim_stats::mser5;
    ///
    /// assert_eq!(mser5(&[1.0; 40])?.deleted_observations(), 0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn deleted_observations(&self) -> usize {
        self.deleted_batches * MSER_BATCH_SIZE
    }

    /// The number of leading batches to delete, d.
    ///
    /// ```
    /// use taktsim_stats::mser5;
    ///
    /// assert_eq!(mser5(&[1.0; 40])?.deleted_batches(), 0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn deleted_batches(&self) -> usize {
        self.deleted_batches
    }

    /// The number of complete batches, k. Observations after the last
    /// complete batch are not used.
    ///
    /// ```
    /// use taktsim_stats::mser5;
    ///
    /// assert_eq!(mser5(&[1.0; 43])?.batches(), 8);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn batches(&self) -> usize {
        self.batches
    }

    /// The minimum of the MSER statistic, MSER(d).
    ///
    /// ```
    /// use taktsim_stats::mser5;
    ///
    /// assert_eq!(mser5(&[2.0; 20])?.statistic(), 0.0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn statistic(&self) -> f64 {
        self.statistic
    }

    /// Whether the minimum lies at the end of the search range, the middle
    /// of the series. The statistic was then still falling, so the
    /// transient may be longer than half the series: record a longer one.
    ///
    /// ```
    /// use taktsim_stats::mser5;
    ///
    /// // A series that falls all the way: no steady state in sight.
    /// let falling: Vec<f64> = (0..100).map(|i| 100.0 - f64::from(i)).collect();
    /// assert!(mser5(&falling)?.reached_search_limit());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn reached_search_limit(&self) -> bool {
        self.deleted_batches == self.batches / 2
    }
}

/// The MSER-5 truncation point of `observations`, e.g. the WIP sampled at
/// regular times or the lead time of each unit in completion order, all in
/// one declared unit (see the crate documentation).
///
/// Ties go to the smallest truncation. The result is the same on every
/// platform: batch means and sums are formed in a fixed order.
///
/// # Errors
///
/// [`StatsError::SeriesTooShort`] below 15 observations (3 batches),
/// [`StatsError::NotFinite`] for NaN or infinity, [`StatsError::Overflow`]
/// if a batch sum or the statistic overflows (values beyond about 1e154).
///
/// ```
/// use taktsim_stats::{mser5, StatsError};
///
/// assert!(matches!(
///     mser5(&[1.0; 14]),
///     Err(StatsError::SeriesTooShort { found: 14, required: 15, .. })
/// ));
/// # Ok::<(), StatsError>(())
/// ```
pub fn mser5(observations: &[f64]) -> Result<MserTruncation, StatsError> {
    truncate(observations.iter().copied())
}

/// MSER-5 over `observations` in order, without copying them.
fn truncate(
    observations: impl ExactSizeIterator<Item = f64>,
) -> Result<MserTruncation, StatsError> {
    let required = MIN_BATCHES * MSER_BATCH_SIZE;
    let n = observations.len();
    if n < required {
        return Err(StatsError::SeriesTooShort {
            what: "MSER-5",
            found: n,
            required,
        });
    }
    let overflow = |found: f64| StatsError::Overflow {
        what: "the MSER-5 statistic",
        found,
        allowed: "observations below about 1e154 in magnitude",
        example: "express the observations in a larger unit",
    };
    // Batch means in a fixed order; observations after the last complete
    // batch are checked but not used.
    let mut batch_means = Vec::with_capacity(n / MSER_BATCH_SIZE);
    let (mut sum, mut in_batch, mut largest) = (0.0_f64, 0, 0.0_f64);
    for (index, x) in observations.enumerate() {
        if !x.is_finite() {
            return Err(StatsError::NotFinite {
                what: "MSER-5 observations",
                index,
                found: x,
            });
        }
        sum += x;
        in_batch += 1;
        largest = largest.max(x.abs());
        if in_batch == MSER_BATCH_SIZE {
            let mean = sum / BATCH_SIZE_F64;
            if !mean.is_finite() {
                return Err(overflow(largest));
            }
            batch_means.push(mean);
            (sum, in_batch) = (0.0, 0);
        }
    }
    let k = batch_means.len();
    let limit = k / 2;
    // Welford's update over the batches from the end, so that after adding
    // batch j the running values describe batches j..k, i.e. truncation d = j.
    let (mut count, mut mean, mut m2) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut best = (0, f64::INFINITY);
    for (d, &z) in batch_means.iter().enumerate().rev() {
        count += 1.0;
        let delta = z - mean;
        mean += delta / count;
        m2 += delta * (z - mean);
        if d <= limit {
            let statistic = m2 / (count * count);
            // Never skip a NaN or infinity: it would silently drop the
            // truncation points that keep the overflowing batches.
            if !statistic.is_finite() {
                return Err(overflow(largest));
            }
            // `<=` while d falls: ties go to the smallest d.
            if statistic <= best.1 {
                best = (d, statistic);
            }
        }
    }
    // d = 0 is always searched, so `best` holds a finite statistic.
    Ok(MserTruncation {
        deleted_batches: best.0,
        batches: k,
        statistic: best.1,
    })
}

/// The MSER-5 truncation point of a series of observations with times,
/// and the time where the warm-up ends.
///
/// ```
/// use taktsim_stats::{mser5_timed, Window};
/// use taktsim_units::SimTime;
///
/// // One observation per minute; the first 20 are a transient.
/// let series: Vec<(SimTime, f64)> = (0..70u64)
///     .map(|i| {
///         let level = if i < 20 { 30.0 } else { 10.0 + if i % 2 == 0 { 0.5 } else { -0.5 } };
///         (SimTime::from_nanos(i * 60_000_000_000), level)
///     })
///     .collect();
/// let cut = mser5_timed(&series)?;
/// assert_eq!(cut.truncation().deleted_observations(), 20);
/// let end = cut.warm_up_end().expect("a transient was found");
/// assert_eq!(end.to_string(), "20min");
/// let window = Window::new(SimTime::START, "70min".parse()?)?.with_warm_up_end(end)?;
/// assert_eq!(window.observed().to_string(), "50min");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedMserTruncation {
    truncation: MserTruncation,
    warm_up_end: Option<SimTime>,
}

impl TimedMserTruncation {
    /// The truncation point in observations.
    ///
    /// ```
    /// use taktsim_stats::mser5_timed;
    /// use taktsim_units::SimTime;
    ///
    /// let flat: Vec<(SimTime, f64)> = (0..20).map(|i| (SimTime::from_nanos(i), 1.0)).collect();
    /// assert_eq!(mser5_timed(&flat)?.truncation().deleted_observations(), 0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn truncation(&self) -> MserTruncation {
        self.truncation
    }

    /// The end of the warm-up: the time of the first observation kept;
    /// `None` if nothing is deleted. A window whose warm-up ends here
    /// excludes exactly the deleted observations when times strictly
    /// increase (with equal times at the cut, the earlier ones are kept
    /// too).
    ///
    /// ```
    /// use taktsim_stats::mser5_timed;
    /// use taktsim_units::SimTime;
    ///
    /// let flat: Vec<(SimTime, f64)> = (0..20).map(|i| (SimTime::from_nanos(i), 1.0)).collect();
    /// assert_eq!(mser5_timed(&flat)?.warm_up_end(), None);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn warm_up_end(&self) -> Option<SimTime> {
        self.warm_up_end
    }
}

/// [`mser5`] for observations with times, which must not decrease; also
/// returns the time where the warm-up ends.
///
/// # Errors
///
/// As [`mser5`], and [`StatsError::TimesOutOfOrder`] if a time is before
/// the one before it.
///
/// ```
/// use taktsim_stats::{mser5_timed, StatsError};
/// use taktsim_units::SimTime;
///
/// let mut series: Vec<(SimTime, f64)> = (0..20).map(|i| (SimTime::from_nanos(i), 1.0)).collect();
/// series.swap(3, 4);
/// assert!(matches!(mser5_timed(&series), Err(StatsError::TimesOutOfOrder { index: 4, .. })));
/// ```
pub fn mser5_timed(observations: &[(SimTime, f64)]) -> Result<TimedMserTruncation, StatsError> {
    if let Some(index) = observations
        .windows(2)
        .position(|pair| pair[1].0 < pair[0].0)
    {
        return Err(StatsError::TimesOutOfOrder {
            what: "MSER-5 observations",
            index: index + 1,
            previous: observations[index].0,
            found: observations[index + 1].0,
        });
    }
    let truncation = truncate(observations.iter().map(|&(_, value)| value))?;
    let deleted = truncation.deleted_observations();
    let warm_up_end = if deleted == 0 {
        None
    } else {
        observations.get(deleted).map(|&(time, _)| time)
    };
    Ok(TimedMserTruncation {
        truncation,
        warm_up_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistic_matches_the_definition() {
        // k = 4 batches with means 9, 1, 3, 5 (each batch constant).
        // d = 0: mean 4.5, SSE 20.25 + 12.25 + 2.25 + 0.25 = 35, /16 = 2.1875.
        // d = 1: mean 3, SSE 4 + 0 + 4 = 8, /9 = 0.888…
        // d = 2: mean 4, SSE 1 + 1 = 2, /4 = 0.5 (the search limit, k/2 = 2).
        let series: Vec<f64> = [9.0, 1.0, 3.0, 5.0].iter().flat_map(|&m| [m; 5]).collect();
        let cut = mser5(&series).unwrap();
        assert_eq!(cut.deleted_batches(), 2);
        assert_eq!(cut.statistic(), 0.5);
        assert!(cut.reached_search_limit());
    }

    #[test]
    fn ties_go_to_the_smallest_truncation() {
        // Batch means 1, 1, 1, 1: every MSER(d) is 0.
        let cut = mser5(&[1.0; 20]).unwrap();
        assert_eq!(cut.deleted_batches(), 0);
    }

    #[test]
    fn rejects_non_finite_and_overflowing_values() {
        let mut series = vec![1.0; 20];
        series[7] = f64::NAN;
        assert!(matches!(
            mser5(&series),
            Err(StatsError::NotFinite { index: 7, .. })
        ));
        let mut series = vec![f64::MAX; 20];
        series[0] = -f64::MAX;
        assert!(matches!(mser5(&series), Err(StatsError::Overflow { .. })));
    }

    #[test]
    fn finite_values_whose_batch_sum_overflows_are_an_error() {
        // Each value is finite, but the first batch's sum is not; the
        // statistic of every truncation that keeps that batch is NaN.
        let mut series = vec![1.0; 30];
        series[..5].fill(f64::MAX);
        let err = mser5(&series).unwrap_err();
        assert!(matches!(err, StatsError::Overflow { .. }), "{err:?}");
        // The same in the last batch, which every truncation keeps.
        let mut series = vec![1.0; 30];
        series[25..].fill(-f64::MAX);
        assert!(matches!(mser5(&series), Err(StatsError::Overflow { .. })));
    }

    #[test]
    fn short_series_error_names_the_method() {
        let err = mser5(&[0.0; 3]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "MSER-5 needs at least 15 observations, found 3; \
             record a longer series, e.g. 1000 observations"
        );
    }
}
