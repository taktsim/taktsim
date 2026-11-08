//! The KPI fold (ADR-0025 §1): one pass over neutral facts about items and places,
//! in integer nanoseconds, so a hand-checked number is the exact number.
//!
//! The fold has no factory word and no engine in it. A bridge in a higher crate turns
//! records into [`Fact`]s, and the numbers a report gives are for that crate to name.
//! It learns its end last: a stored trace states its end in its last record, and a
//! live sink does not know it until the run stops.
//!
//! Levels are integrated as integer item-nanoseconds in a `u128`, which cannot
//! overflow, since a `u64` level times a `u64` span fits. Only reading a mean turns
//! them into `f64`, with IEEE operations alone (ADR-0014).

use std::collections::BTreeMap;

use taktsim_units::{SimDuration, SimTime};

use crate::StatsError;

/// One thing that happened, in plain ids. Records of a run become these in order.
///
/// ```
/// use taktsim_stats::{Creation, Fact, KpiFold, PlaceSpec};
/// use taktsim_units::SimTime;
///
/// let places = [PlaceSpec { states: 0, creation: Creation::HeldUntilFirstMove }];
/// let mut fold = KpiFold::new(&places, SimTime::START);
/// fold.push(Fact::Created { time: "1s".parse()?, item: 7, by: 0 })?;
/// assert!(matches!(fold.push(Fact::Created { time: "2s".parse()?, item: 7, by: 0 }), Err(_)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    /// `by` created `item`, at its place.
    Created {
        /// When.
        time: SimTime,
        /// The item.
        item: u64,
        /// The place that created it.
        by: u32,
    },
    /// `item` moved from one place to another.
    Moved {
        /// When.
        time: SimTime,
        /// The item.
        item: u64,
        /// Where it was.
        from: u32,
        /// Where it is now.
        to: u32,
    },
    /// `by` delivered `item`: it left the model as a good unit.
    Delivered {
        /// When.
        time: SimTime,
        /// The item.
        item: u64,
        /// The place that delivered it.
        by: u32,
    },
    /// `by` removed `item` from the model in another way, such as scrap.
    Removed {
        /// When.
        time: SimTime,
        /// The item.
        item: u64,
        /// The place that removed it.
        by: u32,
    },
    /// `resource` is in `state` from now on, with this many units in progress.
    State {
        /// When.
        time: SimTime,
        /// The resource.
        resource: u32,
        /// A position in what its type declares.
        state: u16,
        /// Units in progress.
        in_progress: u32,
    },
    /// `item` is of `class` in `dimension`. The last one stated counts.
    Class {
        /// The item.
        item: u64,
        /// The dimension, such as a variant or a material.
        dimension: u16,
        /// The class.
        class: u16,
    },
}

/// When an item a place creates counts as work in progress (ADR-0025 §1).
///
/// ```
/// use taktsim_stats::Creation;
///
/// assert_ne!(Creation::Never, Creation::InSystem);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Creation {
    /// The place never creates an item; creating one breaks conservation.
    Never,
    /// The item waits at its creator and is released at its first move.
    HeldUntilFirstMove,
    /// The item is released as it is created.
    InSystem,
}

/// What the fold knows about one place.
///
/// ```
/// use taktsim_stats::{Creation, PlaceSpec};
///
/// let machine = PlaceSpec { states: 6, creation: Creation::Never };
/// assert_eq!(machine.states, 6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceSpec {
    /// How many states it declares. With none it writes no [`Fact::State`].
    pub states: u16,
    /// What creating an item here means.
    pub creation: Creation,
}

/// A piecewise-constant whole number and its exact integral.
#[derive(Debug, Clone, Copy)]
struct Level {
    value: u64,
    since: SimTime,
    integral: u128,
}

impl Level {
    const fn new(start: SimTime) -> Self {
        Self {
            value: 0,
            since: start,
            integral: 0,
        }
    }

    /// The integral up to `to`, not counting any time before the window's `since`.
    fn advance(&mut self, to: SimTime) {
        if to > self.since {
            let span = to.as_nanos() - self.since.as_nanos();
            self.integral += u128::from(self.value) * u128::from(span);
            self.since = to;
        }
    }

    fn set(&mut self, to: SimTime, value: u64) {
        self.advance(to);
        self.value = value;
    }

    fn up(&mut self, to: SimTime) {
        let value = self.value + 1;
        self.set(to, value);
    }

    /// One down; the fold's item table proves the level is above zero.
    fn down(&mut self, to: SimTime) {
        let value = self.value.saturating_sub(1);
        self.set(to, value);
    }
}

/// What the fold keeps per place.
#[derive(Debug, Clone)]
struct PlaceFold {
    spec: PlaceSpec,
    level: Level,
    held: Level,
    in_progress: Level,
    /// The current state, and the time from which it is counted.
    state: Option<(u16, SimTime)>,
    state_nanos: Vec<u64>,
    /// When the state known at the start of the window first applied.
    first_state: Option<SimTime>,
    delivered: u64,
    removed: u64,
    created: u64,
    by_class: BTreeMap<(u16, u16), u64>,
    lead_times: Vec<u64>,
}

/// What the fold keeps per item alive.
#[derive(Debug, Clone)]
struct ItemFold {
    at: u32,
    released: Option<SimTime>,
    /// By dimension, one entry per dimension.
    classes: Vec<(u16, u16)>,
}

/// The KPI fold: push facts in order, and say where the window ended last.
///
/// `start` is the start of the window. Items and states before it are known, and no
/// time before it is counted, so a warm-up is a later `start` over the same facts.
///
/// ```
/// use taktsim_stats::{Creation, Fact, KpiFold, PlaceSpec};
/// use taktsim_units::SimTime;
///
/// // A source that holds a part, and a sink.
/// let places = [
///     PlaceSpec { states: 0, creation: Creation::HeldUntilFirstMove },
///     PlaceSpec { states: 0, creation: Creation::Never },
/// ];
/// let mut fold = KpiFold::new(&places, SimTime::START);
/// fold.push(Fact::Created { time: "0s".parse()?, item: 1, by: 0 })?;
/// fold.push(Fact::Moved { time: "2s".parse()?, item: 1, from: 0, to: 1 })?;
/// fold.push(Fact::Delivered { time: "5s".parse()?, item: 1, by: 1 })?;
/// let report = fold.finish("10s".parse()?)?;
///
/// // 2 s held, then 3 s in the system, over 10 s.
/// assert_eq!(report.place(0).and_then(|p| p.mean_held(&report)), Some(0.2));
/// assert_eq!(report.mean_wip(), Some(0.3));
/// assert_eq!(report.place(1).map(|p| p.delivered()), Some(1));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct KpiFold {
    start: SimTime,
    now: SimTime,
    places: Vec<PlaceFold>,
    wip: Level,
    items: BTreeMap<u64, ItemFold>,
    peak_alive: u64,
}

impl KpiFold {
    /// A fold over these places, with its window starting at `start`.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// let report = KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?;
    /// assert_eq!(report.mean_wip(), Some(0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(places: &[PlaceSpec], start: SimTime) -> Self {
        Self {
            start,
            now: SimTime::START,
            places: places
                .iter()
                .map(|spec| PlaceFold {
                    spec: *spec,
                    level: Level::new(start),
                    held: Level::new(start),
                    in_progress: Level::new(start),
                    state: None,
                    state_nanos: vec![0; usize::from(spec.states)],
                    first_state: None,
                    delivered: 0,
                    removed: 0,
                    created: 0,
                    by_class: BTreeMap::new(),
                    lead_times: Vec::new(),
                })
                .collect(),
            wip: Level::new(start),
            items: BTreeMap::new(),
            peak_alive: 0,
        }
    }

    /// Adds the next fact.
    ///
    /// # Errors
    ///
    /// A [`StatsError`] if the fact breaks conservation or the order of time:
    /// [`CreatedAtNoCreator`](StatsError::CreatedAtNoCreator),
    /// [`ItemAlreadyAlive`](StatsError::ItemAlreadyAlive),
    /// [`ItemNotAlive`](StatsError::ItemNotAlive),
    /// [`ItemElsewhere`](StatsError::ItemElsewhere),
    /// [`UnknownPlace`](StatsError::UnknownPlace),
    /// [`UnknownState`](StatsError::UnknownState) or
    /// [`TimesOutOfOrder`](StatsError::TimesOutOfOrder). The fold cannot be used after
    /// one, since it no longer describes a run.
    ///
    /// ```
    /// use taktsim_stats::{Creation, Fact, KpiFold, PlaceSpec, StatsError};
    /// use taktsim_units::SimTime;
    ///
    /// let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// let mut fold = KpiFold::new(&places, SimTime::START);
    /// let refused = fold.push(Fact::Created { time: SimTime::START, item: 1, by: 0 });
    /// assert!(matches!(refused, Err(StatsError::CreatedAtNoCreator { item: 1, by: 0, .. })));
    /// ```
    pub fn push(&mut self, fact: Fact) -> Result<(), StatsError> {
        match fact {
            Fact::Created { time, item, by } => self.created(time, item, by),
            Fact::Moved {
                time,
                item,
                from,
                to,
            } => self.moved(time, item, from, to),
            Fact::Delivered { time, item, by } => self.disposed(time, item, by, true),
            Fact::Removed { time, item, by } => self.disposed(time, item, by, false),
            Fact::State {
                time,
                resource,
                state,
                in_progress,
            } => self.state(time, resource, state, in_progress),
            Fact::Class {
                item,
                dimension,
                class,
            } => self.class(item, dimension, class),
        }
    }

    /// The window has ended at `end`: the report over `[start, end]`.
    ///
    /// # Errors
    ///
    /// [`StatsError::EmptyWindow`] if `end` is before the start,
    /// [`StatsError::TimesOutOfOrder`] if it is before the last fact, and
    /// [`StatsError::NoValueAtStart`] if the window has time in it and a place that
    /// declares states stated none by its start.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// // A window with no time in it has no time-weighted value.
    /// let report = KpiFold::new(&[], SimTime::START).finish(SimTime::START)?;
    /// assert_eq!(report.mean_wip(), None);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn finish(mut self, end: SimTime) -> Result<KpiReport, StatsError> {
        if end < self.start {
            return Err(StatsError::EmptyWindow {
                start: self.start,
                end,
            });
        }
        if end < self.now {
            return Err(StatsError::TimesOutOfOrder {
                what: "the end of the KPI window",
                index: 0,
                previous: self.now,
                found: end,
            });
        }
        let has_time = end > self.start;
        for place in &mut self.places {
            if has_time && place.spec.states > 0 {
                match place.first_state {
                    Some(first) if first <= self.start => {}
                    first => {
                        return Err(StatsError::NoValueAtStart {
                            start: self.start,
                            first,
                        });
                    }
                }
            }
            place.level.advance(end);
            place.held.advance(end);
            place.in_progress.advance(end);
            if let Some((state, since)) = place.state {
                add_state_nanos(&mut place.state_nanos, state, since, end);
            }
        }
        self.wip.advance(end);
        let start = self.start;
        let span = end.as_nanos() - start.as_nanos();
        let items_alive = u64::try_from(self.items.len()).unwrap_or(u64::MAX);
        Ok(KpiReport {
            start,
            end,
            span,
            wip_integral: self.wip.integral,
            wip_end: self.wip.value,
            items_alive,
            peak_alive: self.peak_alive,
            places: self
                .places
                .into_iter()
                .map(|place| PlaceKpis {
                    level_integral: place.level.integral,
                    level_end: place.level.value,
                    held_integral: place.held.integral,
                    held_end: place.held.value,
                    in_progress_integral: place.in_progress.integral,
                    state_nanos: place.state_nanos,
                    delivered: place.delivered,
                    removed: place.removed,
                    created: place.created,
                    by_class: place.by_class,
                    lead_times: LeadTimes::new(place.lead_times),
                })
                .collect(),
        })
    }

    /// Time must not go back.
    fn clock(&mut self, time: SimTime) -> Result<(), StatsError> {
        if time < self.now {
            return Err(StatsError::TimesOutOfOrder {
                what: "KPI facts",
                index: 0,
                previous: self.now,
                found: time,
            });
        }
        self.now = time;
        Ok(())
    }

    fn place_mut(&mut self, place: u32) -> Result<&mut PlaceFold, StatsError> {
        let found = self.places.len();
        self.places
            .get_mut(place as usize)
            .ok_or(StatsError::UnknownPlace {
                place,
                places: found,
            })
    }

    fn created(&mut self, time: SimTime, item: u64, by: u32) -> Result<(), StatsError> {
        self.clock(time)?;
        let in_window = time >= self.start;
        let place = self.place_mut(by)?;
        let released = match place.spec.creation {
            Creation::Never => {
                return Err(StatsError::CreatedAtNoCreator { item, by, time });
            }
            Creation::HeldUntilFirstMove => {
                place.held.up(time);
                None
            }
            Creation::InSystem => Some(time),
        };
        place.level.up(time);
        if in_window {
            place.created += 1;
        }
        if released.is_some() {
            self.wip.up(time);
        }
        let new = ItemFold {
            at: by,
            released,
            classes: Vec::new(),
        };
        if self.items.insert(item, new).is_some() {
            return Err(StatsError::ItemAlreadyAlive { item, by, time });
        }
        let alive = u64::try_from(self.items.len()).unwrap_or(u64::MAX);
        self.peak_alive = self.peak_alive.max(alive);
        Ok(())
    }

    fn moved(&mut self, time: SimTime, item: u64, from: u32, to: u32) -> Result<(), StatsError> {
        self.clock(time)?;
        let Some(state) = self.items.get_mut(&item) else {
            return Err(StatsError::ItemNotAlive {
                item,
                at: from,
                time,
            });
        };
        if state.at != from {
            return Err(StatsError::ItemElsewhere {
                item,
                stated: from,
                found: state.at,
                time,
            });
        }
        state.at = to;
        let first_move = state.released.is_none();
        if first_move {
            state.released = Some(time);
        }
        let place = self.place_mut(from)?;
        place.level.down(time);
        if first_move {
            place.held.down(time);
        }
        self.place_mut(to)?.level.up(time);
        if first_move {
            self.wip.up(time);
        }
        Ok(())
    }

    fn disposed(
        &mut self,
        time: SimTime,
        item: u64,
        by: u32,
        delivered: bool,
    ) -> Result<(), StatsError> {
        self.clock(time)?;
        let in_window = time >= self.start;
        let Some(state) = self.items.remove(&item) else {
            return Err(StatsError::ItemNotAlive { item, at: by, time });
        };
        if state.at != by {
            return Err(StatsError::ItemElsewhere {
                item,
                stated: by,
                found: state.at,
                time,
            });
        }
        let place = self.place_mut(by)?;
        place.level.down(time);
        match state.released {
            Some(released) => {
                if delivered && in_window {
                    place
                        .lead_times
                        .push(time.as_nanos().saturating_sub(released.as_nanos()));
                }
            }
            None => place.held.down(time),
        }
        if in_window {
            if delivered {
                place.delivered += 1;
                for entry in &state.classes {
                    *place.by_class.entry(*entry).or_insert(0) += 1;
                }
            } else {
                place.removed += 1;
            }
        }
        if state.released.is_some() {
            self.wip.down(time);
        }
        Ok(())
    }

    fn state(
        &mut self,
        time: SimTime,
        resource: u32,
        state: u16,
        in_progress: u32,
    ) -> Result<(), StatsError> {
        self.clock(time)?;
        let start = self.start;
        let place = self.place_mut(resource)?;
        if state >= place.spec.states {
            return Err(StatsError::UnknownState {
                place: resource,
                state,
                states: place.spec.states,
            });
        }
        if let Some((previous, since)) = place.state {
            add_state_nanos(&mut place.state_nanos, previous, since, time);
        }
        let since = time.max(start);
        place.state = Some((state, since));
        place.first_state.get_or_insert(since);
        place.in_progress.set(time, u64::from(in_progress));
        Ok(())
    }

    fn class(&mut self, item: u64, dimension: u16, class: u16) -> Result<(), StatsError> {
        let Some(state) = self.items.get_mut(&item) else {
            return Err(StatsError::ItemNotAlive {
                item,
                at: u32::MAX,
                time: self.now,
            });
        };
        match state.classes.iter_mut().find(|(d, _)| *d == dimension) {
            Some(entry) => entry.1 = class,
            None => state.classes.push((dimension, class)),
        }
        Ok(())
    }
}

/// Adds the span from `since` to `to` to a state, if it is later.
fn add_state_nanos(nanos: &mut [u64], state: u16, since: SimTime, to: SimTime) {
    if to > since
        && let Some(total) = nanos.get_mut(usize::from(state))
    {
        *total += to.as_nanos() - since.as_nanos();
    }
}

/// The number as `f64`. Rounds above 2^53: a mean loses no more than an `f64` mean
/// would, and IEEE rounding gives the same bits on every target.
#[allow(clippy::cast_precision_loss)] // The rounding is the documented conversion.
const fn to_f64(n: u128) -> f64 {
    n as f64
}

/// What the fold found over its window.
///
/// ```
/// use taktsim_stats::KpiFold;
/// use taktsim_units::SimTime;
///
/// let report = KpiFold::new(&[], SimTime::START).finish("1h".parse()?)?;
/// assert_eq!(report.end(), "1h".parse::<SimTime>()?);
/// assert_eq!(report.items_alive(), 0);
/// assert_eq!(report.place(0), None);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiReport {
    start: SimTime,
    end: SimTime,
    span: u64,
    wip_integral: u128,
    wip_end: u64,
    items_alive: u64,
    peak_alive: u64,
    places: Vec<PlaceKpis>,
}

impl KpiReport {
    /// The start of the window.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.start(), SimTime::START);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn start(&self) -> SimTime {
        self.start
    }

    /// The end of the window.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.end().to_string(), "1s");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn end(&self) -> SimTime {
        self.end
    }

    /// The window's length.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.span().to_string(), "1s");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn span(&self) -> SimDuration {
        SimDuration::from_nanos(self.span)
    }

    /// The share of `integral` per unit of window: `None` for a window with no time
    /// in it.
    fn mean(&self, integral: u128) -> Option<f64> {
        (self.span > 0).then(|| to_f64(integral) / to_f64(u128::from(self.span)))
    }

    /// The time-weighted mean of items released and not yet disposed of. `None` for a
    /// window with no time in it.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.mean_wip(), Some(0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn mean_wip(&self) -> Option<f64> {
        self.mean(self.wip_integral)
    }

    /// The exact integral of system WIP, in item-nanoseconds.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.wip_integral(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn wip_integral(&self) -> u128 {
        self.wip_integral
    }

    /// Items released and not yet disposed of at the end: the units a lead time has
    /// not seen.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.wip_end(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn wip_end(&self) -> u64 {
        self.wip_end
    }

    /// Items alive at the end, held ones included.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.items_alive(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn items_alive(&self) -> u64 {
        self.items_alive
    }

    /// The most items alive at once.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.peak_alive(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn peak_alive(&self) -> u64 {
        self.peak_alive
    }

    /// What the fold found for the place at `index`.
    ///
    /// ```
    /// use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// use taktsim_units::SimTime;
    ///
    /// let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// assert!(report.place(0).is_some());
    /// assert!(report.place(1).is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn place(&self, index: usize) -> Option<&PlaceKpis> {
        self.places.get(index)
    }

    /// Every place, in the order given.
    ///
    /// ```
    /// use taktsim_stats::KpiFold;
    /// use taktsim_units::SimTime;
    ///
    /// assert!(KpiFold::new(&[], SimTime::START).finish("1s".parse()?)?.places().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn places(&self) -> &[PlaceKpis] {
        &self.places
    }
}

/// What the fold found for one place.
///
/// ```
/// use taktsim_stats::{Creation, KpiFold, PlaceSpec};
/// use taktsim_units::SimTime;
///
/// let places = [PlaceSpec { states: 2, creation: Creation::Never }];
/// let mut fold = KpiFold::new(&places, SimTime::START);
/// fold.push(taktsim_stats::Fact::State { time: SimTime::START, resource: 0, state: 1, in_progress: 2 })?;
/// let report = fold.finish("10s".parse()?)?;
/// let place = report.place(0).ok_or("the place")?;
/// assert_eq!(place.state_nanos(), [0, 10_000_000_000]);
/// assert_eq!(place.mean_in_progress(&report), Some(2.0));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceKpis {
    level_integral: u128,
    level_end: u64,
    held_integral: u128,
    held_end: u64,
    in_progress_integral: u128,
    state_nanos: Vec<u64>,
    delivered: u64,
    removed: u64,
    created: u64,
    by_class: BTreeMap<(u16, u16), u64>,
    lead_times: LeadTimes,
}

impl PlaceKpis {
    /// The time-weighted mean of the items here. `None` for a window with no time.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.mean_level(&report), Some(0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn mean_level(&self, report: &KpiReport) -> Option<f64> {
        report.mean(self.level_integral)
    }

    /// The exact integral of the items here, in item-nanoseconds.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.level_integral(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn level_integral(&self) -> u128 {
        self.level_integral
    }

    /// The items here at the end.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.level_end(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn level_end(&self) -> u64 {
        self.level_end
    }

    /// The time-weighted mean of items created here and not yet released. `None` for
    /// a window with no time.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.mean_held(&report), Some(0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn mean_held(&self, report: &KpiReport) -> Option<f64> {
        report.mean(self.held_integral)
    }

    /// The exact integral of items held here, in item-nanoseconds.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.held_integral(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn held_integral(&self) -> u128 {
        self.held_integral
    }

    /// The items held here at the end.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.held_end(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn held_end(&self) -> u64 {
        self.held_end
    }

    /// The time-weighted mean of units in progress. `None` for a window with no time.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.mean_in_progress(&report), Some(0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn mean_in_progress(&self, report: &KpiReport) -> Option<f64> {
        report.mean(self.in_progress_integral)
    }

    /// The nanoseconds of the window in each state, by position. They sum to the
    /// window for a place that declares states.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert!(place.state_nanos().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn state_nanos(&self) -> &[u64] {
        &self.state_nanos
    }

    /// The share of the window in state `state`. `None` for a window with no time, or
    /// a state that is not declared.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, Fact, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 1, creation: Creation::Never }];
    /// # let mut fold = KpiFold::new(&places, SimTime::START);
    /// # fold.push(Fact::State { time: SimTime::START, resource: 0, state: 0, in_progress: 0 })?;
    /// # let report = fold.finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.state_share(&report, 0), Some(1.0));
    /// assert_eq!(place.state_share(&report, 1), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn state_share(&self, report: &KpiReport, state: u16) -> Option<f64> {
        let nanos = *self.state_nanos.get(usize::from(state))?;
        report.mean(u128::from(nanos))
    }

    /// Items it delivered in the window.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.delivered(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn delivered(&self) -> u64 {
        self.delivered
    }

    /// Items it removed in another way in the window.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.removed(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn removed(&self) -> u64 {
        self.removed
    }

    /// Items it created in the window.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert_eq!(place.created(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn created(&self) -> u64 {
        self.created
    }

    /// Items it delivered in the window, by `(dimension, class)`.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert!(place.delivered_by_class().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn delivered_by_class(&self) -> &BTreeMap<(u16, u16), u64> {
        &self.by_class
    }

    /// The lead times of what it delivered in the window, release to delivery.
    ///
    /// ```
    /// # use taktsim_stats::{Creation, KpiFold, PlaceSpec};
    /// # use taktsim_units::SimTime;
    /// # let places = [PlaceSpec { states: 0, creation: Creation::Never }];
    /// # let report = KpiFold::new(&places, SimTime::START).finish("1s".parse()?)?;
    /// # let place = report.place(0).ok_or("the place")?;
    /// assert!(place.lead_times().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn lead_times(&self) -> &LeadTimes {
        &self.lead_times
    }
}

/// Lead times of delivered units, kept exactly as integer nanoseconds (ADR-0025 §3).
/// A quantile is a value that occurred: nearest rank, `sorted[⌈p·n⌉ − 1]`, in integer
/// arithmetic.
///
/// ```
/// use taktsim_stats::LeadTimes;
///
/// let times = LeadTimes::new(vec![30, 10, 20, 40]);
/// assert_eq!(times.len(), 4);
/// assert_eq!(times.percentile(50).map(|d| d.as_nanos()), Some(20));
/// assert_eq!(times.percentile(51).map(|d| d.as_nanos()), Some(30));
/// assert_eq!(times.percentile(100).map(|d| d.as_nanos()), Some(40));
/// assert_eq!(times.sum_nanos(), 100);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadTimes {
    sorted: Vec<u64>,
    sum: u128,
}

impl LeadTimes {
    /// The lead times `nanos`, in any order.
    ///
    /// ```
    /// assert!(taktsim_stats::LeadTimes::new(vec![]).is_empty());
    /// ```
    #[must_use]
    pub fn new(mut nanos: Vec<u64>) -> Self {
        nanos.sort_unstable();
        let sum = nanos.iter().map(|n| u128::from(*n)).sum();
        Self { sorted: nanos, sum }
    }

    /// How many.
    ///
    /// ```
    /// assert_eq!(taktsim_stats::LeadTimes::new(vec![1, 2]).len(), 2);
    /// ```
    #[must_use]
    pub const fn len(&self) -> usize {
        self.sorted.len()
    }

    /// Whether there are none: no delivered unit has a lead time.
    ///
    /// ```
    /// assert!(!taktsim_stats::LeadTimes::new(vec![1]).is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }

    /// Their sum, exact.
    ///
    /// ```
    /// assert_eq!(taktsim_stats::LeadTimes::new(vec![1, 2]).sum_nanos(), 3);
    /// ```
    #[must_use]
    pub const fn sum_nanos(&self) -> u128 {
        self.sum
    }

    /// The mean in seconds, as the first of ADR-0014's bare numbers. `None` if empty.
    ///
    /// ```
    /// assert_eq!(taktsim_stats::LeadTimes::new(vec![1_000_000_000, 3_000_000_000]).mean_secs(), Some(2.0));
    /// assert_eq!(taktsim_stats::LeadTimes::new(vec![]).mean_secs(), None);
    /// ```
    #[must_use]
    pub fn mean_secs(&self) -> Option<f64> {
        let n = u128::try_from(self.sorted.len()).unwrap_or(u128::MAX);
        (n > 0).then(|| to_f64(self.sum) / to_f64(n) / 1e9)
    }

    /// The shortest.
    ///
    /// ```
    /// assert_eq!(taktsim_stats::LeadTimes::new(vec![5, 3]).min().map(|d| d.as_nanos()), Some(3));
    /// ```
    #[must_use]
    pub fn min(&self) -> Option<SimDuration> {
        self.sorted.first().map(|n| SimDuration::from_nanos(*n))
    }

    /// The longest.
    ///
    /// ```
    /// assert_eq!(taktsim_stats::LeadTimes::new(vec![5, 3]).max().map(|d| d.as_nanos()), Some(5));
    /// ```
    #[must_use]
    pub fn max(&self) -> Option<SimDuration> {
        self.sorted.last().map(|n| SimDuration::from_nanos(*n))
    }

    /// The nearest-rank quantile at `percent` of 100: `None` if empty, or if `percent`
    /// is 0 or above 100.
    ///
    /// ```
    /// let times = taktsim_stats::LeadTimes::new((1..=100).collect());
    /// assert_eq!(times.percentile(95).map(|d| d.as_nanos()), Some(95));
    /// assert_eq!(times.percentile(0), None);
    /// assert_eq!(times.percentile(101), None);
    /// ```
    #[must_use]
    pub fn percentile(&self, percent: u8) -> Option<SimDuration> {
        if percent == 0 || percent > 100 {
            return None;
        }
        let rank = (usize::from(percent) * self.sorted.len()).div_ceil(100);
        let index = rank.checked_sub(1)?;
        self.sorted.get(index).map(|n| SimDuration::from_nanos(*n))
    }
}
