# 10. Takt time and pull

<p class="ts-lede">A line should make what customers need, at the pace they need it, with as little work inside as possible. Takt time sets the pace; a WIP limit keeps the inside small.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- What takt time is and how it differs from cycle time.
- Push and pull, and the two common WIP limits: Kanban and CONWIP.
- How much WIP a line needs to meet its takt, and what Lean practitioners
  expect a model to answer.

</div>

**Builds on:** [3. Little's Law](03-littles-law.md),
[9. Longer lines](09-longer-lines.md)

## Intuition

A bakery can bake croissants all morning and hope they sell, or bake a new
tray whenever the shelf runs low. The first is push: production follows a
plan. The second is pull: production follows consumption. Toyota built its
production system on the second idea, inspired, the story goes, by how
American supermarkets refilled their shelves.

## Takt time

[Takt time](glossary.md#takt-time) is the pace that demand sets: the
available time divided by the number of parts customers want in it.

<div class="ts-panel ts-rule">
<span class="ts-label">Takt time</span>

**takt time = available time / demand**

</div>

If customers want 600 parts per 8-hour shift, the takt time is
28,800 s / 600 = 48 s: one part must leave the line every 48 seconds.
Cycle time is what a station needs; takt time is what the customer needs.
Every station's effective cycle time must stay below the takt time, with
some margin for variability. In line B, welding at 45 s fits. Painting with
its failures, at an effective 48.4 s ([page 9](09-longer-lines.md)), does
not: line B with those failures cannot meet this demand however it is run.
This page therefore uses line B without failures.

## Push and pull

- **Push** releases work into the line by a schedule, whatever is inside:
  here, raw parts arrive on average every 48 s, at random.
- **Pull** releases work only when something leaves.
  [Kanban](glossary.md#kanban) does that between each pair of stations: a
  fixed number of cards, each allowing one part, circulates in each loop.
  [CONWIP](glossary.md#conwip) ("constant work in process") does it for the
  whole line: a new part enters only when a finished part leaves, so the
  WIP never exceeds a fixed cap.

Both pull systems are WIP limits. The spec (§4) gives CONWIP a syntax; Kanban
card counts are named but their syntax is not yet specified.

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.2</span>

```yaml
control:
  release: { policy: conwip, wip_cap: 12 parts }
```

</div>

Release control is part of the v0.2 tier (spec §11).

## The numbers

Line B with unlimited buffers and a CONWIP cap from 1 to 40 parts:

{{#include tables/10-conwip.md}}

<figure class="ts-figure ts-widget" data-widget="conwip" data-config='{"cap": 12}'>
<div class="ts-label">Figure 10.1 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">A small WIP cap already gives most of the output</div>
<div class="ts-plot ts-static">
{{#include img/10-conwip.svg}}
</div>
<div class="ts-live"></div>
<figcaption>Parts per shift against the WIP cap; the thin line is the demand of 600. The highlighted point is the cap in focus, by default the smallest that meets the demand.</figcaption>
</figure>

With one part in the line, output is low: each part walks through five empty
stations alone. Each extra part adds output, until about 12, where the line
meets the demand of 600. Beyond that, output creeps towards the
bottleneck's 640 while lead time keeps growing, as
[Little's Law](03-littles-law.md) says it must: lead time = WIP / throughput.

The dashed and dotted curves are two reference cases from *Factory Physics*.
The best case is a line without variability: output grows by one part per
raw process time T₀ = 208 s (the sum of the cycle times) until it reaches
the bottleneck rate. The two meet at the critical WIP, here 208 / 45 = 4.6
parts. The practical worst case is a balanced line with exponential times.
A real line lies between the two; line B lies closer to the best case.

Now push and pull, both meeting the demand:

{{#include tables/10-push-pull.md}}

Same output, half the WIP and half the lead time. Push lets WIP drift up
whenever arrivals bunch; the CONWIP cap holds it.

## What a model should answer

People who run lines the Lean way tend to ask a simulation these questions:

- Can the line meet the takt time, and by what margin at each station?
- How much WIP does it need for that, and so what Kanban count or CONWIP
  cap?
- What lead time follows from that WIP?
- Which improvement, such as shorter setups ([page 11](11-batching-setups.md)),
  faster repairs or less scrap ([page 12](12-scrap-rework.md)), pays off
  most?
- How do the standard KPIs, such as overall equipment effectiveness (OEE),
  change?

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Every station's effective cycle time must stay below takt. Then cap the
WIP at the smallest value that meets demand: more WIP adds lead time, not
output.

</div>

## Try it yourself

In figure 10.1, lower the cap from 12 to 8 and watch output drop below the
demand; raise it to 30 and watch lead time more than double for about 30 parts more.

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 7 (best and practical worst
  case) and chapter 10, "Push and Pull Production Systems".
- Taiichi Ohno, *Toyota Production System: Beyond Large-Scale Production*.
- Mark L. Spearman, David L. Woodruff and Wallace J. Hopp, "CONWIP: a pull
  alternative to kanban", *International Journal of Production Research* 28
  (1990).

**Next:** [11. Batching and setups](11-batching-setups.md): the price of
changing over.
