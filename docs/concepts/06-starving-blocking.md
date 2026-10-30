# 6. Starving and blocking

<p class="ts-lede">In a line, stations depend on each other. A slow part upstream leaves the next station without work; a slow part downstream stops the one before it.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- What starving and blocking are.
- How to read a machine state timeline.
- Why a bottleneck that waits costs output that is never recovered.

</div>

**Builds on:** [4. Variability](04-variability.md),
[5. Utilization and waiting](05-utilization.md)

## Intuition

Two people pack boxes at a table, one folding, one filling, with room for a
single folded box between them. When the folder struggles with a stiff
sheet, the filler stands with empty hands. When the filler is slow with a
box, the space between them fills up, and the folder has to hold the next
box instead of starting another.

The filler with nothing to do is [starved](glossary.md#starved): ready to
work, but no part has arrived. The folder holding a finished box is
[blocked](glossary.md#blocked): the part is done, but there is no room
downstream to put it. Neither is broken, and neither is slow on average.
They are waiting for each other.

## The running example

Line A again, with variable cycle times at both stations (sigma 0.5,
CV 0.53, from [page 4](04-variability.md)) and a buffer of just one place:

<figure class="ts-figure">
<div class="ts-label">Figure 6.1</div>
<div class="ts-figure-title">Line A, variable, with a one-place buffer</div>
<div class="ts-plot">
{{#include img/06-line.svg}}
</div>
<figcaption>Both stations now vary by about half their mean from part to part.</figcaption>
</figure>

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
  - id: buf1
    type: Buffer
    capacity: 1 parts
observe:
  - throughput(sink)
  - state_shares(cut, weld)
  - wip(buf1)
```

</div>

`state_shares` asks for the share of time each station spends working,
starved, blocked and so on.

## Reading a state timeline

A state timeline has one row per station and shows what the station did
over time. Here each step is 30 seconds, drawn in the state the station
spent most of it in: solid when working, an empty outline when starved,
hatched when blocked. Below the rows is the buffer's fill level, averaged
over each step: a bar at half height means the buffer held a part for half
of those 30 seconds.

<figure class="ts-figure ts-widget" data-widget="timeline" data-config='{"sigma": 0.5, "buffer": 1, "seed": 2}'>
<div class="ts-label">Figure 6.2 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">Each station stops the other</div>
<div class="ts-plot ts-static">
{{#include img/06-timeline.svg}}
</div>
<div class="ts-live"></div>
<figcaption>The first 30 minutes of a shift. At the highlighted step, welding has taken the last part from the buffer and finished it, while a long cut has not yet delivered the next one: welding is starved. The hatched steps in the cutting row are times when welding was slow and the full buffer blocked cutting.</figcaption>
</figure>

## The numbers

How the two stations spent an 8-hour shift with a one-place buffer, for no,
some and more variability:

{{#include tables/06-shares.md}}

Without variability, cutting is blocked 7 % of the time: it is faster than
welding and has to wait for space. That costs nothing, because welding never
stops. With sigma 0.5, welding is starved 10 % of the time, and the line
makes 576 parts instead of 639, also 10 % fewer. That is no coincidence:
welding is the bottleneck, so every minute it waits is a minute of output
the line never gets back. Cutting is now blocked 15 % of the time. Each of
those minutes is one in which it could have worked ahead, building a stock
for its own next slow cut.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Time the bottleneck spends starved or blocked is output lost for good. In a
variable line, the stations around the bottleneck have to be protected
from each other.

</div>

The next page is about the most common protection: a bigger buffer.

## Try it yourself

In figure 6.2, set sigma to 0 and the timeline turns regular: cutting is
blocked a little in every few steps, welding never waits. Raise sigma and
starved steps appear in the welding row; raise the buffer and most of them
disappear again. Press "New run" to see another shift with the same
settings.

## Further reading

- Gershwin, *MIT 2.854*, lecture notes on two-machine lines with a buffer.
- Stanley B. Gershwin, *Manufacturing Systems Engineering*, chapter 3.

**Next:** [7. Buffers](07-buffers.md): how much buffer is enough.
