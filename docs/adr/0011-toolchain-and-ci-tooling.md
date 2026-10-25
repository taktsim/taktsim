# ADR-0011: Toolchain, MSRV and CI tooling

- Status: accepted (2026-10-25, M0 pull request #1 merged)
- Date: 2026-10-25
- Blocks: M0
- Spec: §7 Layer 1 and Tooling baseline, §10 Tooling; ADR-0007, ADR-0010

## Context

The spec fixes Rust stable with a declared MSRV and names several tools as
*defaults*. M0 has to pick concrete versions. Two facts were checked while
building M0:

- Branch coverage in `cargo-llvm-cov` 0.9.1 still needs a nightly compiler
  (`--branch flag requires nightly toolchain`), as ADR-0007 expected.
- The spec's "a doc test for every public item" can be enforced, but only
  with nightly rustdoc (`rustdoc::missing_doc_code_examples` is unstable).
  Tested both ways: an item without an example fails, one with an example
  passes.
- PyO3 0.30 compiles under `#![forbid(unsafe_code)]`, so the binding crate
  needs no `unsafe` exception.

## Decision

**Rust**

- Development and CI toolchain pinned in `rust-toolchain.toml` to an exact
  stable release (1.99.1 today). An exact pin keeps new clippy lints from
  turning CI red on an unrelated PR; Dependabot does not bump it, so it is
  raised by hand in a `gate-change` PR.
- MSRV `rust-version = "1.97"` (latest stable minus two), checked by the
  `msrv` job. Raised only in a commit that says so.
- Edition 2024, resolver 3.
- Pinned nightly (`nightly-2026-10-24`) only for the coverage job and the
  doc-example check; everything else runs on stable. Moved forward by hand
  when needed.

**Lints beyond the spec's list.** In addition to pedantic, `unwrap_used`,
`expect_used`, `panic`: `wildcard_enum_match_arm` (enforces "no wildcard
arms", spec §7), `dbg_macro`, `todo`, `unimplemented`, `print_stdout`,
`print_stderr` (library code does no I/O; the CLI allows printing at its
root). Tests may unwrap, expect, panic and print (`clippy.toml`).

**Tools** (spec defaults, now confirmed): `cargo-nextest` 0.9.149,
`cargo-deny` 0.20.2, `cargo-llvm-cov` 0.9.1, `cargo-mutants` 27.1.0,
`git-cliff` for the changelog. Python: `maturin` 1.15.0, `pytest` 9.1.1,
`ruff` 0.17.2 (all rules except conflicting docstring styles, formatter
conflicts and per-file copyright headers), `pyright` 1.1.414 strict with
warnings as errors. Versions are pinned in the workflow and in
`python/requirements-dev.txt`.

**CI**

- GitHub Actions pinned by commit SHA, kept current by Dependabot (actions,
  cargo, pip), so a moved tag cannot change what CI runs.
- Tool binaries installed with `taiki-e/install-action`; caching with
  `Swatinem/rust-cache`.
- The Python binding crate (`python/`) is a workspace member, linted by
  clippy and checked by the MSRV job, but excluded from `cargo test` and
  coverage: it is tested through pytest on Python 3.13, 3.14 and 3.15.
- `ci/check-gates.sh` requires the `gate-change` label for changes to lints,
  toolchain, MSRV, `deny.toml`, `.gitattributes`, workflows or `ci/`; those
  paths also need the owner's review (`CODEOWNERS`).
- `ci/check-commits.sh` enforces Conventional Commits, DCO sign-off (GitHub
  bots exempt, since they cannot sign off) and "a core commit changes or
  names a test".

## Consequences

- Two toolchains to keep current by hand (stable pin, nightly pin).
- A nightly regression can break the coverage or doc-example job without a
  code change; the fix is to move the nightly pin, in a `gate-change` PR.
- Every job installs prebuilt tools, so CI does not compile them.

## Process note

ADRs for defaults that a milestone has to settle are proposed in that
milestone's pull request; merging the pull request accepts them. A decision
that changes the spec or an accepted ADR still needs a separate answer from
the owner.
