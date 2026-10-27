# 4. Variability

<p class="ts-lede">Real process times are not fixed. How much they spread matters as much as their average.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- Where variability in a line comes from.
- How to measure it with one number, the coefficient of variation.
- How the running example changes from here on.

</div>

**Builds on:** [2. Capacity and the bottleneck](02-bottleneck.md),
[3. Little's Law](03-littles-law.md)

## Intuition

Your commute takes 30 minutes on average. Some days it is 25, some days 60.
If you plan with 30 you are late about half the time, and the 60-minute days
are the ones people remember. The average was never the whole story; the
spread around it decides how much slack you need.

A station's cycle time works the same way. Pages 1 to 3 assumed every cut
takes exactly 42 s. On a real line, one cut takes 35 s and the next 70,
because an operator loads the part by hand, the material varies, or a tool
needs adjusting.

## Where it comes from

[Variability](glossary.md#variability) enters a line in a few ways:

- **Process times.** Manual work, differences between parts and materials,
  small adjustments. This page.
- **Arrivals.** Orders, deliveries and parts from upstream do not come at
  even intervals. [Page 5](05-utilization.md).
- **Failures.** A machine stops and has to be repaired.
  [Page 8](08-unreliable-machines.md).
- **Setups.** Changing over to another product takes time that is spent
  only now and then. [Page 11](11-batching-setups.md).
- **Quality.** Some parts are reworked and need the station twice.
  [Page 12](12-scrap-rework.md).

## Measuring it

The standard deviation says how far times spread, but it depends on the
scale: a spread of 5 s is a lot for a 10-second step and nothing for an
hour-long one. Dividing by the mean fixes that. The result is the
[coefficient of variation](glossary.md#coefficient-of-variation):

<div class="ts-panel ts-rule">
<span class="ts-label">Coefficient of variation</span>

**CV = standard deviation / mean**

</div>

A CV of 0 means no variability. Hopp and Spearman call a CV below 0.75 low,
0.75 to 1.33 moderate, and above 1.33 high. An exponential distribution, the
textbook model of "completely random", has a CV of exactly 1.

<figure class="ts-figure">
<div class="ts-label">Figure 4.1</div>
<div class="ts-figure-title">Same mean, very different spread</div>
<div class="ts-plot">
{{#include img/04-histograms.svg}}
</div>
<figcaption>4,000 cutting times with a mean of 42 s each; the highlighted line is welding's 45 s, which about a third of the widely spread cuts exceed.</figcaption>
</figure>

{{#include tables/04-cv.md}}

The cutting times here follow a lognormal distribution, a common choice for
process times: always positive, with a long tail of slow cases. Its shape
parameter sigma sets the spread; for small values the CV is about equal to
sigma, and in general CV = √(e^(sigma²) − 1).

## The running example

From now on, both stations of line A have variable cycle times: lognormal,
with the same means as before and sigma 0.5 (CV 0.53), as for manual
loading. In taktsim's model format a random time is a named distribution
with explicit parameters:

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
components:
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.5 }
  - id: buf1
    type: Buffer
    capacity: 20 parts
  - id: weld
    type: Machine
    cycle_time: { dist: lognormal, mean: 45s, sigma: 0.5 }
```

</div>

With sigma 0.08, 18 % of the cuts in the table are slower than welding's
45 s, yet as [page 7](07-buffers.md) shows, a line with such cuts and a
fixed welding time loses almost nothing. Whether
variability costs output depends on where it is and what protects against
it. The next three pages work that out.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Describe every time in a line by two numbers: its mean and its CV. The mean
decides the bottleneck; the CV decides how much waiting the line will see.

</div>

## Try it yourself

- Take 20 measured cycle times of any repeated task, your own included:
  divide the standard deviation by the mean. Most manual tasks land between
  0.2 and 0.6.
- In the model, replace `sigma: 0.5` with `{ dist: exponential, mean: 42s }`.
  The CV goes from 0.53 to 1; the next pages show what that does.

## Further reading

- Hopp and Spearman, *Factory Physics*, chapter 8, "Variability Basics".
- Averill M. Law, *Simulation Modeling and Analysis*, chapter 6, on choosing
  distributions such as the lognormal.

**Next:** [5. Utilization and waiting](05-utilization.md): what variability
does to a single station.
