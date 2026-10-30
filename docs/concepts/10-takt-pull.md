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

$$\text{takt time} = \frac{\text{available time}}{\text{demand}}$$

</div>

If customers want 600 parts per 8-hour shift, the takt time is
28,800 s / 600 = 48 s: one part must leave the line every 48 seconds.
Cycle time is what a station needs; takt time is what the customer needs.
Every station's effective cycle time must stay below the takt time, with
some margin for variability. In line B, welding at 45 s fits. Painting with
its failures does not: at an effective 48.4 s
([page 9](09-longer-lines.md)), it cannot meet this demand however the line
is run. This page therefore uses line B without failures.

## Push and pull

Apart from the examples with random arrivals on
[pages 3](03-littles-law.md) and [5](05-utilization.md), the lines so far
never ran out of raw parts: cutting could always start a new one. That is neither push nor
pull.

- **Push** releases work by a plan, whatever is inside. Here the plan is one
  part every 48 s on average, with random deviations as in real deliveries,
  like the arrivals on [page 5](05-utilization.md). The line cannot make
  more than it is given.
- **Pull** releases work only when something leaves.
  [Kanban](glossary.md#kanban) does that between each pair of stations. A
  card travels with each part. When the downstream station takes the part,
  the card goes back upstream and allows one new part. With N cards in this
  loop, at most N parts sit between the two stations, much like a buffer
  with N places: line B's finite buffers on [page 9](09-longer-lines.md)
  already behaved this way. [CONWIP](glossary.md#conwip) ("constant work in
  process") does it for the whole line: a new part enters only when a
  finished part leaves, so the WIP never exceeds a fixed cap.

Both pull systems are WIP limits. In taktsim, a CONWIP cap is one line of
the model:

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.2</span>

```yaml
control:
  release: { policy: conwip, wip_cap: 12 parts }
```

</div>

> **In taktsim.** Release control comes with the v0.2 tier (spec §11). The
> spec (§4) names Kanban card counts but does not give them a syntax yet.

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
stations alone. Each extra part adds output, up to about 12, where the line
meets the demand of 600. These are averages over many shifts. Single shifts
vary more than the ± suggests, so at a cap of 12 some shifts will fall
short of 600
([reading simulated numbers](03-littles-law.md#reading-simulated-numbers)).
Beyond that cap, output creeps towards the bottleneck's 640 while lead time
keeps growing, as
[Little's Law](03-littles-law.md) says it must: lead time = WIP / throughput.

The dashed and dotted curves are two reference cases from *Factory Physics*.
The best case is a line without variability. A part alone needs the raw
process time $T_0$ = 208 s, the sum of the cycle times, to get through, so
one part in the line gives one part every 208 s: 28,800 / 208 ≈ 138 per
shift. Each extra part adds another $1/T_0$, until the bottleneck rate
$r_b$, one part per 45 s, caps the output. The two meet at the
[critical WIP](glossary.md#critical-wip) $W_0$. Little's Law gives it:
WIP = throughput × lead time, so

$$W_0 = r_b \times T_0 = \frac{1 \text{ part}}{45 \text{ s}} \times 208 \text{ s} = 4.6 \text{ parts}$$

The seconds cancel, and parts remain.

The practical worst case is a balanced line, one whose stations are all
equally fast, with exponential times. With $w$ parts in the line its
throughput is

$$\text{TH} = \frac{w}{W_0 + w - 1} \times r_b$$

At a cap of 12, with $r_b$ = 640 parts per shift, that is
12 / 15.6 × 640 ≈ 492 parts per shift. A real line lies between the two
cases; line B lies closer to the best one.

Now push and pull, both meeting the demand:

{{#include tables/10-push-pull.md}}

Same output: push makes what it releases, and the cap was chosen to meet
the demand. (Push's 604.9 ± 5.1 is not more than the plan of 600 per shift:
random arrivals vary around the plan, and the range includes 600.) But pull needs half the WIP and half the lead time.
Push lets WIP drift up whenever arrivals bunch; the CONWIP cap holds it.

The cap of 12 is a ceiling, not the actual WIP. A place freed at the end of
the line is used only when cutting is free to start a new part; while
cutting is still working, the place stays empty. So the line holds 10.5
parts on average, and Little's Law must use that number. At 606.8 parts per
shift, one part leaves every 28,800 / 606.8 ≈ 47.5 s, so the lead time is
10.5 × 47.5 s ≈ 8.3 minutes, as the table shows. The cap of 12 would give
9.5.

## What a model should answer

People who run lines the Lean way tend to ask a simulation these questions:

- Can the line meet the takt time, and by what margin at each station?
- How much WIP does it need for that, and so what Kanban count or CONWIP
  cap?
- What lead time follows from that WIP?
- Which improvement, such as shorter setups ([page 11](11-batching-setups.md)),
  faster repairs or less scrap ([page 12](12-scrap-rework.md)), pays off
  most?
- How do the standard KPIs change, such as overall equipment
  effectiveness (OEE), the product of availability, performance (speed
  against the ideal cycle time) and quality (share of good parts)?

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Every station's effective cycle time must stay below takt. Then cap the
WIP at the smallest value that meets demand: more WIP adds lead time, not
output.

</div>

## Try it yourself

In figure 10.1, lower the cap from 12 to 8 and watch output drop below the
demand. Then raise it to 30: lead time more than doubles for about 30 more
parts per shift.

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 7 (best and practical worst
  case) and chapter 10, "Push and Pull Production Systems".
- Taiichi Ohno, *Toyota Production System: Beyond Large-Scale Production*.
- Mark L. Spearman, David L. Woodruff and Wallace J. Hopp, "CONWIP: a pull
  alternative to kanban", *International Journal of Production Research* 28
  (1990).

**Next:** [11. Batching and setups](11-batching-setups.md): the price of
changing over.
