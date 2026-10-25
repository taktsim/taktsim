# Contributing to taktsim

Thanks for helping. This page is the contract every pull request is checked
against. The rules come from [the specification](docs/spec.md) (sections 2, 7
and 10) and the [architecture decisions](docs/adr/).

## Before you start

- Read the design principles in [CLAUDE.md](CLAUDE.md#design-principles-spec-2).
  They apply to people and agents alike.
- Work on something in the current milestone of the [roadmap](docs/roadmap.md),
  or open an issue first.
- New components and adapters start as an issue from the matching template. A
  component proposal must list its invariants.

## Every commit

- **Conventional Commits** subject: `feat(core): add event queue`,
  `fix(model): reject duplicate ids`, `docs: ...`, `test: ...`, `ci: ...`.
  The changelog is generated from them.
- **DCO sign-off**: commit with `git commit -s`. The `Signed-off-by:` line
  certifies the [Developer Certificate of Origin](https://developercertificate.org/).
- A commit that changes `crates/taktsim-core` also adds or changes a test, or
  names the covering test in a `Test: <name>` trailer.

## Every pull request

CI runs these gates; all must be green:

| Gate | What it checks |
| --- | --- |
| lint | `rustfmt`, `clippy` (pedantic, `-D warnings`, no `unwrap`/`expect`/`panic!` in library code, no wildcard arms on enums), `cargo-deny` (licenses, advisories, duplicates), crate rules (`forbid(unsafe_code)`, `deny(missing_docs)`) |
| test | unit, property and doc tests on Windows x86-64, Linux x86-64 and ARM64, macOS ARM64; tier 2 (Intel macOS, Windows ARM, musl) is reported but does not block |
| msrv | the workspace builds with the minimum supported Rust version |
| coverage | ≥ 90 % lines overall, 100 % branches in `taktsim-core` |
| python | pytest, ruff, pyright strict (warnings fail) on Python 3.13, 3.14, 3.15 |
| docs | rustdoc without warnings; a code example on every public item |
| commits | Conventional Commits, DCO, core changes come with tests |
| gates | changes to lints, CI, toolchain or `deny.toml` carry the `gate-change` label and a reason in the description |
| mutants | mutation testing of changed core code; advisory until v1.0 |

Run the same checks locally before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
ci/check-crates.sh
cargo nextest run --workspace --exclude taktsim-python --all-features
cargo test --workspace --exclude taktsim-python --all-features --doc
# coverage (needs the pinned nightly from .github/workflows/ci.yml):
cargo +nightly-2026-10-24 llvm-cov nextest --workspace --exclude taktsim-python \
  --all-features --branch \
  --json --summary-only --output-path target/coverage.json
ci/coverage-gate.sh target/coverage.json
# Python (from python/):
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop
.venv/bin/ruff check . && .venv/bin/ruff format --check .
.venv/bin/pyright --warnings --pythonpath .venv/bin/python
.venv/bin/pytest -q
```

Never weaken a test, snapshot or gate to make CI pass. If a gate is wrong,
change it in its own PR with the `gate-change` label and say why.

If your PR breaks one of the design principles, say so in its description and
explain why; the PR template asks.

## Decisions

Larger choices are recorded as [ADRs](docs/adr/). Propose one in a PR with
status `proposed`; the project owner accepts it.

## License

Contributions are dual licensed under MIT OR Apache-2.0, without additional
terms (see [README](README.md#license)).
