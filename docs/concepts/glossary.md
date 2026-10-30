# Glossary

<p class="ts-lede">Every term of the learning path, in alphabetical order, with the page that introduces it.</p>

### Availability

The share of time a station is able to work:
$\text{MTBF} / (\text{MTBF} + \text{MTTR})$. Says
nothing about how long the outages are.
[Page 8](08-unreliable-machines.md).

### Batch

A group of parts of the same variant processed together, so that one setup
serves all of them. [Page 11](11-batching-setups.md).

### Blocked

A station is blocked when it has finished a part but cannot pass it on,
because the buffer or station after it is full.
[Page 6](06-starving-blocking.md).

### Bottleneck

The station with the least capacity once its losses, such as failures, are
counted; it limits the output of the whole line. Found by how little it waits, not only by its cycle time.
[Page 2](02-bottleneck.md), [page 9](09-longer-lines.md).

### Buffer

Storage between two stations with a limited number of places, where parts
wait. Decouples the stations. [Page 1](01-production-systems.md),
[page 7](07-buffers.md).

### Calibration

Adjusting a model's parameters until its results match observed data, then
checking it on data it was not fitted to. [Page 14](14-model-to-reality.md).

### Capacity

The most a station can make per unit of time when it never waits:
$1 / \text{cycle time}$. [Page 2](02-bottleneck.md).

### Coefficient of variation

The standard deviation of a time divided by its mean (CV). 0 means no
variability; an exponential distribution has 1.
[Page 4](04-variability.md).

### Common random numbers

Giving two designs the same seeds, so they meet the same slow parts and
failures and their difference is not just chance.
[Page 13](13-trustworthy-results.md).

### Confidence interval

A range around a mean estimated from replications, written as
mean ± half-width. It says how precisely the runs pin down the long-run
(true) mean, not how much single runs vary. "95 %" describes the method: of
many such experiments, about 95 in 100 give a range that contains the true
mean. [Page 3](03-littles-law.md#reading-simulated-numbers),
[page 13](13-trustworthy-results.md).

### CONWIP

"Constant work in process": a pull system that lets a new part into the
line only when a finished part leaves, so WIP never exceeds a cap.
[Page 10](10-takt-pull.md).

### Critical WIP

The WIP at which a line without variability reaches its bottleneck rate
(the bottleneck's capacity): $\text{bottleneck rate} \times \text{raw process time}$,
where the raw process time is the sum of all stations' cycle times. [Page 10](10-takt-pull.md).

### Cycle time

The time a station needs per part. Some textbooks use the word for lead
time; these pages and taktsim's model files do not.
[Page 1](01-production-systems.md).

### Distribution

The rule for how a time varies: which values are possible and how often
each comes up. Part of a model; its mean and CV are exact numbers. Compare
sample. [Page 4](04-variability.md#a-distribution-and-a-sample).

### Effective cycle time

A station's cycle time with its losses included, for example
$\text{cycle time} / A$, with $A$ its availability. [Page 8](08-unreliable-machines.md).

### Exponential distribution

The textbook model of "completely random" times: short times are the most
common, long ones rare, CV exactly 1. [Page 4](04-variability.md),
[page 5](05-utilization.md).

### Half-width

The number after ± in a confidence interval: the interval runs from
mean − half-width to mean + half-width. [Page 13](13-trustworthy-results.md).

### Histogram

A chart of a sample: each bar counts the values that fell into one range.
[Page 4](04-variability.md).

### Inspection

A step that checks parts and sorts them into good, rework and scrap.
[Page 12](12-scrap-rework.md).

### Kanban

A pull system with a fixed number of cards per loop between stations; a
station may only make a part when it holds a free card.
[Page 10](10-takt-pull.md).

### Kingman's formula

An approximation of the mean wait in queue at one station:
$\frac{c_a^2 + c_s^2}{2} \times \frac{u}{1 - u} \times t$, with $c_a$ the CV
of the times between arrivals, $c_s$ the CV of the process time, $u$ the
utilization and $t$ the mean process time. Needs $u$ below 1. Exact when both
arrivals and process times are exponential (M/M/1).
[Page 5](05-utilization.md).

### Kolmogorov–Smirnov distance

A measure of how well a distribution fits measured times: the largest gap
between the share of times below a value and the share the distribution
predicts. 0 is a perfect fit. [Page 14](14-model-to-reality.md).

### Lead time

The time a part spends in the line, from entering to leaving, waiting
included. [Page 1](01-production-systems.md).

### Little's Law

$\text{WIP} = \text{throughput} \times \text{lead time}$, for long-run averages
of almost any system.
[Page 3](03-littles-law.md).

### Lognormal distribution

A common model for process times: always positive, with a long tail of slow
cases. Given by its mean and a shape parameter sigma, which is not the
standard deviation of the times. [Page 4](04-variability.md).

### Median

The middle value when the values are sorted. [Page 13](13-trustworthy-results.md).

### Method of moments

Fitting a distribution by giving it the same mean and, where it can, the
same CV as the measured times. [Page 14](14-model-to-reality.md).

### MSER-5

A rule that finds where a run's warm-up ends: it averages the output in
groups of five and picks the cut-off after which the rest varies least.
[Page 13](13-trustworthy-results.md).

### MTBF

Mean time between failures: how long a station is up (working or waiting),
on average, before it fails. [Page 8](08-unreliable-machines.md).

### MTTR

Mean time to repair: how long a station is down, on average, after a
failure. [Page 8](08-unreliable-machines.md).

### Pull

Releasing work into a line only when something leaves it, as Kanban and
CONWIP do. The opposite of push. [Page 10](10-takt-pull.md).

### Push

Releasing work into a line by a schedule, whatever is inside.
[Page 10](10-takt-pull.md).

### Replication

One run of a model with its own seed, so its random numbers behave as if
unrelated to those of the other runs (independent). Results are means over
replications. [Page 3](03-littles-law.md#reading-simulated-numbers),
[page 13](13-trustworthy-results.md).

### Rework

A faulty part that goes back to an earlier station to be fixed, using its
capacity a second time. [Page 12](12-scrap-rework.md).

### Scrap

A faulty part that cannot be saved and leaves the line.
[Page 12](12-scrap-rework.md).

### Sample

A set of times you actually have: drawn from a distribution in a
simulation, or measured on a line. Its mean and CV estimate the
distribution's and miss them by chance.
[Page 4](04-variability.md#a-distribution-and-a-sample).

### Seed

The starting number of a simulation's random numbers. The same seed gives
exactly the same run again; different seeds give runs that behave as if
unrelated. [Page 3](03-littles-law.md#reading-simulated-numbers),
[page 13](13-trustworthy-results.md).

### Setup

The work of changing a station over to another product or variant, during
which it makes nothing. [Page 11](11-batching-setups.md).

### Sink

Where finished parts leave the line. [Page 1](01-production-systems.md).

### Source

Where parts enter the line. [Page 1](01-production-systems.md).

### Standard deviation

Roughly the typical distance of a single value from the mean; precisely,
the square root of the average squared distance.
[Page 4](04-variability.md).

### Starved

A station is starved when it could work but has no part, because the
buffer or station before it is empty. [Page 6](06-starving-blocking.md).

### Station

A place where work is done to one part (or one batch) at a time: a machine,
a workstation, an operator. [Page 1](01-production-systems.md).

### Steady-state

An experiment about a line's long-run behaviour; it needs a warm-up to be
discarded. [Page 13](13-trustworthy-results.md).

### Takt time

The pace demand sets: $\text{available time} / \text{demand}$. Every station's effective
cycle time must stay below it. [Page 10](10-takt-pull.md).

### Terminating

An experiment with a natural end, such as one shift, that starts from a
defined state. [Page 13](13-trustworthy-results.md).

### Throughput

The number of parts that leave the line per unit of time.
[Page 1](01-production-systems.md).

### Utilization

The share of time a station is working. [Page 2](02-bottleneck.md),
[page 5](05-utilization.md).

### Validation

Checking that a model, with its fitted inputs, behaves like the real line.
[Page 14](14-model-to-reality.md).

### Variability

How much times differ from one part, arrival or failure to the next;
measured by the coefficient of variation. [Page 4](04-variability.md).

### Warm-up

The start of a steady-state run, when the line is not yet typical; it is
left out of the statistics. [Page 3](03-littles-law.md#reading-simulated-numbers),
[page 13](13-trustworthy-results.md).

### Work in progress

The parts inside the line, started but not finished (WIP).
[Page 1](01-production-systems.md).

### Yield

The share of parts that end up good. [Page 12](12-scrap-rework.md).
