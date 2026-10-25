# ADR-0018: Engine architecture

- Status: proposed. Needs the owner's **explicit** acceptance: it reads
  spec §4 and §7 and deviates from accepted ADR-0001 in the points listed
  under "Deviations from ADR-0001".
- Date: 2026-10-25
- Blocks: M2b, and everything built on the engine (M3, M5, M6, M8)
- Spec: §2.7 (small core), §4 (components, conservation, extensibility),
  §7 Layer 3 (runtime self-checks); ADR-0001, ADR-0004, ADR-0013

## Context

`taktsim-core` needs an event loop that knows nothing about machines
(§2.7), lets component packages define behaviour as state machines over
events (§4), and checks invariants and conservation at every event (§4,
§7). It must be deterministic (ADR-0001, ADR-0013) and have no I/O.

## Decision

**Messages are the model's type.** `Engine<M>` is generic over one message
type `M: Message`, usually an enum defined by the component package. The
engine never looks inside a message; it only asks for its priority
(ADR-0001's same-instant order: time, then priority, then scheduling
order) and its `Debug` text for error reports. Components receive a
reference to the message; the engine keeps the message itself for error
reports, so nothing is copied per event.

**Components are trait objects.** `Box<dyn Component<M>>`, so packages
(and later a registry) can add component types without changing the
engine. A component has `start`, `handle(delivery, context)`,
`held_items` and `check_invariants`. Components never call each other:
everything goes through their `Context`, so the engine sees every change.

**`Send`.** `Component` and `Message` require `Send`, so an `Engine` can
move to another thread: the Python layer releases the GIL while a run
works (ADR-0005), and a caller can wrap an engine in a `Mutex` where
`Sync` is needed. Set now, before component packages exist, because
adding the bound later would break every one of them.

**Context operations**, all checked:

- `schedule(after, message)`: a timer to self, cancellable by its owner
  only; `cancel(event)`.
- `send(to, message)`: a message to another component now; not
  cancellable.
- `lookup(name)`: components find neighbours by model id at start, so a
  model needs no two-phase wiring.
- `create_item`, `transfer(item, to, message)`, `receive(item)`,
  `dispose(item, Delivered | Scrapped | Consumed)`: the flowing things the
  spec calls "units" are named `Item` in code, to keep them apart from
  units of measure.
- `stream(purpose)`: a random stream keyed by component id and purpose
  (ADR-0013); only in `start`, so every key conflict is found before the
  first event; a second stream with the same key is an error.

**Items are tracked by the engine.** It records where each live item is
(held by a component, or in transit to one) and counts created, held, in
transit, delivered, scrapped, consumed. A transferred item travels with its
event; the target must receive it while handling that event, or the run
stops. Per-material conservation (§4) extends the counts when materials
arrive in v0.2.

**Checks** (on by default; off with `EngineConfig::without_checks`, the
spec's `--fast`). After a component starts or handles an event, the engine
checks that component's invariants, compares the items it reports holding
with the engine's records (no duplicates, no foreign items, same count),
and checks the global identity created = held + in transit + delivered +
scrapped + consumed.

*Reading of spec §7* ("every run executes the invariant set of every
component plus global conservation at each event"): only the component
that handled the event is checked. This is equivalent, because a
component's state changes only inside its own `start`/`handle`, and an
item's location changes only when its holder transfers it or its target
receives it, so every other component is unchanged since its last check.
It requires that a component's invariants depend only on its own state
(not on other components or on time passing without an event); the
component trait documentation states this. A check costs O(k log k) for
the handling component holding k items, instead of the sum over all
components; it reuses one buffer and does not allocate per event.

**Run control.** `run(RunLimits { until, max_events, cancel })` processes
events up to and including `until`, stops after `max_events`, or when the
caller's `AtomicBool` is set (how a wall-clock budget crosses into the
synchronous engine; the async side is a v0.2 ADR). The first call starts
every component at time zero in the order added. At `Horizon` or `Idle`,
the clock moves to `until`, so time-weighted statistics cover the whole
window; `EventLimit` is reported only if another event is due. A run can
be continued with a later `until`. After any `RunError` from processing,
the engine refuses every later `run` (`AlreadyFailed`): its state may be
inconsistent, and the spec requires the run to abort.

**Errors and reports.** Engine misuse is a typed `EngineError`; a failed
run is a `RunError` naming the component and time and carrying the last 32
events, oldest first (§4: "a violation aborts the run with the event trace
that led to it"). Conservation failures are a typed `ConservationProblem`
(reported twice, not held, count mismatch, unbalanced totals). Item counts
are `taktsim_units::Count`.

## Deviations from ADR-0001 and readings of the spec

1. **Priorities belong to the message type**, not to one fixed table in
   the core: the core cannot know component messages (§2.7). Each
   component package documents its priorities with its message enum.
2. **No crate-private `EventQueue` trait yet.** The queue is one concrete
   type in one file; a trait is added when the benchmark brings a second
   implementation.
3. **Cancellation keeps a set of cancelled sequence numbers** and a map of
   pending timers, instead of generation numbers. Both are lazy deletion;
   messages (most events) touch neither. The benchmark decides whether
   generation numbers pay off.
4. **The queue benchmark is not in M2b** (below).
5. **Spec §4's component "with a typed parameter struct and a list of
   invariants"**: parameters are typed by each component's own Rust type
   (the model loader, M4/M5, builds components from typed parameter
   structs); invariants are one `check_invariants` method returning the
   first violation, instead of a list object.
6. **Spec §7's "every component at each event"**: read as described under
   "Checks".

**Trace.** The engine writes `TraceRecord`s (event delivered, item created,
item disposed) to a caller-provided `TraceSink`; recording cannot fail the
run (ADR-0004). M3 adds storage, replay and any further record kinds.

## Not in M2b

- The queue benchmark that finalises ADR-0001's structure needs the
  benchmark infrastructure of track C (PR #6); it follows once that is
  merged.
- Events per second on the reference line model needs the components of
  M5; measured in M13 (ADR-0007, ADR-0010).
- Trace storage, replay and the cross-platform trace-hash job: M3.

## Consequences

- Component packages own their message enum; components of different
  packages share a model only through a common message type (a package
  can wrap another's).
- Every change to the item counts is in one place (`Context`), which keeps
  the conservation identity easy to reason about.
- A component that keeps cross-component or time-dependent invariants must
  re-check them in its own events.
