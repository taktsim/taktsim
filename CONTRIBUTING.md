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

## Setting up a machine

`rust-toolchain.toml` pins the toolchain, and rustup installs it on first
use. Also install the MSRV toolchain (`workspace.package.rust-version` in
`Cargo.toml`), the pinned nightly with `llvm-tools` (for coverage and
`compile_fail` doc tests; its date is in `.github/workflows/ci.yml`), and
the CI tools in the versions `ci.yml` installs:

```sh
cargo install --locked cargo-nextest@0.9.149 cargo-deny@0.20.2 \
  cargo-llvm-cov@0.9.1 cargo-mutants@27.1.0
```

The Python commands below create their own virtual environment.

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
| test | unit, property and doc tests on Windows x86-64, Linux x86-64 and ARM64, macOS ARM64 |
| msrv | the workspace builds with the minimum supported Rust version |
| coverage | ≥ 90 % lines overall, 100 % branches in `taktsim-core` |
| python | pytest, ruff, pyright strict (warnings fail) on Python 3.13, 3.14, 3.15 |
| docs | rustdoc without warnings; a code example on every public item; `compile_fail` examples fail with the stated error code |
| commits | Conventional Commits, DCO, core changes come with tests |
| gates | changes to lints, CI, toolchain or `deny.toml` carry the `gate-change` label and a reason in the description |
| benchmarks | instruction counts of `taktsim-bench` against the base branch; a rise above 10 % or an unlisted removed benchmark fails (ADR-0016) |
| mutants | mutation testing of changed core code; advisory until v1.0. Timeouts count as caught; known equivalent mutants are listed with reasons in `.cargo/mutants.toml` (adding one is a gate change) |

A pull request that changes only files no build or test reads (`docs/`,
`website/`, the top-level Markdown files, licences, the site and PR
workflows) skips lint, test, msrv, coverage, python, docs, mutants and
benchmarks; the `changes` job says why, and the skipped checks count as
passed. Any other file runs them all. The list is in `ci/changes.sh`, its
tests in `ci/changes-test.sh`.

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
# compile_fail doc tests with their error codes checked (nightly only):
cargo +nightly-2026-10-24 test --workspace --exclude taktsim-python --all-features --doc
# Python (from python/):
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop
.venv/bin/ruff check . && .venv/bin/ruff format --check .
.venv/bin/pyright --warnings --pythonpath .venv/bin/python
.venv/bin/pytest -q
```

Never weaken a test, snapshot or gate to make CI pass. Every gate change
carries the `gate-change` label and a reason. Loosening a gate is its own PR;
tightening one (a new or stricter check) may come with the code it checks.

If your PR breaks one of the design principles, say so in its description and
explain why; the PR template asks.

## Working on the docs

The documentation site is built with [mdBook](https://rust-lang.github.io/mdBook/)
from the Markdown files in `docs/`; `website/` holds only its configuration,
styles and scripts. The pages and their order are listed in
`docs/SUMMARY.md`; a page missing from it is not published.

Install the tools once, in the versions `.github/workflows/site.yml` uses:

```sh
cargo install --locked mdbook@0.5.4 mdbook-mermaid@0.17.1 mdbook-katex@0.10.0 lychee@0.24.2
mdbook-mermaid install website   # writes the Mermaid scripts, not committed
```

Preview with live reload at <http://localhost:3000>, or build and check the
links as CI does:

```sh
mdbook serve website --open
mdbook build website
lychee --offline --include-fragments --exclude-path website/book/404.html website/book
```

Link between pages with relative paths to the `.md` files, and to a folder
(`adr/`) rather than its `README.md`. A broken link or anchor fails the
build. The site is deployed to GitHub Pages on every push to `main`.

Write formulas in LaTeX: `$c_s$` inline, `$$...$$` on a line of its own for
a block. mdbook-katex renders them to HTML when the site is built, so a
page loads no math script; a formula that does not render fails CI. A
literal dollar sign outside code needs a backslash (`\$`). KaTeX's
stylesheet and fonts are served from `website/theme/`, in the version
mdbook-katex bundles; update them together with mdbook-katex.

The concept pages (`docs/concepts/`) never contain a typed-in number: each
page has a script in `docs/concepts/scripts/` (Python 3.13+, standard library
only) that writes its tables, figures and data files, which the pages
include. CI does not run these scripts, so the check is yours: after
changing a script, `svgkit.py`, `linesim.py`, `website/theme/tokens.json` or
`website/components/sim.js`, regenerate everything (about two minutes), run
the tests of the in-browser simulator (Node, version in `.nvmrc`), which
check that it agrees with the scripts, and commit what changed. Running
`build_all.py` again must leave `git status` clean.

```sh
python3 docs/concepts/scripts/build_all.py
node --test website/components/sim.test.js
git status --short docs website   # empty once everything is committed
```

`website/components/sim.js` is a port of `docs/concepts/scripts/linesim.py`:
change both together.

## Benchmarks

- **Wall-clock, locally (any OS):**
  `cargo bench -p taktsim-bench --bench 'wall_*'`. Compare two states of
  your code with `-- --save-baseline before` on the first and
  `-- --baseline before` on the second. Use it to tune; do not cite the
  numbers as evidence (ADR-0010).
- **Instruction counts, locally (Linux with Valgrind only):** install the
  runner once,
  `cargo install --locked gungraun-runner --version "$(ci/bench-runner-version.sh)"`,
  then on each of two commits run `ci/bench-run.sh target/ic-before` and
  `ci/bench-run.sh target/ic-after`, and compare with
  `ci/bench-compare.sh --removed ci/bench-removed.txt target/ic-before target/ic-after`.
  On macOS and Windows, rely on the CI job. The gate's own tests run
  anywhere with bash and jq: `ci/bench-test.sh`.
- **In CI:** the `bench` workflow runs on every pull request that can
  change an instruction count (see "Every pull request"). Its job
  summary has the table; a regression above 10 % or an unlisted removal
  fails the check. The `instruction-counts` artifact has every
  `summary.json`.
- **Adding a benchmark** (no workflow change; `crates/taktsim-bench` is a
  gate path, so the pull request needs the `gate-change` label):
  1. Add the workload to `crates/taktsim-bench/src/` (fixed inputs and
     seed, results folded into one value, a doc example) and test it in
     `tests/workloads.rs`, with expected values from theory or by hand.
  2. Add a `#[library_benchmark]` to `benches/instructions_<area>.rs`, or a
     new file of that name, inside its private `benchmarks` module; put
     setup in a `setup` function so it is not counted, return the inputs
     with the result to `teardown = support::check`, and use
     `support::built` in setup, so a failing input stops the run instead
     of measuring less work.
  3. Add the same workload to `benches/wall_<area>.rs`.
  4. Register a new file in `crates/taktsim-bench/Cargo.toml` as
     `[[bench]] name = "…" harness = false`.

  A renamed benchmark (file, group, function or id) shows up as one
  removed and one new benchmark: add the old name to
  `ci/bench-removed.txt`, and rename in a pull request of its own,
  without code changes, since the renamed one is not compared there.
  Renaming every benchmark at once fails (nothing compared); do it in two
  steps.

## Decisions

Larger choices are recorded as [ADRs](docs/adr/). Propose one in a PR with
status `proposed`; the project owner accepts it, either explicitly or, for an
ADR that settles a detail of the milestone in the same PR, by merging that
PR.

## License

Contributions are dual licensed under MIT OR Apache-2.0, without additional
terms (see [README](README.md#license)).
