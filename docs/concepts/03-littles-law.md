# 3. Little's Law

<p class="ts-lede">Work in progress, throughput and lead time are not three separate numbers. Know any two and you know the third.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- Little's Law: WIP = throughput × lead time.
- Why it holds for almost any line, random or not.
- How to use it: to estimate lead time by counting parts, and to see why
  extra WIP only costs time.
- How to read the numbers from random runs, with their ±.

</div>

**Builds on:** [1. What a production system is](01-production-systems.md)
(the four numbers), [2. Capacity and the bottleneck](02-bottleneck.md)

## Intuition

You buy 50 bottles of wine a year and drink each one four years after you
bought it. How many bottles are in your cellar? About 200: fifty a year,
each staying four years. You did not need to know which bottle came when,
or in what order you drank them.

A production line works like the cellar. Parts go in at some rate and each
stays for a while; on average, the number inside is the rate times the
stay. That is [Little's Law](glossary.md#littles-law), after John Little,
who proved it in 1961.

## The law

<div class="ts-panel ts-rule">
<span class="ts-label">Little's Law</span>

$$\text{WIP} = \text{throughput} \times \text{lead time}$$

All three are long-run averages.

</div>

The units have to match: with throughput in parts per hour, lead time comes
out in hours. In the last four hours of line A's shift on
[page 1](01-production-systems.md), the line held 22 parts and 80 parts per
hour came out, hour after hour. Those steady hours behave like a long run,
so a part needs 22 / 80 h = 16.5 minutes to get through. Nobody had to follow a part with a
stopwatch.

The law asks for almost nothing. Times can vary in any way, parts
can be served in any order, and the line can have any number of stations
and buffers. Only two things are needed: no parts are created or lost
inside, and the averages cover a period long enough that its start and end
do not matter.

## The running example

So far every time in line A was fixed. Figure 3.1 is a first look at a line
whose cutting and welding times vary from part to part, with a buffer of
five places; the [next page](04-variability.md) explains where such
variability comes from. WIP now rises and falls all the time, and Little's
Law is about its average.

<figure class="ts-figure">
<div class="ts-label">Figure 3.1</div>
<div class="ts-figure-title">WIP moves; its average is what Little's Law is about</div>
<div class="ts-plot">
{{#include img/03-wip.svg}}
</div>
<figcaption>Each column is the average number of parts in the line over 5 minutes. The highlighted line is the average over all three hours: the one number Little's Law works with.</figcaption>
</figure>

## The numbers

Three different systems, each simulated for five days of nonstop running,
ten times over. The first day is discarded: a line that starts empty is not
yet typical, which is the "start" the law's condition warns about. WIP,
throughput and lead time are measured separately:

{{#include tables/03-little.md}}

In every row, throughput × lead time matches the measured WIP to within a
fraction of a percent. The first system has no randomness at all. The
second has stations whose times vary from part to part, and a small buffer;
[page 4](04-variability.md) explains the CV in its name. The third is
welding alone, fed from outside at random moments, on average one part
every 54 s. That is about 67 parts per hour, fewer than welding's 80, so
here the arrivals set the throughput, not welding. The law holds for all
three.

## Reading simulated numbers

This table is the first on these pages with results from random runs. Such
numbers need a few words of explanation, and every later table uses them.

- **A run** is one simulation of the model over a stated time, here five
  days.
- **Replications.** When times vary, each run draws different times and
  gives a slightly different answer. So a random model is run several
  times, here ten. Each run starts its random numbers from a different
  [seed](glossary.md#seed), a starting number: the same seed gives exactly
  the same run again.
- **The ±.** A table shows the average over the runs, then ± and a second
  number. Together they give the 95 %
  [confidence interval](glossary.md#confidence-interval): how precisely the
  runs pin down the long-run average, the value endless runs would give. It
  is not the spread of single runs; one shift can land well outside it. More
  runs make it narrower. A line without randomness gives the same answer in
  every run, hence ± 0.00.
- **Comparing.** Two results whose ranges do not overlap are clearly
  different. If they overlap, the gap may be chance; page 13 shows a
  sharper test for that case.
- **Which runs.** Some pages simulate one shift that starts empty, as a real
  shift would. Others, like this one, simulate days and drop the start, the
  [warm-up](glossary.md#warm-up), to measure the long run. A few simulate
  five shifts in a row from empty, so the empty start weighs on one shift in
  five. The three can differ a little for the same line; each table says
  which it uses.

[Page 13](13-trustworthy-results.md) goes deeper: how the interval is
computed, how many runs are enough, and how long a warm-up is.

## Using it

- **Estimate lead time by counting.** Count the parts on the floor and read
  the output counter: lead time = WIP / throughput. This works for a real
  line as well as for a model.
- **See what extra WIP costs.** On page 1, the buffer of 20 did not add a
  single part of output, because welding was the limit. By Little's Law, the
  20 extra parts inside added 20 × 45 s = 15 minutes to every part's lead
  time. At the same throughput, more WIP always means more lead time.
- **Check a model.** If a simulation's WIP, throughput and lead time do not
  satisfy the law over a long run, something in the model or in the
  measurement is wrong.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

At a given throughput, lead time is proportional to WIP. To get parts
through faster without losing output, hold fewer of them.

</div>

## Try it yourself

- On a real shop floor you count 120 parts between the first station and
  the last, and the output counter says 30 parts per hour. How long does a
  part take to get through? (120 / 30 = 4 hours, without following a single
  part.)
- Take a single short shift that starts empty, and compare its WIP with
  throughput × the lead time of the parts that left. Why do they differ?
  (Parts still inside at the end count in WIP but have no lead time yet.
  Little's Law is about long runs.)

## Further reading

- John D. C. Little, "A proof for the queuing formula L = λW",
  *Operations Research* 9 (1961), and his retrospective "Little's Law as
  Viewed on Its 50th Anniversary", *Operations Research* 59 (2011).
- Hopp and Spearman, *Factory Physics*, chapter 7.

**Next:** [4. Variability](04-variability.md): what makes WIP move.
