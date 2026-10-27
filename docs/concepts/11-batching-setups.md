# 11. Batching and setups

<p class="ts-lede">Changing a station over to another product takes time. Large batches waste less of it, but make parts wait. The best batch size sits in between.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- What setups are and why they lead to batches.
- The trade-off between capacity lost to setups and time lost waiting for a
  batch.
- Why the smallest batch that keeps up is usually too small, and the
  largest is always too large.

</div>

**Builds on:** [3. Little's Law](03-littles-law.md),
[5. Utilization and waiting](05-utilization.md)

## Intuition

A copy shop prints flyers in red and in blue. Switching colours means
cleaning the press, which takes a while. Switch for every single flyer and
the press spends its day cleaning. Print a month of red before switching and
every blue customer waits weeks. Somewhere between "every flyer" and "every
month" is a batch size where customers wait least.

## Setups and batches

A [setup](glossary.md#setup) is the work of changing a station over from one
product or variant to another: changing a tool, a fixture or a program. While
it runs, the station makes nothing. To pay for a setup less often, parts of
the same variant are grouped into a [batch](glossary.md#batch) and processed
one after the other.

## The running example

Welding in line A now makes several variants of the bracket, and changing
from one variant to the next takes a 4-minute setup. Orders for single parts
arrive on average every 60 s, at random. Welding collects parts of a variant
until it has a batch, sets up, and welds the batch; the parts leave together.
Every batch is a change of variant, so every batch costs one setup.

The spec gives the setup a syntax:

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
  - id: weld
    type: Machine
    cycle_time: { dist: lognormal, mean: 45s, sigma: 0.5 }
    setup: { per_variant: 4min }
```

</div>

The batch size and the mix of variants belong to the Source component (spec
§4, "variant mix, batch size"), whose syntax is not yet specified; this page
describes them in words. Variants and setups by variant come with the v0.1
component library (M5).

## Capacity: the smallest batch that keeps up

Welding needs 45 s per part plus 240 s per batch. With batches of *b* parts,
its utilization is (45 + 240 / *b*) / 60. That has to stay below 1, so *b*
must be larger than 240 / (60 − 45) = 16. Below that, the queue grows
without limit.

## The numbers

{{#include tables/11-batching.md}}

<figure class="ts-figure ts-widget" data-widget="batching" data-config='{"batch": 22}'>
<div class="ts-label">Figure 11.1 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">Too small loses capacity, too large loses time</div>
<div class="ts-plot ts-static">
{{#include img/11-batching.svg}}
</div>
<div class="ts-live"></div>
<figcaption>Lead time against batch size: steep on the left, where setups use up almost all spare capacity; straight on the right, where parts mostly wait for their batch to fill. The highlighted point is the batch size in focus, by default the shortest lead time.</figcaption>
</figure>

At 17 parts per batch, welding is busy 99 % of the time and parts need over
an hour; the confidence interval is wide, because at that utilization the
queue swings a lot (page 5). The lead time is shortest, about 37 minutes,
around 20 to 22 parts. From there it rises in a straight line: each extra
part in a batch adds half an arrival interval of waiting for the batch to
fill and 45 s of welding the batch. The dashed line in the figure is that
part alone, without any queueing; on the right the simulation runs along it.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Choose batches somewhat larger than the minimum that keeps up with demand;
lead time is shortest there. Better still, shorten the setup: every minute
saved lowers the minimum batch, and with it the whole curve.

</div>

Shortening setups is a Lean discipline of its own, SMED ("single-minute
exchange of die"), developed by Shigeo Shingo at Toyota.

## Try it yourself

In figure 11.1, move the batch size from 17 to 80 and watch the U. Then
think through the effect of a 2-minute setup: the minimum batch falls from
16 to 8, and the lowest lead time with it.

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 9, on process and transfer
  batches.
- Shigeo Shingo, *A Revolution in Manufacturing: The SMED System*.

**Next:** [12. Scrap and rework](12-scrap-rework.md): when parts come out
wrong.
