# 5. Utilization and waiting

<p class="ts-lede">A station that is busy almost all the time makes parts wait a long time. Close to 100 %, waiting explodes.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- Why waiting grows slowly at first and then very fast as utilization rises.
- Kingman's formula, which says how long parts wait in front of a station.
- Why a busy bottleneck always has a queue in front of it.

</div>

**Builds on:** [2. Capacity and the bottleneck](02-bottleneck.md)
(utilization), [4. Variability](04-variability.md) (CV)

## Intuition

A supermarket checkout that is busy half the time rarely has a queue: when
two customers arrive close together, the second waits a little, and the
cashier soon catches up. At 95 % busy there is no time to catch up. Every
slow customer and every little burst of arrivals adds to a queue that has
almost no chance to empty. The same happens on a motorway: at 50 % of
capacity traffic flows; at 95 %, one driver braking starts a jam that lasts
an hour.

## The running example

Zoom in on welding. Parts come from cutting at irregular intervals and wait
in the buffer in front of welding. To study this one effect, treat welding
on its own: parts arrive at random, on average every *a* seconds, and
welding needs 45 s per part. Its [utilization](glossary.md#utilization) is
45 / *a*: arrivals every 50 s make it 90 % busy.

> **In taktsim.** This model needs a source that creates parts at random
> intervals. The spec gives the Source component an arrival distribution
> (§4) but does not show its syntax yet, so this page describes the model in
> words. It arrives with the v0.1 component library (M5).

## The numbers

The simulated wait in the queue in front of welding, with random arrivals
(exponential intervals, CV 1) and two levels of process-time variability:

{{#include tables/05-waiting.md}}

<figure class="ts-figure ts-widget" data-widget="utilization" data-config='{"process_mean": 45, "u": 0.85, "cv": 1.0}'>
<div class="ts-label">Figure 5.1 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">Close to 100 % busy, waiting explodes</div>
<div class="ts-plot ts-static">
{{#include img/05-hockey.svg}}
</div>
<div class="ts-live"></div>
<figcaption>Mean wait in queue against utilization, for a 45 s process time. Dashed: Kingman's formula, for process-time CV 1 and 0.5 (labelled on the curves) or, interactively, for the CV you choose. Squares: simulated means with their 95 % confidence intervals. The highlighted point: at 90 % busy and CV 1, a part waits nine times its process time.</figcaption>
</figure>

From 50 % to 80 % utilization the wait grows from under one minute to three.
From 80 % to 90 % it more than doubles, and from 90 % to 95 % it doubles
again. The confidence intervals widen too: a station that busy has rare but
very long queues, and even a long run sees only a few of them.

## Kingman's formula

The curves in the figure are
[Kingman's formula](glossary.md#kingmans-formula), an approximation for the
mean wait in queue at a single station:

<div class="ts-panel ts-rule">
<span class="ts-label">Kingman's formula</span>

$$W \approx \frac{c_a^2 + c_s^2}{2} \times \frac{u}{1 - u} \times t$$

</div>

Here $W$ is the mean wait in queue, $c_a$ the CV of the time between
arrivals, $c_s$ the CV of the
process time, $u$ the utilization and $t$ the mean process time. It is
three factors multiplied together, which is why Hopp and Spearman call it
the VUT equation:

- **V, variability:** $(c_a^2 + c_s^2) / 2$. No variability, no waiting.
  Halving the process-time CV from 1 to 0.5 lowers the wait by more than a
  third.
- **U, utilization:** $u / (1 - u)$. This is the hockey stick. At 50 % it is
  1, at 90 % it is 9, at 95 % it is 19.
- **T, time:** the process time itself. A slow station makes parts wait
  longer in absolute terms.

For exponential arrivals and process times (the "M/M/1" queue of queueing
theory), the formula is exact; the simulation reproduces it in the first
seven rows of the table. For other cases it is an approximation that is
good at high utilization, which is where it matters.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Do not plan any station close to 100 % busy: the last few percent of
utilization cause most of the waiting. And since the bottleneck is the
busiest station, it always has the longest queue in front of it.

</div>

## Try it yourself

In figure 5.1, move the utilization slider from 80 % to 95 % and watch the
simulated point climb; then lower the CV and see the whole curve flatten.
Or use the table: at which utilization does the wait in front of welding
first exceed ten process times?

## Further reading

- J. F. C. Kingman, "The single server queue in heavy traffic",
  *Proceedings of the Cambridge Philosophical Society* 57 (1961).
- Hopp and Spearman, *Factory Physics*, chapter 8, "Variability Basics"
  (the VUT equation).
- Gershwin, *MIT 2.854*, lecture notes on single-stage queues.

**Next:** [6. Starving and blocking](06-starving-blocking.md): what happens
when two variable stations share a small buffer.
