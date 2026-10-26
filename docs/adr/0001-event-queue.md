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

## M2 measurements

Added with the M2 queue benchmark (pull request #19). The status line
above is unchanged: these numbers do not finish the validation plan (below,
"What this does not show"), and accepting or changing the queue structure is
the owner's decision.

**Source.** The `bench` workflow, run 40388789599 (artifact
`instruction-counts`): the pull request's commit `900fd86` merged into
`main` at `6834c25`, on a GitHub-hosted `ubuntu-24.04` runner (x86-64),
Valgrind 3.22.0, gungraun 0.20.0. Each figure is the Callgrind instruction
count (`Ir`) of one run of about 10 000 events, for an engine that setup
built and started, with checks off (`--fast`). Instruction counts are
deterministic and gate regressions; they are not a speed claim (ADR-0010),
and this section gives no events per second. "Cycles" is gungraun's
estimate from a cache model (L1 hits + 5 × last-level hits + 35 × RAM hits),
a measure of cache pressure, not time. Workloads and their expected values:
`crates/taktsim-bench/src/queue.rs`, `tests/workloads.rs`.

| Workload | Pending | Events | `Ir` | `Ir` / event | Cycles / event |
| --- | ---: | ---: | ---: | ---: | ---: |
| hold, random increments | 1 | 10 000 | 5,657,373 | 566 | 877 |
| hold, random increments | 10 | 10 000 | 8,385,039 | 839 | 1,275 |
| hold, random increments | 100 | 10 000 | 11,739,314 | 1,174 | 1,723 |
| hold, random increments | 1 000 | 10 000 | 14,018,180 | 1,402 | 2,060 |
| hold, random increments | 10 000 | 10 000 | 16,367,909 | 1,637 | 2,431 |
| hold, constant increment (ties) | 100 | 10 000 | 10,944,199 | 1,094 | 1,631 |
| hold, constant increment (ties) | 10 000 | 10 000 | 15,103,351 | 1,510 | 2,189 |
| fan-out of messages (ties, 3 classes) | 100 | 9 999 | 6,101,494 | 610 | 973 |
| fan-out of messages (ties, 3 classes) | 1 000 | 10 010 | 7,252,047 | 724 | 1,161 |
| fan-out of messages (ties, 3 classes) | 10 000 | 10 001 | 8,469,729 | 847 | 1,409 |
| cancellation, 20 machines | about 40 live | 10 000 | 25,369,355 | 2,537 | 3,515 |
| cancellation, 1 000 machines | about 2 000 live | 10 000 | 36,622,637 | 3,662 | 5,062 |

"Pending" is the number of events in the queue (for the cancellation
workload, live events; stale cancelled entries come on top). The hold model
keeps it exactly constant; the fan-out holds one tick, then the lanes'
messages; the machines of the cancellation workload each have a cycle and a
failure timer pending, and about 1 race in 11 ends in a failure (the other
10 cancel a failure timer that is on average ten cycles away).

**What the numbers show.**

- *Cost of the queue's size.* Messages alone (no timers, no cancellation)
  cost 610, 724 and 847 instructions per event at 100, 1 000 and 10 000
  pending: about 120 more per tenfold increase, which is the binary heap's
  log n. Timer events cost 1,174, 1,402 and 1,637 over the same range: about
  230 more per tenfold increase, twice as much, because every timer is also
  inserted into and removed from the engine's map of pending timers. From 1
  to 10 000 pending, a hold event costs 2.9 times as many instructions
  (566 to 1,637); the cache-model cycles grow 2.8 times (877 to 2,431), so at
  these sizes the extra misses add little beyond the extra instructions.
  The reference models have one or two pending events per component
  (a few tens), the range of the first three rows.
- *Timers cost more than messages.* At 100 pending, a hold event costs 1,174
  instructions and a message of the fan-out 610. The hold component also
  draws a random number per event, so the difference is an upper bound for
  the timer bookkeeping. This is the part of the loop that ADR-0018
  deviation 3 is about.
- *Ties are not a problem.* All pending events due at the same instant (the
  constant-increment hold, the fan-out) cost no more per event than the same
  sizes with random times: 1,094 against 1,174 at 100 pending, 1,510
  against 1,637 at 10 000. The `(time, priority, sequence)` key needs no
  extra comparisons that show up here.
- *Cancellation.* An event of the cancellation workload costs 2,537
  instructions with 20 machines and 3,662 with 1 000, 2.2 and 2.6 times a
  hold event at a comparable queue size (1,174 at 100, 1,402 at 1 000
  pending). The figures cannot be split into cancellation and the rest: such
  an event also draws two exponential samples (a logarithm each) and
  schedules two timers where a hold event draws one number and schedules
  one. The 1 000-machine run has 466 thousand last-level cache hits against
  144 for the 20-machine run, so the larger queue, with its stale entries (compaction keeps them below
  half of the heap), does cost cache misses: 5,062 estimated cycles per
  event against 3,515.
- *Allowing for the per-event budget.* Spec §9's initial target of 1 M
  events per second per core is 3 000 cycles per event at 3 GHz. The
  estimated cycles of the hold and fan-out rows are below that, those of the
  cancellation rows above it, with components that do almost nothing and no
  item handling. This is a pointer for M13 on the reference hardware, not a
  finding.

**Generation numbers (ADR-0018 deviation 3).** The benchmark cannot answer
whether generation numbers beat the set of cancelled sequence numbers: both
are lazy deletion, and comparing them needs a second queue implementation in
`taktsim-core`, which this change leaves out of scope. What the numbers do
show is that timers carry a cost that has nothing to do with cancelling:
timer events cost 1.9 times message events at 100 pending (above), although
the timers of the hold model are never cancelled, and a timer that fires
never touches the set. What every timer pays, and a message does not, is the
engine's map of pending timers. A slab of timer slots with generation
numbers would replace that map and the set together, and would also make
cancellation constant-time. **Recommendation:** try it. It is a contained change inside
`taktsim-core`'s queue, and these benchmarks measure it as they are: the
hold, tied-hold and cancellation rows should fall, and the fan-out rows
(messages) should not move; the gate shows both in the pull request. It
changes the meaning of `EventId` (today the sequence number, which the trace
also carries), so it needs the owner's decision, and an ADR amendment if
`EventId` changes. A second experiment of the same size, a heap of small
`(key, slot)` entries with the payload in a slab, would test whether moving
whole events (several tens of bytes each) during sifting matters; it can
be compared with the same rows.

**What this does not show.** (1) The plan above names a 4-ary heap, a
pairing heap (Q2) and, if Q1 exceeds 30 % of the event loop, a calendar
queue (Q3); none of them exists, so only Q1 as built is measured. (2) The
workloads separate the queue from the rest of the loop only by differences
between rows, not as a share of the loop: the CI artifact holds totals, so
the "30 %" test cannot be applied. At the sizes of the reference models the
size-dependent part of a hold event (the rows at 10 and 100 against 1
pending) is 32 % and 52 % of the event, but part of that is the timer map,
and components of real models do more work per event than these. (3) These
are instruction counts and a cache model on one runner; wall-clock time on
the reference hardware (`wall_queue`, run locally; ADR-0010) is not
measured here. The structure therefore stays provisional.
