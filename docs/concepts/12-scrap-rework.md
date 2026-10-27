# 12. Scrap and rework

<p class="ts-lede">Not every part comes out right. Scrapped parts are lost, reworked parts come back for a second pass, and both use capacity that good parts needed.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- What inspection, yield, scrap and rework are.
- Why rework is not free even when the part is saved in the end.
- How to estimate the good output of a line from its scrap and rework
  rates.

</div>

**Builds on:** [2. Capacity and the bottleneck](02-bottleneck.md),
[5. Utilization and waiting](05-utilization.md)

## Intuition

A bakery bakes 100 loaves an hour. Two come out burnt and go in the bin;
five come out pale and go back into the oven for another ten minutes. The
burnt ones are plainly lost. The pale ones are saved, but each took oven
space twice, space that five new loaves could have used. At the end of the
day the bakery sold fewer loaves than its oven could bake, although nobody
was idle.

## Words

- An [inspection](glossary.md#inspection) checks each part and sorts it:
  good, rework or scrap.
- [Scrap](glossary.md#scrap) is a part that cannot be saved; it leaves the
  line.
- [Rework](glossary.md#rework) is a part that goes back to an earlier
  station to be fixed, and then through the inspection again.
- [Yield](glossary.md#yield) is the share of parts that end up good.

## The running example

Line A from [page 7](07-buffers.md), variable, with five buffer places, and
now an inspection of 40 s after welding. A part found faulty is either
scrapped or sent back to welding, where it waits in a queue of its own
that welding serves before new parts.

<figure class="ts-figure">
<div class="ts-label">Figure 12.1</div>
<div class="ts-figure-title">Line C: welding with an inspection after it</div>
<div class="ts-plot">
{{#include img/12-line.svg}}
</div>
<figcaption>The loop above the line sends reworked parts back to welding; the arrow below the inspection is scrap leaving the line.</figcaption>
</figure>

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.2</span>

```yaml
  - id: qc
    type: Inspection
    inspection_time: 40s
    scrap_rate: 0.02
    rework_rate: 0.05
    rework_to: weld
```

</div>

This is the spec's Inspection component (§4), which arrives with the v0.2
tier.

## The numbers

{{#include tables/12-quality.md}}

<figure class="ts-figure">
<div class="ts-label">Figure 12.2</div>
<div class="ts-figure-title">Every scrapped or reworked part takes welding time from a good one</div>
<div class="ts-plot">
{{#include img/12-bars.svg}}
</div>
<figcaption>What welding's 640 possible operations per shift turn into, for each case: good parts, rework passes, scrapped parts and time spent waiting. The highlighted row is 2 % scrap and 5 % rework.</figcaption>
</figure>

Welding is the bottleneck and works 98 to 99 % of the time in every case;
what changes is what it works on. With 2 % scrap and 5 % rework, 7 % of its
operations do not become good parts: 2 % are welds on parts that are then
thrown away, 5 % are second welds on parts that were already welded once.
The good output falls from 626 to 587 parts per shift.

The table's sixth column is a simple formula for that, which the simulation
follows within a few parts: when the reworked station is the bottleneck,
each pass is good, rework or scrap, so

<div class="ts-panel ts-rule">
<span class="ts-label">Good output with scrap and rework at the bottleneck</span>

**good parts ≈ bottleneck capacity × (1 − scrap rate − rework rate)**

</div>

The simulation is a little below the formula because variability costs
about 14 parts per shift on its own, as the first row shows.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

At the bottleneck, a rework pass costs as much capacity as a new part, and
a scrapped part costs every pass it took. Count both when you ask what
quality costs.

</div>

Where a part is inspected matters too: scrap found right after the
bottleneck has cost one bottleneck pass; scrap found at the end of a long
line has cost every station's time.

## Try it yourself

- Set the rework rate to 10 % and the scrap rate to 0. Good output drops by
  about 10 % even though no part is lost: rework is pure capacity.
- Set the scrap rate to 5 % and rework to 0. The formula gives
  640 × 0.95 = 608 good parts per shift; expect the simulation a little
  below, as in the table.
- Rework passes go through welding and inspection alike, so they never
  make the 40-second inspection the bottleneck here. Which station would
  carry the extra load if `rework_to` sent parts back to cutting instead?

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 12, on quality and its
  effect on flow.
- Gershwin, *MIT 2.854*, lecture notes on quality and inspection in lines.

**Next:** [13. Simulation you can trust](13-trustworthy-results.md): how
many runs, and what the ± means.
