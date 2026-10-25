# ADR-0018: Engine architecture

- Status: proposed. Accepted when the M2b pull request is merged, except
  the reading of spec §7 under "Checks", which needs the owner's explicit
  acceptance.
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
order) and its `Debug` text for error reports.

**Components are trait objects.** `Box<dyn Component<M>>`, so packages
(and later a registry) can add component types without changing the
engine. A component has `start`, `handle(delivery, context)`,
`held_items` and `check_invariants`. Components never call each other:
everything goes through their `Context`, so the engine sees every change.

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
  (ADR-0013); a second stream with the same key is an error.

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
component trait documentation states this. It makes checking O(1) per
event instead of O(components).

**Run control.** `run(RunLimits { until, max_events, cancel })` processes
events up to and including `until`, stops after `max_events`, or when the
caller's `AtomicBool` is set (how a wall-clock budget crosses into the
synchronous engine; the async side is a v0.2 ADR). The first call starts
every component at time zero in the order added. At `Horizon` or `Idle`,
the clock moves to `until`, so time-weighted statistics cover the whole
window. A run can be continued with a later `until`.

**Errors and reports.** Engine misuse is a typed `EngineError`; a failed
run is a `RunError` naming the component and time and carrying the last 32
events, oldest first (§4: "a violation aborts the run with the event trace
that led to it").

**Trace.** The engine writes `TraceRecord`s (event delivered, item created,
item disposed) to a caller-provided `TraceSink`; recording cannot fail the
run (ADR-0004). M3 adds storage, replay and any further record kinds.

## Not in M2b

- The queue benchmark that finalises ADR-0001's structure needs the
  benchmark infrastructure of track C; it follows once that is merged.
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
