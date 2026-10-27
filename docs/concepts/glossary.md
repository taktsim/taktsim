# Glossary

<p class="ts-lede">Every term of the learning path, in alphabetical order, with the page that introduces it.</p>

### Availability

The share of time a station is able to work: MTBF / (MTBF + MTTR). Says
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

The station with the least effective capacity; it limits the output of the
whole line. Found by how little it waits, not only by its cycle time.
[Page 2](02-bottleneck.md), [page 9](09-longer-lines.md).

### Buffer

Storage between two stations with a limited number of places, where parts
wait. Decouples the stations. [Page 1](01-production-systems.md),
[page 7](07-buffers.md).

### Capacity

The most a station can make per unit of time when it never waits:
1 / cycle time. [Page 2](02-bottleneck.md).

### Coefficient of variation

The standard deviation of a time divided by its mean (CV). 0 means no
variability; an exponential distribution has 1.
[Page 4](04-variability.md).

### Confidence interval

A range around an estimated mean, computed from replications, that contains
the true mean with a stated probability, usually 95 %. Written as
mean ± half-width. [Page 13](13-trustworthy-results.md).

### CONWIP

"Constant work in process": a pull system that lets a new part into the
line only when a finished part leaves, so WIP never exceeds a cap.
[Page 10](10-takt-pull.md).

### Critical WIP

The WIP at which a line without variability reaches its bottleneck rate:
bottleneck rate × raw process time. [Page 10](10-takt-pull.md).

### Cycle time

The time a station needs per part. Some textbooks use the word for lead
time; these pages and taktsim's model files do not.
[Page 1](01-production-systems.md).

### Effective cycle time

A station's cycle time with its losses included, for example
cycle time / availability. [Page 8](08-unreliable-machines.md).

### Inspection

A step that checks parts and sorts them into good, rework and scrap.
[Page 12](12-scrap-rework.md).

### Kanban

A pull system with a fixed number of cards per loop between stations; a
station may only make a part when it holds a free card.
[Page 10](10-takt-pull.md).

### Kingman's formula

An approximation of the mean wait in queue at one station:
(cₐ² + cₛ²) / 2 × u / (1 − u) × t. Exact for exponential times.
[Page 5](05-utilization.md).

### Lead time

The time a part spends in the line, from entering to leaving, waiting
included. [Page 1](01-production-systems.md).

### Little's Law

WIP = throughput × lead time, for long-run averages of almost any system.
[Page 3](03-littles-law.md).

### MTBF

Mean time between failures: how long a station runs, on average, before it
fails. [Page 8](08-unreliable-machines.md).

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

One independent run of a model with its own random seed. Results are means
over replications. [Page 13](13-trustworthy-results.md).

### Rework

A faulty part that goes back to an earlier station to be fixed, using its
capacity a second time. [Page 12](12-scrap-rework.md).

### Scrap

A faulty part that cannot be saved and leaves the line.
[Page 12](12-scrap-rework.md).

### Setup

The work of changing a station over to another product or variant, during
which it makes nothing. [Page 11](11-batching-setups.md).

### Sink

Where finished parts leave the line. [Page 1](01-production-systems.md).

### Source

Where parts enter the line. [Page 1](01-production-systems.md).

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

The pace demand sets: available time / demand. Every station's effective
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

### Variability

How much times differ from one part, arrival or failure to the next;
measured by the coefficient of variation. [Page 4](04-variability.md).

### Warm-up

The start of a steady-state run, when the line is not yet typical; it is
left out of the statistics. [Page 13](13-trustworthy-results.md).

### Work in progress

The parts inside the line, started but not finished (WIP).
[Page 1](01-production-systems.md).

### Yield

The share of parts that end up good. [Page 12](12-scrap-rework.md).
