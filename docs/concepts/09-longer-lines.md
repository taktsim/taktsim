# 9. Longer lines

<p class="ts-lede">In a line of many stations, a stop anywhere spreads: upstream as blocking, downstream as starving. And the bottleneck is not always the station with the longest cycle time.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- How blocking and starving travel along a line.
- Why failures can move the bottleneck to another station.
- Why more buffer everywhere is an expensive way to buy a little output.

</div>

**Builds on:** [2. Capacity and the bottleneck](02-bottleneck.md),
[6. Starving and blocking](06-starving-blocking.md),
[7. Buffers](07-buffers.md), [8. Unreliable machines](08-unreliable-machines.md)

## Intuition

A traffic light turns red on a busy road. The cars right behind it stop
first, then the ones behind them, and the queue grows backwards, street by
street. Beyond the light the road empties. When it turns green, the queue
dissolves from the front, car by car. A line of stations does the same when
one of them stops.

## The running example

Line A grows into line B. Deburring follows cutting, painting follows
welding, and packing comes at the end. Every station varies (sigma 0.5),
each pair of stations has five buffer places between them, and painting
fails now and then: on average every two hours, for fifteen minutes.

<figure class="ts-figure">
<div class="ts-label">Figure 9.1</div>
<div class="ts-figure-title">Line B: five stations</div>
<div class="ts-plot">
{{#include img/09-line.svg}}
</div>
<figcaption>Cycle times under each station; the numbers above the buffers are their places. Painting, marked for its failures, is highlighted because it turns out to be the bottleneck.</figcaption>
</figure>

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
flow:
  - [source, cut, buf1, deburr, buf2, weld, buf3, paint, buf4, pack, sink]
```

</div>

Each new station and buffer is declared in `components` as on the earlier
pages; `flow` lists them in order.

## A stop travels

<figure class="ts-figure">
<div class="ts-label">Figure 9.2</div>
<div class="ts-figure-title">A stop at paint travels up and down the line</div>
<div class="ts-plot">
{{#include img/09-timeline.svg}}
</div>
<figcaption>Ninety minutes of line B in 90-second steps. From the highlighted moment, painting is down; the hatched blocking spreads upstream, welding first and cutting last, while packing runs dry.</figcaption>
</figure>

Within minutes of painting's failure, welding has filled the buffer in front
of painting and is blocked. Deburring is blocked next, then cutting.
Packing, after painting, is starved the whole time. When painting is
repaired, the line restarts from painting outwards. A single stop has idled
all five stations.

## The bottleneck moves

On [page 2](02-bottleneck.md) the bottleneck was simply the station with the
longest cycle time. In line B that is welding, at 45 s. Painting needs only
43 s, but with an availability of 88.9 % its effective cycle time is
43 / 0.889 = 48.4 s. The table shows how busy each station is: the share of
time it is neither starved nor blocked, so either working or down. The
busiest station is the bottleneck:

{{#include tables/09-bottleneck.md}}

Without failures, welding is the busiest. With painting's failures,
painting is, and the line loses another 64 parts per shift. Improving
welding now would gain almost nothing.

## Where to spend the effort

{{#include tables/09-options.md}}

Doubling every buffer, 20 more places in all, gains 17 parts per shift and
adds 9 minutes to every part's lead time. Four times the buffer gains 29
parts and triples the lead time. Moving all 20 places next to painting
sounds right, but the other stations vary too, and with one place between
them they starve and block each other: the line loses another 34 parts.
Halving painting's repair time, with the buffers as they are, gains 33
parts and even shortens the lead time. Spreading the same downtime over
shorter outages helps too, as on page 8.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Find the bottleneck by measuring how much each station waits, not by
reading cycle times. Fix the bottleneck first; buffers everywhere buy little
output at a high price in lead time.

</div>

For two stations, the effect of a buffer can be computed exactly. For five
stations with failures there is no closed formula, and this is where
simulation earns its keep.

## Try it yourself

- Take the failures away from painting: welding becomes the bottleneck
  again, and the line makes 617 parts per shift.
- Keep painting's failures but make it faster, 35 s instead of 43 s. Is it
  still the bottleneck? (No: its effective cycle time is
  35 / 0.889 = 39.4 s, so welding takes over again.)

## Further reading

- Gershwin, *Manufacturing Systems Engineering*, chapter 4, "Decomposition
  of Long Transfer Lines": their approximate analysis.
- Hopp and Spearman, *Factory Physics*, section 8.7, "Effects of Blocking".

**Next:** [10. Takt time and pull](10-takt-pull.md): how much should the line
make, and how much should it hold?
