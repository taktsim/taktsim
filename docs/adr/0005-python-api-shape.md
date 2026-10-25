# ADR-0005: Python API shape

- Status: accepted (2026-10-25)
- Date: 2026-10-25
- Blocks: M12 (Python bindings)
- Spec: §15 "Python API shape", §3, §7 Layer 1 (stubs, pyright strict)

## Context

The Python bindings, the CLI and the MCP server are thin layers over one
public Rust API, so all three behave identically (§3). v0.1 exposes
`validate` and `run`. Stubs (`.pyi`) are generated from the Rust types,
committed, and checked for freshness; the package passes `pyright` strict.
Spec default recommendation: thin binding, generated where possible.

## Options

- **P1. Thin one-to-one binding.** One Python function per Rust API
  function; Rust result types exposed as frozen `pyclass` objects with
  typed attributes; errors as one exception type that carries the typed
  error list.
- **P2. Idiomatic Python layer** on top of a private binding: builder
  objects, pandas DataFrames as return values, context managers, etc.
- **P3. P1 plus a small, pure-Python convenience module** (e.g.
  `to_pandas()` / `to_polars()` on result tables, path-or-string model
  input), each helper only calling P1 functions.

## Trade-offs

| | P1 | P2 | P3 |
| --- | --- | --- | --- |
| Drift between clients | none | high: a second API to keep in sync | low: helpers add no behaviour |
| Stub generation | fully generated | hand-written for the idiomatic layer | generated, plus typed helpers |
| Python feel | plain | best | good |
| Heavy dependencies | none | pandas required | optional extras |

## Recommendation

**P1 in v0.1**, designed so P3 can be added without breaking anything:

- Functions: `taktsim.validate(model) -> ValidationReport` and
  `taktsim.run(model, *, seeds, horizon=None, observe=None, budget=None)
  -> RunResult`, mirroring the Rust signatures. `model` accepts a path or
  a document string, as the CLI does.
- Validation failures are returned as a report, not raised, matching the
  Rust API; I/O or budget errors raise `taktsim.TaktsimError` carrying the
  same typed error as Rust.
- `run` releases the GIL; replications run on the Rust thread pool.
- Stubs generated with `pyo3-stub-gen` (default to confirm in M12);
  freshness check in CI; `pyright --strict` on the stubs and the test
  suite.
- abi3 wheels with the minimum set to the oldest of the three latest
  Python versions (3.13 if 3.15 has shipped on its October 2026 schedule;
  checked at M12). Free-threaded builds are not covered by abi3 and are
  out of scope for v0.1.
- Results tables exposed through the Arrow C data interface from v0.2 on,
  so pandas and Polars can read them without a hard dependency.

## Consequences

- The Python API looks like the Rust API; the docs say so and show the
  tutorial in both.
- Anything Pythonic is an optional helper, never the only way to reach a
  feature.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Thin binding in v0.1 with optional helpers later: agreed?
2. Should `validate` raise on invalid models instead of returning a
   report? Recommended: return a report, as in Rust and MCP.
