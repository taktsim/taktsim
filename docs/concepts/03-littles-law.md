# 3. Little's Law

<p class="ts-lede">Work in progress, throughput and lead time are not three separate numbers. Know any two and you know the third.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- Little's Law: WIP = throughput × lead time.
- Why it holds for almost any line, random or not.
- How to use it: to estimate lead time by counting parts, and to see why
  extra WIP only costs time.

</div>

**Builds on:** [1. What a production system is](01-production-systems.md)
(the four numbers), [2. Capacity and the bottleneck](02-bottleneck.md)

## Intuition

You buy 50 bottles of wine a year and drink each one four years after you
bought it. How many bottles are in your cellar? About 200: fifty a year,
each staying four years. You did not need to know which bottle came when,
or in what order you drank them.

A production line is the cellar. Parts go in at some rate, each stays for a
while, and the number inside at any moment is, on average, the rate times
the stay. That is [Little's Law](glossary.md#littles-law), after John Little,
who proved it in 1961.

## The law

<div class="ts-panel ts-rule">
<span class="ts-label">Little's Law</span>

**WIP = throughput × lead time**, as long-run averages.

</div>

The units have to match: with throughput in parts per hour, lead time goes in
hours. Line A in [page 1](01-production-systems.md) ended the shift with 22
parts inside and 80 parts per hour coming out, so a part needs
22 / 80 h = 16.5 minutes to get through. Nobody had to follow a part with a
stopwatch.

The law needs almost nothing: no particular distribution, no order of
service, any number of stations and buffers. It needs only that parts are
not created or lost inside, and that you compare averages over a period long
enough that the start and the end do not matter.

## The running example

Figure 3.1 is line A with variable process times and a buffer of five; the
[next page](04-variability.md) is about where such variability comes from.
WIP now moves all the time. Little's Law speaks about its average.

<figure class="ts-figure">
<div class="ts-label">Figure 3.1</div>
<div class="ts-figure-title">WIP moves; its average is what Little's Law is about</div>
<div class="ts-plot">
{{#include img/03-wip.svg}}
</div>
<figcaption>Each column is the average number of parts in the line over 5 minutes; the highlighted step is the busiest stretch.</figcaption>
</figure>

## The numbers

Three different systems, each run for five days with the first day
discarded, and WIP, throughput and lead time measured separately:

{{#include tables/03-little.md}}

In each row the last column is the product of throughput and lead time,
which matches the measured WIP within the simulation's noise. The first row
has no randomness at all, the second has variable stations and a small
buffer, the third has random arrivals in front of a single station. The law
does not care.

## Using it

- **Estimate lead time by counting.** Count the parts on the floor and read
  the output counter: lead time = WIP / throughput. This works for a real
  line as well as for a model.
- **See what extra WIP costs.** On page 1, the buffer of 20 did not add a
  single part of output; welding was the limit. By Little's Law, the 20
  extra parts inside added 20 × 45 s = 15 minutes to every part's lead time.
  At the same throughput, more WIP always means more lead time.
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

- In line A from page 1, set the buffer to `5 parts`. WIP settles at 7,
  throughput stays at 80 per hour, so lead time drops to 7 / 80 h, about
  5 minutes.
- Over a single short shift that starts empty, compare WIP with throughput
  times the lead time of the parts that left. They differ, because parts
  still inside at the end count in WIP but not in the lead times. Little's
  Law is about long runs.

## Further reading

- John D. C. Little, "A proof for the queuing formula L = λW",
  *Operations Research* 9 (1961), and his retrospective "Little's Law as
  Viewed on Its 50th Anniversary", *Operations Research* 59 (2011).
- Hopp and Spearman, *Factory Physics*, chapter 7.

**Next:** [4. Variability](04-variability.md): what makes WIP move.
