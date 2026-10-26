# ADR-0003: Distribution syntax

- Status: accepted (2026-10-25); MCP mentions amended by ADR-0019
  (2026-10-26)
- Date: 2026-10-25
- Blocks: M4 (model format)
- Spec: §15 "Distribution syntax", §4 Rules

## Context

Every stochastic parameter is a named distribution with explicit
parameters; a bare quantity means a constant. Spec §15: pick inline objects
(`{dist: lognormal, mean: 42s, sigma: 0.08}`) or a compact string
(`lognormal(42s, 0.08)`); whichever is chosen must round-trip through the
schema.

Agents are first-class users and discover parameters from `schema.get`
without reading docs (§5). Editors with JSON Schema support (VS Code YAML
extension and others) give completion and inline errors from the same
schema.

## Options

- **D1. Inline object only.** Each distribution is a tagged variant in the
  schema (`oneOf` on `dist`), with named, typed, unit-checked fields.
- **D2. Compact string only.** `lognormal(42s, 0.08)`. The schema can say
  only "string matching a pattern"; the real checks happen in a separate
  parser.
- **D3. Object canonical, string accepted as input sugar**, normalized to
  the object before hashing.

## Trade-offs

| | D1 object | D2 string | D3 both |
| --- | --- | --- | --- |
| Schema can describe parameters | yes, per field | no, regex only | yes for the object |
| Error messages | field path (`components[0].cycle_time.sigma`) | character offset in a string | both kinds |
| Positional ambiguity | none, parameters are named | `lognormal(42s, 0.08)`: is 0.08 sigma or CV? | string form keeps it |
| Brevity | verbose | compact | compact where wanted |
| Things to test | one form | parser plus schema | both, plus normalization |

## Recommendation

**D1 for v0.1.** It is the only form whose parameters the schema can
describe, which matters most for agents and for error messages with a
field path. D3 can be added later without breaking files, because the
string would normalize to the object before hashing.

Proposed v0.1 set, all parameters named and unit-checked (`T` = the
quantity's dimension, e.g. time):

| `dist` | Fields | Notes |
| --- | --- | --- |
| (bare quantity) | — | constant, e.g. `cycle_time: 45s` |
| `exponential` | `mean: T` | |
| `normal` | `mean: T`, `sd: T` | truncated at 0 for non-negative quantities; truncation stated in the schema description |
| `lognormal` | `mean: T`, `sigma: number` | `mean` is the distribution mean, `sigma` the shape (sd of the log); μ = ln(mean) − σ²/2 |
| `uniform` | `min: T`, `max: T` | |
| `triangular` | `min: T`, `mode: T`, `max: T` | |
| `gamma` | `mean: T`, `cv: number` | |
| `erlang` | `mean: T`, `k: integer ≥ 1` | |
| `weibull` | `scale: T`, `shape: number` | |

`empirical` (from observed samples) waits for the observation format in
v0.3. Load-time checks: positivity, `min ≤ mode ≤ max`, finite values.

## Consequences

- Shorthand stays available only for constants.
- The parameterization of every distribution is defined in the schema
  descriptions, which also feed the MCP tool reference (the CLI reference
  since ADR-0019).
- Sampling must be platform-independent (pure-Rust math), handled in the
  M2 RNG ADR.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Lognormal: keep `mean` + `sigma` (shape) as in the spec example, or use
   `mean` + `cv`, which engineers often find easier to read? For small
   values they are nearly equal (σ = 0.08 ↔ CV ≈ 0.080), which makes the
   choice easy to misread. Recommended: keep `sigma` as in the spec and
   name it clearly in the schema.
2. Is the v0.1 set above enough, or are other distributions needed in
   v0.1 (e.g. beta for yields comes with Split in v0.2)?
