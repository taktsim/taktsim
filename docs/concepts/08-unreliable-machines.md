# 8. Unreliable machines

<p class="ts-lede">Machines fail and get repaired. Two machines that are down for the same share of time can hurt a line very differently.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- MTBF, MTTR and availability.
- Why rare, long outages hurt more than frequent, short ones of the same
  availability.
- How failures add to a station's variability, and what buffers can and
  cannot do about them.

</div>

**Builds on:** [4. Variability](04-variability.md),
[6. Starving and blocking](06-starving-blocking.md), [7. Buffers](07-buffers.md)

## Intuition

Two coffee machines in an office are each out of order 5 % of the time. The
first jams for three minutes every hour; someone clears it and the queue
barely notices. The second breaks for half a day every two weeks; on that day
nobody gets coffee for hours. On paper they are equally reliable. In
practice the second one is the one people complain about.

## Failures and availability

A failure is a stop that nobody planned: a jam, a broken tool, an electrical
fault. Two averages describe a station's failures:

- [MTBF](glossary.md#mtbf), the mean time between failures: how long the
  station runs, on average, before it fails.
- [MTTR](glossary.md#mttr), the mean time to repair: how long, on average,
  it is down once it has failed.

Together they give the station's [availability](glossary.md#availability),
the share of time it is able to work:

<div class="ts-panel ts-rule">
<span class="ts-label">Availability</span>

$$A = \frac{\text{MTBF}}{\text{MTBF} + \text{MTTR}}$$

</div>

One hour between failures and three minutes of repair gives
*A* = 60 / 63 = 95.2 %. So do ten hours and thirty minutes. Availability
alone cannot tell the two apart.

## The running example

Line A again, variable as before, but now cutting also fails. With an
availability of 95.2 %, cutting's
[effective cycle time](glossary.md#effective-cycle-time), its cycle time
with the outages spread over it, is 42 s / 0.952 = 44.1 s. That is still
faster than welding on average, but only just.

<figure class="ts-figure">
<div class="ts-label">Figure 8.1</div>
<div class="ts-figure-title">Line A with failures at cutting</div>
<div class="ts-plot">
{{#include img/08-line.svg}}
</div>
<figcaption>The small mark on the cutting station stands for its failures; the buffer between the stations varies from 5 to 50 places on this page.</figcaption>
</figure>

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.5 }
    mtbf: { dist: exponential, mean: 1h }
    mttr: { dist: exponential, mean: 3min }
```

</div>

> **In taktsim.** For v0.1 the spec counts failures in calendar time: the
> clock to the next failure runs whether cutting is working or waiting.
> Failures that count only operating time come with the plant extensions
> (§4). The spec's own example writes `mtbf: 6h` without a distribution, and
> whether that means a fixed or a random interval is not settled yet, so
> this page names the distribution.

## The numbers

Both failure patterns, each with 5, 20 and 50 buffer places, simulated five
shifts at a time:

{{#include tables/08-failures.md}}

With frequent, short stops, 20 places recover almost everything: 630 parts
per shift against 640 without any failures. With rare, long stops, the same
20 places leave the line at 618, and even 50 places do not catch up. During
a 30-minute outage, welding empties a full buffer of 20 in 15 minutes and is
then starved for the rest of the repair.

<figure class="ts-figure ts-widget" data-widget="failures" data-config='{"mtbf": 10, "buffer": 20, "seed": 1}'>
<div class="ts-label">Figure 8.2 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">A long outage empties the buffer, then welding stops too</div>
<div class="ts-plot ts-static">
{{#include img/08-timeline.svg}}
</div>
<div class="ts-live"></div>
<figcaption>Machine states over a stretch of time with long failures at cutting. Cutting goes down (solid colour); the parts in the buffer last a few minutes; from the highlighted step on, welding is starved until cutting is repaired.</figcaption>
</figure>

## Why long outages hurt more

A failure is variability in one big piece. *Factory Physics* folds it into
an effective CV of the station's cycle time. With exponential repair times:

<div class="ts-panel ts-rule">
<span class="ts-label">Effective variability with failures</span>

$$c_e^2 = c_0^2 + 2A(1 - A)\,\frac{\text{MTTR}}{t_0}$$

</div>

Here $c_e$ is the effective CV, $c_0$ the CV without failures and $t_0$ the
cycle time. At the same
availability, the added term grows with the repair time. Three-minute
repairs raise cutting's CV from 0.53 to 0.82; thirty-minute repairs raise
it to 2.04. [Page 5](05-utilization.md) showed what higher variability
costs.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Availability is not enough: ask how long the outages are. A buffer covers
an outage only if the next station needs longer to empty it than the
repair takes. For longer outages, shorten the repairs.

</div>

## Try it yourself

In figure 8.2, slide MTBF from 10 hours down to 15 minutes; MTTR follows so
that availability stays at 95.2 %. Watch the long red stretches turn into
many short ones, and the parts per shift rise. Then, at an MTBF of 10 hours,
raise the buffer and see how little it helps.

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 8, on preemptive outages
  and effective variability.
- Gershwin, *Manufacturing Systems Engineering*, chapter 3, on two-machine
  lines with unreliable machines.

**Next:** [9. Longer lines](09-longer-lines.md): five stations, and a
bottleneck that moves.
