<p align="center">
  <img alt="taktsim" src="docs/assets/logo-banner.svg" width="471">
</p>

<p align="center">
  Simulate a production line from a text file, before you change the real one.
</p>

<p align="center">
  <a href="https://github.com/taktsim/taktsim/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/taktsim/taktsim/actions/workflows/ci.yml/badge.svg"></a>
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-0A0A0A"></a>
  <a href="Cargo.toml"><img alt="MSRV: Rust 1.97" src="https://img.shields.io/badge/MSRV-1.97-0A0A0A"></a>
</p>

> [!IMPORTANT]
> **Early days.** taktsim is pre-release. The engine core works and is
> tested, but it cannot run a factory model yet. See [where it
> stands](#where-it-stands) and the [roadmap](docs/roadmap.md).

## What it does

taktsim plays out a factory in software. You describe a production line
in a text file: its machines, how long each step takes, how much can wait
between them. taktsim simulates a shift, a week or a year in seconds and
reports what comes out and where work piles up.

Take two machines with a shelf between them. Cutting takes 42 s per part
on average, welding 45 s, and both vary from part to part. Arithmetic says
welding sets the pace: 80 parts an hour, 640 per shift. In practice each
machine sometimes runs slow and the other has to wait. taktsim plays the
shift event by event, many times over, and reports the throughput you
would really get, with an error bar: about 635 parts with a shelf of 20,
about 576 with a shelf of one. The
[concepts guide](https://taktsim.github.io/taktsim/concepts/07-buffers.html)
explains why.

Use it to:

- **ask what-if questions:** a second welder, or a bigger shelf?
- **find the bottleneck** that holds the whole line back;
- **fit the model to real data** from your line, so its answers match
  what you see on the shop floor;
- **search for better settings** such as buffer sizes or staffing.

## What a model looks like

> [!NOTE]
> Planned for v0.1. The format is specified in [spec §4](docs/spec.md);
> the loader and the `run` command are not built yet.

The line above, as a model file:

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

Check it, then run it:

```sh
taktsim validate line-a.yaml          # mistakes reported with the field and the fix
taktsim run line-a.yaml               # throughput and WIP, each with a confidence interval
```

Python and Rust get the same two calls, `validate` and `run`.

## Why taktsim

Most simulation tools for manufacturing are graphical desktop
applications: models live in binary project files, results are hard to
reproduce, and automation is an afterthought. taktsim takes the opposite
approach.

- **Your model lives in git.** It is a plain YAML file you can diff,
  review in a pull request and check in CI.
- **Mistakes are caught before the run.** Every quantity has a unit
  (`42s`, `20 parts`). A wrong field is reported with its path and a
  valid example, never halfway through a simulation.
- **Same input, same answer.** The same model and seed give the same
  result, byte for byte, on Windows, Linux and macOS.
- **Numbers come with error bars.** Every result has a confidence
  interval, and the engine is tested against queueing problems whose
  exact answers are known.
- **AI agents can use it too.** The `taktsim` command describes its own
  commands and results as JSON, so an agent can build, run and refine
  models the same way a person does.
- **Safe near real machines.** Data adapters only read from production
  systems; no code path writes to a controller.
- **Free and open.** MIT or Apache-2.0, no paid tiers, no telemetry, runs
  on an ordinary PC.

## Who it is for

- **Production and industrial engineers** who want answers about a line
  without buying and learning a large graphical tool.
- **Developers** who want to embed a simulation engine in their own
  software, through the Rust crates or the Python bindings.
- **AI agents** that build and test models on a person's behalf.

## Where it stands

**Works today**

- Units and their parsing (`42s`, `6h`, `20 parts`).
- The engine core: event queue, simulation clock, reproducible random
  numbers, duration distributions, runtime correctness checks.
- Output statistics: confidence intervals, deciding how many runs are
  enough, warm-up detection.
- A small expression language and a CSV reader for observed data.

**Planned releases** ([spec §11](docs/spec.md#11-release-tiers-and-acceptance-criteria))

| Release | What you get |
| --- | --- |
| v0.1 Engine | Machines, buffers, sources and sinks; `validate` and `run` from the CLI and Python |
| v0.2 Agent | The full CLI for agents, more components (assembly, inspection, shifts), live machine data, a browser viewer |
| v0.3 Calibrate | Fit a model to observed data, with an accuracy report |
| v0.4 Optimize | Optimization, what-if experiments, bottleneck explanation |
| v0.5 Plant | Transport, maintenance, energy and cost, whole-plant models |

## Try it today

You need Rust 1.97 or newer ([rustup](https://rustup.rs) installs the
pinned toolchain automatically). Nothing is published on crates.io or
PyPI yet, so build from source:

```sh
git clone https://github.com/taktsim/taktsim.git
cd taktsim
cargo build
cargo test --workspace --exclude taktsim-python   # the engine's test suite
cargo run -p taktsim-cli -- --version
```

## Get involved

taktsim is early enough that a contribution shapes it. Help is welcome
with:

- **components** (new kinds of stations) and **data adapters**, each
  proposed through an issue template;
- **reference models** with known analytical answers, which the engine is
  tested against;
- **feedback from practice:** which questions do you ask about your
  lines, and which tools fall short?

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. In short: open an issue or
pick something from the current milestone, add tests with every change,
use [Conventional Commits](https://www.conventionalcommits.org/), and
sign off every commit (`git commit -s`). See also the [code of
conduct](CODE_OF_CONDUCT.md) and the [security policy](SECURITY.md).

## Under the hood

taktsim is a Rust library with Python bindings and a CLI, all thin layers
over one public API. The engine knows nothing about machines: machines
are components built on engine primitives. Every change is checked
against seven [design principles](docs/spec.md#2-design-principles).

| Crate | Purpose |
| --- | --- |
| [`taktsim-units`](crates/taktsim-units) | Unit newtypes and parsing |
| [`taktsim-core`](crates/taktsim-core) | Event loop, random streams, trace records and invariants; no I/O |
| [`taktsim-stats`](crates/taktsim-stats) | Confidence intervals, replication control, warm-up detection |
| [`taktsim-model`](crates/taktsim-model) | Model types and the expression language; loader and schema to come |
| [`taktsim-adapters`](crates/taktsim-adapters) | Data adapters, each behind a Cargo feature (`csv`) |
| [`taktsim-trace`](crates/taktsim-trace) | Trace writer, reader and replay (M3, empty) |
| [`taktsim-components`](crates/taktsim-components) | Source, Machine, Buffer, Sink (M5, empty) |
| [`taktsim-cli`](crates/taktsim-cli) | The `taktsim` command (M7; `--version` and `--help` only) |
| [`taktsim-bench`](crates/taktsim-bench) | Benchmarks for the regression gate |

The Python bindings live in [`python/`](python) (M12). Supported
platforms are Windows x86-64, Linux x86-64 and ARM64, and macOS ARM64
([ADR-0010](docs/adr/0010-platforms-and-reference-hardware.md)).

## Documentation

The documentation site is at **<https://taktsim.github.io/taktsim/>**.

- [Concepts](https://taktsim.github.io/taktsim/concepts/): how production
  lines behave, a learning path for developers new to manufacturing.
- [Specification](docs/spec.md): what taktsim is and how it behaves. The
  spec still uses the placeholder name `simcode`; read it as `taktsim`.
- [Roadmap](docs/roadmap.md): milestones and the acceptance criteria each
  proves.
- [Architecture decisions](docs/adr/): every choice the spec leaves open.
- [Contributing](CONTRIBUTING.md): setup, the checks CI runs, and the
  rules for commits and pull requests.

## License

Code is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Documentation and benchmark data are licensed under
[CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/).

### Contributions

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

Every commit must carry a `Signed-off-by:` line (`git commit -s`),
certifying the [Developer Certificate of Origin](https://developercertificate.org/).
