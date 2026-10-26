# Architecture decision records

One file per decision, numbered in order of creation. Status is one of
`proposed`, `accepted`, `superseded by ADR-NNNN`, `rejected`. Only the
project owner accepts an ADR: explicitly, or, for an ADR that settles a spec
*default* or a detail inside a milestone, by merging the milestone's pull
request that proposes it. An ADR that changes the spec or an accepted ADR
always needs the owner's explicit answer. The status line is updated in a
later commit, never before the owner has decided.

A spec item marked *default* is used as-is only once an ADR accepts it.
Every number marked *initial target* is confirmed or revised in an ADR with
measurements.

## Index

| ADR | Title | Status | Blocks |
| --- | --- | --- | --- |
| [0001](0001-event-queue.md) | Event queue and simulation time | accepted | M1, M2 |
| [0002](0002-model-language.md) | Model language and canonical hashing | accepted | M4 |
| [0003](0003-distribution-syntax.md) | Distribution syntax | accepted | M4 |
| [0004](0004-trace-storage.md) | Trace storage | accepted | M3 |
| [0005](0005-python-api-shape.md) | Python API shape | accepted | M12 |
| [0006](0006-license-and-contributions.md) | License and contributor agreement | accepted | M0 |
| [0007](0007-initial-targets-v0.1.md) | Initial targets and gate levels for v0.1 | accepted | M0, M13 |
| [0008](0008-project-name.md) | Project name | accepted | M0 |
| [0009](0009-expression-language.md) | Expression language | accepted | M6 |
| [0010](0010-platforms-and-reference-hardware.md) | Supported platforms and reference hardware | accepted | M0, M2, M13 |
| [0011](0011-toolchain-and-ci-tooling.md) | Toolchain, MSRV and CI tooling | accepted | M0 |
| [0012](0012-units-implementation.md) | Units implementation | accepted | M1 |
| [0013](0013-random-streams-and-math.md) | Random streams and platform-independent math | accepted | M2 |
| [0014](0014-output-analysis.md) | Output analysis: t quantiles, replication control, time-weighted statistics, MSER-5 | accepted | M8, M9 |
| [0015](0015-expression-language-details.md) | Expression language details (amends ADR-0009) | accepted | M6 |
| [0016](0016-benchmarks-and-regression-gate.md) | Benchmarks and the instruction-count regression gate | accepted | M2 |
| [0017](0017-csv-adapter-and-observation-table.md) | CSV adapter and the observation table | accepted | M11 |
| [0018](0018-engine-architecture.md) | Engine architecture | accepted | M2b |

Open decisions from spec §15 that do **not** block v0.1, to be written
before the tier that needs them: async boundary (v0.2), viewer rendering
stack (v0.2), optimizers (v0.3), benchmark data (v0.3), parameter library
(v0.3).

## Template

```markdown
# ADR-NNNN: Title

- Status: proposed
- Date: YYYY-MM-DD
- Blocks: milestone(s)

## Context
## Options
## Trade-offs
## Recommendation
## Consequences
## Questions for the owner
```
