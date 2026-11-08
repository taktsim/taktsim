//! From a part to a route (ADR-0023 §3): which edge, then which target on it. The
//! table is built from facts by `Flow::prepare`; a plain edge has none, so a chain
//! costs what it did.

use std::sync::Arc;

use taktsim_core::ComponentError;
use taktsim_model::expr::{Condition, Input, Value, Values};

use crate::dispatch::TakerPlan;
use crate::message::{ItemAttrs, RouteRule};

/// How a part leaves a component whose edges are not plain.
#[derive(Debug, Clone)]
pub(crate) struct RouteTable {
    /// The leaving edges, in written order.
    pub(crate) edges: Vec<RouteEdge>,
    /// The targets, each once, in the order `flow` first names them: the outlet's.
    pub(crate) targets: Vec<TargetPlan>,
    /// A condition on some leaving edge: every part leaving writes a `when` record.
    pub(crate) has_when: bool,
    /// A split on some leaving edge: the holder takes a `route` stream.
    pub(crate) has_split: bool,
    /// The model's variant names, for a condition.
    pub(crate) variants: Arc<Vec<String>>,
}

/// One leaving edge.
#[derive(Debug, Clone)]
pub(crate) struct RouteEdge {
    /// The condition with its canonical text; absent on the last edge.
    pub(crate) when: Option<(Condition, String)>,
    /// Its targets, as positions in [`RouteTable::targets`], in list order; sorted
    /// by id for a split.
    pub(crate) targets: Vec<usize>,
    /// How a part chooses among them.
    pub(crate) pick: Pick,
}

/// How a part chooses among the targets of an edge.
#[derive(Debug, Clone)]
pub(crate) enum Pick {
    /// There is one.
    One,
    /// Offers to every target; the first `Accept` takes the part.
    FirstFree,
    /// One standing offer, at the best-ranked target.
    Status(StatusRule),
    /// One draw, at readiness.
    Split {
        /// Each target's fraction, in list order.
        shares: Vec<f64>,
        /// The fractions added up, in list order.
        cumulative: Vec<f64>,
    },
}

/// A choice rule that needs to know about its targets (ADR-0023 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusRule {
    Preferred,
    ShortestQueue,
    LowestUtilization,
}

impl StatusRule {
    pub(crate) const fn rule(self) -> RouteRule {
        match self {
            Self::Preferred => RouteRule::Preferred,
            Self::ShortestQueue => RouteRule::ShortestQueue,
            Self::LowestUtilization => RouteRule::LowestUtilization,
        }
    }
}

/// What a holder knows about one of its targets, built from facts.
#[derive(Debug, Clone)]
pub(crate) struct TargetPlan {
    /// The component's id.
    pub(crate) name: String,
    /// Its declared dispatching rule, if it has one.
    pub(crate) taker: Option<Arc<TakerPlan>>,
    /// How many places it has, for `lowest_utilization`.
    pub(crate) places: u64,
    /// This holder is a gate of a loop whose return target this is: a part for it
    /// goes straight into its return lane.
    pub(crate) returns: bool,
}

/// A part's route: decided once, when it becomes ready, and kept beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Route {
    /// The position of the edge in [`RouteTable::edges`].
    pub(crate) edge: u32,
    /// The target's position in the outlet, if the route names one: one target or
    /// a split. Otherwise a rule chooses among the edge's targets until it leaves.
    pub(crate) target: Option<u32>,
    /// A split's draw, as `f64` bits, for the record.
    pub(crate) draw: u64,
}

/// A part's attributes as a condition reads them (ADR-0023 §2).
struct UnitValues<'a> {
    variant: &'a str,
    priority: i64,
}

impl Values for UnitValues<'_> {
    fn value(&self, input: &Input) -> Option<Value<'_>> {
        match input.as_str() {
            "variant" => Some(Value::Text(self.variant)),
            "priority" => Some(Value::Int(self.priority)),
            _ => None,
        }
    }
}

impl RouteTable {
    /// The first edge whose condition holds, in written order. The last has no
    /// condition, so every part has a way out.
    pub(crate) fn edge_of(&self, attrs: ItemAttrs) -> Result<usize, ComponentError> {
        let variant = self
            .variants
            .get(usize::from(attrs.variant.get()))
            .map_or("", String::as_str);
        let values = UnitValues {
            variant,
            priority: i64::from(attrs.priority),
        };
        for (index, edge) in self.edges.iter().enumerate() {
            let Some((condition, _)) = &edge.when else {
                return Ok(index);
            };
            let holds = condition
                .eval(&values)
                .map_err(|e| ComponentError::Unexpected {
                    state: "a condition that does not evaluate",
                    event: e.to_string(),
                })?;
            if holds {
                return Ok(index);
            }
        }
        Err(ComponentError::Unexpected {
            state: "no leaving edge for a part",
            event: format!("{attrs:?}"),
        })
    }
}

/// The target of a split: `draw` against cumulative fractions, the last taking what
/// rounding leaves, as for `variants` (ADR-0023 §4).
pub(crate) fn split_target(cumulative: &[f64], draw: f64) -> usize {
    cumulative
        .iter()
        .position(|up_to| draw < *up_to)
        .unwrap_or_else(|| cumulative.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draw_falls_in_the_share_it_is_below() {
        let cumulative = [0.05, 1.0];
        assert_eq!(split_target(&cumulative, 0.01), 0);
        assert_eq!(split_target(&cumulative, 0.05), 1);
        assert_eq!(split_target(&cumulative, 0.99), 1);
        // What rounding leaves goes to the last.
        assert_eq!(split_target(&[0.3, 0.999_999], 0.999_999_9), 1);
    }

    #[test]
    fn a_status_rule_names_its_record_rule() {
        assert_eq!(StatusRule::Preferred.rule(), RouteRule::Preferred);
        assert_eq!(StatusRule::ShortestQueue.rule(), RouteRule::ShortestQueue);
        assert_eq!(
            StatusRule::LowestUtilization.rule(),
            RouteRule::LowestUtilization
        );
    }
}
