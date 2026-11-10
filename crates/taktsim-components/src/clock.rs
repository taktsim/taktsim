//! The end of the clock: simulation time ends after about 584 years, and a wait
//! that ends past that never ends (ADR-0022 §6).
//!
//! No horizon lies past the clock, so such an end is after every horizon, and
//! like anything after the horizon it does not happen. A long `mtbf` that means
//! "practically never" means just that. Whether a model loads, and whether a run
//! of it fails, must not depend on a seed (principle 2, ADR-0022).
//!
//! The engine keeps its typed error for scheduling past the clock (ADR-0001): this
//! is what a component does so that it never asks for that. It says so on the run's
//! [`ClockEnd`], so that a run whose items such a wait holds reaches its horizon
//! instead of reading as a stall (ADR-0024 §3).

use taktsim_core::random::{DurationDistribution, Stream};
use taktsim_core::{ComponentError, Context, EventId};
use taktsim_units::{SimDuration, SimTime, UnitError};

use crate::message::FlowMessage;
use crate::run::ClockEnd;

/// One draw, or `None` if it is longer than the clock can hold.
///
/// The draw is taken from the stream either way, so a later draw of the stream
/// does not depend on where this one landed. A sample that is not finite can only
/// be too long: every distribution's parameters are finite and above zero
/// (`DurationDistribution::sample`), so no sampler reaches a NaN.
pub(crate) fn draw(
    distribution: &DurationDistribution,
    stream: &mut Stream,
) -> Result<Option<SimDuration>, ComponentError> {
    match distribution.sample(stream) {
        Ok(after) => Ok(Some(after)),
        Err(UnitError::Overflow { .. } | UnitError::NotFinite { .. }) => Ok(None),
        Err(other) => Err(other.into()),
    }
}

/// Schedules `message` after `after`, unless the end falls past the clock; returns
/// the event and its time if it was scheduled. A wait past the clock is said on
/// `clock_end`.
pub(crate) fn schedule(
    after: Option<SimDuration>,
    message: FlowMessage,
    clock_end: &ClockEnd,
    ctx: &mut Context<'_, FlowMessage>,
) -> Result<Option<(EventId, SimTime)>, ComponentError> {
    let now = ctx.now();
    let Some((after, ends)) =
        after.and_then(|after| now.checked_add(after).ok().map(|ends| (after, ends)))
    else {
        clock_end.wait_never_ends();
        return Ok(None);
    };
    let event = ctx.schedule(after, message)?;
    Ok(Some((event, ends)))
}

#[cfg(test)]
mod tests {
    use taktsim_core::random::StreamKey;

    use super::*;

    fn stream(seed: u64) -> Stream {
        Stream::new(seed, StreamKey::new("m", "cycle_time"))
    }

    /// Exponential of mean 100,000 days against a clock of 213,503.98 days: a draw
    /// is past it with probability exp(-2.135) = 12 %.
    #[test]
    fn a_draw_too_long_for_the_clock_is_no_error() {
        let distribution = DurationDistribution::exponential("100000d".parse().unwrap()).unwrap();
        let mut short = 0;
        let mut long = 0;
        for seed in 1..=64 {
            let expected = distribution.sample(&mut stream(seed));
            let got = draw(&distribution, &mut stream(seed)).unwrap();
            match expected {
                Ok(after) => {
                    assert_eq!(got, Some(after));
                    short += 1;
                }
                Err(UnitError::Overflow { .. }) => {
                    assert_eq!(got, None);
                    long += 1;
                }
                Err(other) => panic!("{other}"),
            }
        }
        assert!(short > 0 && long > 0, "{short} short, {long} long");
    }

    /// `x^1000` overflows `f64` once `x` is above 2.03: `exp(-2.03)` = 13 % of draws.
    #[test]
    fn a_draw_too_long_for_an_f64_is_no_error() {
        let distribution = DurationDistribution::weibull("1d".parse().unwrap(), 0.001).unwrap();
        let mut infinite = 0;
        for seed in 1..=64 {
            if let Err(UnitError::NotFinite { .. }) = distribution.sample(&mut stream(seed)) {
                assert_eq!(draw(&distribution, &mut stream(seed)).unwrap(), None);
                infinite += 1;
            }
        }
        assert!(infinite > 0);
    }

    /// The draw that is dropped still moves the stream.
    #[test]
    fn a_draw_too_long_takes_as_much_of_the_stream_as_any_other() {
        let distribution = DurationDistribution::exponential("100000d".parse().unwrap()).unwrap();
        let seed = (1..=64)
            .find(|seed| distribution.sample(&mut stream(*seed)).is_err())
            .unwrap();
        let mut sampled = stream(seed);
        let mut drawn = stream(seed);
        assert!(distribution.sample(&mut sampled).is_err());
        assert_eq!(draw(&distribution, &mut drawn).unwrap(), None);
        assert_eq!(sampled.next_open01(), drawn.next_open01());
    }
}
