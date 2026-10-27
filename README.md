<p align="center">
  <img alt="taktsim" src="docs/assets/logo-banner.svg" width="471">
</p>

<p align="center">
  Simulation as code for production systems.
</p>

<p align="center">
  <a href="https://github.com/taktsim/taktsim/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/taktsim/taktsim/actions/workflows/ci.yml/badge.svg"></a>
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-0A0A0A"></a>
  <a href="Cargo.toml"><img alt="MSRV: Rust 1.97" src="https://img.shields.io/badge/MSRV-1.97-0A0A0A"></a>
</p>

taktsim is a typed, deterministic discrete-event simulation engine for
production systems: lines, cells, warehouses and logistics loops. It is a
Rust library with Python bindings and a self-describing CLI that people
and AI agents use alike.

- **Models are code.** A model is a YAML or JSON text file under version
  control, validated against a JSON Schema generated from the Rust types.
- **Typed end to end.** Every quantity carries a unit. Invalid input is
  rejected at load time with the field path and the fix, never during a
  run.
- **Deterministic.** Same model, seed and engine version: the same trace,
  byte for byte, on every supported platform.
- **Verifiable.** Reference models with analytical solutions, runtime
  invariants, and confidence intervals on every reported number.
- **One interface for people and agents.** Agents use the same `taktsim`
  CLI as people. It describes its own commands, inputs, results and exit
  codes as JSON; there is no separate agent server
  ([ADR-0019](docs/adr/0019-cli-agent-interface.md)).

## Status

Pre-release. Nothing is published on crates.io or PyPI yet, and the CLI
cannot run a model yet. The [roadmap](docs/roadmap.md) tracks progress
toward v0.1.

What exists today:

- Unit newtypes and parsing (`42s`, `6h`, `20 parts`).
- The engine core: event queue with deterministic tie-breaking and
  cancellation, simulation clock and run loop, counter-based random
  streams (one per component and purpose), duration distributions,
  runtime invariant and conservation checks, trace records.
- Output statistics: confidence intervals, sequential replication
  control, time-weighted statistics, MSER-5 warm-up detection.
- The expression language: parser, type checker, evaluator and printer.
- A CSV adapter for observed data.
- Benchmarks with a regression gate in CI.

Not there yet: trace storage and replay (M3), the model loader (M4),
components such as Machine and Buffer (M5), the CLI commands `describe`,
`validate` and `run` (M7), and Python `validate` and `run` (M12).

## Planned usage

> [!NOTE]
> Planned for v0.1 ([spec §4, §5](docs/spec.md)). None of this works yet.

A model (spec §4, shortened):

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
  - id: source
    type: Source
    interarrival: { dist: exponential, mean: 40s }
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.08 }
  - id: buf1
    type: Buffer
    capacity: 20 parts
  - id: weld
    type: Machine
    cycle_time: 45s
  - id: sink
    type: Sink
flow:
  - [source, cut, buf1, weld, sink]
observe:
  - throughput(sink)
  - wip(buf1)
```

The same commands serve people and agents:

```sh
taktsim describe                           # every command, as JSON
taktsim validate line-a.yaml               # errors with path and fix
taktsim run line-a.yaml --format json      # results with confidence intervals
```

## Design principles

Every change is checked against the seven principles of
[spec §2](docs/spec.md#2-design-principles): models are code; typed end to
end; deterministic by default; verifiable; self-improving within limits;
open standards first; a small, composable core. The engine knows nothing
about machines: machines are components built on engine primitives.

## Crates

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

The Python bindings live in [`python/`](python) and are a thin layer over
the same Rust API (M12). `taktsim-core` depends on no taktsim crate but
`taktsim-units`; every other crate depends on the core.

Tier-1 platforms are Windows x86-64, Linux x86-64 and ARM64, and macOS
ARM64 ([ADR-0010](docs/adr/0010-platforms-and-reference-hardware.md)).

## Documentation

- [Specification](docs/spec.md): what taktsim is and how it behaves. The
  spec still uses the placeholder name `simcode`; read it as `taktsim`.
- [Roadmap](docs/roadmap.md): milestones and the acceptance criteria each
  proves.
- [Architecture decisions](docs/adr/): every choice the spec leaves open.
- [Contributing](CONTRIBUTING.md): setup, the checks CI runs, and the
  rules for commits and pull requests.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. In short: work on the
current milestone or open an issue, add tests with every change, use
[Conventional Commits](https://www.conventionalcommits.org/), and sign off
every commit (`git commit -s`). See also the
[code of conduct](CODE_OF_CONDUCT.md) and the
[security policy](SECURITY.md).

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
