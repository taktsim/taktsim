# 7. Buffers

<p class="ts-lede">A buffer lets two stations work at their own pace for a while. The first few places help a lot; every further place helps less and costs lead time.</p>

<div class="ts-panel">
<span class="ts-label">You will learn</span>

- How a buffer decouples two stations.
- Why more buffer has diminishing returns.
- What buffers cost, and when a line hardly needs one.

</div>

**Builds on:** [3. Little's Law](03-littles-law.md),
[6. Starving and blocking](06-starving-blocking.md)

## Intuition

A town takes water from a river. When the river runs low, the town runs dry;
when it floods, the water runs past unused. A reservoir between them fixes
both: it fills in good times and drains in bad ones. A small reservoir
smooths a dry afternoon, a bigger one a dry week. But a reservoir ten times
the size of the dry spells adds nothing except water standing still.

A [buffer](glossary.md#buffer) between two stations is that reservoir. When
cutting has a slow spell, welding takes parts from the buffer instead of
starving. When welding has a slow spell, cutting puts parts into the buffer
instead of being blocked.

## The running example

Line A again, with sigma 0.5 at both stations, now with buffers from none
at all up to 30 places. With no buffer, a finished cut can move on only
when welding is free.

## The numbers

{{#include tables/07-buffers.md}}

<figure class="ts-figure ts-widget" data-widget="buffers" data-config='{"sigma": 0.5, "buffer": 5}'>
<div class="ts-label">Figure 7.1 · interactive · teaching simulator, not the taktsim engine</div>
<div class="ts-figure-title">More buffer helps, but less each time</div>
<div class="ts-plot ts-static">
{{#include img/07-buffers.svg}}
</div>
<div class="ts-live"></div>
<figcaption>Parts per shift against buffer size, with the confidence band; the highlighted point is the buffer size in focus, five places by default, which recover most of what variability took away. The dashed line is a line with little variability (cutting CV 0.08, welding fixed).</figcaption>
</figure>

Without a buffer the variable line makes 517 parts per shift. One place gets
it to 576, two to 600, five to 624. After that the curve flattens: ten places
give 633, fifteen 635, thirty 635.5. Each of these has a ± of about 5
parts, and the ranges overlap. Compared run by run on the same seeds
([page 13](13-trustworthy-results.md#comparing-two-designs)), fifteen
places still beat ten by a part or two; beyond fifteen, the runs cannot
tell the sizes apart.
Each extra place helps only in a longer slow spell than the places before it
could absorb, and the longer a spell, the rarer it is.

In one shift that starts with an empty line, no buffer reaches the 640 that
welding could make: early in the shift the buffer is still nearly empty and
cannot yet protect welding. This does not contradict
[page 8](08-unreliable-machines.md), which simulates five shifts in a row
and, without failures, does reach 640.

Meanwhile the cost keeps growing. By [Little's Law](03-littles-law.md),
parts that sit in a buffer add to lead time. With 5 places a part needs
about 4 minutes to get through the line, with 20 places about 12, and with
30 about 15: almost four times the lead time for 2 % more output. These are
averages over one shift that starts empty, while the buffer is still
filling. A line that keeps running fills its buffer, because cutting is
faster on average, and its lead time climbs well above these figures.

## The README's example

The [README](https://github.com/taktsim/taktsim#readme)'s example is this
line with a buffer of 20 places. Arithmetic says welding allows 640 parts
per shift; the line makes 635. With a single place it would make 576, and
without a buffer 517. The buffer earns its place. The last 5 parts are lost
partly to variability and partly to the empty start of the shift, while the
buffer is still filling.

## When a buffer hardly matters

The last column of the table is a line with the same means and little
variability: cutting lognormal with sigma 0.08, welding a fixed 45 s.

- With 20 places it makes 638.7 parts per shift, out of the 640 that
  8 h / 45 s promises. The one part missing is the start-up, the 42 s
  before the first part reaches welding.
- With a single place it makes the same 638.7. Even with no buffer at all it
  makes 633.

Cutting is 3 s faster than welding on average and varies by only about
3 s. When a cut is slower than a weld, it is slower by a second or two, and
the next cut usually makes up for it. Welding never varies, so it never has
a slow spell that cutting would need room to work through. How much buffer
a line needs depends on how its
variability compares with the slack between its stations. This line has
little variability and enough slack, so the
[bottleneck rule](02-bottleneck.md) holds almost exactly.

## Rule of thumb

<div class="ts-panel ts-rule">
<span class="ts-label">Rule of thumb</span>

Size a buffer to the variability it has to absorb, not to the space on the
floor. The first few places give most of the gain; beyond that, every place
mostly adds lead time.

</div>

For two stations there are exact formulas for the output as a function of
buffer size (Gershwin's two-machine line). For longer lines there are not,
which is one reason to simulate; see [page 9](09-longer-lines.md).

## Try it yourself

In figure 7.1, move the buffer slider and watch the highlighted point walk
along the curve; then change sigma. At sigma 0.1 the curve is flat from one
place on, as for the low-variability line; at sigma 1 it keeps rising well
past ten places.

In the model, the buffer is one number:

<div class="ts-panel">
<span class="ts-label">Model change · planned for v0.1</span>

```yaml
  - id: buf1
    type: Buffer
    capacity: 5 parts
```

</div>

## Further reading

- Stanley B. Gershwin, *Manufacturing Systems Engineering*, chapter 3, on
  two-machine lines with a finite buffer.
- Hopp and Spearman, *Factory Physics*, chapter 9, section 9.2, "Variability
  Laws": buffering against variability with inventory, capacity or time.

**Next:** [8. Unreliable machines](08-unreliable-machines.md): variability
that comes in big pieces.
