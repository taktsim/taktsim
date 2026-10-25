#!/usr/bin/env bash
# Prints the gungraun version that a Cargo.lock resolves, or nothing if it
# has none (ADR-0016). The gungraun-runner binary must have exactly this
# version, or the benchmarks stop with an error (gungraun guide,
# "Installation"); the bench workflow installs it.
# Usage: ci/bench-runner-version.sh [Cargo.lock]   (default: ./Cargo.lock)
# Tests: ci/bench-test.sh.
set -euo pipefail

lock=${1:-Cargo.lock}
if [ ! -f "$lock" ]; then
  echo "error: $lock does not exist" >&2
  exit 1
fi

# The version line of every [[package]] named exactly "gungraun" (not
# gungraun-runner or gungraun-macros).
versions=$(awk '
  /^\[\[package\]\]/ { found = 0 }
  $0 == "name = \"gungraun\"" { found = 1; next }
  found && /^version = "/ { gsub(/^version = "|"$/, ""); print; found = 0 }
' "$lock")

if [ "$(grep -c . <<<"$versions" || true)" -gt 1 ]; then
  echo "error: $lock has several gungraun versions: $(tr '\n' ' ' <<<"$versions")" >&2
  exit 1
fi
if [ -n "$versions" ]; then
  echo "$versions"
fi
