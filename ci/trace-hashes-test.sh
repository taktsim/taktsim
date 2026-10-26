#!/usr/bin/env bash
# Tests for ci/trace-hashes.sh (ADR-0021): it passes when every target
# reports the same golden-trace hashes, and fails when one target differs,
# misses a case, reports nothing, or is missing. CI runs it before the
# comparison, so a broken comparison fails instead of passing every run.
# Usage: ci/trace-hashes-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

compare=${TRACE_HASHES:-ci/trace-hashes.sh}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

# A case file as the golden harness writes it.
line() { echo "$1 records=$2 stream=$3 file=$4"; }

# setup <case>: four targets with the same two cases.
setup() {
  case_dir="$work/$1"
  for target in linux-x64 linux-arm64 windows-x64 macos-arm64; do
    mkdir -p "$case_dir/$target"
    line toy_a 10 aaaa 1111 >"$case_dir/$target/toy_a.txt"
    line toy_b 20 bbbb 2222 >"$case_dir/$target/toy_b.txt"
  done
}

# expect <case> <exit code> <text in the output>
expect() {
  local name=$1 want=$2 text=$3 out got=0
  out=$("$compare" "$work/$name" 4 2>&1) || got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $name: exit $got, expected $want"
    sed 's/^/    /' <<<"$out"
    failures=$((failures + 1))
  elif ! grep -Fq -- "$text" <<<"$out"; then
    echo "FAIL $name: no '$text' in"
    sed 's/^/    /' <<<"$out"
    failures=$((failures + 1))
  else
    echo "ok   $name"
  fi
}

setup same
expect same 0 "Set 1, from \`linux-arm64\`, \`linux-x64\`, \`macos-arm64\`, \`windows-x64\`:"

setup stream_differs
line toy_b 20 bbbc 2222 >"$work/stream_differs/windows-x64/toy_b.txt"
expect stream_differs 1 "error: the targets disagree: 2 different sets of golden-trace hashes"

setup file_differs
line toy_a 10 aaaa 1112 >"$work/file_differs/macos-arm64/toy_a.txt"
expect file_differs 1 "| \`macos-arm64\` | 2 | set 2 |"

# The odd one out sorts first, so it is set 1.
setup case_missing
rm "$work/case_missing/linux-arm64/toy_b.txt"
expect case_missing 1 "set 1 (\`linux-arm64\`) vs set 2 (\`linux-x64\`, \`macos-arm64\`, \`windows-x64\`)"

setup nothing_reported
rm "$work/nothing_reported/linux-x64/"*.txt
expect nothing_reported 1 "error: linux-x64 reported no golden-trace hashes"

setup target_missing
rm -r "$work/target_missing/macos-arm64"
expect target_missing 1 "error: expected hashes from 4 targets, found 3"

mkdir -p "$work/no_targets"
expect no_targets 1 "found 0: none"

if [ "$failures" -ne 0 ]; then
  echo "$failures failed"
  exit 1
fi
echo "all passed"
