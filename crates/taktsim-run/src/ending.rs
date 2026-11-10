//! How a run ended, in a result's words (ADR-0024 §3). Core's `RunOutcome::Idle`
//! says only that no events are left; what follows depends on what is still in
//! the system, and on whether a wait that never ends is pending.

use taktsim_core::RunOutcome;

/// What a run's end means for its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// A result with this explanation; `complete` says what follows.
    Ended {
        stop_reason: &'static str,
        complete: bool,
    },
    /// Items are left, no event, and no wait that never ends: no result.
    Stalled,
}

/// The ending of a run with this `outcome` and this many `items_alive`.
/// `wait_never_ends` says that a component has a wait ending past the end of the
/// clock (ADR-0022 §6): its event is after every horizon, so `Idle` with it is the
/// horizon's row, whatever the items.
pub(crate) const fn ending(outcome: RunOutcome, items_alive: u64, wait_never_ends: bool) -> Ending {
    const HORIZON: Ending = Ending::Ended {
        stop_reason: "horizon",
        complete: true,
    };
    match outcome {
        RunOutcome::Horizon => HORIZON,
        RunOutcome::Idle => {
            if wait_never_ends {
                HORIZON
            } else if items_alive == 0 {
                Ending::Ended {
                    stop_reason: "drained",
                    complete: true,
                }
            } else {
                Ending::Stalled
            }
        }
        RunOutcome::EventLimit => Ending::Ended {
            stop_reason: "event_limit",
            complete: false,
        },
        RunOutcome::Cancelled => Ending::Ended {
            stop_reason: "cancelled",
            complete: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_of_adr_0024() {
        let ended = |stop_reason, complete| Ending::Ended {
            stop_reason,
            complete,
        };
        for alive in [0, 3] {
            for never in [false, true] {
                assert_eq!(
                    ending(RunOutcome::Horizon, alive, never),
                    ended("horizon", true)
                );
                assert_eq!(
                    ending(RunOutcome::EventLimit, alive, never),
                    ended("event_limit", false)
                );
                assert_eq!(
                    ending(RunOutcome::Cancelled, alive, never),
                    ended("cancelled", false)
                );
            }
            assert_eq!(
                ending(RunOutcome::Idle, alive, true),
                ended("horizon", true)
            );
        }
        assert_eq!(ending(RunOutcome::Idle, 0, false), ended("drained", true));
        assert_eq!(ending(RunOutcome::Idle, 1, false), Ending::Stalled);
    }
}
