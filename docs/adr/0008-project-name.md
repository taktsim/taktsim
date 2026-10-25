# ADR-0008: Project name

- Status: accepted (2026-10-25)
- Date: 2026-10-25
- Blocks: M0 (crate and package names, repository, `README`)
- Spec: §15 "Project name"

## Context

`simcode` is a placeholder. The name must be checked on crates.io, PyPI and
for trademarks before the first release. It appears in about a dozen crate
names, the Python package, the CLI binary, the model header key
(`simcode: "1.0"`), and the JSON Schema `$id`s. Changing it after v0.1 is
published is costly: published crates cannot be renamed, and the model
header key would become a format migration.

## Availability check (2026-10-25)

Checked through the crates.io and PyPI JSON APIs; "free" means no package
of that name exists yet. Trademark status was **not** checked.

| Name | crates.io | PyPI | Notes |
| --- | --- | --- | --- |
| `simcode` | free (also `simcode-core`, `-units`, `-model`) | free | `github.com/simcode` already exists (a user or organisation) |
| `taktsim` | free (also `taktsim-core`) | free | "Takt" is familiar to manufacturing engineers; `github.com/taktsim` free |
| `simline` | free | free | generic |
| `fabsim` | free | free | collides with FabSim3, an established simulation toolkit |
| `plantcode` | free | free | |
| `linesim`, `flowsim` | free | taken | |
| `desim` | taken | free | |

## Options

- **N1. Keep `simcode`.** Descriptive ("simulation as code"), available on
  both registries. Generic, so it is hard to search for and weak as a
  trademark; the GitHub name is taken.
- **N2. A distinctive name** such as `taktsim`: available on both
  registries and GitHub, recognisable to the target users, easier to
  search for and to protect.
- **N3. Something else** you prefer, checked the same way.

## Recommendation

Decide before M0 so the workspace starts with its final crate names.
Between the two checked candidates I lean towards **`taktsim`** (N2) for
searchability and the free GitHub name. Keeping `simcode` (N1) is
reasonable too, but it needs a different GitHub organisation name. Either
way, before accepting:

1. Trademark search in the EUIPO / TMview and USPTO databases (classes 9
   and 42) — this needs a person; I cannot do it reliably.
2. Reserve the names: publish 0.0.0 placeholder crates and a placeholder
   PyPI project only once you decide (outward-facing, so I will not do it
   without your go-ahead), and create the GitHub organisation.
3. Decide whether the model header key follows the project name or stays
   neutral (e.g. `format: "1.0"`), so a later rename would not break model
   files. Recommended: neutral key.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Which name?
2. Neutral model header key (`format:`) instead of `simcode:`?

Decided:

- The name is **`taktsim`**: crates `taktsim-*`, Python package `taktsim`,
  CLI binary `taktsim`.
- Model header key: `format: "1.0"`. Spec examples that use `simcode:`
  are read as `format:`.
- Still to do before the first publish (v0.1): the trademark search
  (needs a person), and reserving the crates.io / PyPI names and the
  GitHub organisation (outward-facing, so it needs the owner's go-ahead).
