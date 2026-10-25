#!/usr/bin/env bash
# Writes a Markdown summary of a cargo-mutants run (ADR-0007), for the CI
# job summary. Usage: ci/mutants-summary.sh [mutants.out]
set -euo pipefail
out=${1:-mutants.out}

count() { if [ -f "$out/$1.txt" ]; then grep -c . "$out/$1.txt" || true; else echo 0; fi; }

echo "## Mutation testing (taktsim-core, changed code)"
echo
if [ ! -d "$out" ]; then
  echo "No mutants: this change does not touch taktsim-core."
  exit 0
fi
caught=$(count caught) missed=$(count missed) timeout=$(count timeout) unviable=$(count unviable)
echo "| caught | timed out (counts as caught) | missed | unviable |"
echo "| --- | --- | --- | --- |"
echo "| $caught | $timeout | $missed | $unviable |"
if [ "$missed" -gt 0 ]; then
  echo
  echo "Missed mutants: add a test that fails for each, or, if it is"
  echo "equivalent, list it in \`.cargo/mutants.toml\` with the reason."
  echo
  sed 's/^/- `/; s/$/`/' "$out/missed.txt"
fi
