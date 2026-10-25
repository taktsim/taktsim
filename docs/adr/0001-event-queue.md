# ADR-0001: Event queue and simulation time

- Status: accepted (2026-10-25); the queue structure stays provisional
  until the M2 benchmark, run on the reference hardware from ADR-0010
- Date: 2026-10-25
- Blocks: M1 (time type), M2 (engine core)
- Spec: §15 "Event queue", §2.3, §9 Performance and Reproducibility

## Context

`taktsim-core` needs a future-event list. Spec §15 asks for binary heap,
pairing heap or calendar queue, decided by benchmark before v0.1. Three
requirements shape the choice more than raw speed:

1. **Total, deterministic order.** Same model, seed and engine version must
   give the same trace byte for byte on every platform. Two events at the
   same time must always pop in the same order. `std::collections::BinaryHeap`
   does not order equal keys stably, so the key itself must be total.
2. **Cancellation.** A failure interrupts a running cycle; its scheduled
   completion must not fire. Blocked or preempted activities cause the
   same need.
3. **Throughput.** Initial target ≥ 1 M events/s/core with checks off
   (§9). The queue is one part of that budget, not all of it.

The queue key depends on how time is represented, so this ADR decides
both.

## Options

### Time representation

- **T1. `f64` seconds.** Natural for sampled durations. Not `Ord`; repeated
  addition accumulates rounding error that differs by summation order;
  equal-time detection needs epsilons.
- **T2. Integer ticks, fixed resolution (`u64` nanoseconds).** Total order,
  exact addition. `u64` ns covers ~584 years. Sampled `f64` durations are
  rounded to the tick once, at the point of sampling, with one documented
  rounding rule.
- **T3. Integer ticks, resolution from `model.time_unit`.** Same as T2 but
  coarser models round more; time resolution then changes when a user
  changes the display unit, which surprises.

### Queue structure

- **Q1. Binary (or 4-ary) heap**, key `(time, class, seq)` where `seq` is a
  monotonic insertion counter. Cancellation by lazy deletion: each pending
  event carries a generation number; a cancelled event is skipped on pop.
- **Q2. Pairing heap.** O(1) insert and decrease-key, so cancellation can be
  eager. Pointer-heavy, worse cache behaviour; a safe-Rust implementation
  uses an arena with indices.
- **Q3. Calendar queue.** O(1) amortized when event times are evenly
  spread; needs bucket-width tuning and resizing, and degrades on skewed
  distributions (e.g. many near-term cycle ends plus a few far-off shift
  changes). Determinism holds only if the tie rule is applied inside
  buckets as well.

## Trade-offs

| | Q1 heap | Q2 pairing | Q3 calendar |
| --- | --- | --- | --- |
| Implementation risk | lowest; tiny, easy to reach 100 % branch coverage | medium | highest; tuning logic adds many branches to cover |
| Cancellation | lazy, heap grows with stale entries | eager | lazy or eager |
| Expected speed on 20–1,000 components | O(log n) on a small n, cache-friendly | similar or slower in practice | fastest only on large, evenly spread queues |
| Determinism | from the key | from the key | key plus bucket rule |

Pending-event counts in the reference models are small (roughly one or two
per component), so log n is around 10. At that size, cache behaviour
usually matters more than asymptotic cost.

## Recommendation

- **Time: T2**, `u64` nanosecond ticks in a `SimTime` / `SimDuration`
  newtype in `taktsim-units`. Sampled durations are rounded half-to-even
  to the nearest tick, then clamped at ≥ 0. Arithmetic is checked;
  overflow is a typed error, not a wrap.
- **Queue: Q1**, a binary heap keyed by `(time, class, seq)`, behind a
  crate-private `EventQueue` trait so the structure can be swapped without
  touching components. `class` is a small fixed priority (e.g. failures
  before completions at the same instant), documented in one table; `seq`
  breaks remaining ties by insertion order. Lazy cancellation with
  generation numbers; compaction when stale entries exceed half the heap.
- **Validation:** in M2, benchmark Q1 (binary and 4-ary) against Q2 on the
  reference line model (20 components) and a 1,000-component synthetic
  model, recording events/s and instruction counts. Q3 is benchmarked only
  if Q1 alone exceeds 30 % of event-loop time. This ADR is updated with the
  numbers before it is accepted.

## Consequences

- All engine and component code uses integer time; floats appear only in
  sampling and in statistics.
- Same-instant ordering is part of the engine contract and of golden
  traces; changing the `class` table changes traces and requires a version
  bump.
- Stale cancelled entries cost memory; compaction bounds that.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Is 1 ns resolution acceptable, or do you expect models needing finer
   resolution (none in the spec do)?
2. Do you accept that the decision is final only after the M2 benchmark?
