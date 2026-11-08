//! Source: where parts come from (spec §4, ADR-0022 §7).

use std::collections::{BTreeMap, VecDeque};

use taktsim_core::random::{DurationDistribution, Stream};
use taktsim_core::{Component, ComponentError, Context, Delivery, ItemId, Violation};
use taktsim_model::{
    ComponentParams, Decode, Decoder, Distribution, Encode, Fields, Json, LoadErrorKind, Node,
    Record, Schemas, Segment, Value,
};
use taktsim_units::{Count, Ratio};

use crate::flow::{Flow, FlowShared, StationFacts, variant_id};
use crate::message::{FlowMessage, ItemAttrs, SourceStateNote, VariantId};
use crate::params::{Interarrival, Units, VARIANT_NAME_RULE, is_variant_name};
use crate::port::Outlet;
use crate::run::{BuildError, ComponentType, Ports, Wiring};
use crate::state::SourceState;

/// How far shares may be from 100 %, in percentage points: `Ratio` holds a
/// percentage as an `f64`.
const SHARE_TOLERANCE: f64 = 1e-9;

/// The variants of a Source with their shares, sorted by name.
#[derive(Debug, Clone)]
pub(crate) struct VariantMix(BTreeMap<String, Ratio>);

impl Decode for VariantMix {
    const EXPECTED: &'static str =
        "a mapping from variant to share, such as `{basic: 80%, premium: 20%}`";
    const EXAMPLE: &'static str = "{basic: 80%, premium: 20%}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Map(entries) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        if entries.is_empty() {
            let kind = LoadErrorKind::TooFew {
                what: "a Source's variants",
                min: 1,
                found: 0,
                example: Self::EXAMPLE,
            };
            cx.error(node.start(), kind);
            return None;
        }
        let mut shares = BTreeMap::new();
        let mut ok = true;
        for entry in entries {
            let name = entry.key();
            cx.within(Segment::Key(name.to_owned()), |cx| {
                if !is_variant_name(name) {
                    let kind = LoadErrorKind::NotAllowed {
                        found: name.to_owned(),
                        allowed: VARIANT_NAME_RULE.to_owned(),
                        example: Self::EXAMPLE.to_owned(),
                    };
                    cx.error(entry.key_start(), kind);
                    ok = false;
                    return;
                }
                let Some(share) = Ratio::decode(entry.value(), cx) else {
                    ok = false;
                    return;
                };
                if share.as_percent() <= 0.0 {
                    cx.not_allowed(
                        entry.value(),
                        share.to_string(),
                        "a share above 0%; leave out a variant that is never made",
                        "20%",
                    );
                    ok = false;
                    return;
                }
                shares.insert(name.to_owned(), share);
            });
        }
        if !ok {
            return None;
        }
        let total: f64 = shares.values().map(|s| s.as_percent()).sum();
        if (total - 100.0).abs() > SHARE_TOLERANCE {
            cx.not_allowed(
                node,
                format!("shares summing to {total}%"),
                "shares summing to 100%",
                Self::EXAMPLE,
            );
            return None;
        }
        Some(Self(shares))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("object")),
            ("minProperties", Json::from(1)),
            ("propertyNames", schemas.of::<taktsim_model::ComponentRef>()),
            ("additionalProperties", schemas.of::<Ratio>()),
            (
                "description",
                Json::from(
                    "The share of each variant, each above 0 %, summing to 100 %. One variant and \
                     one priority per batch. Default: one variant, `default`.",
                ),
            ),
        ])
    }
}

impl Encode for VariantMix {
    fn encode(&self) -> Json {
        Json::Object(
            self.0
                .iter()
                .map(|(name, share)| (name.clone(), share.encode()))
                .collect(),
        )
    }
}

/// A Source's `priority`: 0 to 255, higher goes first.
#[derive(Debug, Clone, Copy)]
struct Priority(u8);

impl Decode for Priority {
    const EXPECTED: &'static str = "a whole number from 0 to 255";
    const EXAMPLE: &'static str = "1";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let number = u32::decode(node, cx)?;
        let Ok(priority) = u8::try_from(number) else {
            cx.not_allowed(node, number.to_string(), Self::EXPECTED, Self::EXAMPLE);
            return None;
        };
        Some(Self(priority))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("integer")),
            ("minimum", Json::from(0)),
            ("maximum", Json::from(255)),
            (
                "description",
                Json::from(
                    "The priority of its parts in a buffer with the `priority` discipline: higher \
                     goes first, default 0.",
                ),
            ),
        ])
    }
}

impl Encode for Priority {
    fn encode(&self) -> Json {
        u32::from(self.0).encode()
    }
}

/// One entry of `priority_mix`.
#[derive(Debug, Clone, Copy)]
struct PriorityShare {
    priority: Priority,
    share: Ratio,
}

impl Record for PriorityShare {
    const EXPECTED: &'static str = "a priority and its share, such as `{priority: 1, share: 20%}`";
    const EXAMPLE: &'static str = "{priority: 1, share: 20%}";

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let priority = fields.required("priority");
        let share = fields.required("share");
        Some(Self {
            priority: priority?,
            share: share?,
        })
    }
}

impl Encode for PriorityShare {
    fn encode(&self) -> Json {
        Json::object([])
            .with("priority", &self.priority)
            .with("share", &self.share)
    }
}

/// `priority_mix`: at least two entries, distinct priorities, shares above 0 %
/// summing to 100 %, sorted by priority (ADR-0023 §7). A list, because the keys of a
/// JSON mapping are text and a priority is a number.
#[derive(Debug, Clone)]
struct PriorityMix(Vec<PriorityShare>);

impl Decode for PriorityMix {
    const EXPECTED: &'static str = "a list of priorities with shares, such as `[{priority: 1, share: 20%}, {priority: 0, share: 80%}]`";
    const EXAMPLE: &'static str = "[{priority: 1, share: 20%}, {priority: 0, share: 80%}]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::List(items) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        if items.len() < 2 {
            let kind = LoadErrorKind::TooFew {
                what: "a priority_mix",
                min: 2,
                found: items.len(),
                example: Self::EXAMPLE,
            };
            cx.error(node.start(), kind);
            return None;
        }
        let mut entries = Vec::<PriorityShare>::decode(node, cx)?;
        let mut ok = true;
        for (i, entry) in entries.iter().enumerate() {
            if entries[..i]
                .iter()
                .any(|e| e.priority.0 == entry.priority.0)
            {
                cx.within(Segment::Index(i), |cx| {
                    cx.not_allowed(
                        &items[i],
                        format!("the priority {} twice", entry.priority.0),
                        "each priority once",
                        Self::EXAMPLE,
                    );
                });
                ok = false;
            }
            if entry.share.as_percent() <= 0.0 {
                cx.within(Segment::Index(i), |cx| {
                    cx.not_allowed(
                        &items[i],
                        entry.share.to_string(),
                        "a share above 0%; leave out a priority that never occurs",
                        Self::EXAMPLE,
                    );
                });
                ok = false;
            }
        }
        if !ok {
            return None;
        }
        let total: f64 = entries.iter().map(|e| e.share.as_percent()).sum();
        if (total - 100.0).abs() > SHARE_TOLERANCE {
            cx.not_allowed(
                node,
                format!("shares summing to {total}%"),
                "shares summing to 100%",
                Self::EXAMPLE,
            );
            return None;
        }
        entries.sort_by_key(|e| e.priority.0);
        Some(Self(entries))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<PriorityShare>()),
            ("minItems", Json::from(2)),
            (
                "description",
                Json::from(
                    "The share of each priority, instead of `priority`, each above 0 %, summing to \
                     100 %. One priority per batch.",
                ),
            ),
        ])
    }
}

impl Encode for PriorityMix {
    fn encode(&self) -> Json {
        self.0.encode()
    }
}

/// `due_in`: the time from a part's creation to its due date. Zero is allowed.
#[derive(Debug, Clone, Copy)]
struct DueIn(Distribution);

impl Decode for DueIn {
    const EXPECTED: &'static str = Distribution::EXPECTED;
    const EXAMPLE: &'static str = "{dist: uniform, min: 2h, max: 8h}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        Distribution::decode(node, cx).map(Self)
    }

    fn schema(schemas: &mut Schemas) -> Json {
        schemas.of::<Distribution>().with(
            "description",
            &"The due date of its parts, from creation. Needs `interarrival`: an on-demand part \
              is created when it is accepted, so a rule would rank it by a date it does not yet \
              have. Absent: no due date.",
        )
    }
}

impl Encode for DueIn {
    fn encode(&self) -> Json {
        self.0.encode()
    }
}

/// `type: Source`: where parts come from.
///
/// `interarrival` is the time between batches, counted from the last part of
/// the batch leaving; absent, the Source makes a part whenever the next
/// component takes one. `batch` (default `1 parts`) needs `interarrival`.
/// `variants` gives the shares of each variant, `priority` (0 to 255, default
/// 0) the priority of its parts, or `priority_mix` the share of each priority.
/// `due_in`, which needs `interarrival`, gives their due date from creation.
///
/// ```
/// use taktsim_components::Source;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components:\n\
///      - {id: source, type: Source, interarrival: 40s, batch: 2 parts, variants: {premium: 20%, basic: 80%}, priority: 1}\n\
///      - {id: sink, type: Sink}\n\
///      flow: [[source, sink]]\n",
///     Format::Yaml,
/// )?;
/// let source = model.document().components()[0].params::<Source>();
/// assert!(source.is_some_and(|s| !s.on_demand()));
/// assert!(model.canonical_json().contains(r#""variants":{"basic":"80%","premium":"20%"}"#));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Source {
    interarrival: Option<Distribution>,
    batch: Option<Count>,
    variants: Option<VariantMix>,
    priority: Option<Priority>,
    priority_mix: Option<PriorityMix>,
    due_in: Option<DueIn>,
}

impl Source {
    /// Whether it makes parts on demand: no `interarrival`.
    ///
    /// ```
    /// use taktsim_components::Source;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, cut, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let source = model.document().components()[0].params::<Source>();
    /// assert!(source.is_some_and(Source::on_demand));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn on_demand(&self) -> bool {
        self.interarrival.is_none()
    }

    /// The names of the variants it makes, sorted; `None` if it declares none.
    pub(crate) fn variant_names(&self) -> Option<impl Iterator<Item = &str>> {
        self.variants
            .as_ref()
            .map(|mix| mix.0.keys().map(String::as_str))
    }
}

impl Encode for Source {
    fn encode(&self) -> Json {
        Json::object([])
            .with_optional("interarrival", self.interarrival.as_ref())
            .with_optional("batch", self.batch.as_ref())
            .with_optional("variants", self.variants.as_ref())
            .with_optional("priority", self.priority.as_ref())
            .with_optional("priority_mix", self.priority_mix.as_ref())
            .with_optional("due_in", self.due_in.as_ref())
    }
}

impl ComponentParams for Source {
    const TYPE: &'static str = "Source";
    const EXAMPLE: &'static str =
        "{id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let interarrival = fields.optional::<Interarrival>("interarrival");
        let batch = fields.optional::<Units>("batch");
        let variants = fields.optional("variants");
        let priority = fields.optional("priority");
        let priority_mix = fields.optional("priority_mix");
        let due_in = fields.optional("due_in");
        if let Some(entry) = fields.entry("batch")
            && fields.entry("interarrival").is_none()
        {
            let cx = fields.decoder();
            cx.within(Segment::Key("batch".to_owned()), |cx| {
                cx.not_allowed(
                    entry.value(),
                    "a batch on a Source without `interarrival`",
                    "`batch` together with `interarrival`: an on-demand Source makes one part at \
                     a time",
                    "{id: source, type: Source, interarrival: 40s, batch: 2 parts}",
                );
            });
            return None;
        }
        if let Some(entry) = fields.entry("due_in")
            && fields.entry("interarrival").is_none()
        {
            let cx = fields.decoder();
            cx.within(Segment::Key("due_in".to_owned()), |cx| {
                cx.not_allowed(
                    entry.value(),
                    "a due date on a Source without `interarrival`",
                    "`due_in` together with `interarrival`: an on-demand part is created when it \
                     is accepted, so a rule would rank it by a date it does not yet have, and \
                     counting from the offer would measure time blocked, not an order's age",
                    "{id: source, type: Source, interarrival: 40s, due_in: 8h}",
                );
            });
            return None;
        }
        if let Some(entry) = fields.entry("priority_mix")
            && fields.entry("priority").is_some()
        {
            let cx = fields.decoder();
            cx.within(Segment::Key("priority_mix".to_owned()), |cx| {
                cx.not_allowed(
                    entry.value(),
                    "`priority_mix` together with `priority`",
                    "one of them: one priority for every part, or the share of each",
                    "{id: source, type: Source, priority_mix: [{priority: 1, share: 20%}, {priority: 0, share: 80%}]}",
                );
            });
            return None;
        }
        Some(Self {
            interarrival: interarrival?.map(|i| i.0),
            batch: batch?.map(|b| b.0),
            variants: variants?,
            priority: priority?,
            priority_mix: priority_mix?,
            due_in: due_in?,
        })
    }
}

impl ComponentType<Flow> for Source {
    const PORTS: Ports = Ports::Outlet;

    fn facts(&self) -> StationFacts {
        StationFacts {
            routes: true,
            ..StationFacts::default()
        }
    }

    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        let interarrival =
            self.interarrival
                .map(|d| d.sampler())
                .transpose()
                .map_err(|source| BuildError::Distribution {
                    path: wiring.path().clone(),
                    source: Box::new(source),
                })?;
        let shared = wiring.shared();
        let default = [("default", 1.0)];
        let mut mix = Vec::new();
        let mut names = Vec::new();
        let mut cumulative = 0.0;
        let declared: Vec<(&str, f64)> = self.variants.as_ref().map_or_else(
            || default.to_vec(),
            |m| {
                m.0.iter()
                    .map(|(name, share)| (name.as_str(), share.as_fraction()))
                    .collect()
            },
        );
        for (name, fraction) in declared {
            let id = variant_id(shared, name, wiring.path())?;
            cumulative += fraction;
            mix.push((id, cumulative));
            names.push((id, name.to_owned()));
        }
        Ok(Box::new(SourceBehaviour {
            interarrival,
            interval_stream: None,
            variant_stream: None,
            batch: self.batch.map_or(1, Count::get),
            mix,
            names,
            priority: self.priority.map_or(0, |p| p.0),
            downstream: wiring.downstream().to_vec(),
            outlet: Outlet::default(),
            held: VecDeque::new(),
            created: 0,
            emitted: 0,
            noted: None,
        }))
    }
}

/// A Source at run time.
struct SourceBehaviour {
    interarrival: Option<DurationDistribution>,
    interval_stream: Option<Stream>,
    /// Taken with two or more variants.
    variant_stream: Option<Stream>,
    batch: u64,
    /// Each variant with its cumulative fraction, in the order of its name.
    mix: Vec<(VariantId, f64)>,
    names: Vec<(VariantId, String)>,
    priority: u8,
    downstream: Vec<String>,
    outlet: Outlet,
    /// Created and not yet released, in creation order.
    held: VecDeque<(ItemId, ItemAttrs)>,
    created: u64,
    emitted: u64,
    noted: Option<SourceStateNote>,
}

impl SourceBehaviour {
    /// The attributes of the next part or batch: the `variant` stream against
    /// cumulative fractions over sorted names, the last name taking what
    /// rounding leaves (ADR-0022 §8).
    fn next_attrs(&mut self) -> Result<ItemAttrs, ComponentError> {
        let draw = self.variant_stream.as_mut().map(Stream::next_open01);
        let chosen = draw
            .and_then(|u| self.mix.iter().find(|(_, up_to)| u < *up_to))
            .or_else(|| self.mix.last())
            .ok_or_else(|| ComponentError::Unexpected {
                state: "a source without variants",
                event: "NextArrival".to_owned(),
            })?;
        Ok(ItemAttrs::new(chosen.0, self.priority))
    }

    /// Starts the next interval, if the Source has intervals.
    fn next_interval(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if let (Some(distribution), Some(stream)) =
            (&self.interarrival, self.interval_stream.as_mut())
        {
            let after = distribution.sample(stream)?;
            ctx.schedule(after, FlowMessage::NextArrival)?;
        }
        Ok(())
    }

    /// Offers while it has something to give; writes a note if anything
    /// changed.
    fn settle(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if self.interarrival.is_none() || !self.held.is_empty() {
            self.outlet.offer(ctx)?;
        } else {
            self.outlet.withdraw(ctx)?;
        }
        let state = if self.held.is_empty() {
            SourceState::Waiting
        } else {
            SourceState::Blocked
        };
        // At most 65,535 parts (`batch`).
        let held = u32::try_from(self.held.len()).unwrap_or(u32::MAX);
        let note = SourceStateNote { state, held };
        if self.noted != Some(note) {
            self.noted = Some(note);
            ctx.note(&FlowMessage::SourceState(note));
        }
        Ok(())
    }
}

impl Component<FlowMessage> for SourceBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.outlet.connect(&self.downstream, ctx)?;
        if self.interarrival.is_some() {
            self.interval_stream = Some(ctx.stream("interarrival")?);
        }
        if self.mix.len() > 1 {
            self.variant_stream = Some(ctx.stream("variant")?);
        }
        for (id, name) in &self.names {
            ctx.note(&FlowMessage::VariantName {
                id: *id,
                name: name.clone(),
            });
        }
        self.next_interval(ctx)?;
        self.settle(ctx)
    }

    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        match d.message {
            FlowMessage::NextArrival => {
                let attrs = self.next_attrs()?;
                for _ in 0..self.batch {
                    let item = ctx.create_item();
                    self.created += 1;
                    self.held.push_back((item, attrs));
                }
            }
            FlowMessage::Accept => {
                self.outlet.accepted(d.from, ctx)?;
                if self.interarrival.is_none() {
                    // On demand: the part exists from this moment.
                    let attrs = self.next_attrs()?;
                    let item = ctx.create_item();
                    self.created += 1;
                    self.emitted += 1;
                    ctx.transfer(item, d.from, FlowMessage::Arrive(attrs))?;
                } else if let Some((item, attrs)) = self.held.pop_front() {
                    self.emitted += 1;
                    ctx.transfer(item, d.from, FlowMessage::Arrive(attrs))?;
                    if self.held.is_empty() {
                        self.next_interval(ctx)?;
                    }
                } else {
                    ctx.send(d.from, FlowMessage::Decline)?;
                }
            }
            message @ (FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::CycleDone { .. }
            | FlowMessage::SetupDone
            | FlowMessage::Arrive(_)
            | FlowMessage::Decline
            | FlowMessage::Offer
            | FlowMessage::Withdraw
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }
            | FlowMessage::Status(_)
            | FlowMessage::OfferOf(_)
            | FlowMessage::Return(_)
            | FlowMessage::RouteDecision(_)
            | FlowMessage::DispatchDecision(_)
            | FlowMessage::EdgeCondition { .. }) => {
                return Err(ComponentError::Unexpected {
                    state: "source",
                    event: format!("{message:?}"),
                });
            }
        }
        self.settle(ctx)
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        for (item, _) in &self.held {
            visit(*item);
        }
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        let held = self.held.len() as u64;
        if self.created != self.emitted + held {
            return Err(Violation {
                invariant: "created = emitted + held",
                detail: format!(
                    "{} created, {} emitted, {held} held",
                    self.created, self.emitted
                ),
            });
        }
        if held > self.batch {
            return Err(Violation {
                invariant: "held <= batch",
                detail: format!("{held} held, batch {}", self.batch),
            });
        }
        if self.interarrival.is_none() && held != 0 {
            return Err(Violation {
                invariant: "on demand => held = 0",
                detail: format!("{held} held"),
            });
        }
        if (held > 0 || self.interarrival.is_none()) && !self.outlet.all_offered() {
            return Err(Violation {
                invariant: "held > 0 or on demand => offer outstanding",
                detail: format!("{held} held"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(on_demand: bool) -> SourceBehaviour {
        let distribution = DurationDistribution::constant(taktsim_units::SimDuration::ZERO);
        SourceBehaviour {
            interarrival: (!on_demand).then_some(distribution),
            interval_stream: None,
            variant_stream: None,
            batch: 1,
            mix: vec![(VariantId::new(0), 1.0)],
            names: Vec::new(),
            priority: 0,
            downstream: Vec::new(),
            outlet: Outlet::default(),
            held: VecDeque::new(),
            created: 0,
            emitted: 0,
            noted: None,
        }
    }

    fn hold(s: &mut SourceBehaviour, n: u64) {
        let attrs = ItemAttrs::new(VariantId::new(0), 0);
        s.held.push_back((ItemId::from_raw(n), attrs));
    }

    #[test]
    fn its_invariants_catch_what_they_say() {
        let mut s = source(false);
        assert!(s.check_invariants().is_ok());
        s.created = 1;
        let v = s.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "created = emitted + held");

        let mut s = source(false);
        s.created = 2;
        hold(&mut s, 0);
        hold(&mut s, 1);
        let v = s.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "held <= batch");

        let mut s = source(true);
        s.created = 1;
        hold(&mut s, 0);
        let v = s.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "on demand => held = 0");

        let mut s = source(false);
        s.created = 1;
        hold(&mut s, 0);
        s.outlet = Outlet::with_unoffered_target();
        let v = s.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "held > 0 or on demand => offer outstanding");

        let mut s = source(true);
        s.outlet = Outlet::with_unoffered_target();
        let v = s.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "held > 0 or on demand => offer outstanding");
    }

    #[test]
    fn a_source_without_variants_cannot_choose_one() {
        let mut s = source(false);
        s.mix.clear();
        assert!(s.next_attrs().is_err());
    }
}
