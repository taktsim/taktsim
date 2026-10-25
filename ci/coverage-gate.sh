#!/usr/bin/env bash
# Coverage floors (spec §7, ADR-0007): >= 90 % lines over the workspace and
# 100 % branches in taktsim-core. Input: `cargo llvm-cov --json` export.
# Usage: ci/coverage-gate.sh coverage.json
set -euo pipefail
json=$1
min_lines=90

read -r lines_count lines_covered < <(jq -r '.data[0].totals.lines | "\(.count) \(.covered)"' "$json")
read -r br_count br_covered < <(jq -r '
  [.data[0].files[] | select(.filename | test("[/\\\\]crates[/\\\\]taktsim-core[/\\\\]src[/\\\\]")) | .summary.branches]
  | "\(map(.count) | add // 0) \(map(.covered) | add // 0)"' "$json")

fail=0
if [ "$lines_count" -eq 0 ]; then
  echo "lines: no instrumented lines"
elif [ $((lines_covered * 100)) -lt $((lines_count * min_lines)) ]; then
  echo "error: line coverage $lines_covered/$lines_count is below $min_lines %" >&2
  fail=1
else
  echo "lines: $lines_covered/$lines_count covered (floor $min_lines %)"
fi

if [ "$br_covered" -lt "$br_count" ]; then
  echo "error: taktsim-core branch coverage $br_covered/$br_count is below 100 %; uncovered:" >&2
  jq -r '.data[0].files[] | select(.filename | test("[/\\\\]crates[/\\\\]taktsim-core[/\\\\]src[/\\\\]"))
    | select(.summary.branches.covered < .summary.branches.count)
    | "  \(.filename): \(.summary.branches.covered)/\(.summary.branches.count)"' "$json" >&2
  fail=1
else
  echo "taktsim-core branches: $br_covered/$br_count covered (floor 100 %)"
fi

exit "$fail"
