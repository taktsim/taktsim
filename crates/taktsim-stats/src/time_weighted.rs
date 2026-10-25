//! Time-weighted statistics of piecewise-constant signals, such as WIP over
//! time (spec §4: "WIP … time-weighted").
//!
//! A signal is given by its change points `(time, value)`: from each time on,
//! the signal has that value until the next change. Several changes at the
//! same time are allowed; only the last one counts, as if the others had
//! never happened. The statistics cover the window from the end of the
//! warm-up to the end, so the warm-up is excluded (spec §6).
//!
//! Values are bare `f64` in one unit that the caller declares and keeps for
//! the whole signal (e.g. parts for WIP); the statistics are unit-agnostic,
//! and the KPI layer wraps them in typed quantities (ADR-0014).

use taktsim_units::{SimDuration, SimTime};

use crate::StatsError;

/// The observation window [start, end] with an optional warm-up
/// [start, warm-up end) that the statistics exclude.
///
/// ```
/// use taktsim_stats::Window;
/// use taktsim_units::SimTime;
///
/// let w = Window::new(SimTime::START, "8h".parse()?)?.with_warm_up("1h".parse()?)?;
/// assert_eq!(w.warm_up_end(), "1h".parse::<SimTime>()?);
/// assert_eq!(w.observed().to_string(), "7h");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    start: SimTime,
    warm_up_end: SimTime,
    end: SimTime,
}

impl Window {
    /// The window [start, end] without warm-up.
    ///
    /// # Errors
    ///
    /// [`StatsError::EmptyWindow`] unless start < end.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let end: SimTime = "8h".parse()?;
    /// assert_eq!(Window::new(SimTime::START, end)?.end(), end);
    /// assert!(Window::new(end, end).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(start: SimTime, end: SimTime) -> Result<Self, StatsError> {
        if start < end {
            Ok(Self {
                start,
                warm_up_end: start,
                end,
            })
        } else {
            Err(StatsError::EmptyWindow { start, end })
        }
    }

    /// The same window with a warm-up that ends at `warm_up_end`, e.g. the
    /// time an MSER-5 detection returns.
    ///
    /// # Errors
    ///
    /// [`StatsError::WarmUpOutsideWindow`] unless start ≤ `warm_up_end` < end.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "8h".parse()?)?;
    /// assert!(w.with_warm_up_end("30min".parse()?).is_ok());
    /// assert!(w.with_warm_up_end("8h".parse()?).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_warm_up_end(self, warm_up_end: SimTime) -> Result<Self, StatsError> {
        if self.start <= warm_up_end && warm_up_end < self.end {
            Ok(Self {
                warm_up_end,
                ..self
            })
        } else {
            Err(StatsError::WarmUpOutsideWindow {
                found: warm_up_end,
                start: self.start,
                end: self.end,
            })
        }
    }

    /// The same window with a fixed warm-up period from its start.
    ///
    /// # Errors
    ///
    /// [`StatsError::WarmUpOutsideWindow`] unless the warm-up ends before
    /// the window does.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "8h".parse()?)?;
    /// assert_eq!(w.with_warm_up("2h".parse()?)?.warm_up().to_string(), "2h");
    /// assert!(w.with_warm_up("9h".parse()?).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_warm_up(self, warm_up: SimDuration) -> Result<Self, StatsError> {
        // A warm-up past the largest time is past the end, too.
        let warm_up_end = self
            .start
            .checked_add(warm_up)
            .unwrap_or(SimTime::from_nanos(u64::MAX));
        self.with_warm_up_end(warm_up_end)
    }

    /// The start of the window.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new("1h".parse()?, "2h".parse()?)?;
    /// assert_eq!(w.start(), "1h".parse::<SimTime>()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn start(&self) -> SimTime {
        self.start
    }

    /// The end of the warm-up, where the statistics start; the window start
    /// if there is no warm-up.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new("1h".parse()?, "2h".parse()?)?;
    /// assert_eq!(w.warm_up_end(), w.start());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn warm_up_end(&self) -> SimTime {
        self.warm_up_end
    }

    /// The end of the window.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new("1h".parse()?, "2h".parse()?)?;
    /// assert_eq!(w.end(), "2h".parse::<SimTime>()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn end(&self) -> SimTime {
        self.end
    }

    /// The length of the warm-up.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "2h".parse()?)?;
    /// assert_eq!(w.warm_up().to_string(), "0s");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn warm_up(&self) -> SimDuration {
        duration(self.start, self.warm_up_end)
    }

    /// The length of the observed part, from the end of the warm-up to the
    /// end.
    ///
    /// ```
    /// use taktsim_stats::Window;
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "2h".parse()?)?.with_warm_up("30min".parse()?)?;
    /// assert_eq!(w.observed().to_string(), "90min");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn observed(&self) -> SimDuration {
        duration(self.warm_up_end, self.end)
    }
}

/// `later − earlier` for `earlier ≤ later`.
fn duration(earlier: SimTime, later: SimTime) -> SimDuration {
    SimDuration::from_nanos(later.as_nanos().saturating_sub(earlier.as_nanos()))
}

/// Time-weighted statistics of a piecewise-constant signal over the
/// observed part of a window.
///
/// The mean is (1/T) ∫ v(t) dt and the variance (1/T) ∫ (v(t) − mean)² dt
/// over the observed part of length T: the spread of the path itself, not
/// the variance of an estimator.
///
/// ```
/// use taktsim_stats::{time_weighted, Window};
/// use taktsim_units::SimTime;
///
/// let t = |s: &str| s.parse::<SimTime>().unwrap();
/// // WIP 2 for 1 h, then 4 for 3 h.
/// let wip = [(t("0s"), 2.0), (t("1h"), 4.0)];
/// let stats = time_weighted(&wip, Window::new(t("0s"), t("4h"))?)?;
/// assert_eq!(stats.mean(), 3.5);
/// assert_eq!(stats.variance(), 0.75);
/// assert_eq!((stats.min(), stats.max()), (2.0, 4.0));
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeWeighted {
    window: Window,
    mean: f64,
    variance: f64,
    min: f64,
    max: f64,
}

impl TimeWeighted {
    /// The window, with the warm-up that was excluded.
    ///
    /// ```
    /// use taktsim_stats::{time_weighted, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "1h".parse()?)?;
    /// assert_eq!(time_weighted(&[(SimTime::START, 1.0)], w)?.window(), w);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn window(&self) -> Window {
        self.window
    }

    /// The time-weighted mean.
    ///
    /// ```
    /// use taktsim_stats::{time_weighted, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "1h".parse()?)?;
    /// assert_eq!(time_weighted(&[(SimTime::START, 7.0)], w)?.mean(), 7.0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// The time-weighted variance of the signal around its mean.
    ///
    /// ```
    /// use taktsim_stats::{time_weighted, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let w = Window::new(SimTime::START, "1h".parse()?)?;
    /// assert_eq!(time_weighted(&[(SimTime::START, 7.0)], w)?.variance(), 0.0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn variance(&self) -> f64 {
        self.variance
    }

    /// The time-weighted standard deviation, √variance.
    ///
    /// ```
    /// use taktsim_stats::{time_weighted, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let t = |s: &str| s.parse::<SimTime>().unwrap();
    /// let s = time_weighted(&[(t("0s"), 0.0), (t("1h"), 2.0)], Window::new(t("0s"), t("2h"))?)?;
    /// assert_eq!(s.std_dev(), 1.0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        self.variance.sqrt()
    }

    /// The smallest value the signal takes in the observed part.
    ///
    /// ```
    /// use taktsim_stats::{time_weighted, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let t = |s: &str| s.parse::<SimTime>().unwrap();
    /// let s = time_weighted(&[(t("0s"), 5.0), (t("1h"), 2.0)], Window::new(t("0s"), t("2h"))?)?;
    /// assert_eq!(s.min(), 2.0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn min(&self) -> f64 {
        self.min
    }

    /// The largest value the signal takes in the observed part.
    ///
    /// ```
    /// use taktsim_stats::{time_weighted, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let t = |s: &str| s.parse::<SimTime>().unwrap();
    /// let s = time_weighted(&[(t("0s"), 5.0), (t("1h"), 2.0)], Window::new(t("0s"), t("2h"))?)?;
    /// assert_eq!(s.max(), 5.0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn max(&self) -> f64 {
        self.max
    }
}

/// Time-weighted statistics of the signal with change points `changes`
/// over the observed part of `window`; see [`TimeWeightedAccumulator`].
///
/// # Errors
///
/// As [`TimeWeightedAccumulator::record`] and
/// [`TimeWeightedAccumulator::finish`].
///
/// ```
/// use taktsim_stats::{time_weighted, StatsError, Window};
/// use taktsim_units::SimTime;
///
/// let t = |s: &str| s.parse::<SimTime>().unwrap();
/// let window = Window::new(t("0s"), t("10min"))?.with_warm_up_end(t("2min"))?;
/// // The warm-up value 100 is excluded; 1 from 2 min to 6 min, 3 after.
/// let changes = [(t("0s"), 100.0), (t("2min"), 1.0), (t("6min"), 3.0)];
/// assert_eq!(time_weighted(&changes, window)?.mean(), 2.0);
/// assert!(matches!(
///     time_weighted(&[(t("5min"), 1.0)], window),
///     Err(StatsError::NoValueAtStart { .. })
/// ));
/// # Ok::<(), StatsError>(())
/// ```
pub fn time_weighted(
    changes: &[(SimTime, f64)],
    window: Window,
) -> Result<TimeWeighted, StatsError> {
    let mut accumulator = TimeWeightedAccumulator::new(window);
    for &(time, value) in changes {
        accumulator.record(time, value)?;
    }
    accumulator.finish()
}

/// Time-weighted statistics computed while a run records the signal, one
/// change at a time, without storing the changes.
///
/// Changes at the same instant collapse to the last of them, and segments
/// of equal value are merged before they are weighted, so neither
/// splitting a segment nor a zero-length blip to another value ever changes
/// a bit of the result. The weighted update is West's (1979) form of
/// Welford's algorithm, with weights in seconds of simulated time.
///
/// ```
/// use taktsim_stats::{TimeWeightedAccumulator, Window};
/// use taktsim_units::SimTime;
///
/// let t = |s: &str| s.parse::<SimTime>().unwrap();
/// let mut wip = TimeWeightedAccumulator::new(Window::new(t("0s"), t("1h"))?);
/// wip.record(t("0s"), 0.0)?;
/// wip.record(t("15min"), 4.0)?;
/// assert_eq!(wip.finish()?.mean(), 3.0);
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TimeWeightedAccumulator {
    window: Window,
    /// Changes recorded so far, for error messages.
    recorded: usize,
    /// The latest change, not yet applied: a later change at the same
    /// instant replaces it.
    pending: Option<(SimTime, f64)>,
    /// The value of the open segment, once there is one.
    value: Option<f64>,
    /// Where the open segment starts (never before the warm-up end).
    segment_start: SimTime,
    /// Total weight of the closed segments, in nanoseconds.
    weight: u64,
    mean: f64,
    /// Weighted sum of squared deviations from the mean, in value² · s.
    sum_squares: f64,
    min: f64,
    max: f64,
}

impl TimeWeightedAccumulator {
    /// An accumulator for the observed part of `window`.
    ///
    /// ```
    /// use taktsim_stats::{TimeWeightedAccumulator, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let window = Window::new(SimTime::START, "1h".parse()?)?;
    /// let acc = TimeWeightedAccumulator::new(window);
    /// assert!(acc.finish().is_err()); // no value recorded
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(window: Window) -> Self {
        Self {
            window,
            recorded: 0,
            pending: None,
            value: None,
            segment_start: window.warm_up_end,
            weight: 0,
            mean: 0.0,
            sum_squares: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Records that the signal changes to `value` at `time`. A change at the
    /// same instant as the one before replaces it. Changes before the end of
    /// the warm-up only set the value the observed part starts with; changes
    /// after the end of the window are checked and ignored.
    ///
    /// # Errors
    ///
    /// [`StatsError::NotFinite`] for NaN or infinity,
    /// [`StatsError::TimesOutOfOrder`] if `time` is before the previous
    /// change, [`StatsError::NoValueAtStart`] if the first change comes
    /// after the end of the warm-up. On error nothing changes.
    ///
    /// ```
    /// use taktsim_stats::{TimeWeightedAccumulator, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let t = |s: &str| s.parse::<SimTime>().unwrap();
    /// let mut acc = TimeWeightedAccumulator::new(Window::new(t("0s"), t("1h"))?);
    /// acc.record(t("0s"), 1.0)?;
    /// acc.record(t("10min"), 2.0)?;
    /// assert!(acc.record(t("5min"), 3.0).is_err());
    /// assert!(acc.record(t("20min"), f64::NAN).is_err());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn record(&mut self, time: SimTime, value: f64) -> Result<(), StatsError> {
        if !value.is_finite() {
            return Err(StatsError::NotFinite {
                what: "signal changes",
                index: self.recorded,
                found: value,
            });
        }
        if let Some((previous, _)) = self.pending
            && time < previous
        {
            return Err(StatsError::TimesOutOfOrder {
                what: "signal changes",
                index: self.recorded,
                previous,
                found: time,
            });
        }
        let start = self.window.warm_up_end;
        if self.recorded == 0 && time > start {
            return Err(StatsError::NoValueAtStart {
                start,
                first: Some(time),
            });
        }
        self.recorded += 1;
        if let Some((pending_time, pending_value)) = self.pending
            && pending_time < time
        {
            self.apply(pending_time, pending_value);
        }
        self.pending = Some((time, value));
        Ok(())
    }

    /// The signal takes `value` from `time` on; `time` is later than every
    /// change applied before.
    fn apply(&mut self, time: SimTime, value: f64) {
        match self.value {
            // An identical value extends the open segment instead of
            // closing it, so splitting a segment never changes a result.
            Some(current) if time > self.window.warm_up_end => {
                if value.to_bits() != current.to_bits() {
                    self.close_segment(current, time.min(self.window.end));
                    self.value = Some(value);
                }
            }
            // At or before the end of the warm-up: the value the observed
            // part starts with.
            Some(_) | None => self.value = Some(value),
        }
    }

    /// Adds the segment [segment start, `until`) with `value` to the
    /// statistics and opens the next segment at `until`.
    fn close_segment(&mut self, value: f64, until: SimTime) {
        let nanos = until
            .as_nanos()
            .saturating_sub(self.segment_start.as_nanos());
        self.segment_start = self.segment_start.max(until);
        if nanos == 0 {
            return;
        }
        let previous = SimDuration::from_nanos(self.weight).as_secs_f64();
        // Cannot overflow: all segments lie inside the window.
        self.weight = self.weight.saturating_add(nanos);
        let total = SimDuration::from_nanos(self.weight).as_secs_f64();
        let w = SimDuration::from_nanos(nanos).as_secs_f64();
        let delta = value - self.mean;
        // w / total is exactly 1 for the first segment, so its mean is exact.
        let r = delta * (w / total);
        self.mean += r;
        self.sum_squares += previous * delta * r;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    /// The statistics up to the end of the window: the last recorded value
    /// holds until then.
    ///
    /// # Errors
    ///
    /// [`StatsError::NoValueAtStart`] if no change was recorded,
    /// [`StatsError::Overflow`] if the values are so large that their
    /// squared deviations overflow.
    ///
    /// ```
    /// use taktsim_stats::{TimeWeightedAccumulator, Window};
    /// use taktsim_units::SimTime;
    ///
    /// let t = |s: &str| s.parse::<SimTime>().unwrap();
    /// let mut acc = TimeWeightedAccumulator::new(Window::new(t("0s"), t("1h"))?);
    /// acc.record(t("0s"), 6.0)?;
    /// let stats = acc.finish()?;
    /// assert_eq!((stats.mean(), stats.variance()), (6.0, 0.0));
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn finish(mut self) -> Result<TimeWeighted, StatsError> {
        if let Some((time, value)) = self.pending.take() {
            self.apply(time, value);
        }
        let Some(value) = self.value else {
            return Err(StatsError::NoValueAtStart {
                start: self.window.warm_up_end,
                first: None,
            });
        };
        self.close_segment(value, self.window.end);
        let total = SimDuration::from_nanos(self.weight).as_secs_f64();
        let variance = (self.sum_squares / total).max(0.0);
        if !self.mean.is_finite() || !variance.is_finite() {
            return Err(StatsError::Overflow {
                what: "the time-weighted variance",
                found: self.min.abs().max(self.max.abs()),
                allowed: "signal values whose spread is below about 1e154",
                example: "express the signal in a larger unit",
            });
        }
        Ok(TimeWeighted {
            window: self.window,
            mean: self.mean,
            variance,
            min: self.min,
            max: self.max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> SimTime {
        text.parse().unwrap()
    }

    #[test]
    fn window_errors_carry_the_times() {
        let err = Window::new(t("2h"), t("1h")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the window must end after it starts, found start 2h and end 1h; \
             example: start 0s, end 8h"
        );
        let w = Window::new(t("1h"), t("2h")).unwrap();
        assert!(matches!(
            w.with_warm_up_end(t("30min")),
            Err(StatsError::WarmUpOutsideWindow { .. })
        ));
        assert!(w.with_warm_up(SimDuration::MAX).is_err());
    }

    #[test]
    fn changes_after_the_end_are_ignored() {
        let w = Window::new(t("0s"), t("1h")).unwrap();
        let s = time_weighted(&[(t("0s"), 2.0), (t("1h"), 50.0), (t("2h"), 70.0)], w).unwrap();
        assert_eq!((s.mean(), s.max()), (2.0, 2.0));
    }

    #[test]
    fn the_last_of_simultaneous_changes_holds() {
        let w = Window::new(t("0s"), t("2h")).unwrap();
        let changes = [
            (t("0s"), 1.0),
            (t("1h"), 9.0),
            (t("1h"), 5.0),
            (t("1h"), 3.0),
        ];
        let s = time_weighted(&changes, w).unwrap();
        assert_eq!((s.mean(), s.min(), s.max()), (2.0, 1.0, 3.0));
    }

    #[test]
    fn a_change_exactly_at_the_warm_up_end_starts_the_observed_part() {
        let w = Window::new(t("0s"), t("2h"))
            .unwrap()
            .with_warm_up_end(t("1h"))
            .unwrap();
        let s = time_weighted(&[(t("1h"), 4.0), (t("90min"), 6.0)], w).unwrap();
        assert_eq!(s.mean(), 5.0);
        let err = time_weighted(&[(t("61min"), 4.0)], w).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the signal has no value at 1h, where the statistics start; its first change is \
             at 61min; record its value at or before 1h"
        );
        let err = time_weighted(&[], w).unwrap_err();
        assert!(err.to_string().contains("its first change is missing"));
    }

    #[test]
    fn rejects_bad_changes_by_index() {
        let w = Window::new(t("0s"), t("2h")).unwrap();
        let err = time_weighted(&[(t("0s"), 1.0), (t("1h"), f64::NAN)], w).unwrap_err();
        assert!(matches!(err, StatsError::NotFinite { index: 1, .. }));
        let err =
            time_weighted(&[(t("0s"), 1.0), (t("1h"), 2.0), (t("30min"), 3.0)], w).unwrap_err();
        assert_eq!(
            err.to_string(),
            "signal changes: times must not decrease, but value 2 at 30min follows one at 1h"
        );
    }

    #[test]
    fn a_blip_at_one_instant_changes_nothing() {
        // A change to another value and back at the same instant has no
        // duration, so it must not split the segment around it.
        let w = Window::new(t("0s"), t("3h")).unwrap();
        let base = [(t("0s"), 0.1), (t("1h"), 0.7)];
        let blip = [
            (t("0s"), 0.1),
            (t("1h"), 0.7),
            (t("100min"), 9.0),
            (t("100min"), 0.7),
        ];
        let a = time_weighted(&base, w).unwrap();
        let b = time_weighted(&blip, w).unwrap();
        assert_eq!(a.mean().to_bits(), b.mean().to_bits());
        assert_eq!(a.variance().to_bits(), b.variance().to_bits());
        assert_eq!((a.min(), a.max()), (b.min(), b.max()));
    }

    #[test]
    fn huge_values_overflow_with_an_error() {
        let w = Window::new(t("0s"), t("2h")).unwrap();
        let err = time_weighted(&[(t("0s"), f64::MAX), (t("1h"), -f64::MAX)], w).unwrap_err();
        assert!(matches!(err, StatsError::Overflow { .. }));
    }

    #[test]
    fn is_accurate_for_values_with_a_large_offset() {
        // Values 1e9 + {0, 2} for equal times: mean 1e9 + 1, variance 1. A
        // naive (1/T)∫v² − mean² would lose every digit of the variance
        // (1e18 · 2⁻⁵² ≈ 200). The running mean is only resolved to one ulp
        // of 1e9, 1.2e-7, which bounds the attainable error of the variance
        // to a few times 1e-7.
        let w = Window::new(t("0s"), t("4h")).unwrap();
        let changes = [
            (t("0s"), 1e9),
            (t("1h"), 1e9 + 2.0),
            (t("2h"), 1e9),
            (t("3h"), 1e9 + 2.0),
        ];
        let s = time_weighted(&changes, w).unwrap();
        assert!((s.mean() - (1e9 + 1.0)).abs() < 1e-6, "{}", s.mean());
        assert!((s.variance() - 1.0).abs() < 1e-6, "{}", s.variance());
    }
}
