# ADR-0012: Units implementation

- Status: accepted (2026-10-25, M1 pull request #2 merged by the owner)
- Date: 2026-10-25
- Blocks: M1
- Spec: §4 Rules ("every quantity carries a unit"), §7 Layer 1 ("units are
  types", default: newtypes over `uom`; errors with `thiserror`); ADR-0001

## Context

Every quantity in a model carries a unit, and a unit mismatch must not
compile. The spec's default is newtypes over `uom`. Three requirements from
earlier ADRs constrain the choice:

- Simulation time is integer nanoseconds (ADR-0001), so durations must be
  exact, not floating point.
- Parsing must be exact and platform-independent: `0.1h` is 360 s on every
  machine, or the model hash and the trace change.
- Errors name what was found, what is allowed and a valid example (spec §5).

## Options

- **U1. Newtypes over `uom`.** Full dimensional analysis (any product or
  quotient of units gets a type).
- **U2. Own newtypes**, one per quantity the model format needs, with
  explicit conversions between them (`Count::per(SimDuration) -> Rate`).

## Trade-offs

| | U1 `uom` | U2 own newtypes |
| --- | --- | --- |
| Integer nanosecond time | possible with an integer storage type, but conversions go through `uom`'s generic machinery | direct: `SimDuration(u64)` |
| Parsing `42s`, `20 parts` | not provided; `parts` is not a `uom` unit | exact decimal parser, written once |
| Arbitrary unit algebra | yes | only the combinations we define |
| Error messages and docs | deeply generic types | plain types, readable errors |
| Compile time, dependency size | large | none beyond `thiserror` |

The model format needs few dimensions: time, counts, rates, percentages in
v0.1; material quantities and speeds in v0.2. The algebra that `uom` offers
is not needed, and its generic types would surface in every error message
and in the generated JSON Schema.

## Decision

**U2.** `taktsim-units` defines:

- `SimDuration`, `SimTime`: `u64` nanoseconds. Parsed exactly from
  `d`, `h`, `min`, `s`, `ms`; displayed in the largest whole unit of `d`,
  `h`, `min`, `s`, otherwise decimal seconds. Sampled `f64` seconds are
  rounded half to even and truncated at zero (ADR-0001, ADR-0003).
- `Count`: whole `parts`. `Rate`: `f64` per second, parsed from `parts/s`,
  `parts/min`, `parts/h`. `Ratio`: a percentage, stored as written.
- Checked arithmetic only (`checked_add`, `checked_sub`, `checked_mul`,
  `checked_duration_since`). No `+`/`-` operators, because they would have
  to panic or wrap on overflow.
- One error type, `UnitError`, with `thiserror` (the spec's default,
  confirmed here).
- Every `Display` output parses back to the same value (property-tested).

Material dimensions (`kg`, `l`, `m`) and speeds are added in v0.2 with
their first use (BOMs, Conveyor). Their representation is decided there:
material quantities such as `0.2 l` need either fixed-point or exact
decimal storage, and that choice depends on how lineage and conservation
checks use them.

"Does not compile" is tested with `compile_fail` doc tests. Stable rustdoc
only checks that such an example fails to compile; the nightly run in CI
also checks the error code (E0308, mismatched types), so the test fails if
the example breaks for an unrelated reason.

## Consequences

- No unit algebra beyond what we write; each new combination is a small,
  explicit function.
- Rates and percentages are `f64`. Their text round-trips because Rust's
  float formatting writes the shortest text that parses back to the same
  value, and both are pure Rust, identical on every platform.
