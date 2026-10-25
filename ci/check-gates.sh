#!/usr/bin/env bash
# Fails if a PR changes a quality gate without the `gate-change` label, so a
# gate is never loosened by accident (CLAUDE.md, Tests).
# Usage: GATE_LABEL=true|false ci/check-gates.sh <base>
set -euo pipefail
base=$1

gate_paths=(
  rust-toolchain.toml clippy.toml deny.toml rustfmt.toml .gitattributes
  .github/workflows/ .github/CODEOWNERS ci/ .cargo/
  # The benchmarks the instruction-count gate measures (ADR-0016).
  crates/taktsim-bench/
)

changed=$(git diff --name-only "$base"...HEAD -- "${gate_paths[@]}")

# The root Cargo.toml at the base; empty if it did not exist yet.
base_manifest() { git show "$base:Cargo.toml" 2>/dev/null || true; }
# The [workspace.lints.*] sections of the root Cargo.toml.
lints() { awk '/^\[/{on = /^\[workspace\.lints/} on' ; }
if ! diff -q <(base_manifest | lints) <(lints < Cargo.toml) >/dev/null; then
  changed+=$'\n'"Cargo.toml [workspace.lints]"
fi
# rust-version (MSRV) in the root Cargo.toml.
if [ "$(base_manifest | grep '^rust-version' || true)" != "$(grep '^rust-version' Cargo.toml || true)" ]; then
  changed+=$'\n'"Cargo.toml rust-version"
fi

changed=$(sed '/^$/d' <<<"$changed")
if [ -z "$changed" ]; then
  echo "no gate changes"
  exit 0
fi

echo "gate files changed:"
sed 's/^/  /' <<<"$changed"
if [ "${GATE_LABEL:-false}" = "true" ]; then
  echo "label gate-change present: ok"
  exit 0
fi
echo "error: add the 'gate-change' label and say in the PR description why the gate changes" >&2
exit 1
