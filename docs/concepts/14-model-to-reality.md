# 14. From model to reality

<p class="ts-lede">A model is only as good as the data in it and the checks it passed. Measure, fit, then compare the model with the real line before you trust it.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- What data a model of a line needs, and where it comes from.
- How to fit a distribution to measured times, and how to tell a good fit
  from a bad one.
- How to validate a model against the real line, and what taktsim's
  calibration will add.

</div>

**Builds on:** [4. Variability](04-variability.md),
[13. Simulation you can trust](13-trustworthy-results.md)

## Intuition

A weather forecast is checked against the weather. A forecaster who
predicted sun every day for a year and never looked out of the window would
not be trusted, however clever the method. A model of a line is a forecast
too: before anyone decides on a new buffer or a WIP cap from it, someone has
to check that it gets today's line right.

## The data a model needs

Every number on the earlier pages has a real counterpart on the shop floor:

- **Cycle times** per station, from machine logs, the manufacturing
  execution system (MES) or a stopwatch. Take them only from periods when
  the station was actually working; a cycle that includes a failure or a
  break is not a cycle time.
- **Failures and repairs**: when each stop began and ended, from the
  controller's state log or the maintenance system.
- **Setups, scrap and rework**: how often and how long, from the same logs
  and from quality records.
- **Output and WIP** over time: the numbers the model will be checked
  against.

taktsim reads such data from CSV files (ADR-0017); Parquet, OPC UA and MQTT
come later (spec §8). Adapters only read from production systems; nothing
ever writes to a controller.

## Fitting a distribution

The running example is welding. 300 measured welding times have a mean of
45.2 s and a CV of 0.61. (The data here are made up: they were drawn from a
lognormal with mean 45 s and sigma 0.6, so that we know the right answer.)

The simplest fit, the method of moments, uses just those two numbers: pick a
distribution and give it the same mean and, where it can, the same CV.
Three candidates:

- **Constant**: 45.2 s every time. Ignores variability.
- **Exponential** with mean 45.2 s. Its CV is always 1.
- **Lognormal** with mean 45.2 s and sigma 0.56, the value that gives a CV
  of 0.61.

<figure class="ts-figure">
<div class="ts-label">Figure 14.1</div>
<div class="ts-figure-title">The lognormal follows the data; the exponential does not</div>
<div class="ts-plot">
{{#include img/14-fit.svg}}
</div>
<figcaption>Histogram of the 300 measured welding times with the fitted lognormal (dashed) and exponential (dotted) curves. Both curves have the data's mean (the highlighted line), so the mean cannot tell them apart; the shape can. The exponential expects the most welds near 0 s, where there are almost none.</figcaption>
</figure>

One number for how well a distribution fits is the Kolmogorov–Smirnov
distance. For every value, compare the share of measured times below it
with the share the distribution predicts; the distance is the largest gap.
0 is a perfect fit.

## Validating the model

A good fit is not yet the test. The test is whether the model, with the
fitted times, behaves like the real line. Here the "real" line is line A
with the true welding times, watched for 20 shifts, and each model is
simulated 30 times:

{{#include tables/14-validation.md}}

The constant model is too optimistic by 7 parts per shift: it leaves out
welding's variability, so the two stations wait for each other less than
they really do. The exponential model is too
pessimistic by 29: it invents far more variability than there is. Only the
lognormal model agrees with the observed output within the confidence
intervals. Both sides need intervals, because the 20 observed shifts are a
sample too.

Agreement on one number is necessary, not sufficient. Check several:
output, but also how often each station is starved or blocked, and how full
the buffers are. And check on data the model was not fitted to.

## Calibration in taktsim

Fitting each input by hand works for a two-station line. For a real plant,
taktsim's calibration (spec §6, the v0.3 tier) will close the loop
automatically. It replays the observed period with many seeds and scores
the gap for each metric. It then adjusts the parameters you declared free,
within their bounds, and stops when every metric is within its threshold or
the budget is spent. The result is a report: the fitted parameters, the
error before and after, the runs and seeds, a check against held-out data,
and a verdict of calibrated, partially calibrated or rejected. The model
itself is never silently changed; calibration returns a new version.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Fit the mean and the variability, not just the mean. Then compare the
model with the real line on several numbers, each with its confidence
interval, before you use it to decide anything.

</div>

## Try it yourself

- Take 30 of your own measured times of a repeated task and compute their
  mean and CV. Which of the three candidates would you choose?
- In the table, the constant model is closer to the observed output than
  the exponential one, yet both fail. Which would mislead you more if you
  used it to size a buffer? (The constant model: it says no buffer is
  needed.)

## Further reading

- Averill M. Law, *Simulation Modeling and Analysis*, chapter 5 ("Building
  Valid, Credible, and Appropriately Detailed Simulation Models") and
  chapter 6 ("Selecting Input Probability Distributions").
- Spec §6, "Calibration and self-optimization", and
  [ADR-0017](../adr/0017-csv-adapter-and-observation-table.md), the CSV
  adapter.

This is the end of the learning path. The [glossary](glossary.md) collects
every term, with the page that introduces it.
