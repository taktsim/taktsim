# 13. Simulation you can trust

<p class="ts-lede">A simulation with random times gives a different answer on every run. One run is an anecdote; the answer is a mean with a confidence interval, from runs that have left their start-up behind.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- Replications, and how to read a confidence interval.
- How many runs are enough, and how taktsim decides.
- Warm-up: why a line that starts empty misleads, and how to cut the start
  off.

</div>

**Builds on:** [4. Variability](04-variability.md),
[8. Unreliable machines](08-unreliable-machines.md); every page with a ±

## Intuition

Throw a die once and you learn little about its average. Throw it a hundred
times, and the average of your throws is close to 3.5, and you can say how
close. A simulation run with random cycle times and failures is one throw.
Every table on these pages averages many throws, and every ± says how close
that average is likely to be to the true one.

## Replications

A [replication](glossary.md#replication) is one independent run of the same
model with a different random seed. In taktsim, the same model, seed and
engine version always give the same result, byte for byte, so a replication
is fully defined by its seed and can be repeated.

The running example is the line from page 8, with short failures at cutting
and 20 buffer places, simulated one shift at a time:

{{#include tables/13-replications.md}}

<figure class="ts-figure ts-widget" data-widget="replications" data-config='{"n": 100}'>
<div class="ts-label">Figure 13.1 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">One run is one draw; the confidence interval is about the mean</div>
<div class="ts-plot ts-static">
{{#include img/13-spread.svg}}
</div>
<div class="ts-live"></div>
<figcaption>Parts per shift over many runs; the bracket above the bars is the 95 % confidence interval of the mean, the highlighted line is the mean.</figcaption>
</figure>

Single shifts range from 587 to 664 parts. The
[confidence interval](glossary.md#confidence-interval) is much narrower:
after 100 runs, the mean is 624 ± 2.8. It says where the long-run average
shift lies, not where the next shift will land. Read "95 %" as a statement
about the method: if you repeated the whole experiment many times, about
95 of 100 intervals would contain the true mean.

The table also shows how that can go wrong. Three runs give 606.7 ± 16.0,
an interval that ends below 623 and so misses the mean that 100 runs find.
With so few runs, even the spread is badly estimated, and the first three
seeds happened to be low.

## How many runs

The half-width shrinks with the square root of the number of runs: halving
it takes four times as many runs. So taktsim asks for the precision you
need rather than for a number of runs:

<div class="ts-panel">
<span class="ts-label">Model · planned for v0.1</span>

```yaml
experiment:
  type: terminating
  horizon: 8h
  precision: { throughput(sink): 2% }
```

</div>

taktsim adds runs one at a time until the half-width is within 2 % of the
mean or a budget is spent (spec §6). It always starts with at least ten
runs (ADR-0014), for the reason the three-run row shows. The result states
the half-width it reached, the number of runs and their seeds, and whether
the precision was met (spec §5). From v0.4 on, when two designs are
compared, both get the same random numbers, so that the difference between
them is not just noise (spec §6, common random numbers).

## Warm-up

A shift starts with whatever the line holds. The examples on most pages
start empty, which is fine for a [terminating](glossary.md#terminating)
experiment such as one shift. A [steady-state](glossary.md#steady-state)
experiment asks about the line in the long run, and there the empty start
is a bias: for a while the line holds less WIP, and parts flow faster, than
it ever will again. That stretch is the [warm-up](glossary.md#warm-up), and
the statistics must leave it out.

<figure class="ts-figure">
<div class="ts-label">Figure 13.2</div>
<div class="ts-figure-title">An empty line is not a typical line: discard the warm-up</div>
<div class="ts-plot">
{{#include img/13-warmup.svg}}
</div>
<figcaption>Average WIP of 20 runs of the variable line A with 20 buffer places, starting empty. The shaded stretch is what to discard: up to the dashed line, where the curve visibly settles. The highlighted point is where MSER-5 ends the warm-up in the median run, while WIP is still climbing.</figcaption>
</figure>

{{#include tables/13-warmup.md}}

MSER-5, taktsim's default warm-up detector (spec §6), looks for the cut-off
point after which the remaining data vary least around their mean. Here it
ends the warm-up after 1.7 hours in the median run, before the curve has
settled, and in one run only after 13 hours. That is a known weakness:
ADR-0014 measured that MSER-5 often misses on output that changes slowly
from one moment to the next, such as WIP. Look at the curve yourself, and
when in doubt, discard more.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Never trust one run. Report a mean with its confidence interval, from at
least ten replications. For long-run questions, discard a warm-up, and look
at the curve to check it.

</div>

## Try it yourself

In figure 13.1, start at 2 replications and slide up to 100. The bracket
shrinks fast at first and slowly later. Press "New seeds" a few times at 5
replications and watch the mean jump around; at 100 it hardly moves.

## Further reading

- Averill M. Law, *Simulation Modeling and Analysis*, chapter 9, "Output
  Data Analysis for a Single System".
- K. Preston White, Jr., "An effective truncation heuristic for bias
  reduction in simulation output", *Simulation* 69 (1997), on MSER.
- [ADR-0014](../adr/0014-output-analysis.md), taktsim's choices for output
  analysis, with the measurements behind them.

**Next:** [14. From model to reality](14-model-to-reality.md): is the model
right about the real line?
