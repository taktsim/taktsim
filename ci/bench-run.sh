#!/usr/bin/env bash
# Runs the instruction-count benchmarks (gungraun, Callgrind) of the
# checked-out commit and writes one summary.json per benchmark under
# <out-dir>, for ci/bench-compare.sh (ADR-0016). Needs Linux, Valgrind and
# gungraun-runner with the version of ci/bench-runner-version.sh.
#
# Benchmarks live in crates/taktsim-bench. Its bench targets named
# `instructions_*` run here; `wall_*` (criterion) are for local use. With
# --strict, used for the pull request's head, any other bench target, in
# taktsim-bench or elsewhere, is an error, so a benchmark cannot drop out
# of the gate because of its name or place.
#
# A commit without instruction-count benchmarks (a base from before
# ADR-0016) writes nothing; with --strict that is an error.
#
# Usage: ci/bench-run.sh [--strict] <out-dir>
set -euo pipefail

strict=false
if [ "${1:-}" = --strict ]; then
  strict=true
  shift
fi
if [ $# -ne 1 ]; then
  echo "usage: ci/bench-run.sh [--strict] <out-dir>" >&2
  exit 2
fi
rm -rf "$1"
mkdir -p "$1"
out=$(cd "$1" && pwd)

# "<package> <target>" for every bench target in the workspace.
targets=$(cargo metadata --no-deps --format-version 1 --locked |
  jq -r '.packages[] | .name as $p | .targets[] | select(any(.kind[]; . == "bench")) | "\($p) \(.name)"')

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
if [ -z "$gated" ]; then
  if [ "$strict" = true ]; then
    echo "error: no instruction-count benchmarks (crates/taktsim-bench/benches/instructions_*.rs)" >&2
    exit 1
  fi
  echo "no instruction-count benchmarks in this commit; nothing measured"
  exit 0
fi

want=$("$(dirname "$0")/bench-runner-version.sh")
if ! command -v gungraun-runner >/dev/null; then
  echo "error: gungraun-runner not found; install it with" >&2
  echo "       cargo install --locked gungraun-runner --version $want" >&2
  exit 1
fi
have=$(gungraun-runner --version | awk '{ print $NF }')
if [ "$have" != "$want" ]; then
  echo "error: gungraun-runner $have is installed, Cargo.lock has gungraun $want; they must match" >&2
  exit 1
fi

args=()
for target in $gated; do
  args+=(--bench "$target")
done
echo "measuring with gungraun-runner $have, $(valgrind --version): ${gated//$'\n'/ }"

# gungraun needs debug info in the bench profile (gungraun guide,
# "Prerequisites"); set here rather than in Cargo.toml so it only applies
# to these runs. Base and head are built the same way, so it cannot cause
# a difference between them.
CARGO_PROFILE_BENCH_DEBUG=true GUNGRAUN_HOME="$out" GUNGRAUN_SAVE_SUMMARY=json \
  cargo bench --locked -p taktsim-bench "${args[@]}"
