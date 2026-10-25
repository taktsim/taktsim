#!/usr/bin/env bash
# Runs the instruction-count benchmarks (gungraun, Callgrind) of the
# checked-out commit and writes one summary.json per benchmark under
# <out-dir>, for ci/bench-compare.sh (ADR-0016). Needs Linux, Valgrind and
# gungraun-runner with the version of ci/bench-runner-version.sh.
#
# Which targets run is decided by ci/bench-targets.sh: the taktsim-bench
# targets named `instructions_*`. With --strict, used for the pull
# request's head, a bench target with another name or in another package,
# or none at all, is an error. A commit without such benchmarks (a base
# from before ADR-0016) writes nothing.
#
# The bench workflow runs this script from the base commit when the base
# has it (see ci/bench-compare.sh): keep its arguments stable.
# Usage: ci/bench-run.sh [--strict] <out-dir>
set -euo pipefail

here=$(dirname "$0")
strict=()
if [ "${1:-}" = --strict ]; then
  strict=(--strict)
  shift
fi
if [ $# -ne 1 ]; then
  echo "usage: ci/bench-run.sh [--strict] <out-dir>" >&2
  exit 2
fi
rm -rf "$1"
mkdir -p "$1"
out=$(cd "$1" && pwd)

gated=$(cargo metadata --no-deps --format-version 1 --locked | "$here/bench-targets.sh" "${strict[@]+"${strict[@]}"}")
if [ -z "$gated" ]; then
  echo "no instruction-count benchmarks in this commit; nothing measured"
  exit 0
fi

want=$("$here/bench-runner-version.sh")
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
