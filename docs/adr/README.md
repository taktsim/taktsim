# Architecture decision records

One file per decision, numbered in order of creation. Status is one of
`proposed`, `accepted`, `superseded by ADR-NNNN`, `rejected`. Only the
project owner moves an ADR from `proposed` to `accepted`. An ADR that settles
a *default* inside a milestone is proposed in that milestone's pull request
and accepted when the owner merges it.

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
| [0012](0012-units-implementation.md) | Units implementation | proposed (M1 PR) | M1 |

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
