//! Dispatching rules: the key of a unit and the rank among candidates
//! (ADR-0023 §6). Lower wins; ties go to the oldest ready unit, then the place in
//! the station's inlet.

use std::collections::BTreeMap;

use taktsim_model::DispatchRule;
use taktsim_units::SimTime;

use crate::message::{ItemAttrs, VariantId};

/// The most candidates a dispatching record lists; the rest rank after the last
/// (ADR-0023 §9, C2).
pub(crate) const MAX_LISTED: usize = 1024;

/// A station's declared rule, with the numbers it needs, built from facts.
#[derive(Debug, Clone)]
pub(crate) struct TakerPlan {
    pub(crate) rule: DispatchRule,
    /// The mean time of a unit in ns, for a variant `times` does not list.
    pub(crate) default_time: u64,
    /// The mean time of a unit in ns, by variant.
    pub(crate) times: BTreeMap<VariantId, u64>,
    /// The mean change in ns, for a change `changes` does not list; `None` if such
    /// a change takes no time.
    pub(crate) per_variant: Option<u64>,
    /// The mean change in ns, by `(from, to)`.
    pub(crate) changes: BTreeMap<(VariantId, VariantId), u64>,
}

impl TakerPlan {
    /// The key of a unit: lower wins (ADR-0023 §6, the table). `follows` is what
    /// the next accepted unit follows; `None` before the first unit.
    pub(crate) fn key(&self, attrs: ItemAttrs, since: SimTime, follows: Option<VariantId>) -> u64 {
        match self.rule {
            DispatchRule::Fifo => since.as_nanos(),
            DispatchRule::EarliestDueDate => attrs.due.map_or(u64::MAX, SimTime::as_nanos),
            DispatchRule::ShortestProcessingTime => self
                .times
                .get(&attrs.variant)
                .copied()
                .unwrap_or(self.default_time),
            DispatchRule::MinimumSetup => self.setup_key(follows, attrs.variant),
            DispatchRule::PriorityClass => u64::from(u8::MAX - attrs.priority),
        }
    }

    /// `2 × mean setup ns`, plus 1 if the variant changes: the same variant first,
    /// then a free change, then the cheapest. The first unit, and a machine with no
    /// `setup`, have no change to rank by, so the oldest goes first.
    fn setup_key(&self, follows: Option<VariantId>, variant: VariantId) -> u64 {
        let Some(from) = follows else {
            return 0;
        };
        if from == variant {
            return 0;
        }
        let mean = self
            .changes
            .get(&(from, variant))
            .copied()
            .or(self.per_variant)
            .unwrap_or(0);
        mean.saturating_mul(2).saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(rule: DispatchRule) -> TakerPlan {
        TakerPlan {
            rule,
            default_time: 45,
            times: BTreeMap::from([(VariantId::new(1), 60)]),
            per_variant: Some(240),
            changes: BTreeMap::from([
                ((VariantId::new(0), VariantId::new(1)), 360),
                ((VariantId::new(1), VariantId::new(0)), 0),
            ]),
        }
    }

    fn unit(variant: u16, priority: u8, due: Option<u64>) -> ItemAttrs {
        let mut attrs = ItemAttrs::new(VariantId::new(variant), priority);
        attrs.due = due.map(SimTime::from_nanos);
        attrs
    }

    #[test]
    fn each_rule_has_its_key() {
        let since = SimTime::from_nanos(7);
        let key = |rule, attrs, follows| plan(rule).key(attrs, since, follows);
        assert_eq!(key(DispatchRule::Fifo, unit(0, 0, None), None), 7);
        assert_eq!(
            key(DispatchRule::EarliestDueDate, unit(0, 0, Some(9)), None),
            9
        );
        assert_eq!(
            key(DispatchRule::EarliestDueDate, unit(0, 0, None), None),
            u64::MAX
        );
        assert_eq!(
            key(DispatchRule::ShortestProcessingTime, unit(0, 0, None), None),
            45
        );
        assert_eq!(
            key(DispatchRule::ShortestProcessingTime, unit(1, 0, None), None),
            60
        );
        assert_eq!(
            key(DispatchRule::PriorityClass, unit(0, 2, None), None),
            253
        );
        assert_eq!(
            key(DispatchRule::PriorityClass, unit(0, 255, None), None),
            0
        );
    }

    #[test]
    fn a_change_is_measured_against_what_the_unit_follows() {
        let rule = DispatchRule::MinimumSetup;
        let key = |variant, follows: Option<u16>| {
            plan(rule).key(
                unit(variant, 0, None),
                SimTime::START,
                follows.map(VariantId::new),
            )
        };
        // The first unit, and the same variant.
        assert_eq!(key(1, None), 0);
        assert_eq!(key(1, Some(1)), 0);
        // A free change comes after the same variant.
        assert_eq!(key(0, Some(1)), 1);
        // The matrix, then `per_variant`.
        assert_eq!(key(1, Some(0)), 721);
        assert_eq!(key(2, Some(0)), 481);
        // No setup at all: every change is free.
        let mut none = plan(rule);
        none.per_variant = None;
        none.changes.clear();
        assert_eq!(
            none.key(unit(1, 0, None), SimTime::START, Some(VariantId::new(0))),
            1
        );
        // A mean beyond what a key holds still ranks last.
        let mut huge = plan(rule);
        huge.per_variant = Some(u64::MAX);
        assert_eq!(
            huge.key(unit(2, 0, None), SimTime::START, Some(VariantId::new(0))),
            u64::MAX
        );
    }
}
