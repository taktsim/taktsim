//! The protocol between neighbours (ADR-0022 §4, ADR-0023 §3 to §6), shared by every
//! component: an [`Outlet`] offers parts downstream, an [`Inlet`] takes offers from
//! upstream and reserves places.
//!
//! - An `Accept` always gets exactly one reply at the same instant, so a
//!   reservation never lasts longer than an instant.
//! - A holder calls the helpers within every event: a free place and a waiting
//!   offer never coexist, nor a part to give and no offer outstanding, so a
//!   wake-up cannot be lost. A station with a declared rule is the exception
//!   within an instant: it picks at its `Pick`, after every offer of the instant
//!   (ADR-0023 §6).
//! - Between instants, a target holds an entry exactly when its upstream has an
//!   offer outstanding.
//!
//! An `Accept` can cross a `Withdraw` at one instant. Offers and withdrawals
//! keep their order (one priority), and `Accept` carries no number, so the
//! outlet cannot tell which offer an `Accept` answers once it has withdrawn
//! one this instant. It answers the `Accept` as usual and sends `Withdraw`
//! again: the last message on its way to that target then says what the
//! outlet believes, so both sides agree when the instant ends. A description sent
//! again is treated the same way, since it would make a new entry at a target that
//! has already picked the old one.
//!
//! A plain outlet has no route table: every part is for every target, as in M5, so a
//! chain costs what it did. A routed outlet counts the parts meant for each target
//! and each edge, and offers accordingly.

use std::collections::VecDeque;
use std::sync::Arc;

use taktsim_core::random::Stream;
use taktsim_core::{ComponentError, ComponentId, Context, ItemId, Violation};
use taktsim_units::{SimDuration, SimTime};

use crate::dispatch::{MAX_LISTED, TakerPlan};
use crate::message::{
    DispatchCandidate, DispatchDecision, FlowMessage, ItemAttrs, OfferedUnit, RouteCandidate,
    RouteDecision, RouteRule, TargetStatus, VariantId,
};
use crate::route::{Pick, Route, RouteTable, StatusRule, split_target};

/// A component's number as a candidate's id.
fn number(id: ComponentId) -> u32 {
    u32::try_from(id.index()).unwrap_or(u32::MAX)
}

/// A position in a list of at most 1,024 entries, as a record states it.
fn short(position: usize) -> u16 {
    u16::try_from(position).unwrap_or(u16::MAX)
}

/// One downstream neighbour.
#[derive(Debug)]
struct Target {
    id: ComponentId,
    /// An offer is outstanding.
    offered: bool,
    /// When a withdrawal, or a description sent again, last went out: at that
    /// instant an `Accept` can cross it.
    crossing_at: Option<SimTime>,
}

/// What a holder last heard from a target, or assumes before it hears: room and
/// queue 0, which is true at the start (ADR-0023 §5).
#[derive(Debug, Clone, Copy)]
struct Known {
    room: bool,
    queue: u64,
    running: u32,
    worked: u128,
    /// When it heard.
    at: SimTime,
    /// The target's `Accept` of this holder's offer is on its way.
    accept_due: bool,
    follows: Option<VariantId>,
}

impl Known {
    const START: Self = Self {
        room: true,
        queue: 0,
        running: 0,
        worked: 0,
        at: SimTime::START,
        accept_due: false,
        follows: None,
    };

    /// Nanoseconds worked per place since the run began, rounded down (ADR-0023 §5):
    /// the integer that ranks targets as their utilization does.
    fn worked_per_place(&self, places: u64, now: SimTime) -> u64 {
        let elapsed = now.as_nanos().saturating_sub(self.at.as_nanos());
        let worked = self
            .worked
            .saturating_add(u128::from(self.running) * u128::from(elapsed));
        u64::try_from(worked / u128::from(places.max(1))).unwrap_or(u64::MAX)
    }
}

/// The offer that stands for the parts of one status edge: one per edge, so a part
/// keeps its turn at a merge, with the ranking that placed it.
#[derive(Debug)]
struct StandingOffer {
    rule: StatusRule,
    /// Where it stands, as a position in the edge's list.
    stand: Option<usize>,
    /// The positions it moved away from at `left_at`: an `Accept` from one of them may
    /// cross the move, and gets the part (§5).
    left: Vec<usize>,
    left_at: SimTime,
    /// The candidates when the offer was last placed or moved, and the time.
    copy: Vec<RouteCandidate>,
    copied_at: SimTime,
}

/// What was last said about the unit behind an offer to a station with a rule, by a
/// token that names the unit.
#[derive(Debug, Clone, Default)]
struct Described {
    current: Option<(OfferedUnit, u64)>,
    /// The descriptions sent at `said_at` that an `Accept` may yet answer, oldest first.
    /// An `Accept` carries no number, so it is taken to answer the oldest: a station
    /// may have acted on any of them, in the order they reached it.
    unanswered: VecDeque<u64>,
    said_at: Option<SimTime>,
}

impl Described {
    /// A description goes out at `now`. If the station already holds an offer, it can
    /// still answer the one before.
    fn said(&mut self, unit: OfferedUnit, token: u64, offer_stood: bool, now: SimTime) {
        if self.said_at != Some(now) {
            self.unanswered.clear();
            if offer_stood && let Some((_, before)) = self.current {
                self.unanswered.push_back(before);
            }
            self.said_at = Some(now);
        }
        self.unanswered.push_back(token);
        self.current = Some((unit, token));
    }

    /// The token of the description an `Accept` at `now` answers.
    fn answered(&mut self, now: SimTime) -> Option<u64> {
        if self.said_at == Some(now)
            && let Some(token) = self.unanswered.pop_front()
        {
            return Some(token);
        }
        self.current.map(|(_, token)| token)
    }
}

/// A holder's routing state.
#[derive(Debug)]
struct Routed {
    table: Arc<RouteTable>,
    /// The `route` stream, with a split on a leaving edge.
    stream: Option<Stream>,
    /// Parts whose route names each target.
    meant: Vec<u32>,
    /// Parts on each edge whose route names no target.
    on_edge: Vec<u32>,
    known: Vec<Known>,
    described: Vec<Described>,
    /// The standing offer of each edge with a status rule.
    standing: Vec<Option<StandingOffer>>,
    /// Which targets have a part for them, worked out when settling.
    want: Vec<bool>,
    /// A known room flag changed, or the holder was repaired: rank again.
    rerank: bool,
}

/// The rank of target `position` of a status edge: lower wins.
fn rank(
    rule: StatusRule,
    known: &Known,
    places: u64,
    position: usize,
    now: SimTime,
) -> (bool, u64, usize) {
    (
        !known.room,
        status_key(rule, known, places, position, now),
        position,
    )
}

/// The rule's key: the term after room (ADR-0023 §5, §9).
fn status_key(rule: StatusRule, known: &Known, places: u64, position: usize, now: SimTime) -> u64 {
    match rule {
        StatusRule::Preferred => position as u64,
        StatusRule::ShortestQueue => known.queue,
        StatusRule::LowestUtilization => known.worked_per_place(places, now),
    }
}

impl Routed {
    fn new(table: Arc<RouteTable>, stream: Option<Stream>) -> Self {
        let targets = table.targets.len();
        let edges = table.edges.len();
        let standing = table
            .edges
            .iter()
            .map(|edge| match edge.pick {
                Pick::Status(rule) => Some(StandingOffer {
                    rule,
                    stand: None,
                    left: Vec::new(),
                    left_at: SimTime::START,
                    copy: Vec::new(),
                    copied_at: SimTime::START,
                }),
                Pick::One | Pick::FirstFree | Pick::Split { .. } => None,
            })
            .collect();
        Self {
            table,
            stream,
            meant: vec![0; targets],
            on_edge: vec![0; edges],
            known: vec![Known::START; targets],
            described: vec![Described::default(); targets],
            standing,
            want: vec![false; targets],
            rerank: false,
        }
    }

    /// The best-ranked position on edge `edge`.
    fn best(&self, edge: usize, rule: StatusRule, now: SimTime) -> Option<usize> {
        let list = &self.table.edges.get(edge)?.targets;
        (0..list.len()).min_by_key(|&position| self.rank_at(list, position, rule, now))
    }

    fn rank_at(
        &self,
        list: &[usize],
        position: usize,
        rule: StatusRule,
        now: SimTime,
    ) -> (bool, u64, usize) {
        let target = list[position];
        rank(
            rule,
            &self.known[target],
            self.table.targets[target].places,
            position,
            now,
        )
    }

    /// The candidates of edge `edge` as a record states them.
    fn candidates(
        &self,
        edge: usize,
        rule: StatusRule,
        ids: &[Target],
        now: SimTime,
    ) -> Vec<RouteCandidate> {
        let list = self.table.edges.get(edge).map_or(&[][..], |e| &e.targets);
        list.iter()
            .enumerate()
            .map(|(position, &target)| {
                let known = &self.known[target];
                RouteCandidate {
                    id: number(ids[target].id),
                    room: Some(known.room),
                    key: status_key(
                        rule,
                        known,
                        self.table.targets[target].places,
                        position,
                        now,
                    ),
                }
            })
            .collect()
    }

    /// Places or moves the standing offers (ADR-0023 §5). It moves when a target
    /// with room outranks the one where the offer stands, and only after a room flag
    /// changed or a repair. While a target's `Accept` of this holder is on its way, the
    /// part is leaving, and nothing moves.
    fn place_standing_offers(&mut self, ids: &[Target], now: SimTime) {
        let rerank = std::mem::take(&mut self.rerank);
        for edge in 0..self.standing.len() {
            let Some(offer) = self.standing[edge].as_ref() else {
                continue;
            };
            let (rule, stand) = (offer.rule, offer.stand);
            if self.on_edge[edge] == 0 {
                if let Some(offer) = self.standing[edge].as_mut() {
                    offer.stand = None;
                }
                continue;
            }
            let Some(best) = self.best(edge, rule, now) else {
                continue;
            };
            let list = &self.table.edges[edge].targets;
            let place = match stand {
                None => true,
                Some(at) => {
                    let due = self.known[list[at]].accept_due;
                    rerank
                        && !due
                        && best != at
                        && self.known[list[best]].room
                        && self.rank_at(list, best, rule, now) < self.rank_at(list, at, rule, now)
                }
            };
            if place {
                let copy = self.candidates(edge, rule, ids, now);
                if let Some(offer) = self.standing[edge].as_mut() {
                    if let Some(at) = stand {
                        if offer.left_at != now {
                            offer.left.clear();
                            offer.left_at = now;
                        }
                        offer.left.push(at);
                    }
                    offer.stand = Some(best);
                    offer.copy = copy;
                    offer.copied_at = now;
                }
            }
        }
    }

    /// Whether a part with `route` is for the target at `position`: the target its
    /// route names, any target of its first-free edge, or the target where its status
    /// edge's offer stands. With `crossing`, also a target the offer moved away from at
    /// that instant, whose `Accept` may cross the move (§5). Several edges may name one
    /// target (§1), so a part on a status edge is not for every target of its edge.
    fn is_for(&self, route: Route, position: usize, crossing: Option<SimTime>) -> bool {
        if let Some(target) = route.target {
            return usize::try_from(target).is_ok_and(|t| t == position);
        }
        let Some(index) = usize::try_from(route.edge).ok() else {
            return false;
        };
        let Some(edge) = self.table.edges.get(index) else {
            return false;
        };
        match &edge.pick {
            Pick::FirstFree => edge.targets.contains(&position),
            Pick::Status(_) => {
                let Some(offer) = self.standing.get(index).and_then(Option::as_ref) else {
                    return false;
                };
                let stands = |at: usize| edge.targets.get(at) == Some(&position);
                offer.stand.is_some_and(stands)
                    || crossing.is_some_and(|now| {
                        offer.left_at == now && offer.left.iter().any(|&at| stands(at))
                    })
            }
            Pick::One | Pick::Split { .. } => false,
        }
    }

    /// Which targets have a part for them.
    fn work_out_wants(&mut self) {
        for (want, meant) in self.want.iter_mut().zip(&self.meant) {
            *want = *meant > 0;
        }
        for (index, edge) in self.table.edges.iter().enumerate() {
            if self.on_edge[index] == 0 {
                continue;
            }
            match &edge.pick {
                Pick::FirstFree => {
                    for &target in &edge.targets {
                        self.want[target] = true;
                    }
                }
                Pick::Status(_) => {
                    if let Some(at) = self.standing[index].as_ref().and_then(|o| o.stand) {
                        self.want[edge.targets[at]] = true;
                    }
                }
                Pick::One | Pick::Split { .. } => {}
            }
        }
    }
}

/// A station with a rule, asking its upstream which unit stands behind an offer.
pub(crate) struct Asking<'a> {
    routed: &'a Routed,
    position: usize,
    /// The station's rule.
    pub(crate) plan: &'a TakerPlan,
    /// What the holder knows the station's next unit follows.
    pub(crate) follows: Option<VariantId>,
}

impl Asking<'_> {
    /// Whether a part with `route` is meant for this station, where offers stand now.
    pub(crate) fn wants(&self, route: Route) -> bool {
        self.routed.is_for(route, self.position, None)
    }
}

/// The upstream end of the protocol: at most one outstanding offer per neighbour, in
/// the order `flow` first names them.
#[derive(Debug, Default)]
pub(crate) struct Outlet {
    targets: Vec<Target>,
    /// How parts leave, until `connect`; `None` for a plain edge.
    table: Option<Arc<RouteTable>>,
    routed: Option<Box<Routed>>,
}

impl Outlet {
    /// An outlet with `table`, or a plain one.
    pub(crate) fn new(table: Option<Arc<RouteTable>>) -> Self {
        Self {
            targets: Vec::new(),
            table,
            routed: None,
        }
    }

    /// Looks up the targets, in the order `flow` first names them; takes the `route`
    /// stream with a split on a leaving edge, and writes each condition in the trace.
    pub(crate) fn connect(
        &mut self,
        names: &[String],
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        for name in names {
            self.targets.push(Target {
                id: ctx.lookup(name)?,
                offered: false,
                crossing_at: None,
            });
        }
        if let Some(table) = self.table.take() {
            let stream = if table.has_split {
                Some(ctx.stream("route")?)
            } else {
                None
            };
            for (edge, route_edge) in table.edges.iter().enumerate() {
                if let Some((_, text)) = &route_edge.when {
                    ctx.note(&FlowMessage::EdgeCondition {
                        edge: u32::try_from(edge).unwrap_or(u32::MAX),
                        text: text.clone(),
                    });
                }
            }
            self.routed = Some(Box::new(Routed::new(table, stream)));
        }
        Ok(())
    }

    /// Whether parts have routes. A plain outlet keeps M5's path.
    pub(crate) const fn is_routed(&self) -> bool {
        self.routed.is_some()
    }

    /// A part's route, decided once, when it becomes ready (ADR-0023 §3): the first
    /// edge whose condition holds, then one target or a split's draw. A plain outlet
    /// has nothing to decide.
    pub(crate) fn route(&mut self, attrs: ItemAttrs) -> Result<Route, ComponentError> {
        let Some(routed) = self.routed.as_deref_mut() else {
            return Ok(Route::default());
        };
        let index = routed.table.edge_of(attrs)?;
        let edge = &routed.table.edges[index];
        let (target, draw) = match &edge.pick {
            Pick::One => (edge.targets.first().copied(), 0),
            Pick::Split { cumulative, .. } => {
                let draw = routed.stream.as_mut().map_or(1.0, Stream::next_open01);
                (
                    edge.targets.get(split_target(cumulative, draw)).copied(),
                    draw.to_bits(),
                )
            }
            Pick::FirstFree | Pick::Status(_) => (None, 0),
        };
        match target {
            Some(target) => routed.meant[target] += 1,
            None => routed.on_edge[index] += 1,
        }
        Ok(Route {
            edge: u32::try_from(index).unwrap_or(u32::MAX),
            target: target.and_then(|t| u32::try_from(t).ok()),
            draw,
        })
    }

    /// The target a part with `route` goes to straight, if it is coming back into a
    /// loop: it does not offer and wait (ADR-0023 §8).
    pub(crate) fn returns_to(&self, route: Route) -> Option<(usize, ComponentId)> {
        let routed = self.routed.as_deref()?;
        let target = usize::try_from(route.target?).ok()?;
        routed.table.targets.get(target)?.returns.then(|| {
            let id = self.targets.get(target).map(|t| t.id);
            id.map(|id| (target, id))
        })?
    }

    /// Whether an `Accept` from the target at `position` at `now` may take a part with
    /// `route`: an offer to that target stood for it, or crossed a move at `now`.
    pub(crate) fn meant_for(&self, route: Route, position: usize, now: SimTime) -> bool {
        self.routed
            .as_deref()
            .is_none_or(|routed| routed.is_for(route, position, Some(now)))
    }

    /// The plan of the target at `position` if it has a rule, with what the holder
    /// knows it follows.
    pub(crate) fn taker(&self, position: usize) -> Option<(&TakerPlan, Option<VariantId>)> {
        let routed = self.routed.as_deref()?;
        let plan = routed.table.targets.get(position)?.taker.as_deref()?;
        Some((plan, routed.known.get(position)?.follows))
    }

    /// Whether the `Accept` of the target at `position` answers a description of a
    /// unit no longer here, by `present`. Then the holder answers `Decline`: the
    /// station ranked what it was told, and the unit it would get now may rank lower
    /// than the ones it passed over (ADR-0023, disagreement 14). Each description is
    /// answered once, so an instant ends: a new offer after a `Decline` is the next
    /// to be answered.
    pub(crate) fn answers_a_unit_gone(
        &mut self,
        position: usize,
        now: SimTime,
        mut present: impl FnMut(u64) -> bool,
    ) -> bool {
        self.routed
            .as_deref_mut()
            .and_then(|routed| routed.described.get_mut(position))
            .and_then(|described| described.answered(now))
            .is_some_and(|token| !present(token))
    }

    /// Offers to every target without an outstanding offer, in declared order.
    pub(crate) fn offer(
        &mut self,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        for target in &mut self.targets {
            if !target.offered {
                target.offered = true;
                ctx.send(target.id, FlowMessage::Offer)?;
            }
        }
        Ok(())
    }

    /// Withdraws every outstanding offer: there is nothing left to give.
    pub(crate) fn withdraw(
        &mut self,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let now = ctx.now();
        for target in &mut self.targets {
            if target.offered {
                target.offered = false;
                target.crossing_at = Some(now);
                ctx.send(target.id, FlowMessage::Withdraw)?;
            }
        }
        Ok(())
    }

    /// Offers, moves and withdraws what the parts of a routed holder call for
    /// (ADR-0023 §3, §5). `describe` gives the unit behind an offer to a station
    /// with a rule, and a token that names it; a holder sends `OfferOf` again when
    /// that changes, and the entry keeps its place (ADR-0023 §6).
    pub(crate) fn settle_routed(
        &mut self,
        ctx: &mut Context<'_, FlowMessage>,
        mut describe: impl FnMut(&Asking<'_>) -> Option<(OfferedUnit, u64)>,
    ) -> Result<(), ComponentError> {
        let Some(routed) = self.routed.as_deref_mut() else {
            return Ok(());
        };
        let now = ctx.now();
        routed.place_standing_offers(&self.targets, now);
        routed.work_out_wants();
        for (target, want) in self.targets.iter_mut().zip(&routed.want) {
            if target.offered && !want {
                target.offered = false;
                target.crossing_at = Some(now);
                ctx.send(target.id, FlowMessage::Withdraw)?;
            }
        }
        for position in 0..self.targets.len() {
            if !routed.want[position] {
                continue;
            }
            let target = &mut self.targets[position];
            let Some(plan) = routed.table.targets[position].taker.as_deref() else {
                if !target.offered {
                    target.offered = true;
                    ctx.send(target.id, FlowMessage::Offer)?;
                }
                continue;
            };
            let asking = Asking {
                routed,
                position,
                plan,
                follows: routed.known[position].follows,
            };
            let Some((unit, token)) = describe(&asking) else {
                continue;
            };
            let described = &mut routed.described[position];
            let same = described.current.is_some_and(|(said, _)| said == unit);
            if target.offered && same {
                continue;
            }
            if target.offered {
                target.crossing_at = Some(now);
            }
            described.said(unit, token, target.offered, now);
            target.offered = true;
            ctx.send(target.id, FlowMessage::OfferOf(Box::new(unit)))?;
        }
        Ok(())
    }

    /// Takes in a `Status` from `from`. A change of a room flag calls for ranking
    /// again at the end of this event; queue or work alone only update the ranks
    /// (ADR-0023 §5).
    pub(crate) fn status(&mut self, from: ComponentId, status: &TargetStatus, now: SimTime) {
        let Some(routed) = self.routed.as_deref_mut() else {
            return;
        };
        let Some(position) = self.targets.iter().position(|t| t.id == from) else {
            return;
        };
        let known = &mut routed.known[position];
        routed.rerank |= known.room != status.room;
        *known = Known {
            room: status.room,
            queue: status.queue,
            running: status.running,
            worked: status.worked,
            at: now,
            // Until the `Accept` lands, whatever else is said before it.
            accept_due: known.accept_due || status.accept_follows,
            follows: status.follows,
        };
    }

    /// The holder is up again: rank once more, since it moved nothing while down.
    pub(crate) fn repaired(&mut self) {
        if let Some(routed) = self.routed.as_deref_mut() {
            routed.rerank = true;
        }
    }

    /// Takes in an `Accept` from `from`, which the caller answers with an `Arrive` or
    /// a `Decline`, and returns the position of `from`. If a withdrawal to `from`, or a
    /// description sent again, went out at this instant and an offer stands, that
    /// offer may be on its way, so it is withdrawn too (module docs). A standing offer
    /// learns whether the `Accept` crossed a move only when a part leaves (`leaving`),
    /// since an `Accept` from a target several edges name answers one of them.
    pub(crate) fn accepted(
        &mut self,
        from: ComponentId,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<usize, ComponentError> {
        let now = ctx.now();
        let Some(position) = self.targets.iter().position(|t| t.id == from) else {
            return Err(ComponentError::Unexpected {
                state: "an accept from a component that is not downstream",
                event: format!("Accept from component #{}", from.index()),
            });
        };
        let target = &mut self.targets[position];
        let stood = target.offered;
        target.offered = false;
        if stood && target.crossing_at == Some(now) {
            ctx.send(from, FlowMessage::Withdraw)?;
        }
        if let Some(routed) = self.routed.as_deref_mut() {
            routed.known[position].accept_due = false;
        }
        Ok(position)
    }

    /// A part with `route` leaves for the target at `position`: states the decisions
    /// that sent it there, the moment it leaves, so every part has an item and a record
    /// exists exactly when its choice had an effect (ADR-0023 §9).
    pub(crate) fn leaving(
        &mut self,
        route: Route,
        position: usize,
        item: ItemId,
        ctx: &mut Context<'_, FlowMessage>,
    ) {
        let Some(routed) = self.routed.as_deref_mut() else {
            if self.targets.len() > 1 {
                ctx.decide(&plain_first_free(&self.targets, position, item));
            }
            return;
        };
        let index = usize::try_from(route.edge).unwrap_or(usize::MAX);
        match route.target.and_then(|t| usize::try_from(t).ok()) {
            Some(target) => routed.meant[target] = routed.meant[target].saturating_sub(1),
            None => routed.on_edge[index] = routed.on_edge[index].saturating_sub(1),
        }
        if routed.table.has_when {
            ctx.decide(&when_record(route, index, item));
        }
        let Routed {
            table, standing, ..
        } = routed;
        let edge = &table.edges[index];
        let at = edge
            .targets
            .iter()
            .position(|t| *t == position)
            .unwrap_or(usize::MAX);
        let chosen = short(at);
        let id = |target: usize| self.targets.get(target).map_or(u32::MAX, |t| number(t.id));
        let stated = |rule, crossed, detail, candidates| RouteDecision {
            rule,
            item,
            edge: route.edge,
            chosen,
            crossed,
            detail,
            candidates,
        };
        let record = match &edge.pick {
            Pick::One => return,
            Pick::FirstFree => {
                let candidates = edge.targets.iter().map(|&target| RouteCandidate {
                    id: id(target),
                    room: None,
                    key: 0,
                });
                stated(RouteRule::FirstFree, false, 0, candidates.collect())
            }
            Pick::Split { shares, .. } => {
                let candidates =
                    edge.targets
                        .iter()
                        .zip(shares)
                        .map(|(&target, share)| RouteCandidate {
                            id: id(target),
                            room: None,
                            key: share.to_bits(),
                        });
                stated(RouteRule::Split, false, route.draw, candidates.collect())
            }
            Pick::Status(rule) => {
                let Some(offer) = standing[index].as_mut() else {
                    return;
                };
                // The part left from a target where the offer no longer stands: its
                // `Accept` crossed a move, and the offer stays where it moved to.
                let crossed = offer.stand != Some(at);
                if crossed {
                    offer.left.retain(|&left| left != at);
                } else {
                    offer.stand = None;
                }
                stated(
                    rule.rule(),
                    crossed,
                    offer.copied_at.as_nanos(),
                    offer.copy.clone(),
                )
            }
        };
        ctx.decide(&FlowMessage::RouteDecision(Box::new(record)));
    }

    /// Whether every target has an offer outstanding.
    pub(crate) fn all_offered(&self) -> bool {
        self.targets.iter().all(|t| t.offered)
    }

    /// Whether what the parts of a holder call for stands: per part and per status
    /// edge when routed, every target otherwise (ADR-0023 §12).
    pub(crate) fn parts_offered(&self) -> bool {
        let Some(routed) = self.routed.as_deref() else {
            return self.all_offered();
        };
        routed.table.edges.iter().enumerate().all(|(index, edge)| {
            if routed.on_edge[index] == 0 {
                return true;
            }
            match &edge.pick {
                Pick::FirstFree => edge.targets.iter().all(|&t| self.targets[t].offered),
                Pick::Status(_) => routed.standing[index]
                    .as_ref()
                    .and_then(|o| o.stand)
                    .is_some_and(|at| self.targets[edge.targets[at]].offered),
                Pick::One | Pick::Split { .. } => true,
            }
        }) && routed
            .meant
            .iter()
            .zip(&self.targets)
            .all(|(meant, target)| *meant == 0 || target.offered)
    }

    /// An up holder with an offer at a target it knows has no room knows of no target
    /// on that edge with room (ADR-0023 §5, §12): checked on its own state.
    pub(crate) fn check_standing_offers(&self) -> Result<(), Violation> {
        let Some(routed) = self.routed.as_deref() else {
            return Ok(());
        };
        for (index, offer) in routed.standing.iter().enumerate() {
            let Some(at) = offer.as_ref().and_then(|o| o.stand) else {
                continue;
            };
            let list = &routed.table.edges[index].targets;
            let target = list[at];
            let known = &routed.known[target];
            if known.room || known.accept_due {
                continue;
            }
            if let Some(free) = list.iter().find(|t| routed.known[**t].room) {
                return Err(Violation {
                    invariant: "an up holder with an offer at a target it knows has no room \
                                knows of no target on that edge with room",
                    detail: format!(
                        "the offer stands at #{}, and #{} has room",
                        self.targets[target].id.index(),
                        self.targets[*free].id.index()
                    ),
                });
            }
        }
        Ok(())
    }
}

/// M5's default on one edge: nothing but which target took the part.
fn plain_first_free(targets: &[Target], position: usize, item: ItemId) -> FlowMessage {
    let candidates = targets
        .iter()
        .map(|t| RouteCandidate {
            id: number(t.id),
            room: None,
            key: 0,
        })
        .collect();
    FlowMessage::RouteDecision(Box::new(RouteDecision {
        rule: RouteRule::FirstFree,
        item,
        edge: 0,
        chosen: short(position),
        crossed: false,
        detail: 0,
        candidates,
    }))
}

/// The edges tried for a part, in written order, up to the first match: its own.
fn when_record(route: Route, index: usize, item: ItemId) -> FlowMessage {
    let candidates = (0..=index)
        .map(|edge| RouteCandidate {
            id: u32::try_from(edge).unwrap_or(u32::MAX),
            room: None,
            key: u64::from(edge == index),
        })
        .collect();
    FlowMessage::RouteDecision(Box::new(RouteDecision {
        rule: RouteRule::When,
        item,
        edge: route.edge,
        chosen: short(index),
        crossed: false,
        detail: 0,
        candidates,
    }))
}

/// One offer at a station with a rule: what it says about its unit.
#[derive(Debug, Clone, Copy)]
struct DescribedOffer {
    from: ComponentId,
    unit: OfferedUnit,
}

/// A place reserved by a rule's pick. The candidates wait beside it for the `Arrive`
/// that confirms the pick; a `Decline` discards them (ADR-0023 §9).
#[derive(Debug)]
struct Pending {
    from: ComponentId,
    /// The variant its offer described, until the unit comes.
    variant: VariantId,
    /// What the unit follows, for the key of the unit that comes.
    follows: Option<VariantId>,
    candidates: Vec<DispatchCandidate>,
    total: u64,
    /// The pick was made while handling a `Decline`: a pick that failed came first,
    /// so it is on record even with one candidate.
    after_decline: bool,
}

/// A station's declared rule, with the offers it ranks.
#[derive(Debug)]
struct InletRule {
    plan: Arc<TakerPlan>,
    /// Offers in the order they came: the place in the inlet.
    offers: Vec<DescribedOffer>,
    pending: Vec<Pending>,
    /// A `Decline` came at this instant, after the last pick.
    declined: bool,
    /// A `Pick` is on its way, at this instant.
    pick_due: bool,
}

/// One upstream that hears from this target, and what it heard last.
#[derive(Debug)]
struct Subscriber {
    name: String,
    id: Option<ComponentId>,
    last: TargetStatus,
}

/// Who hears from this target.
#[derive(Debug)]
struct Reporting {
    subscribers: Vec<Subscriber>,
    /// Whose offers this event accepted.
    accepted: Vec<ComponentId>,
}

/// What a target states of itself at the end of an event.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetView {
    /// A place is free, it is not down and its return lane is empty.
    pub(crate) room: bool,
    /// Units at the target: waiting to start, in cycle, finished, returning, or in a
    /// Buffer's places.
    pub(crate) units: u64,
    pub(crate) running: u32,
    pub(crate) worked: u128,
    pub(crate) follows: Option<VariantId>,
}

/// The downstream end of the protocol: offers waiting in the order they
/// arrived, and places reserved.
#[derive(Debug, Default)]
pub(crate) struct Inlet {
    waiting: VecDeque<ComponentId>,
    reserved: u64,
    rule: Option<Box<InletRule>>,
    reporting: Option<Box<Reporting>>,
}

impl Inlet {
    /// An inlet with this rule, reporting to these subscribers.
    pub(crate) fn new(plan: Option<Arc<TakerPlan>>, subscribers: &[String]) -> Self {
        Self {
            waiting: VecDeque::new(),
            reserved: 0,
            rule: plan.map(|plan| {
                Box::new(InletRule {
                    plan,
                    offers: Vec::new(),
                    pending: Vec::new(),
                    declined: false,
                    pick_due: false,
                })
            }),
            reporting: (!subscribers.is_empty()).then(|| {
                Box::new(Reporting {
                    subscribers: subscribers
                        .iter()
                        .map(|name| Subscriber {
                            name: name.clone(),
                            id: None,
                            // Upstream assumes this at the start, which is true then.
                            last: TargetStatus {
                                room: true,
                                accept_follows: false,
                                queue: 0,
                                running: 0,
                                worked: 0,
                                follows: None,
                            },
                        })
                        .collect(),
                    accepted: Vec::new(),
                })
            }),
        }
    }

    /// Looks up who hears from this target.
    pub(crate) fn connect(&mut self, ctx: &Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if let Some(reporting) = self.reporting.as_deref_mut() {
            for subscriber in &mut reporting.subscribers {
                subscriber.id = Some(ctx.lookup(&subscriber.name)?);
            }
        }
        Ok(())
    }

    /// Whether this station has a declared rule.
    pub(crate) const fn has_rule(&self) -> bool {
        self.rule.is_some()
    }

    /// Whether anyone hears from this target.
    pub(crate) const fn reports(&self) -> bool {
        self.reporting.is_some()
    }

    /// Whether the end of an event has nothing to do: no rule and no one to tell, so
    /// a chain costs what it did.
    pub(crate) const fn is_plain(&self) -> bool {
        self.rule.is_none() && self.reporting.is_none()
    }

    /// Whether what the next unit follows is worth stating: only a `minimum_setup`
    /// taker's upstream ranks by it (ADR-0023 §6), and a change of it is no news to
    /// anyone else.
    pub(crate) fn states_follows(&self) -> bool {
        self.rule
            .as_ref()
            .is_some_and(|rule| rule.plan.rule == taktsim_model::DispatchRule::MinimumSetup)
    }

    /// An offer from `from` joins the queue.
    pub(crate) fn offered(&mut self, from: ComponentId) {
        self.waiting.push_back(from);
    }

    /// An offer from `from` that says what its unit is. Sent again, it keeps its
    /// place in the queue with the new description (ADR-0023 §6).
    pub(crate) fn offered_unit(&mut self, from: ComponentId, unit: OfferedUnit) {
        let Some(rule) = self.rule.as_deref_mut() else {
            self.waiting.push_back(from);
            return;
        };
        match rule.offers.iter_mut().find(|o| o.from == from) {
            Some(offer) => offer.unit = unit,
            None => rule.offers.push(DescribedOffer { from, unit }),
        }
    }

    /// The offer from `from` leaves the queue. No entry, no effect.
    pub(crate) fn withdrawn(&mut self, from: ComponentId) {
        if let Some(position) = self.waiting.iter().position(|&u| u == from) {
            self.waiting.remove(position);
        }
        if let Some(rule) = self.rule.as_deref_mut()
            && let Some(position) = rule.offers.iter().position(|o| o.from == from)
        {
            rule.offers.remove(position);
        }
    }

    /// Accepts the oldest waiting offers, as far as `free` places go.
    pub(crate) fn accept(
        &mut self,
        mut free: u64,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        while free > 0 {
            let Some(upstream) = self.waiting.pop_front() else {
                break;
            };
            self.reserved += 1;
            free -= 1;
            if let Some(reporting) = self.reporting.as_deref_mut() {
                reporting.accepted.push(upstream);
            }
            ctx.send(upstream, FlowMessage::Accept)?;
        }
        Ok(())
    }

    /// The station's `Pick` has come: the next settling picks.
    pub(crate) fn pick_came(&mut self) {
        if let Some(rule) = self.rule.as_deref_mut() {
            rule.pick_due = false;
        }
    }

    /// Settles a station's rule: it picks only when its `Pick` comes (`picking`),
    /// after every offer of the instant, so that the rule and not the order in which
    /// offers were delivered decides between units offered together (ADR-0023 §6).
    /// Otherwise a free place beside a waiting offer asks for a `Pick`, unless one
    /// is on its way or a `minimum_setup` pick waits for its reply.
    pub(crate) fn settle_by_rule(
        &mut self,
        picking: bool,
        free: u64,
        follows: Option<VariantId>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        if picking {
            return self.accept_by_rule(free, follows, ctx);
        }
        let asks = !self.waits_for_a_pick();
        if let Some(rule) = self.rule.as_deref_mut()
            && asks
            && free > 0
            && !rule.offers.is_empty()
        {
            rule.pick_due = true;
            ctx.schedule(SimDuration::ZERO, FlowMessage::Pick)?;
        }
        Ok(())
    }

    /// Accepts by the station's rule, as far as `free` places go: the rank is (key,
    /// since, place in the inlet), measured against what the next unit follows, which
    /// is `follows` until a place is reserved and what its offer described then
    /// (ADR-0023 §6).
    fn accept_by_rule(
        &mut self,
        mut free: u64,
        follows: Option<VariantId>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let Some(rule) = self.rule.as_deref_mut() else {
            return self.accept(free, ctx);
        };
        // What a unit follows is known for the next pick only: a station upstream ranks
        // by it when the `Accept` arrives, so `minimum_setup` takes one place at a time,
        // and the next when this one's reply has landed, at this instant (ADR-0023,
        // disagreement 17).
        let one_at_a_time = rule.plan.rule == taktsim_model::DispatchRule::MinimumSetup;
        if one_at_a_time && !rule.pending.is_empty() {
            return Ok(());
        }
        let mut follows = rule.pending.last().map_or(follows, |p| Some(p.variant));
        while free > 0 && !rule.offers.is_empty() {
            let mut ranked: Vec<(u64, SimTime, usize)> = rule
                .offers
                .iter()
                .enumerate()
                .map(|(place, offer)| {
                    let mut attrs = ItemAttrs::new(offer.unit.variant, offer.unit.priority);
                    attrs.due = offer.unit.due;
                    let key = rule.plan.key(attrs, offer.unit.since, follows);
                    (key, offer.unit.since, place)
                })
                .collect();
            ranked.sort_unstable();
            let total = ranked.len() as u64;
            let candidates: Vec<DispatchCandidate> = ranked
                .iter()
                .take(MAX_LISTED)
                .map(|&(key, since, place)| DispatchCandidate {
                    from: rule.offers[place].from,
                    item: rule.offers[place].unit.item,
                    key,
                    since,
                })
                .collect();
            let chosen = rule.offers.remove(ranked[0].2);
            rule.pending.push(Pending {
                from: chosen.from,
                variant: chosen.unit.variant,
                follows,
                candidates,
                total,
                after_decline: rule.declined,
            });
            self.reserved += 1;
            free -= 1;
            follows = Some(chosen.unit.variant);
            if let Some(reporting) = self.reporting.as_deref_mut() {
                reporting.accepted.push(chosen.from);
            }
            ctx.send(chosen.from, FlowMessage::Accept)?;
            if one_at_a_time {
                break;
            }
        }
        Ok(())
    }

    /// Whether a free place may wait beside a waiting offer: until the `Pick` on its
    /// way comes, or the reply to a `minimum_setup` pick lands, both at this instant.
    pub(crate) fn waits_for_a_pick(&self) -> bool {
        self.rule.as_deref().is_some_and(|rule| {
            rule.pick_due
                || (rule.plan.rule == taktsim_model::DispatchRule::MinimumSetup
                    && !rule.pending.is_empty())
        })
    }

    /// The variant of the newest reserved place, as its offer described it.
    pub(crate) fn newest_reserved(&self) -> Option<VariantId> {
        self.rule.as_deref()?.pending.last().map(|p| p.variant)
    }

    /// A part arrived from `from`, after its reservation ended. A rule's pick is
    /// confirmed, and goes on record with the entry of the chosen unit corrected to the
    /// unit that came (ADR-0023 §9).
    pub(crate) fn confirm_pick(
        &mut self,
        from: ComponentId,
        item: ItemId,
        attrs: ItemAttrs,
        ctx: &mut Context<'_, FlowMessage>,
    ) {
        let Some(rule) = self.rule.as_deref_mut() else {
            return;
        };
        let Some(position) = rule.pending.iter().position(|p| p.from == from) else {
            return;
        };
        let mut pending = rule.pending.remove(position);
        if pending.candidates.len() < 2 && !pending.after_decline {
            return;
        }
        if let Some(chosen) = pending.candidates.first_mut() {
            chosen.item = Some(item);
            chosen.key = rule.plan.key(attrs, chosen.since, pending.follows);
        }
        ctx.decide(&FlowMessage::DispatchDecision(Box::new(DispatchDecision {
            rule: rule.plan.rule,
            taker: ctx.me(),
            total: pending.total,
            candidates: pending.candidates,
        })));
    }

    /// A `Decline` from `from` released its place. A rule's pick is discarded, since
    /// it had no effect, and the next pick of this instant goes on record.
    pub(crate) fn declined(&mut self, from: ComponentId) -> Result<(), ComponentError> {
        self.release("Decline")?;
        if let Some(rule) = self.rule.as_deref_mut() {
            if let Some(position) = rule.pending.iter().position(|p| p.from == from) {
                rule.pending.remove(position);
            }
            rule.declined = true;
        }
        Ok(())
    }

    /// A reservation ended: a part arrived, or a `Decline` released it.
    pub(crate) fn release(&mut self, event: &'static str) -> Result<(), ComponentError> {
        self.reserved = self
            .reserved
            .checked_sub(1)
            .ok_or_else(|| ComponentError::Unexpected {
                state: "no place reserved",
                event: event.to_owned(),
            })?;
        Ok(())
    }

    /// Ends an event: sends each subscriber what changed for it (ADR-0023 §5). Its
    /// `queue` leaves out its own waiting offer, so neither an offer on its way nor a
    /// withdrawal on its way changes what it reads.
    pub(crate) fn end_event(
        &mut self,
        view: TargetView,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        // A `Decline` marks the pick it causes, which may come at a later event of
        // this instant.
        if let Some(rule) = self.rule.as_deref_mut()
            && !rule.pick_due
        {
            rule.declined = false;
        }
        let Some(reporting) = self.reporting.as_deref_mut() else {
            return Ok(());
        };
        let waiting =
            self.waiting.len() as u64 + self.rule.as_deref().map_or(0, |r| r.offers.len() as u64);
        for subscriber in &mut reporting.subscribers {
            let Some(id) = subscriber.id else {
                continue;
            };
            let own = u64::from(self.waiting.contains(&id))
                + self
                    .rule
                    .as_deref()
                    .map_or(0, |r| u64::from(r.offers.iter().any(|o| o.from == id)));
            let status = TargetStatus {
                room: view.room,
                accept_follows: reporting.accepted.contains(&id),
                queue: view.units + self.reserved + waiting - own,
                running: view.running,
                worked: view.worked,
                follows: view.follows,
            };
            let last = &subscriber.last;
            // Work alone is no news: the receiver works it out from `running`.
            if status.room != last.room
                || status.queue != last.queue
                || status.running != last.running
                || status.follows != last.follows
                || status.accept_follows
            {
                subscriber.last = status;
                ctx.send(id, FlowMessage::Status(Box::new(status)))?;
            }
        }
        reporting.accepted.clear();
        Ok(())
    }

    /// Places reserved.
    pub(crate) const fn reserved(&self) -> u64 {
        self.reserved
    }

    /// Whether an offer is waiting.
    pub(crate) fn has_waiting(&self) -> bool {
        !self.waiting.is_empty() || self.rule.as_deref().is_some_and(|r| !r.offers.is_empty())
    }
}

#[cfg(test)]
impl Outlet {
    /// An outlet with one target and no offer to it.
    pub(crate) fn with_unoffered_target() -> Self {
        Self {
            targets: vec![Target {
                id: ComponentId::from_index(0),
                offered: false,
                crossing_at: None,
            }],
            table: None,
            routed: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use taktsim_model::DispatchRule;

    use super::*;

    fn unit(variant: u16) -> OfferedUnit {
        OfferedUnit {
            item: None,
            variant: VariantId::new(variant),
            priority: 0,
            due: None,
            since: SimTime::START,
        }
    }

    #[test]
    fn a_resent_offer_keeps_its_place_and_a_withdrawn_one_loses_it() {
        let plan = TakerPlan {
            rule: DispatchRule::Fifo,
            default_time: 0,
            times: BTreeMap::new(),
            per_variant: None,
            changes: BTreeMap::new(),
        };
        let mut inlet = Inlet::new(Some(Arc::new(plan)), &[]);
        let (a, b) = (ComponentId::from_index(0), ComponentId::from_index(1));
        inlet.offered_unit(a, unit(0));
        inlet.offered_unit(b, unit(1));
        inlet.offered_unit(a, unit(2));
        let order = |inlet: &Inlet| -> Vec<(ComponentId, u16)> {
            let rule = inlet.rule.as_deref().unwrap();
            rule.offers
                .iter()
                .map(|o| (o.from, o.unit.variant.get()))
                .collect()
        };
        assert_eq!(order(&inlet), [(a, 2), (b, 1)]);
        inlet.withdrawn(a);
        inlet.offered_unit(a, unit(3));
        assert_eq!(order(&inlet), [(b, 1), (a, 3)]);
        assert!(inlet.has_waiting());
    }

    #[test]
    fn worked_per_place_is_exact_and_rounds_down() {
        let known = Known {
            running: 3,
            worked: 10,
            at: SimTime::from_nanos(5),
            ..Known::START
        };
        // 10 + 3 × 2 over 3 places.
        assert_eq!(known.worked_per_place(3, SimTime::from_nanos(7)), 5);
        // No places is one, so nothing divides by zero.
        assert_eq!(known.worked_per_place(0, SimTime::from_nanos(5)), 10);
    }
}
