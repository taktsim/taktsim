# ADR-0022: Components: Source, Machine, Buffer, Sink

- Status: proposed (needs the owner's explicit acceptance)
- Date: 2026-11-08
- Blocks: M5; used by M6 (routing), M7 (`taktsim run`), M8 (KPIs from
  traces), M10 (analytical suite), M12 (Python), and every later component
  package, which copies its shape
- Spec: §2 principles 2, 3, 4 and 7, §4 (component table, rules,
  extensibility, routing), §5, §7 Layers 1 to 3, §9, §10; ADR-0001, ADR-0003,
  ADR-0013, ADR-0018, ADR-0020, ADR-0021; `docs/concepts` pages 5, 6, 8, 11

## Why explicit acceptance

Most of this ADR settles details M5 needs and would be accepted by merging,
including the replacement of the spec's default registry, `inventory` (§11).
Three points change the spec or an accepted ADR, so the whole needs the
owner's answer:

1. The spec's invariant "transitions follow idle → busy → blocked / down"
   cannot hold literally with calendar-time failures (§5 below). This ADR
   replaces it with a full transition table.
2. The trace gains a record kind, amending the record table of ADR-0021 and
   the record list of ADR-0018 (§9). ADR-0021 foresaw this, and it is a change
   to `taktsim-core`'s public API, which is why it is called out.
3. Two sentences are added to ADR-0020 (§12).

## Context

This is the first domain package. Later ones copy its vocabulary and its
shape, so its names, parameters, states and message protocol are expensive to
change after v0.1 ships.

### What exists today

`crates/taktsim-model/src/component.rs` holds the entry and the catalog
(`ComponentParams`, `Catalog`, reserved keys) and no vocabulary: ADR-0020 §6
keeps every component type out of the model crate. The only vocabulary so far
is the stand-in types in `crates/taktsim-model/tests/support/mod.rs`.
`schemas/model/1.0.json` admits no component and no observable.
`crates/taktsim-components` is empty.

| M5 list | Stand-in today | This ADR |
| --- | --- | --- |
| Source: arrival distribution | `interarrival` | same key |
| Source: batch size | `batch` | same key, above zero |
| Source: variant mix | none | new `variants` |
| Source: priority (for the buffer's `priority`, §7) | none | new `priority` |
| Machine: cycle time | `cycle_time` | same key, not a constant zero |
| Machine: MTBF, MTTR | `mtbf`, `mttr` | same keys, both or neither, not a constant zero |
| Machine: setup matrix by variant | `setup: {per_variant}`, a duration | `per_variant` becomes a distribution; new `matrix` |
| Machine: capacity | `capacity` | same key, above zero, default `1 parts` |
| Buffer: capacity | `capacity` | same key, required, above zero |
| Buffer: discipline | none | new `discipline` |
| Sink | no parameters | same |

Spec §4's table also lists Source `material` and Sink "accepted materials".
Materials arrive in v0.2 with their section; both keys can be added then
without breaking a model. M5 does not read them.

## The vocabulary

Every key is optional unless marked. Absent means the default stated here.

```yaml
- id: source
  type: Source
  interarrival: { dist: exponential, mean: 40s }   # required; time between batches
  batch: 2 parts                                   # default 1 parts
  variants: { basic: 80%, premium: 20% }           # default: one variant, `default`
  priority: 1                                      # 0 to 255; default 0; higher goes first

- id: cut
  type: Machine
  cycle_time: { dist: lognormal, mean: 42s, sigma: 0.08 }   # required
  mtbf: 6h                                                  # with mttr; absent: never fails
  mttr: { dist: exponential, mean: 12min }                  # with mtbf
  setup:                                                    # absent: variant changes cost nothing
    per_variant: 4min                                       # any change not in the matrix
    matrix:                                                 # at least one of the two keys
      - { from: basic, to: premium, time: 6min }
  capacity: 2 parts                                         # default 1 parts

- id: buf1
  type: Buffer
  capacity: 20 parts                                        # required
  discipline: fifo                                          # fifo, lifo or priority; default fifo

- id: sink
  type: Sink
```

Rules checked at load time, each with a field path and an example:

- `batch`, `capacity` above zero. A buffer of zero places is a direct edge.
- `interarrival`, `cycle_time`, `mtbf`, `mttr` and setup times are not a
  constant `0s`: a repeating zero-length timer never lets time advance, and a
  constant zero cycle is not a machine. Relaxable later.
- `mtbf` and `mttr` come together.
- `variants`: at least one entry, names of the component-id form (ADR-0015),
  each share above 0 %, shares summing to 100 % within 10⁻⁹ percentage points
  (`Ratio` holds a percentage as an `f64`), at most 65,535 names in the model.
- `setup.matrix`: `from` differs from `to`, no pair twice, both names declared
  by some Source (or `default`, if some Source declares no `variants`).
- `setup: {}` is an error.

`mtbf` means the up time from the end of a repair (or time zero) to the next
failure, as page 8 defines it, not failure-to-failure. The schema description
says so, because a reliability engineer may read it the other way.

Names follow the spec's examples, the stand-ins and the existing lowercase
words (`terminating`, `shortest_queue`). Rejected alternatives: `arrival` for
`interarrival` (the spec's example already says `interarrival`); `FIFO` in
capitals (every other enumerated word in the format is lowercase).

### Observables

`observe` and `experiment.precision` need declared functions, or spec §4's
own example does not load. M5 registers the four names of spec §4's example
and the concept pages, with what they accept: `throughput(Sink)`,
`lead_time(Sink)`, `wip(Buffer)`, `state_shares(Machine, …)`. ADR-0015 answer 5
leaves their result types to M8, which may refine them without changing a
model or a hash. M5 computes nothing.

### What stays out of the factory vocabulary

A component parameter says what the system is. It never says how a run is
computed or what is asked of it: no seed, replication count, warm-up, check
switch, trace option or solver setting on a component. This follows ADR-0020
§8's rule for the system hash. `group:` (spec §14) and the reserved entry keys
(`description`, `x-*`) are not parameters either. Factory words (`variant`,
`priority`, `due`) live in this package's messages, never in core, the record
format or the trace header.

## Decisions

### 1. Model hash and normalized form

No pinned vector changes. Real types encode exactly as the stand-ins did for
every key spec §4's example uses, so `spec-line-a`'s canonical form, model
hash `56d63bb3…` and system hash `18c3cbaa…` stay. New optional keys are left
out when absent (ADR-0020 §6), so they change no existing hash.

New normalizations: `variants` is a mapping, so its keys sort; `matrix` is
sorted by `(from, to)`; a constant `per_variant` stays a bare duration.

| | N1. Written defaults stay written | N2. Written defaults are removed |
| --- | --- | --- |
| `capacity: 1 parts` against nothing | two hashes | one hash |
| Normalization depends on component defaults | no | yes |
| A default changed in a later format | visible: stated models keep their meaning and hash | stated and assumed models alias |

**N1.** The two files say different things, one of which leaves the value to
the format. A test pins this.

### 2. From document to run

**Dependency direction.** `taktsim-core` stays free of the model and of I/O.

| | D1. components → model, trace, core | D2. model → components | D3. behaviour crate plus a binding crate |
| --- | --- | --- | --- |
| Fits ADR-0020 ("M5 implements `ComponentParams`", the model crate knows no type) | yes | no | yes |
| Fits ADR-0021 (packages depend on trace for `TraceMessage`) | yes | yes | yes |
| New external crates | none | none | none |
| Crates per domain package | one | one | two |
| Use without the YAML parser | no | no | yes, but the browser engine needs the loader anyway |

**D1.** The graph becomes units ← core ← model, trace ← components ← cli,
python. M5 adds no external crate, so `Cargo.lock` gains nothing outside the
workspace and `deny.toml` does not change.

**The API**, in `taktsim-components`, used by M7 and M12:

```rust
pub trait ComponentType: ComponentParams {
    fn build(&self, wiring: &Wiring<'_>)
        -> Result<Box<dyn Component<FlowMessage>>, BuildError>;
}

pub struct Registry;   // catalog + one builder per type, filled by one call
impl Registry {
    pub fn standard() -> Result<Self, CatalogError>;
    pub fn with<T: ComponentType>(self) -> Result<Self, CatalogError>;
    pub fn catalog(&self) -> &Catalog;
    pub fn build(&self, document: &Document, config: EngineConfig)
        -> Result<Engine<FlowMessage>, BuildError>;
    pub fn header(&self, model: &Model, engine: &Engine<FlowMessage>) -> Header;
    pub fn run(&self, model: &Model, options: RunOptions<'_>,
               trace: &mut dyn TraceSink<FlowMessage>) -> Result<RunReport, RunFailure>;
}
```

`build` adds components in document order (ADR-0020 §7) and can be called once
per seed, from several threads, for parallel replications (M8). `run` uses
`experiment.horizon` unless the options give another. The behaviour types stay
crate-private: only the registry, the messages and the state enums are public.

**Load-time checks.** What spans a document (flow shape, variant names) must
fail at load, not at build (principle 2), and `taktsim validate` must see it.
So `Catalog` gains `with_check(fn(&Document, &mut Findings))`, run by the
loader after its own checks. The check reports `LoadErrorKind::NotAllowed` at
a place and field path, which needs `ComponentRef::path` and `Component::path`
public. This is additive to `taktsim-model`; the loader itself learns no
component type. `build` runs the same check again, so a document loaded with
another catalog cannot reach the engine unchecked. A `BuildError` names the
component's field path.

### 3. Flow in M5

`Edge` stays `{from, to}`. M5 gives it one meaning, the one M6 extends.

| Shape | Rule |
| --- | --- |
| Allowed topology | a directed acyclic graph: a Source has outgoing edges only, a Sink incoming only, Machine and Buffer both, and no component is left unconnected |
| Branch (several targets) | the part goes to the first target with room: all are offered in declared order, so a tie goes to the first declared. This is the spec's "first free" choice rule, and M6's default when `choose` is absent |
| Merge (several sources) | waiting offers are served in the order they arrived; ties follow event order (§8) |
| Declared order | edges in the order written, then `to` in list order (ADR-0020 §7 kept the order for this) |
| Loop back | rejected at load, with the reason. A loop with no condition or ratio would send parts round for ever, and finite buffers in a loop can deadlock. M6 lifts the rule together with conditions and states how such deadlocks are handled |

| | F1. Chains only | F2. Chains, branch and merge, no loops | F3. Any graph |
| --- | --- | --- | --- |
| M6 changes what M5 defined | yes: a new protocol for branches | no: M6 filters targets (`when`), picks one (`ratio`), or adds rules (`choose`) | no |
| Deadlock possible | no | no (§4) | yes |
| Roadmap's M5 property tests | covered | covered, and more | covered |

**F2.** The rules are in the factory catalog's check, not in the loader, so
the loader's stand-in tests and `routing.yaml` are untouched. M6 adds keys to
`Edge`; nothing M5 defines changes. Its only expected message change (the
current variant on `Accept`, for minimum setup) is a new `FORMAT` version,
free until v0.1 ships a trace.

### 4. Blocking and starving: the protocol between neighbours

**Blocking after service.** A machine starts a part without asking whether
there is room downstream. When the cycle ends it holds the finished part, and
that part keeps its place in the machine, until the next component has room.
This is the glossary's "blocked" and page 6's folder holding a finished box.

Downstream room must be promised before a part moves, because the engine
requires that a transferred item is received (ADR-0018).

| | P1. Offer, then accept, then send | P2. Credits (downstream grants places in advance) | P3. Send, and refuse if full |
| --- | --- | --- | --- |
| Events per hop | 3 | 2 | 1 to many |
| Merge | correct: one queue of offers | a credit held by an idle neighbour leaves another blocked with a place free, so a wrong answer | parts bounce |
| Branch | correct | as merge | parts bounce |
| Chosen part (discipline, M6 dispatching) | decided at the last moment | decided in advance | decided in advance |
| Needs a part in transit between tries | no | no | yes |

**P1.** Every component speaks it; a `Source` is only an outlet, a `Sink` only
an inlet, a `Machine` and a `Buffer` both. Two small helpers, `Outlet` and
`Inlet`, hold it, and later packages reuse them.

| Message | From → to | Meaning |
| --- | --- | --- |
| `Offer` | upstream → downstream | "I have a part for you." At most one outstanding per neighbour. |
| `Accept` | downstream → upstream | "A place is reserved for you." |
| `Arrive(attrs)` + item | upstream → downstream | The part moves; the reservation becomes the part. |
| `Decline` | upstream → downstream | "I have nothing for you any more." The reservation is released. |

- An `Accept` always gets exactly one reply, at the same instant, so a
  reservation never lasts longer than an instant.
- A component with free room and a waiting offer always accepts within the
  same event; a component with a part and no outstanding offer to a neighbour
  always offers within the same event. Both are invariants of the helpers,
  checked on own state (ADR-0018), so a wake-up cannot be lost.
- A down machine moves nothing: it queues offers without accepting, answers
  any `Accept` with `Decline`, and offers again on repair. A part whose place
  was promised before the failure arrives and waits.

**No deadlock.** A waiting chain in a directed acyclic graph ends at a
component that can act: a Sink always accepts, a running machine finishes, a
repair ends. There is no cycle of waiting. A Source that cannot emit holds its
parts and creates no more (§7), so a capacity-limited start of the line blocks
it instead of stopping anything. Property tests run generated chains and
acyclic graphs, including a fast Source in front of a one-place buffer.

**Cost.** Line `[source, cut, buf1, weld, sink]`: 15 events per part, 12 of
them the handshake on four hops. Instruction counts come with the benchmark in
the plan; nothing here is a speed claim (ADR-0010).

### 5. The six machine states

The state is computed from what the machine holds, never set by hand, so it
cannot disagree with it. The first line that holds wins. Six states partition
time, so shares sum to one.

| State | When | What the machine does | Typical cause |
| --- | --- | --- | --- |
| `down` | failed, repair running | nothing; nothing moves in or out | its own failure |
| `setup` | a changeover is running | changes over; no cycle runs | a part of another variant |
| `busy` | at least one unit is in its cycle | works | |
| `blocked` | no unit in a cycle, at least one finished unit waiting | holds a finished part, no room downstream | a slower or failed station after it |
| `idle` | nothing above, and no machine upstream of it | waits for a part | no demand |
| `starved` | nothing above, and a machine upstream of it | waits for a part | a station before it has not delivered |

**Idle against starved.** A practitioner will ask. Both wait with an empty
machine; the difference is where to look. *Starved* means the line let this
station down: another station is upstream and has not delivered, so look
upstream (page 6's welding). *Idle* means nobody asked: only Sources feed this
station, directly or through buffers, so its waiting says something about
demand, not about the line. In an M/M/1 model the server is idle, 1 − ρ of the
time, never starved. The first station of a line is idle; every later one is
starved.

| | I1. Static: is there a machine upstream? | I2. Idle only before the first part | I3. Dynamic: does anything upstream hold work? |
| --- | --- | --- | --- |
| Decidable from own state (ADR-0018) | yes, fixed at build | yes | no: needs other components' state |
| M/M/1 server | idle | starved | idle |
| Glossary's "buffer or station before it is empty" | yes | yes | yes |
| Cost | one bool | one bool | new messages |

**I1.** Its limit: a low-demand line's later stations show starved even though
the root cause is demand. That is still what happened at those stations, and
the first station's idle share shows the cause.

**Capacity above one.** The rule above applies to the machine as a whole: any
unit in a cycle makes it `busy`. Utilization of an M/M/c server is the
time-weighted count of units in a cycle over `capacity`, which the trace
carries (§9).

**Spec conflict.** The spec's invariant, "state transitions follow idle → busy
→ blocked / down", read literally, forbids a failure while idle. Calendar-time
failures (§6, spec §4 "Plant extensions", page 8) require it. It also omits
`starved` and `setup`. *Recommendation:* the arrow stays as the path a part
takes, and the invariant checked is this table.

| From | Allowed to |
| --- | --- |
| `idle` | `busy`, `setup`, `down` |
| `starved` | `busy`, `setup`, `down` |
| `setup` | `busy`, `down` |
| `busy` | `blocked`, `setup`, `down` |
| `blocked` | `busy`, `setup`, `idle`, `starved`, `down` |
| `down` | `idle`, `starved`, `busy`, `blocked`, `setup` |

The spec's arrow is inside it: `busy` never goes straight to `idle` or
`starved`, since a finished part always passes through `blocked`, for zero time
if there is room. `idle` and `starved` never turn into each other. A machine
starts `idle` or `starved` at time zero. The table is one function with an
exhaustive match on the state, without a wildcard arm.

A Source has two states: `waiting` (its interval runs) and `blocked` (it holds
parts). A Buffer has no state; its fill says everything.

### 6. Failures

| Question | Options | Choice | Effect on throughput, for M10 |
| --- | --- | --- | --- |
| Which clock? | calendar; operating time | **Calendar** (spec §4, page 8). The clock runs while up, whatever the machine does, and pauses while down | Down share is MTTR/(MTBF + MTTR) at any load. With operating time it would fall with load: at 1 h and 3 min, half busy, 4.76 % against 2.44 %. Equal when saturated |
| Can it fail while idle, starved or blocked? | yes; only while busy | **Yes**, and during setup, as the clock implies | Failures during waiting cost nothing unless a part arrives or room opens before the repair ends. A saturated machine sees no difference |
| An interrupted cycle? | resume with the remaining time; restart it | **Resume** (page 8's formula and *Factory Physics*' preemptive outages assume it) | Saturated: throughput A / mean cycle exactly. Restart would be 44.36 s against 44.10 s per part at 42 s, 1 h, 3 min, and 6,495 s against 3,780 s for a one-hour cycle |
| What does `mtbf: 6h` mean? | a constant; an exponential with that mean | **A constant.** Spec §4: "bare numbers mean constants"; ADR-0003. Page 8's open note closes | A machine fails every 6 h of up time exactly. Availability is unchanged; an outage's timing is regular |
| Up time distribution restarts? | at each repair end; at each failure | **At each repair end** | Up and down times alternate independently, so availability A = MTBF/(MTBF + MTTR) holds exactly in the long run |

**Mechanics.** At a failure the machine cancels each running cycle's timer and
any setup timer (ADR-0001, ADR-0018) and keeps each remaining time. At repair
it schedules them again in `ItemId` order and draws the next up time. A
failure at the same instant as a cycle end comes first (ADR-0001), so that
cycle resumes with zero remaining. All units of a machine stop together: one
machine, one failure process.

**Room for v0.5.** An operating-time clock is a new optional key (say
`failure_clock: operating`), absent meaning calendar for ever, so no model or
hash changes. M5 has no such key. The failure timer is held in one small type
that can then pause it on the same cancellation mechanism.

### 7. Setup, batch, capacity, Source, attributes

**Setup.** A machine remembers the variant it is set up for. The first unit
costs no setup: the machine starts ready for whatever comes first. A unit of
another variant needs a changeover: `matrix` if it has the pair, otherwise
`per_variant`, otherwise none. Times are distributions drawn when the setup
starts; a failure pauses a setup as it pauses a cycle.

**Capacity above one.** Each unit has its own cycle, drawn from one stream when
it starts, its own timer and its own place until it leaves. Finished units
keep their places (blocking after service). Units start in arrival order.
A unit whose change costs time waits at the front until no unit is in a cycle,
then the changeover runs, without waiting for finished units to leave; a change
that costs nothing does not wait. So capacity and setup combine with one rule
instead of being rejected.

**Batch.** An arrival creates `batch` units at one instant, one variant and
one priority for all of them (page 11's "runs of one variant"), then offers
them one at a time in creation order.

**Source when downstream is full.**

| | S1. Keep creating, unbounded backlog | S2. Hold, and the next interval starts when the batch has left | S3. Hold, the clock keeps running with one arrival pending |
| --- | --- | --- | --- |
| Parts lost | none | none | none |
| Memory | unbounded in an overloaded model | at most one batch | at most two batches |
| Never blocked | the declared process | the declared process, exactly | the declared process |
| Under blocking | demand stays at the declared rate; the backlog grows | rate falls: the interval counts from departure | rate falls less |
| Extra state | a queue | none | a flag |

**S2.** A blocked Source does not lose parts: it holds them, counted as held,
and its invariant is `created = emitted + held`. Spec §4's "units created =
units emitted" is that with nothing held. S1 would make spec §4's own example
grow without limit: parts every 40 s into a machine of 42 s.

The price: the declared interval is an offered rate, reached only if there is
room. A Source right in front of a machine is blocked whenever the machine is
busy, so with exponential gaps of 88.2 s and a 42 s machine with failures, a
part leaves every 100 s, not every 88.2 s (see "Checked with a throwaway
model"). An M/M/1 model therefore takes a buffer with capacity far above what ρ
reaches. The first part appears after the first interval, not at time zero.
Lead time runs from creation, so it includes time held at the Source.

**Item attributes.**

| | A1. In the message that carries the item | A2. A table inside the engine | A3. A shared table between components |
| --- | --- | --- | --- |
| Core change | none | large: `Engine` learns attributes | none |
| Core stays domain-free (principle 7) | yes | no, unless generic | yes |
| In the trace, checked by replay | automatically | needs a record | no |
| Components see each other only through `Context` (ADR-0018) | yes | yes | no |
| Cost | 12 bytes per `Arrive` | | |

**A1.** `ItemAttrs { variant: VariantId, priority: u8, due: Option<SimTime> }`
travels on `Arrive`; holders keep it beside the `ItemId`. `ItemId` and the
conservation check do not change. `VariantId` is a `u16`, an index into the
model's sorted variant names. `due` is always absent in M5; carrying it now
spares M6 a format change. Roadmap M6 gives priority classes to M6, but
Buffer's `priority` discipline is M5's and cannot be tested otherwise, so M5
adds one Source key, `priority`, and M6 adds its mix beside it.

**Priority direction.** Higher goes first, then oldest; absent is 0, so a part
marked `priority: 1` passes every unmarked part. (The other direction would
leave no way to outrank an unmarked part.) This is unrelated to a message's
priority, where lower goes first.

**Buffer.** It chooses the part when an `Accept` arrives, not when it offers:
oldest (`fifo`), newest (`lifo`), or highest priority then oldest (`priority`).

### 8. Streams and ties

Streams are keyed by component id and purpose (ADR-0013). The names are part
of the trace contract: renaming one changes every draw.

| Component | Purpose | Drawn | Taken |
| --- | --- | --- | --- |
| Source | `interarrival` | at each interval's start | always |
| Source | `variant` | once per batch | with two or more variants |
| Machine | `cycle_time` | at each cycle's start | always |
| Machine | `failures` | at time zero and each repair end | with `mtbf` |
| Machine | `repairs` | at each failure | with `mtbf` |
| Machine | `setup` | at each changeover | with `setup` |

M6 adds `priority`, `due_in` and `route` without shifting these. Variant
choice uses `next_open01` against cumulative fractions over sorted names, the
last name taking what rounding leaves; only exact float operations (ADR-0013).

**Same-instant order** (ADR-0001: time, priority, then scheduling order;
ADR-0018 deviation 1: the package owns the table). Lower goes first. Finishing
and landing come before asking, so an offer is judged after everything that
frees room at that instant.

| Priority | Message | Kind | Why here |
| --- | --- | --- | --- |
| 0 | `Failure` | timer | ADR-0001: failures before completions |
| 1 | `Repaired` | timer | up again before anything else is judged |
| 2 | `CycleDone { item }`, `SetupDone` | timer | completions free room |
| 3 | `NextArrival` | timer | new parts after completions at that instant |
| 4 | `Arrive` | message with item | a promised part lands first |
| 5 | `Accept` | message | |
| 6 | `Decline` | message | releases a place before offers are judged |
| 7 | `Offer` | message | asks last |
| 8 | notes | never delivered | |

### 9. Trace: messages, encoding, and what M8 can read

`FlowMessage` implements `TraceMessage`, `FORMAT = "taktsim-components/1"`, an
enum tag `u8` followed by its fields (ADR-0021).

| Tag | Message | Fields |
| --- | --- | --- |
| 0 | `Failure` | |
| 1 | `Repaired` | |
| 2 | `CycleDone` | item u64 |
| 3 | `SetupDone` | |
| 4 | `NextArrival` | |
| 5 | `Arrive` | variant u16, priority u8, has due u8, due u64 (0 if none) |
| 6 | `Accept` | |
| 7 | `Decline` | |
| 8 | `Offer` | |
| 9 | `MachineState` (note) | state u8 (0 idle, 1 busy, 2 blocked, 3 starved, 4 down, 5 setup), units in cycle u32, has variant u8, setup variant u16 (0 if none) |
| 10 | `SourceState` (note) | state u8 (0 waiting, 1 blocked), held u32 |
| 11 | `VariantName` (note) | id u16, name text |

**States are not in today's trace.** A component's state is internal; the
existing records show events and items only (ADR-0021).

| | T1. Note record: `Context::note(&M)` writes `TraceRecord::Note` | T2. Self-message per change | T3. Rebuild states from events in M8 | T4. Components keep counters, reported at the end |
| --- | --- | --- | --- | --- |
| Core change | **yes**: one `Context` method, one `TraceRecord` variant | none | none | a new `Component` method |
| Events on the line above | 15 per part | 23 per part, more than half again | 15 | 15 |
| Cost per change | one call into the sink | one message event (610 to 850 instructions in ADR-0001's table) | none in the run | one counter |
| State history in the trace, compared by replay | yes | yes | no | no |
| Logic duplicated | no | no | yes: the state machine again, in M8, and it can drift | no |
| Viewer can show state at any event (spec §14) | yes | yes | only through M8's copy | no |
| Serves M6's "every control decision in the trace" | yes, same record | yes, at the same cost | no | no |

**T1, a change to core,** flagged as a one-way door. It is additive to
`Context` and leaves `Component` unchanged; `TraceRecord` is deliberately not
`#[non_exhaustive]` (ADR-0021), so `taktsim-trace` must add the kind. The note
is generic: the engine never looks inside and knows no state or factory word
(principle 7).

- Record kind 4, after the prefix: `by u32`, message (the rest). A reader that
  only frames skips it (ADR-0021's rule for new kinds).
- `RECORD_FORMAT` becomes `taktsim-core/2`. No file format version changes.
- Replay compares notes with every other record, so a machine's state history
  is checked byte for byte, which narrows ADR-0021's question 1. Internal state
  outside the notes is still left to invariants and the v0.4 snapshots.
- The toy cases' stream hashes do not change, since no toy emits a note; their
  file hashes and snapshot header lines do, and the commit says why.

If the owner declines T1, T2 works with no core change and the same message
tags, at the cost in the table.

A machine writes one note after an event if its state or its count of units in
a cycle changed, and one at start. A Source does the same, and writes each
`VariantName` at start, so a trace names its variants on its own.

**What M8 reads, from the trace alone:**

| KPI | From | Enough? |
| --- | --- | --- |
| Throughput | `ItemDisposed` delivered, by sink | yes; per variant through the `Arrive` that carried the item |
| State shares | `MachineState` notes, from the start note | yes, given the window (below) |
| Utilization, capacity above one | units in cycle on the notes | yes |
| Setups, per variant change | `setup` notes with variant | yes |
| WIP per buffer, time-weighted | events with an item: `+1` into the buffer, `-1` out of it | yes, also for a machine's units |
| WIP in the system | `ItemCreated`, `ItemDisposed` | yes |
| Lead time per unit | `ItemCreated` to `ItemDisposed` delivered, by item | yes |
| Warm-up cut-off (M9) | any of the above by time | yes |

**What M8 would miss:**

1. **The end of the window.** No record says where the run stopped: the last
   record is the last event, not the horizon. A state or fill holds until the
   horizon, so time weights need it. The header holds the model hash, and the
   horizon is in the model, so M8 takes it from the model the trace names. That
   holds only if M7's `--horizon` writes its value into the normalized model
   before hashing; otherwise the trace's model hash does not describe the run.
   M7 must do this. A field in the end marker would be a new file format
   version and is not needed.
2. **Units still inside at the horizon.** Lead time covers delivered units only,
   which understates it when the line is filling. M8's report must say so. The
   count of units inside is in the trace.
3. **A dependency for M8 to choose.** Reading notes needs `FlowMessage`, so
   `taktsim-stats` would depend on components. Recommendation for M8: a small
   fold in `taktsim-components` turns records into neutral facts, which
   `taktsim-stats` consumes, and a live `TraceSink` and a read-back trace feed
   the same fold.

### 10. Runtime invariants

`check_invariants` on own state (ADR-0018 deviation 6), first violation
returned.

| Component | Invariants |
| --- | --- |
| Source | `created = emitted + held`; `held ≤ batch`; `held > 0` ⇒ offer outstanding |
| Machine | units in cycle + waiting to start + finished + reserved places ≤ `capacity`; last state change allowed by §5's table; state equals what it holds implies; no changeover running while a unit is in its cycle; unless down, a free place and a waiting offer never coexist, nor a finished unit and no outstanding offer |
| Buffer | `fill + reserved ≤ capacity` (so `0 ≤ fill ≤ capacity`, `fill` being unsigned); a free place and a waiting offer never coexist; a unit to give and no outstanding offer never coexist |
| Sink | units received = units delivered; holds nothing |

Global conservation stays with the engine. A check costs O(k log k) for a
component holding k units (ADR-0018), so a very full buffer with checks on is
slower per event, not wrong.

### 11. Registry

| | R1. `inventory` (spec's default) | R2. Compile-time table: `Registry::standard()` and `with::<T>()` |
| --- | --- | --- |
| `#![forbid(unsafe_code)]` | compiles, but the rule no longer means what it says (below) | holds, no `unsafe` anywhere |
| WebAssembly (v0.2 target) | supported; constructors run through a linker heuristic. For a module called into several times, as a Web Worker's is, its docs advise exporting and calling `__wasm_call_ctors`, an `unsafe` call. Without that they call repeated runs "safe and mostly harmless" for `inventory` | plain function, nothing to run first |
| Bill of materials | one crate, plus `rustversion` on WebAssembly | nothing |
| Order | "no guarantee" (its docs), link order | sorted by `Catalog`'s `BTreeMap` |
| Works before `main`, before any check | yes: static constructors | no: an ordinary call |
| Adding a type | one `submit!` beside it | one line in the table |
| Forgetting to add a type | possible: the `submit!` can be left out, and a crate never linked is silently absent | possible: closed by the test below |
| A second package | links and appears | `Registry::standard()?.with::<Theirs>()?` |

*Measured,* inventory 0.3.25, rustc 1.97.1 and 1.99.1, Linux x86-64: `submit!` compiles in
a crate with `#![forbid(unsafe_code)]`, while a hand-written `unsafe` block in
the same crate is still rejected. The macro expands to an
`unsafe extern "C" fn __ctor` and a linker-section static in our crate, which
the lint does not report. So the gate would pass while the crate holds code the
rule exists to exclude, and CLAUDE.md requires an ADR, an isolated module and
Miri for such code. *Not measured:* a WebAssembly build (that target is not
installed here) and Miri on it (no Miri component here). Both table rows rest
on its documentation.

**R2.** It is still "registered at compile time" (spec §4). One `with::<T>()`
fills both the catalog and the builder, so a type cannot be readable but not
runnable. The schema generator calls `Registry::standard()`, so it picks up
every registered type with no list of its own.

The test `every_component_type_is_registered`: (1) `$defs/Component.oneOf` lists
exactly `catalog().types()`; (2) registering a stand-in with `with::<T>()` makes
it appear in the schema, with the generator untouched; (3) a scan of the crate's
source for `impl ComponentType for` matches the registered list, which closes
R2's one weakness.

### 12. Two sentences for ADR-0020

Both keep the shared part of the format free of factory assumptions, and neither
changes a byte today.

- §8: "How a system is computed is not part of what it is: a setting of the
  method, such as a step size or a tolerance, is not a component parameter, so
  changing one does not change the system hash."
- §11: "`flow` belongs to the queue engine. What may connect to what is decided
  by the catalog's check, not by the loader; that `flow` is required is the
  loader's rule for format 1.0 today, and can become the catalog's without
  changing any model."

M5 keeps envelope and model apart in `$defs` as ADR-0020 §11 asks: a test checks
that no envelope definition (`Model`, `Experiment`, `Include`, `ObservableRef`)
reaches `Component`, `Observable` or `FlowEntry`. New factory value types stay
inline in their entries.

## Checked with a throwaway model

Before writing this down, the rules above were run in a small throwaway Python
model of the protocol, with the invariants of §10 and the table of §5 asserted
after every event. It is not project code, proves nothing about the engine, and
says nothing about speed. It shows the rules give the answers the closed forms
and the concept pages expect. M5's tests repeat the checks on the engine, with
fixed seeds.

| Case | Expected | Got |
| --- | --- | --- |
| Events and notes per part, five-component line, no blocking | 15 and 8 (§4, §9) | 15.0 and 8.0 |
| Two exponential stations, no buffer, saturated | throughput ⅔ of a station's rate (known result for blocking after service) | 0.6667 |
| Saturated machine, 42 s, MTBF 1 h, MTTR 3 min | 44.1 s per part | 44.11 s; constant `mtbf` and `mttr`: 44.10 s |
| Down share at that MTBF and MTTR, saturated / half loaded / blocked half the time | 4.76 % | 4.78 % / 4.84 ± 0.12 % / 4.73 % |
| Page 6, buffer 1, sigma 0 | 639 parts, cut blocked 7 %, weld starved 0 % | 639, 6.5 %, 0.1 % |
| Page 6, buffer 1, sigma 0.5 | 576 ± 4 parts, cut blocked 15 %, weld starved 10 % | 580, 15 %, 10 % |
| Page 8, 1 h and 3 min; 5, 20, 50 places | 606, 631, 636 parts | 605, 629, 635 |
| Page 8, 10 h and 30 min; 5, 20, 50 places | 599 ± 7, 618 ± 6, 626 ± 6 parts | 590, 614, 620 |
| M/M/1 at ρ = 0.9: time in system; idle share | 90 s; 10 % | 92 s; 10 % |
| M/M/2 at ρ = 0.8: time in system | 44.4 s | 43.3 s |
| Setups per batch, two equal variants; setup share | 0.5; 9.1 % | 0.497; 9.0 % |
| 300 random acyclic graphs with branches, merges, failures, capacities and all disciplines | no deadlock, no broken invariant | none |

The page rows use the pages' teaching simulator as the reference, with a
saturated Source. The long-outage page 8 row runs a little low with these 30
seeds, within or just outside its interval; M10 settles such comparisons.

## One-way doors

Before v0.1 ships, each costs a code change and regenerated files. The cost
below is after v0.1 ships.

| Door | Commits to | Cost to reverse | Test that proves it |
| --- | --- | --- | --- |
| Type names and keys (vocabulary) | the words of every model and of later packages | a format migration for every model; every later package | `ci/check-schemas.sh`, `schema_contract`, `malformed` corpus |
| Meaning of `mtbf`, `mttr`, `interarrival`, `batch` | every result of every model | silently different results; only a new format version is honest | `availability_holds_at_every_load`, `saturated_throughput_is_a_over_cycle`, `next_interval_starts_when_the_batch_has_left` |
| Normalized form (N1, sorted `variants` and `matrix`) | model and system hashes of real models | every hash, certificate and run id | `spec_example_keeps_its_pinned_hashes`, `full_vocabulary_hash_is_pinned`, `written_default_is_part_of_the_hash`, spelling property test |
| Six states, their definitions and codes | KPIs, viewer colours, what users report | every state share, every comparison with history | `state_equals_what_the_machine_holds`, `every_transition_in_the_table`, `idle_only_without_a_machine_upstream` |
| Blocking after service and the handshake | every golden trace, every queueing result | every trace; throughput of every finite-buffer line | `two_stations_without_buffer_give_two_thirds`, `reservation_never_lasts_past_its_instant`, golden cases |
| Failure rules (calendar, any state, resume) | throughput of unreliable lines | every result with failures | `failure_while_blocked_or_idle`, `interrupted_cycle_resumes_with_remaining_time` |
| Blocked Source (S2) | offered load under blocking | results of overloaded lines | `blocked_source_holds_and_loses_nothing`, `created_equals_emitted_plus_held` |
| Setup rules (first unit free, FIFO, one variant at a time) | throughput with variants | results with setups | `first_unit_costs_no_setup`, `changeover_waits_for_units_in_cycle` |
| Flow defaults (first free in declared order, oldest offer first) | M6's defaults, every branch and merge | every model with a branch | `branch_ties_go_to_the_first_declared`, `merge_serves_oldest_offer` |
| Message priorities and encoding (`taktsim-components/1`) | trace contract | a new `FORMAT`, every golden trace | `priorities_are_pinned`, `messages_round_trip`, golden cases on every target |
| Stream purposes | common random numbers, every draw | every draw of every model | `stream_purposes_are_pinned`, `adding_a_line_shifts_nothing`, `changing_mtbf_keeps_cycle_draws` |
| Core: `Context::note`, `TraceRecord::Note`, kind 4, `taktsim-core/2` | public engine API, record format | a record format version, all traces | `notes_reach_the_trace_and_no_component`, replay test with notes, pinned hashes |
| Attributes in the message, not in core | core stays domain-free | a public `Engine` change | `attributes_travel_with_the_item`; core's unchanged item API |
| Dependency direction (D1), no new external crate | layout every package copies; bill of materials | moving public types between crates | `ci/check-crates.sh` gains a direction check; `cargo deny check` |
| `ComponentType`, `Registry`, `build`, `run`, `Catalog::with_check` | M7, M12, every later package | every client | doc tests; `every_component_type_is_registered`; `a_loaded_model_always_builds` property test |

## Spec and code disagreements

| # | What | Where | Recommendation |
| --- | --- | --- | --- |
| 1 | The literal state arrow forbids failing while idle and omits two states | spec §4 table against §4 "Plant extensions", page 8 | check §5's table; amend the spec's cell |
| 2 | "units created = units emitted" fails while a Source holds parts | spec §4 table | `created = emitted + held`; amend the cell |
| 3 | Sink's "units consumed" uses the word core's `Disposal::Consumed` keeps for assembly | spec §4 table, `Disposal` | check received = delivered; amend the cell |
| 4 | Priority classes are M6's, but Buffer's `priority` discipline is M5's | roadmap M5, M6 | one Source key in M5, the mix in M6 |
| 5 | Page 11's example cannot be built in v0.1: welding "collects orders until it has a full batch", parts "leave together", and the next batch is "always a different variant". M5's `batch` creates parts at the Source, and a random mix can repeat a variant | `docs/concepts/11-batching-setups.md` | correct the page's "In taktsim" note in the M5 pull request; collecting is `BatchProcess` (v0.2) |
| 6 | Page 8 says `mtbf: 6h` is not settled | page 8 note | a constant (§6); update the note |

## Consequences

- A model that loads with the standard catalog always builds, and runs on every
  tier-1 target with the same bytes; golden cases check it.
- `taktsim validate` gets flow and variant errors with paths for free.
- Fifteen events per part on the five-component line, plus 8 notes. Traces grow
  accordingly; memory does not (ADR-0021).
- `ci/check-schemas.sh` and `ci/schemas-test.sh` run the generator from
  `taktsim-components`. The check is unchanged and now sees real types. It is a
  gate file, so the pull request carries `gate-change`.
- M6 extends the edge and the helpers without changing M5's meaning, and must
  decide how loops avoid deadlock.
- M7 must write a `--horizon` override into the normalized model before hashing.

## Order of work

1. `taktsim-model`: `Catalog::with_check`, public paths.
2. `taktsim-core`, `taktsim-trace`: the note record.
3. `taktsim-components`: messages, helpers, then Sink, Buffer, Source, Machine.
4. Registry, whole-document check, build and run; schema regenerated, generator
   moved.
5. Golden cases, properties, the trace-to-KPI test, analytical smoke tests.
6. Concept page notes.

## Questions for the owner

Implementation proceeds on the recommendations; each can still change before
the M5 pull request merges.

1. **Core change (§9).** Add `Context::note` and record kind 4, so machine
   states are in the trace at the cost of one call per change? *Recommended:*
   yes. Without it, self-messages add about half again as many events.
2. **Idle against starved (§5).** Starved if a machine is upstream, idle if only
   Sources are? *Recommended:* yes.
3. **Spec cells (§5, §7, §10).** May this ADR's acceptance amend spec §4's
   invariant cells for Machine, Source and Sink as listed? *Recommended:* yes.
4. **Failures (§6).** Calendar clock, any state, resume, `mtbf: 6h` a constant?
   *Recommended:* yes to all.
5. **Blocked Source (§7).** Hold, with the next interval starting when the batch
   has left? It gives up "demand stays at the declared rate" under blocking in
   exchange for bounded memory. *Recommended:* yes; a backlog can be an optional
   key later.
6. **First unit costs no setup (§7)?** *Recommended:* yes; an initial variant can
   be an optional key later.
7. **Priority (§7).** A Source `priority` in M5, higher goes first, absent 0?
   *Recommended:* yes.
8. **Loops rejected until M6 (§3)?** *Recommended:* yes.
9. **Registry (§11).** A table instead of `inventory`? *Recommended:* yes, because
   of what `submit!` puts in a `forbid(unsafe_code)` crate.
10. **Observables (vocabulary).** Register the four names in M5 so spec §4's
    example loads, with result types still M8's? *Recommended:* yes.
11. **Written defaults (§1).** `capacity: 1 parts` and nothing are two hashes?
    *Recommended:* yes.
12. **ADR-0020's two sentences (§12).** *Recommended:* yes.
13. **Window for M8 (§9).** Take the horizon from the model the trace names, and
    have M7 write overrides into the model before hashing? *Recommended:* yes.
