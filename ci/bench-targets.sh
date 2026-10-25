#!/usr/bin/env bash
# Picks the instruction-count benchmarks from `cargo metadata` output on
# stdin (ADR-0016) and prints their target names, one per line: the bench
# targets of taktsim-bench named `instructions_*`. Its `wall_*` targets
# (criterion) are for local use.
#
# With --strict, used for the pull request's head, any other bench target,
# in taktsim-bench or in another package, is an error, and so is having
# none: a benchmark cannot drop out of the gate because of its name or
# place.
#
# Usage: cargo metadata --no-deps --format-version 1 | ci/bench-targets.sh [--strict]
# Tests: ci/bench-test.sh.
set -euo pipefail

strict=false
if [ "${1:-}" = --strict ]; then
  strict=true
fi

# "<package> <target>" for every bench target in the workspace.
targets=$(jq -r '.packages[] | .name as $p | .targets[] | select(any(.kind[]; . == "bench")) | "\($p) \(.name)"')

if [ "$strict" = true ]; then
  bad=$(awk '$1 != "taktsim-bench" || $2 !~ /^(instructions|wall)_/' <<<"$targets" | sed '/^$/d')
  if [ -n "$bad" ]; then
    echo "error: bench targets must be crates/taktsim-bench/benches/instructions_*.rs (gungraun, gated)" >&2
    echo "       or wall_*.rs (criterion, local) (ADR-0016); found:" >&2
    sed 's/^/         /' <<<"$bad" >&2
    exit 1
  fi
fi

gated=$(awk '$1 == "taktsim-bench" && $2 ~ /^instructions_/ { print $2 }' <<<"$targets")
if [ -z "$gated" ] && [ "$strict" = true ]; then
  echo "error: no instruction-count benchmarks (crates/taktsim-bench/benches/instructions_*.rs)" >&2
  exit 1
fi
if [ -n "$gated" ]; then
  echo "$gated"
fi
