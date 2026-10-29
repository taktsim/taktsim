<h1 class="ts-sr-only">taktsim documentation</h1>

<div class="ts-hero-block">
{{#include assets/hero.svg}}
<p class="ts-hero-tagline">Simulate a production line from a text file, before you change the real one.</p>
<p class="ts-label">Pre-release · Rust · Python · CLI</p>
</div>

taktsim is a deterministic discrete-event simulation engine for production
systems: lines, cells, warehouses and logistics loops. You describe a line in
a text file: its machines, how long each step takes, and how many parts can
wait between them. taktsim then plays out a shift, a week or a year and
reports what comes out and where work piles up, every number with a
confidence interval. People and AI agents use it through the same command
line.

## Start here

<div class="ts-cards">
<a class="ts-card" href="concepts/index.html"><span class="ts-label">01 · Learn</span><strong>Concepts</strong><span class="ts-card-text">How production lines behave, in 14 short pages: from stations and buffers to simulation results you can trust.</span></a>
<a class="ts-card" href="spec.html"><span class="ts-label">02 · Reference</span><strong>Specification</strong><span class="ts-card-text">What taktsim will do: the model format, the command line, calibration and verification.</span></a>
<a class="ts-card" href="roadmap.html"><span class="ts-label">03 · Plan</span><strong>Roadmap</strong><span class="ts-card-text">The milestones toward v0.1, and what each one proves.</span></a>
<a class="ts-card" href="adr/index.html"><span class="ts-label">04 · Decisions</span><strong>Architecture decisions</strong><span class="ts-card-text">Every choice the specification leaves open, with its trade-offs.</span></a>
</div>

## A line in a few lines

Two machines with a shelf between them. Cutting takes 42 s per part on
average, welding 45 s, and both vary from part to part:

<div class="ts-panel">
<span class="ts-label">Model · planned for v0.1</span>

```yaml
format: "1.0"
model:
  name: line-a
  time_unit: s
  seed: 42
experiment:
  type: terminating
  horizon: 8h
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
flow:
  - [source, cut, buf1, weld, sink]
observe:
  - throughput(sink)
  - wip(buf1)
```

</div>

On paper, welding sets the pace at 640 parts per shift. Played out part by
part, the line makes about 635: now and then one machine runs slow and the
other has to wait for it. With room for one part on the shelf instead of 20,
it would make about 576. Page 7 of the concepts,
[Buffers](concepts/07-buffers.md), explains why.

## Where it stands

taktsim is pre-release and cannot run a model yet. The engine core, the
statistics, the units, the expression language and a CSV reader exist and
are tested; the model loader, the components and the `run` command are being
built. The [roadmap](roadmap.md) tracks the way to v0.1, and the
[README](https://github.com/taktsim/taktsim#readme) has the current status
and how to build it.

<div class="ts-steprow" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i></div>
