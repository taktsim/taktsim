# 1. What a production system is

<p class="ts-lede">A production line is a chain of stations with storage in between. Four numbers tell you how well it works.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- The parts of a line: stations, buffers, a source and a sink.
- The four numbers that describe it: throughput, work in progress, cycle time and lead time.
- The example line that runs through every page of this path.

</div>

**Builds on:** nothing yet; this is the start.

## Intuition

Picture a sandwich counter with two people. One slices the bread, the other
fills it. Between them is a tray where sliced bread can wait. Bread comes
from the shelf behind them, and finished sandwiches go out over the counter.

That counter is a production line. Each person is a
[station](glossary.md#station): a place where work is done to one item at a
time. The tray is a [buffer](glossary.md#buffer): storage where items wait
between stations, with a limited number of places. The shelf is the
[source](glossary.md#source), where items enter, and the counter is the
[sink](glossary.md#sink), where finished items leave.

To judge the counter, you would ask four things:

- How many sandwiches go out per hour? That is the
  [throughput](glossary.md#throughput).
- How many sandwiches are being made right now, sliced but not yet handed
  over? That is the [work in progress](glossary.md#work-in-progress), or
  WIP.
- How long does each person need per sandwich? That is the station's
  [cycle time](glossary.md#cycle-time).
- How long does one sandwich take from the shelf to the counter, waiting
  included? That is the [lead time](glossary.md#lead-time).

Throughput is what the customer gets. WIP and lead time are what the line
costs: money tied up in half-finished parts, floor space, and customers
waiting. A line that looks good on throughput can be bad on the other two.

> **A word on "cycle time".** In this guide and in taktsim's model files,
> cycle time is the time a station needs per part. Some textbooks, such as
> *Factory Physics*, use "cycle time" for the whole time a part spends in
> the line; we call that lead time.

## The running example

The same line, called line A, runs through every page and grows a little
each time. It makes steel brackets. A cutting station cuts a blank in
42 seconds, a welding station welds it in 45 seconds, and a buffer with 20
places sits between them. For now nothing is random: every cut takes exactly
42 s, every weld exactly 45 s, and raw material never runs out.

<figure class="ts-figure">
<div class="ts-label">Figure 1.1</div>
<div class="ts-figure-title">Line A: two stations and a buffer</div>
<div class="ts-plot">
{{#include img/01-line.svg}}
</div>
<figcaption>Parts flow from left to right; the number under each station is its cycle time.</figcaption>
</figure>

In taktsim, line A is a short text file:

<div class="ts-panel">
<span class="ts-label">Model · planned for v0.1</span>

```yaml
format: "1.0"
model:
  name: line-a
  time_unit: s
  seed: 42
experiment:
  type: terminating
  horizon: 8h
components:
  - id: cut
    type: Machine
    cycle_time: 42s
  - id: buf1
    type: Buffer
    capacity: 20 parts
  - id: weld
    type: Machine
    cycle_time: 45s
flow:
  - [source, cut, buf1, weld, sink]
observe:
  - throughput(sink)
  - wip(buf1)
```

</div>

`experiment` says what to simulate: one 8-hour shift (`horizon: 8h`) that
starts empty (`terminating`). `observe` names the numbers to report.

## The numbers

One simulated shift of line A, hour by hour:

{{#include tables/01-shift.md}}

The first part leaves after 87 s (42 s cutting plus 45 s welding). From then
on one part leaves every 45 s: 80 per hour, 639 in the shift. But WIP keeps
growing. Cutting finishes a part every 42 s, welding takes one every 45 s,
so the buffer gains a part every few minutes until it is full.

<figure class="ts-figure">
<div class="ts-label">Figure 1.2</div>
<div class="ts-figure-title">Work piles up in front of the slower station</div>
<div class="ts-plot">
{{#include img/01-wip.svg}}
</div>
<figcaption>Each column is 10 minutes; the lit steps are the parts in the line. After three and a half hours the buffer is full and the line holds 22 parts.</figcaption>
</figure>

Once the buffer is full, cutting has to wait for space, and WIP stays at
22: 20 in the buffer and one in each station. Throughput did not change at
all. Lead time did: the first part needed 87 s, a part that enters a full
line waits behind 21 others and needs 16.5 minutes.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Judge a line by all four numbers. Throughput alone hides the parts piling
up inside, and those cost floor space, money and lead time.

</div>

## Try it yourself

Change the model and predict the result before you look it up on the next
pages:

- Set the cutting time to `45s`. Both stations now work at the same pace,
  so the buffer never fills: WIP stays at 2 and every part needs 90 s.
- Set the buffer to `5 parts`. Output stays at 639; WIP stops growing at 7
  and lead time at about 5 minutes. The extra 15 places in the original line
  only stored parts.

## Further reading

- Wallace J. Hopp and Mark L. Spearman, *Factory Physics*, chapter 7,
  "Basic Factory Dynamics", for the same four measures and their names.
- Stanley B. Gershwin, *MIT 2.854 Introduction to Manufacturing Systems*,
  lecture notes on [MIT OpenCourseWare](https://ocw.mit.edu/courses/2-854-introduction-to-manufacturing-systems-fall-2016/).

**Next:** [2. Capacity and the bottleneck](02-bottleneck.md): why welding
alone decides the 639.
