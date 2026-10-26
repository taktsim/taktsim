# Simulation-as-Code: Software Specification

Oct 25, 2026 · @Jerome

This document specifies an open-source library, built on a Rust core with Python bindings, that lets humans and AI agents build, run, calibrate and optimize discrete-event simulations of production systems as code. It is written for the agent that will turn it into a roadmap and ship it.

**How to read this spec.** Rust for the core is the one fixed technology decision. Every other named tool or library is marked *default*: a recommendation the implementing agent confirms or replaces in an architecture decision record (ADR). Every number is an *initial target* to be validated with a prototype before the release that depends on it.

## 1. Purpose and scope

The software is a typed, code-first toolkit for discrete-event simulation of production systems: lines, cells, warehouses, logistics loops. A model is a versioned text file; the engine runs it; the calibrator fits it to observed data; the optimizer searches its parameter space; an MCP server exposes all of this to AI agents.

**Users**

- AI agents that build, run and refine models on behalf of a person, through the MCP interface.
- Engineers who write or review models in an editor, run them from a CLI and inspect results in CI.
- Developers who embed the engine in their own tools through the Rust crates or the Python bindings.

**In scope for v1**

- Discrete-event simulation: material flow, cycle times, buffers, failures, setups, shifts, operators, assembly by bill of materials, batch processes, inspection with scrap and rework, routing and dispatching rules, order-driven production, standard manufacturing KPIs.
- A declarative model format with a published schema.
- A typed component library and a registry for user-defined components.
- Calibration against observed data, with a quantified accuracy report.
- Parameter optimization and automated what-if experiments.
- Adapters for CSV, Parquet, OPC UA and MQTT inputs, a browser-based viewer with semantic zoom, and opt-in sharing of calibrated parameters.

**Out of scope for v1**

- 3D visualization and physics (kinematics, collisions). The architecture leaves a seam for FMI co-simulation later.
- Control code (PLC logic). The system reads data from machines; it never writes to them.
- A graphical editor. Models are written as code; the visual interface (section 14) shows, explains and replays them and edits parameters only.

## 2. Design principles

Every design decision is checked against these seven rules. A pull request that breaks one needs an explicit exception in its description.

1. **Models are code.** A model is a text file under version control. Anything a person can do in the system, an agent can do through the same API, and vice versa.
2. **Typed end to end.** Every model, parameter, event, result and tool call has a schema. Invalid input is rejected at load time with a message that names the field and the fix, never at tick 40,000 of a run.
3. **Deterministic by default.** Same model, same seed, same engine version: same trace, byte for byte. Nondeterminism is opt-in and labelled.
4. **Verifiable.** The system proves its own correctness continuously: reference models with analytical solutions, runtime invariants, and calibration reports with explicit error bounds. A result without an accuracy statement is not a result.
5. **Self-improving within limits.** Calibration and optimization run as closed loops with declared stopping criteria, budgets and acceptance thresholds. The system reports what it changed and why; it never silently rewrites a model.
6. **Open standards first.** Data in and out uses existing formats (JSON Schema, Parquet, OPC UA, MQTT, FMI, AutomationML) before anything proprietary.
7. **Small, composable core.** The engine knows nothing about machines; machines are components built on engine primitives. New domains are new component packages, not engine changes.

## 3. Architecture

The crates share one data flow: a model file is validated and typed, the engine runs it with registered components, results and trace are compared with observed data, and the calibrator returns a fitted model that re-enters the loop.

&#91;embedded content: crate architecture · one calibration loop\]

`simcode-core` has no I/O and depends only on the units crate; every other crate depends on the core, never the other way round. The Python bindings and the MCP server are thin layers over the same public Rust API, so all three clients see identical behaviour.

## 4. Model format and component library

A model is a YAML or JSON document validated against a published JSON Schema, which is generated from the Rust types (default: \`schemars\`) so that the two can never drift.

**Model document**

```yaml
simcode: "1.0"           # format version, semver
model:
  name: line-a
  time_unit: s
  seed: 42
experiment:
  type: terminating        # or steady_state, which requires a warm-up rule
  horizon: 8h
  precision: { throughput(sink): 2% }   # replications until the CI half-width is within 2 %
components:
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.08 }
    mtbf: 6h
    mttr: { dist: exponential, mean: 12min }
  - id: buf1
    type: Buffer
    capacity: 20 parts
  - id: weld
    type: Machine
    cycle_time: 45s
    setup: { per_variant: 4min }
flow:
  - [source, cut, buf1, weld, sink]   # shorthand for a chain of edges
shifts:
  - { calendar: two-shift, applies_to: [cut, weld] }
observe:
  - throughput(sink)
  - state_shares(cut, weld)
  - wip(buf1)
```

**Rules**

- Every quantity carries a unit (`42s`, `6h`, `20 parts`). Unit mismatches are load-time errors.
- Every stochastic parameter is a named distribution with explicit parameters; bare numbers mean constants.
- Every component has an `id` that is unique in the document; references are checked at load time.
- Unknown keys are errors, not warnings.
- A model file is self-contained or uses `include:` with content hashing, so a run can always be reproduced from a model hash.

**Materials and bills of materials**

- Every flowing item has a declared material type with a unit (`parts`, `kg`, `m`, `l`). Continuous materials are handled as discrete lots of a declared size in v1.
- A bill of materials (BOM) is declared once per product variant and referenced by Assembly stations. Multi-level BOMs are modelled by chaining assemblies, each producing a subassembly that the next one consumes.
- Inputs arrive from Sources (purchased parts), Stores (inventory with a replenishment policy) or upstream stations (subassemblies).
- Every produced unit carries a lineage record of the inputs it consumed, so results report material consumption, WIP and scrap per input material, and a finished unit can be traced back to its inputs.

```yaml
materials:
  - { id: frame, unit: parts }
  - { id: bolt, unit: parts }
  - { id: sealant, unit: l }
products:
  - id: frame-assembly
    bom:
      - { material: frame, qty: 1 parts }
      - { material: bolt, qty: 6 parts }
      - { material: sealant, qty: 0.2 l }
components:
  - id: asm
    type: Assembly
    produces: frame-assembly
    cycle_time: 3min
  - id: qc
    type: Inspection
    inspection_time: 40s
    scrap_rate: 0.02
    rework_rate: 0.05
    rework_to: asm
```

**Core component library (v1)**

| Component | Parameters | Invariants checked at runtime |
| --- | --- | --- |
| Source | material, arrival distribution, variant mix, batch size | units created = units emitted |
| Machine | cycle time, MTBF, MTTR, setup matrix, capacity | never more than `capacity` units in process; state transitions follow idle → busy → blocked / down |
| Assembly | product and its BOM, cycle time, MTBF, MTTR, input buffers per material | starts only when every BOM input is present in the required quantity; consumes exactly the BOM quantities; emits one product unit per cycle |
| Split | output materials and quantities per input unit, cycle time, yield distribution | outputs per consumed input match the declared or sampled yield |
| BatchProcess | min and max batch size, process time, start policy (full batch or maximum waiting time), capacity | batch size within bounds; no unit enters a running batch; all units of a batch leave together |
| Inspection | inspection time, scrap rate, rework rate, rework destination | units inspected = units passed + units scrapped + units sent to rework |
| Buffer | capacity, discipline (FIFO, LIFO, priority) | 0 ≤ fill ≤ capacity |
| Conveyor | length, speed, spacing | units keep order; transit time = length / speed |
| Store | materials, capacity, replenishment policy, lead time | stock never negative |
| Operator | skills, walk times, shift | one task at a time |
| ShiftCalendar | shift pattern, breaks, holidays | a resource is unavailable exactly when the calendar says so |
| Sink | accepted materials | units consumed = units delivered |

Across the whole model the engine checks conservation at every event, per material type: units created = units in process + units in buffers and stores + units consumed by assembly or split + units scrapped + units delivered to sinks. A violation aborts the run with the event trace that led to it.

**Extensibility.** A component is a Rust type implementing a `Component` trait: a typed parameter struct, a behaviour expressed as a state machine over engine events, and a list of invariants. Components register at compile time through a registry (default: the `inventory` crate), and the schema generator picks them up automatically. Python users can prototype components through the bindings, but a component ships in the library only as Rust code with its tests.

**Routing and control logic**

- The `flow` section is a directed graph, not a chain: edges may branch, merge and loop back (rework). A plain list of ids is shorthand for a chain.
- Edges can carry a condition on unit attributes (product, variant, quality state) or a split ratio. Conditions use a small typed expression language that is parsed and type-checked at load time and has no loops, no I/O and no user functions.
- A step can name a pool of equivalent resources with a declared choice rule: first free, shortest queue, lowest utilization, or preferred with fallback.
- Dispatching rules decide which waiting unit a resource takes next: FIFO, earliest due date, shortest processing time, minimum setup (same variant first), priority class. In v1 they are typed enum values; custom rules are Rust components.
- Release control decides when orders enter the system: push by plan, Kanban loops with a declared card count, or CONWIP with a WIP cap.
- Every control decision is written to the trace with the rule applied and the candidates it chose from, so `explain` can show why a unit went where it did.

**Orders and demand**

- Demand is either stochastic (Sources with arrival distributions) or an explicit order list: order id, product, quantity, release date, due date, priority.
- Order lists are imported from CSV or Parquet first and through ERP and MES adapters later; they are validated against the products and BOMs in the model.
- Every unit belongs to an order, so results report completion time, lateness and status per order.

```yaml
flow:
  - { from: source, to: cut }
  - { from: cut, to: [weld-1, weld-2], choose: shortest_queue }
  - { from: [weld-1, weld-2], to: qc }
  - { from: qc, to: paint, when: "variant == 'premium'" }
  - { from: qc, to: pack }
control:
  dispatch: { weld-1: earliest_due_date, weld-2: earliest_due_date }
  release: { policy: conwip, wip_cap: 40 parts }
orders:
  from: orders/week-43.csv
```

**Observables and KPIs**

KPI definitions follow ISO 22400 where it defines them, so results are comparable with what factories already report.

| KPI | Definition | Notes |
| --- | --- | --- |
| Throughput | units delivered per time unit | per product and material |
| State shares | share of time busy, idle, blocked, starved, down, in setup | per resource |
| OEE | availability × performance × quality | per machine; planned downtime taken from ShiftCalendar |
| Lead time | release to completion, per order when an order list exists, otherwise per unit | reported as a distribution, not only a mean |
| On-time delivery | share of orders completed by due date; mean and maximum lateness | requires an order list |
| Setup share | share of available time spent in setup; number of setups | per resource and per variant change |
| WIP | units in the system and per buffer over time | time-weighted |
| Scrap and rework | rates per inspection and per material | from Inspection and lineage records |

User-defined KPIs are typed expressions over these observables and are validated like every other part of the model.

**Plant extensions (v0.5)**

These features complete the model of a whole plant. They are specified now so the v0.1 format leaves room for them, and built after optimization works.

- **Transport resources.** AGV fleets, forklifts and cranes as resources with a layout graph (nodes, paths, distances), speed, load capacity, charging or refuelling, and a dispatching rule for transport orders. Invariants: a vehicle carries at most its capacity; two vehicles never occupy the same exclusive path segment.
- **Maintenance.** Planned maintenance by calendar or by operating hours, tool wear with tool changes after a declared number of cycles, and failures whose MTBF counts operating time instead of calendar time (declared per machine). Maintenance uses Operator skills, so staff shortages delay repairs.
- **Energy and cost.** Power draw per resource state (busy, idle, standby, setup, down), an electricity tariff as a time series, and cost rates for labour, material, scrap and machine hours. Results report energy, cost and CO₂ per unit, per order and per resource. Invariant: totals equal the sum of per-state contributions.
- **Hierarchical models and scenarios.** A model can be used as a component in another model through a declared interface (input and output ports, exposed parameters), so a line becomes a building block of a plant. A named scenario is a set of overrides on a base model; scenarios are compared without copying model files, and `compare` reports only the declared differences.
- **Result charts.** Generated from traces, never drawn by hand: a state Gantt chart per resource, WIP and stock over time, lead-time distributions, Pareto fronts. Output as SVG and as chart specifications (default: Vega-Lite) so agents and other tools can render them.

## 5. Agent interface

Agents use the system through an MCP server whose tools mirror the public Rust API one to one. The server is a Rust binary (default: the official rmcp SDK). Every tool takes and returns a typed, schema-published structure; no tool returns free text.

| Tool | Input | Output | Notes |
| --- | --- | --- | --- |
| `schema.get` | component type or `all` | JSON Schema | lets an agent discover parameters without reading docs |
| `model.validate` | model document | list of errors with path, message, fix hint | always run before `run` |
| `model.render` | model document | Mermaid or SVG of the flow | agents show the model to people |
| `run` | model, seed(s), horizon, observables | run id, metrics with confidence intervals, event trace reference | multiple seeds run in parallel |
| `trace.query` | run id, filter | events | paginated; never the whole trace at once |
| `compare` | run id, observed data reference, metrics | per-metric error, pass/fail against thresholds | the accuracy report |
| `calibrate` | model, observed data, free parameters, budget | fitted model, accuracy report, change log | see section 6 |
| `optimize` | model, decision variables, objective, constraints, budget | Pareto set or best design, run ids for evidence | see section 6 |
| `experiment` | model, factor grid | results table | design of experiments for what-if questions |
| `explain` | run id, metric | ranked contributing factors with evidence | bottleneck and root-cause analysis |
| `view.focus` | component id, simulation time, zoom level | acknowledgement that the viewer moved | points a person to what the agent found; see section 14 |
| `share.preview` | calibrated model | the exact sharing payload | preview only; sending needs a human in the CLI; see section 12 |

**Contracts**

- Idempotent reads, explicit writes: only `calibrate` and `optimize` produce new model versions, and they return them rather than overwriting the input.
- Every output that contains a number contains its uncertainty: confidence interval, number of replications, seed list.
- Every error message is actionable: field path, what was found, what is allowed, one example of a valid value.
- Budgets are mandatory on long-running tools (wall-clock seconds and maximum simulated runs) so an agent cannot start an unbounded loop.
- The MCP server is stateless across calls except for a content-addressed cache of runs; a run id is the hash of model, seed, horizon and engine version.

## 6. Calibration and self-optimization

The system closes two loops on its own: one that fits a model to reality, one that searches a model for better designs. Both are bounded, logged and reversible.

**Calibration loop (fit model to observed data)**

1. Load observed data as a typed time series: throughput per interval, machine states, buffer levels, with timestamps and units.
2. Replay the same horizon in simulation with N seeds; compute each metric with a confidence interval.
3. Score the gap per metric: MAPE for rates, Wasserstein distance for distributions (cycle times, downtime lengths), state-sequence agreement for machine states.
4. Adjust the declared free parameters with a derivative-free optimizer (Bayesian optimization by default, Nelder-Mead and CMA-ES selectable), within declared bounds.
5. Stop when every metric is within its acceptance threshold, the budget is exhausted, or improvement stalls for k iterations.
6. Emit a calibration report: starting and fitted parameters, per-metric error before and after, number of runs, seeds, and a verdict (`calibrated`, `partially calibrated`, `rejected`). A rejected model is returned with the metric that failed, never silently accepted.

**Optimization loop (find better designs)**

- Decision variables are declared in the model (`decide: buf1.capacity in [5, 50]`); nothing outside that set is changed.
- Objectives and constraints are typed expressions over observables (`maximize throughput(sink) subject to wip(buf1) <= 30 parts`).
- Multi-objective by default; the output is a Pareto set with the runs that back each point.
- Every candidate is evaluated with common random numbers across designs, so differences are not noise.

**Guard rails**

- A model gains a calibration certificate (data hash, engine version, accuracy, date) that is checked on each run; a stale or missing certificate is reported.
- Overfitting check: a held-out slice of the observed data must score within threshold; otherwise the verdict is `partially calibrated`.
- Parameter drift is tracked across calibrations; a drift beyond a declared bound raises a warning that the physical system may have changed.
- The `explain` tool must be able to justify any fitted parameter change with the metric it improved.

**Output analysis**

Most wrong simulation results come from bad statistics, not bad models. The engine therefore makes the statistical method part of the model and refuses runs that leave it undefined.

- Every experiment declares its type: **terminating** (a natural end, such as one shift or one order list) or **steady-state** (long-run behaviour). The loader rejects a steady-state experiment without a warm-up rule.
- **Warm-up** is a fixed period or detected automatically (default: MSER-5); the detected length is reported, and statistics exclude it.
- **Replications** are chosen to reach a declared relative precision for each target KPI with a sequential procedure, within the budget. The report states the achieved confidence interval half-width and whether the precision was reached.
- Confidence intervals come from independent replications; batch means are allowed for single long steady-state runs only after an autocorrelation check passes.
- Design comparisons use paired differences with common random numbers and correct for multiple comparisons (default: Bonferroni). When the interval of a difference contains zero, the result says "no significant difference" instead of ranking noise.
- The method is itself tested: on analytical reference cases, 95 % confidence intervals must contain the true value in 93 to 97 % of 1,000 seeded experiments, and warm-up detection must recover a known transient.

## 7. Verification and quality

Correctness is enforced in four layers, from the type checker to the physical world. CI fails if any layer regresses.

**Layer 1: types**

- Rust stable with a declared minimum supported version, tested in CI. `#![forbid(unsafe_code)]` in every crate; any exception needs an ADR, an isolated module and Miri in CI.
- `clippy` with `pedantic` lints and `-D warnings`. `unwrap`, `expect` and `panic!` are denied in library code; panics are reserved for violated internal invariants.
- Units are types (default: newtypes over `uom`), so `Duration + Rate` does not compile.
- Component states are enums handled with exhaustive `match` and no wildcard arms: adding a state forces every handler to be updated.
- Errors are typed enums (default: `thiserror`), never strings; each variant maps to a field-path error in the MCP layer.
- The Rust model types are the single source of truth. JSON Schema and Python type stubs (`.pyi`) are generated from them and committed; CI fails if either is stale.
- The Python bindings (default: PyO3 with maturin) are checked with `pyright` in strict mode against the generated stubs.

**Layer 2: tests**

| Kind | Tool (default) | What it proves | Gate |
| --- | --- | --- | --- |
| Unit | `cargo test`, `cargo-nextest` | each component and each engine primitive in isolation | required |
| Property-based | `proptest` | invariants hold for generated models: conservation, bounds, ordering, determinism under replay | required |
| Analytical reference | `cargo test` with `rstest` | M/M/1, M/M/c, tandem queues and serial lines with closed-form throughput and WIP; simulation within the confidence interval of theory | required |
| Golden traces | `insta` snapshots | reference models reproduce stored event traces byte for byte for a given engine version | required |
| Schema contract | `cargo test`, `pytest` on the bindings | every MCP tool and Python function rejects malformed input with a path-level error and accepts every documented example | required |
| Mutation | `cargo-mutants` | the suite kills ≥ 80 % of mutants in `simcode-core` (initial target) | required for core, advisory elsewhere |
| Performance | `criterion` locally, instruction counts in CI (`iai-callgrind`) | no regression > 10 % on reference models; instruction counts avoid noisy CI runners | required |
| Fuzz | `cargo-fuzz` | random model files never crash the loader or engine; they are rejected at load or abort with an invariant report | nightly |
| Unsafe code | Miri | only if an `unsafe` exception exists | required when applicable |

Line coverage ≥ 90 % overall and 100 % branch coverage in `simcode-core` (initial targets), measured with `cargo-llvm-cov`. Coverage is a floor, not the goal; the analytical and property tests are what give confidence.

**Layer 3: runtime self-checks**

- Every run executes the invariant set of every component plus global conservation at each event. Checks are on by default; a `--fast` flag disables them for calibration inner loops after one checked warm-up run.
- Every run records its provenance: model hash, engine version, seed list, platform. A result without provenance is rejected by `compare`.
- Replay mode re-executes a run from its trace and asserts identical state at every event.

**Layer 4: validation against reality**

- Calibration reports with explicit thresholds (section 6) are the acceptance test for a model, and are stored next to it.
- A public benchmark set of anonymized production datasets with known outcomes; each release reports its calibration accuracy on them, and a drop fails the release.

**Tooling baseline**

- `rustfmt`, `clippy` pedantic, `cargo-deny` for licenses, duplicate crates and advisories; `ruff` and `pyright` strict for the Python package; TypeScript in strict mode with ESLint (default) for the viewer.
- `#![deny(missing_docs)]` in every crate and a doc test for every public item (`cargo test --doc`).
- CI on Linux, macOS and Windows; Python wheels built and tested for the three latest Python versions.

## 8. Data adapters and standards

Adapters convert outside data into one internal observation format, so calibration and comparison never depend on where data came from.

**Internal observation format.** A typed table with columns `timestamp`, `entity`, `signal`, `value`, `unit`, stored as Parquet. Every adapter emits this and nothing else.

| Direction | Format or protocol | Version | Use |
| --- | --- | --- | --- |
| In | CSV, Parquet | v0.1 | historical data exports, the default path |
| In | OPC UA (read-only client) | v0.2 | machine states and counters from controllers |
| In | MQTT (subscriber) | v0.2 | sensor and counter streams |
| In | AutomationML | v1.0 | import of line topology from engineering tools |
| In | Asset Administration Shell (AAS) | after v1.0 | machine master data and capabilities |
| Out | JSON Schema | v0.1 | the model format and every tool contract |
| Out | Parquet, CSV | v0.1 | results, traces, calibration reports |
| Out | Mermaid, SVG | v0.1 | rendered model diagrams |
| Out | FMI 3.0 (co-simulation) | after v1.0 | embedding the engine in other simulators |

**Rules**

- Adapters are read-only toward production systems. The package contains no code path that writes to a controller.
- Each adapter ships with a recorded fixture so its tests run without hardware.
- Timestamps are stored in UTC with the source time zone kept as metadata.
- Missing or implausible values (negative counters, gaps) are flagged in a data-quality report before calibration starts; calibration refuses data below a declared quality score.

## 9. Non-functional requirements

| Area | Requirement |
| --- | --- |
| Performance | Initial targets: ≥ 1 million events per second per core on the reference line model (20 components, 8 h horizon) with invariant checks off, ≥ 200,000 with checks on. Replications run in parallel across cores (default: `rayon`); a single replication stays single-threaded for determinism. |
| Reproducibility | Identical trace for identical model hash, seed and engine version on every supported platform. The RNG is counter-based (default: Philox or PCG64), one stream per component, so adding a component does not shift the others. No floating-point operation whose result depends on platform or thread scheduling. |
| Model size | 1,000 components and 10 million events in a run within 2 GB of memory (initial target). Traces stream to disk, never fully in memory. |
| Observability | Structured logs and spans (default: `tracing` with an OpenTelemetry exporter) for runs and tool calls; a progress stream for long calibrations. |
| Security | No network access from the engine crates. Adapters are opt-in Cargo features. Models are data, never executed as code; components are compiled in, never loaded from model files. |
| Documentation | Every public item documented; a tutorial that builds, calibrates and optimizes one line end to end in Rust and in Python; the MCP tool reference generated from the schemas. |
| Licensing | Apache-2.0 for the open core (default; see section 13); CC-BY-4.0 for documentation and benchmark data. `cargo-deny` rejects incompatible dependency licenses in CI. |
| Compatibility | Semantic versioning for crates and the Python package. The model format version is independent of the package version; the engine reads every format version back to 1.0 and migrates on request. |
| Accessibility for agents | Every error, report and result is machine-readable first and human-readable second. An agent must be able to complete the end-to-end tutorial from the MCP tools alone, with no access to the docs. |
| Viewer smoothness | Frame time p95 ≤ 16.7 ms and p99 ≤ 25 ms on the reference load, no main-thread task over 50 ms, scrubbing within 200 ms (initial targets; details in section 14). |

## 10. Repository and delivery

One Cargo workspace plus one Python package, published to crates.io and PyPI on every tagged release.

```text
simcode/
  Cargo.toml             workspace, shared lints and dependency versions
  crates/
    simcode-units/       unit newtypes and parsing ("42s", "6h", "20 parts")
    simcode-core/        event loop, RNG streams, trace, invariants; no I/O
    simcode-model/       model types, expression language, loader, schema generation, migrations
    simcode-components/  Machine, Assembly, Split, BatchProcess, Inspection, Buffer, Conveyor, Store, Operator, ShiftCalendar, Source, Sink
    simcode-stats/       KPIs, warm-up detection, replication control, confidence intervals
    simcode-calibrate/   observation format, metrics, optimizers, reports
    simcode-adapters/    csv, parquet, opcua, mqtt, automationml (each a Cargo feature)
    simcode-share/       opt-in sharing client: preview, send, withdraw, receipts
    simcode-wasm/        WebAssembly build of the engine for the viewer
    simcode-mcp/         MCP server binary, tool contracts, run cache
    simcode-cli/         `simcode validate | run | compare | calibrate | optimize | render | view | share`
  python/                PyO3 bindings, generated .pyi stubs, pytest suite
  viewer/                TypeScript web app, component symbols, visual regression tests
  benchmarks/            reference models, analytical cases, anonymized datasets
  docs/                  tutorial and guides; rustdoc and Python reference generated
  schemas/               committed JSON Schemas, one per format version
```

**Tooling**

- Shared `Cargo.lock`; workspace-level lint configuration so every crate runs the same `clippy` rules. Python wheels built with maturin as abi3 wheels for Linux, macOS and Windows (default).
- Conventional Commits; changelog generated from them (default: `git-cliff`). A commit that touches `simcode-core` must reference a test.
- CI pipeline order: rustfmt → clippy → cargo-deny → unit and property tests → analytical and golden tests → schema and stub freshness → coverage gate → Python binding tests → WebAssembly build → viewer type check and tests → mutation (core only) → engine and frame-time benchmarks → docs build. Every step is a required check on `main`.
- Releases are cut by tag: crates to crates.io, wheels to PyPI with trusted publishing, docs deployed (default: mdBook plus rustdoc), benchmark report attached to the GitHub release.
- Nightly jobs: fuzzing, the full calibration benchmark set, `cargo audit`.

**Open-source hygiene**

- `README` with a 30-second example, `CONTRIBUTING` with the CI contract above, `CODE_OF_CONDUCT`, `SECURITY` with a disclosure address, an architecture decision record (ADR) folder from day one.
- Issue templates for bug, component proposal and adapter proposal; a component proposal must include its invariants.
- A `good first issue` label is applied only to issues with a failing test already written.

## 11. Release tiers and acceptance criteria

Each tier is shippable on its own and is accepted only when every criterion in its row is demonstrated by an automated test or a benchmark report. The implementing agent derives the roadmap from this table.

| Tier | Scope | Acceptance criteria |
| --- | --- | --- |
| v0.1 Engine | units, core, model format, Source / Machine / Buffer / Sink, CLI `validate` and `run`, flow graph with conditions and resource pools, dispatching rules, terminating and steady-state experiments with warm-up and replication control, throughput, state shares, WIP and lead time, CSV adapter, Python bindings for `validate` and `run` | analytical reference suite passes (M/M/1, M/M/c, 3-station serial line); 95 % confidence intervals contain the true value in 93 to 97 % of 1,000 seeded experiments; warm-up detection recovers a known transient; deterministic replay test passes on all three platforms; schema and stubs committed and fresh; 100 % branch coverage in core; clippy pedantic clean, no `unsafe`; README example runs in under 1 s from Rust and from Python |
| v0.2 Agent | MCP server with `schema.get`, `model.validate`, `model.render`, `run`, `trace.query`, `compare`; Conveyor, Store, ShiftCalendar, Assembly, Split, BatchProcess, Inspection, materials and BOMs, order lists, release control (push, Kanban, CONWIP), OEE, on-time delivery, setup share, scrap and rework KPIs; Parquet, OPC UA, MQTT adapters; WebAssembly engine build; viewer with line and station levels, state glow and live reload | an agent completes the tutorial from MCP tools alone in an automated test; every tool rejects every malformed example with a path-level error; adapters pass fixture tests; every KPI matches hand-computed reference cases; WIP never exceeds a CONWIP cap; per-material conservation and lineage hold on a reference model with a three-level BOM, scrap and rework; the viewer renders every reference model with a symbol for every component type and passes visual regression tests; performance targets met or revised in an ADR with measurements |
| v0.3 Calibrate | observation format, metrics, `calibrate`, calibration certificate, data-quality report, sharing client with preview and withdrawal, parameter library priors | on the public benchmark set, ≥ 80 % of datasets reach `calibrated` within the published budget (initial target); held-out overfitting check implemented; the full test suite passes with network access blocked; the previewed sharing payload equals the sent payload byte for byte; library priors measurably reduce calibration runs on the benchmark set; certificate validated on every `run` |
| v0.4 Optimize | `optimize`, `experiment`, `explain`, Operator component, common random numbers, viewer playback with scrubbing, scenario and calibration comparison views, parameter editing with write-back | optimizer recovers the known optimum on three synthetic benchmarks; `explain` names the true bottleneck on every reference model; Pareto output verified against brute force on a small grid; the viewer shows the same state as the engine at any replayed event; parameter edits round-trip to the model file without other changes |
| v0.5 Plant | transport resources (AGV, forklift, crane), planned maintenance, tool wear and usage-based failures, energy and cost per resource state, hierarchical models and named scenarios, result charts, semantic zoom from plant to station through the model hierarchy, GOVERNANCE document | energy and cost totals reconcile with per-state sums on reference models; a line reused as a component in a plant model gives the same KPIs as when run alone, within confidence intervals; scenarios differ from their base only in declared overrides; charts are generated from traces in CI; zooming from plant to station keeps playback in sync and meets the frame-rate target |
| v1.0 Stable | format version 1.0 frozen, migrations, AutomationML import, docs complete, mutation gate on core | all previous gates plus: mutation score ≥ 80 % in core; no `unwrap` or `expect` in library code; every public item documented; two external contributors have merged a component through the proposal template |

After v1.0: FMI co-simulation, AAS import, 3D views built from manufacturers' machine models (glTF). A hosted run service is out of scope unless separately funded.

## 12. Data sharing and the parameter library

The project collects data only through explicit, informed and revocable opt-in. Everything works fully offline; nothing leaves a user's machine without a deliberate action by a person.

**What can be shared**

- Calibrated parameters with their uncertainty: distributions of cycle times, MTBF, MTTR, setup times, scrap rates and yields.
- Context tags: component type, machine category from a published taxonomy, industry code (NACE), and the accuracy from the calibration certificate.
- Never: raw observations, timestamps, model topology, order data, company or site names, or anything about individual operators. The sharing schema has no fields for these, so they cannot be sent by mistake.

**How sharing works**

1. Sharing is off by default and is switched on per model.
2. `simcode share --preview` shows the exact payload, field by field, before anything leaves the machine. The payload is a typed, schema-published document.
3. A person confirms; the payload is sent and a receipt (id, hash, date) is stored next to the model.
4. Any contribution can be withdrawn with its receipt; withdrawn data is excluded from all later library releases.

Agents can call `share.preview` through MCP, but sending requires a human confirmation in the CLI. An agent can never share data on its own.

**Parameter library: give to get**

- Contributors get access to the full library: priors per machine category and industry that calibration uses as starting points, so new models start realistic and converge faster.
- A value is published only when it aggregates contributions from at least five organisations (initial target). Categories below the threshold stay hidden, and aggregates are released as distributions, never as individual contributions.
- Library releases are versioned and signed. A calibration certificate records the library version it used, so results stay reproducible.
- Everyone can use the public subset: synthetic values and openly licensed benchmark data.

**No telemetry.** The software sends no usage statistics, crash reports or update checks unless a user enables each one explicitly. Outside the opt-in adapters and the sharing client, the default build contains no network code.

**Purpose limitation.** Shared data is used only to build and publish the parameter library. Any other use, such as research reports or industry benchmarks, requires a new, separate opt-in from each contributor, and contributions made before that opt-in are never reused for the new purpose. The contribution terms state this in plain words.

**Compliance.** Privacy by design under the GDPR; the library service is operated in the EU; contribution terms and consent texts are reviewed by a lawyer before launch.

## 13. Licensing and sustainability

Everything is free for every user. The project charges for nothing; what it gains is the parameter library, which grows with every contribution.

- All code (engine, components, CLI, MCP server, Python bindings, adapters, viewer) is open source (default: Apache-2.0). Documentation and the public subset of the parameter library are CC-BY-4.0.
- The full parameter library is free for contributors (give to get); the public subset is free for everyone.
- There are no paid tiers, no paid adapters and no features held back. Everything needed to build, run, calibrate, optimize and view a model stays in the open project.
- Running costs (the library service, CI, the legal review of the contribution terms) are covered by open-source grants or donations. The core never depends on a commercial partner or a hosted service to work.
- Whether contributors sign a Contributor License Agreement is an open decision (section 15): a CLA keeps a later license change possible but deters some contributors.

## 14. Visual interface

A browser-based viewer shows the process and lets people zoom from the whole plant down into a single station, while the simulation plays. Models are still written as code; the viewer shows, explains and replays them, and edits parameters only.

**Semantic zoom**

Zooming does not just enlarge the picture; each level shows the information that matters at that scale. The levels follow the model hierarchy (section 4, hierarchical models); flat models are grouped by an optional `group:` tag on components.

| Level | What is drawn | What glows |
| --- | --- | --- |
| Plant | halls and areas as blocks, material flows between them as lines | each block by its aggregate state: utilization, and red if anything inside is down |
| Area | lines and stores as blocks | each line by its throughput against target; its bottleneck highlighted |
| Line | stations, buffers, conveyors, transport paths; moving units | each resource by its current state |
| Station | queue contents, current order and variant, setup, assigned operator, internal state machine | the active state and the next scheduled event |
| Detail | parameters and their distributions, simulated against observed values, invariant status, calibration fit | parameters that drifted or failed calibration |

The glow is the guide: red at plant level means something inside is down, and following the glow down the levels leads to the root cause. When `explain` names a bottleneck, the viewer can highlight the path to it at every level.

**Visual language**

- Isometric fine-line drawings in which activity is light: a working machine glows, an idle one is dark, a failed one glows red, a blocked buffer pulses. The iso-glow skill (MIT license) is the reference for this style.
- Every component type has a symbol in the library, drawn once and reused; a component proposal must include its symbol. Symbols scale with the zoom level: a simple glyph from far away, the full drawing up close.
- Colour is never the only signal: states also differ in pattern and icon, for colour-blind users and for printouts. Light and dark themes. All interface text is translatable from the start; German and English ship with the first viewer release.
- Layout is computed automatically from the flow graph (default: ELK layered layout) and can be adjusted by hand; positions are stored in a separate `*.layout.json` file so the model itself stays clean.

**Time controls**

Play, pause, speed, scrub and jump to any event, the same at every zoom level; zooming while playing keeps the simulation clock in sync. Because runs are deterministic, the viewer can replay any trace and show the exact state at any event.

**Working with agents**

- `simcode view model.yaml` opens the viewer locally and watches the file: when an agent changes the model, the view updates within a second, so a person sees what the agent builds and can step in.
- An MCP tool `view.focus` lets the agent point the person to a component at a given time, for example the station it just identified as the bottleneck.
- Parameter edits in the viewer are written back to the model file as a reviewable diff that the person confirms; no other part of the file changes.

**Architecture**

- A static web app in TypeScript with `strict` mode; its types are generated from the same JSON Schema as everything else.
- The engine runs in the browser as WebAssembly inside a Web Worker. There is no backend: production data never leaves the machine.
- Rendering runs on the GPU and is specified under Smoothness below.
- Quality gates: component tests, visual regression tests on the reference models (default: Playwright screenshots), a test that every registered component type has a symbol, and the frame-time tests specified under Smoothness.

**Smoothness**

The viewer must feel fluid at every zoom level and playback speed. "Smooth" is defined by measurable frame times, tested in CI, not by impression.

| Measure | Target (initial) |
| --- | --- |
| Frame time | p95 ≤ 16.7 ms (60 fps), p99 ≤ 25 ms; 120 Hz displays use their full rate when the budget allows |
| Main-thread long tasks | none longer than 50 ms during play, pan, zoom or scrub |
| Input response | pan and zoom react in the next frame; any click responds within 100 ms |
| Scrubbing | jumping to any point of an 8 h run shows the correct state within 200 ms |
| Reference load | 500 stations and 5,000 moving units at line level on a five-year-old laptop with integrated graphics |
| Live reload | an agent's model change appears within 1 s without flicker, keeping camera position and playhead |

**How it is achieved**

- **Simulation and drawing are separate.** The engine runs in a Web Worker and simulates ahead of the playhead with a buffer of at least two seconds of playback. The main thread only draws; it never waits for the simulation. State reaches the renderer as typed-array deltas (shared memory where the browser allows it; the local server sends the required cross-origin isolation headers).
- **Continuous motion from discrete events.** The engine only knows events, which would make units jump. The renderer interpolates positions between event times, so units glide along conveyors and transport paths at any playback speed.
- **Instant scrubbing.** The engine stores state snapshots at fixed intervals; jumping in time restores the nearest snapshot and replays only the events after it, instead of rerunning the simulation from the start.
- **GPU drawing throughout.** Symbols and units are drawn with instancing, the glow with a bloom shader, and labels with signed-distance-field fonts. No SVG or CSS filters during animation, because they are the most common cause of stutter.
- **Draw only what is visible.** Off-screen elements are culled with a spatial index. Only the current zoom level is drawn in full; higher levels show units as counts or density. The detail of the next level is prepared in the background as the zoom approaches it, and levels crossfade instead of switching abruptly.
- **No garbage-collection pauses.** No allocations per frame in the render loop: object pools and preallocated typed arrays.
- **Adaptive quality.** If the frame budget is exceeded, the viewer first lowers glow quality, then shows units as density, and only then drops frames. The current quality level is visible in a debug overlay.

**Tests**

- Automated frame-time benchmarks on the reference models in a browser with GPU acceleration, recording p95 and p99 frame times, long tasks and scrub latency; a regression of more than 10 % fails CI.
- Before each release, a manual check on the reference low-end laptop, recorded in the release notes.

## 15. Open decisions for the implementing agent

These choices are left open on purpose. Decide each one in an ADR before the tier that needs it, with the trade-off stated.

- [ ] **Event queue.** Binary heap, pairing heap or calendar queue in `simcode-core`. Decide by benchmark on the reference models before v0.1.
- [ ] **Model language.** YAML with a strict loader, or JSON only with YAML as a convenience layer. Default recommendation: JSON is canonical; YAML is accepted and normalized to JSON before hashing.
- [ ] **Distribution syntax.** Inline objects (`{dist: lognormal, mean: 42s, sigma: 0.08}`) versus a compact string form (`lognormal(42s, 0.08)`). Pick one; both must round-trip through the schema.
- [ ] **Trace storage.** Arrow and Parquet via `arrow-rs`, or an append-only binary format with an Arrow reader. Decide on the 10-million-event memory target.
- [ ] **Optimizers.** Implement Bayesian optimization and CMA-ES in Rust for determinism and a self-contained build, or call Python optimizers (for example Optuna) through the bindings in v0.3 for speed of delivery. State the effect on reproducibility.
- [ ] **Async boundary.** The engine stays synchronous; the MCP server and network adapters need an async runtime (default: `tokio`). Define where the boundary sits and how cancellation and budgets cross it.
- [ ] **Python API shape.** A thin one-to-one binding, or an idiomatic Python layer on top. Default recommendation: thin binding, generated where possible, so the three clients cannot drift.
- [ ] **License.** Apache-2.0 alone or MIT/Apache-2.0 dual licensing for the open core; whether to require a CLA at all, and if so, its text and signing bot in place before the first external pull request.
- [ ] **Initial targets.** Validate every number marked as an initial target with a prototype, and record the confirmed or revised value in an ADR.
- [ ] **Benchmark data.** Which anonymized datasets can be published under CC-BY-4.0 from day one, and which synthetic generator stands in until then.
- [ ] **Project name.** `simcode` is a placeholder; check crates.io, PyPI and trademark availability before the first release.
- [ ] **Parameter library.** The machine taxonomy, the aggregation threshold and anonymization method (at least five organisations; consider differential privacy), and who operates the service.
- [ ] **Expression language.** The grammar and type rules for edge conditions and user-defined KPIs, kept small enough to verify exhaustively.
- [ ] **Viewer rendering stack.** A 2D canvas or WebGL library (for example PixiJS) for the isometric views, or three.js from the start to share one scene graph with a later 3D view. Also WebGL2 as the baseline with WebGPU as an optional faster path, or WebGPU only. Decide by measuring the frame-time targets on the reference low-end laptop, and on bundle size.
