#!/usr/bin/env bash
# Prints the gungraun version that Cargo.lock in the current directory
# resolves, or nothing if it has none (ADR-0016). The gungraun-runner
# binary must have exactly this version, or the benchmarks stop with an
# error (gungraun guide, "Installation"); the bench workflow installs it.
# Usage: ci/bench-runner-version.sh
set -euo pipefail

versions=$(awk '
  $0 == "name = \"gungraun\"" { found = 1; next }
  found && /^version = "/ { gsub(/^version = "|"$/, ""); print; found = 0 }
' Cargo.lock)

if [ "$(grep -c . <<<"$versions")" -gt 1 ]; then
  echo "error: Cargo.lock has several gungraun versions: $(tr '\n' ' ' <<<"$versions")" >&2
  exit 1
fi
echo "$versions"
