# 2. Capacity and the bottleneck

<p class="ts-lede">A line can go no faster than its slowest station. Speeding up any other station changes nothing.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- What a station's capacity is and how to read it off its cycle time.
- Why the slowest station, the bottleneck, sets the output of the whole line.
- What utilization is, and why a station that is not the bottleneck waits.

</div>

**Builds on:** [1. What a production system is](01-production-systems.md)

## Intuition

A row of people passes buckets of water from a well to a fire. Most can pass
a bucket every two seconds; one person in the middle needs five. However fast
the others are, the fire gets one bucket every five seconds. Buckets pile up
in front of the slow person, and everyone after them stands idle between
buckets.

That slow person is the [bottleneck](glossary.md#bottleneck): the station
with the least capacity, which limits the whole line. Making anyone else
faster only makes the pile in front of the bottleneck grow faster.

## Capacity and utilization

A station's [capacity](glossary.md#capacity) is the most it can make per
unit of time when it never waits. With a cycle time of 45 s, that is
3600 / 45 = 80 parts per hour. Cutting in line A, at 42 s, could do about 86.

A station's [utilization](glossary.md#utilization) is the share of time it
spends working. If line A makes 80 parts per hour, cutting works
80 × 42 s = 56 minutes of every hour, so its utilization is 93 %. Welding
works all 60 minutes: 100 %.

## The running example

Line A from [page 1](01-production-systems.md), still without randomness:

<figure class="ts-figure">
<div class="ts-label">Figure 2.1</div>
<div class="ts-figure-title">Welding is line A's bottleneck</div>
<div class="ts-plot">
{{#include img/02-line.svg}}
</div>
<figcaption>Each station's cycle time and capacity per hour; the highlighted station has the least capacity.</figcaption>
</figure>

The model is the one from page 1. To try a different cutting time, change one
line:

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
  - id: cut
    type: Machine
    cycle_time: 50s   # was 42s
```

</div>

## The numbers

Simulate one shift for cutting times from 30 to 60 s, with welding fixed at
45 s:

{{#include tables/02-sweep.md}}

<figure class="ts-figure">
<div class="ts-label">Figure 2.2</div>
<div class="ts-figure-title">Output follows the slowest station</div>
<div class="ts-plot">
{{#include img/02-sweep.svg}}
</div>
<figcaption>Parts per shift against the cutting time; the simulated points lie on the dashed formula line, flat while welding is slower and falling once cutting is.</figcaption>
</figure>

While cutting is faster than welding, output stays at 639 parts: welding
decides. Making cutting faster, from 42 s down to 30 s, gains nothing; it
only lowers cutting's utilization from 96 % to 69 %. Once cutting is slower
than 45 s, it becomes the bottleneck, welding starts to wait, and output
falls to 28,800 s divided by the cutting time. The simulation is one part
below the formula in every case: the first part needs both stations before
anything comes out.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Output = 1 / (the largest cycle time). To make more, make the bottleneck
faster; time saved anywhere else is time spent waiting.

</div>

This holds exactly only without randomness. The next pages show what
randomness takes away from it, and why a station that is not the bottleneck
on average can still hold the line back.

## Try it yourself

- Set cutting to `45s`, the same as welding. Both are now bottlenecks, both
  are busy 100 % of the time, and the buffer stays empty.
- Set welding to `40s` and leave cutting at `42s`. Which station is the
  bottleneck now, and how many parts does a shift make? (Cutting, and 684:
  28,800 / 42 ≈ 686, less about two for the first part's way through.)

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 7: bottleneck rate and raw
  process time.
- Eliyahu M. Goldratt, *The Goal*: the same idea as a novel, from the
  shop floor.

**Next:** [3. Little's Law](03-littles-law.md): what ties the four numbers
together.
