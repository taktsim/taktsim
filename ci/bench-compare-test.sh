#!/usr/bin/env bash
# Tests for the instruction-count gate, ci/bench-compare.sh (ADR-0016).
# The bench workflow runs them before measuring, so a broken gate fails the
# job instead of passing every pull request.
#
# Each case writes gungraun summaries for a base and a head run and checks
# the exit code and lines of the output. The summaries are copies of
# ci/fixtures/bench/gungraun-0.20.0-example.summary.json (the schema v7
# example from the gungraun 0.20.0 guide, MIT OR Apache-2.0) with the
# benchmark name and instruction count replaced, so the gate reads the
# same structure gungraun writes. Expected values are worked out by hand
# from the rule "head > base × 1.10 fails".
#
# Usage: ci/bench-compare-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

template=ci/fixtures/bench/gungraun-0.20.0-example.summary.json
# The gate under test; another path checks that a broken copy fails here.
gate=${BENCH_COMPARE:-ci/bench-compare.sh}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

# start <case>: a new case with empty base and head runs.
start() {
  case_dir="$work/$1"
  mkdir -p "$case_dir/base" "$case_dir/head"
}

# put <base|head> <file::group::function[::id]> <Ir> [jq filter]: one
# summary in the current case; the filter can break it on purpose.
put() {
  local run=$1 name=$2 ir=$3 filter=${4:-.}
  local module_path id dir
  module_path=$(cut -d: -f1-5 <<<"$name")
  id=$(cut -d: -f7 <<<"$name")
  dir="$case_dir/$run/$(tr ':' '_' <<<"$name")"
  mkdir -p "$dir"
  jq --arg mp "$module_path" --arg id "$id" --argjson ir "$ir" \
    ".module_path = \$mp | .id = (if \$id == \"\" then null else \$id end)
     | .profiles[0].data.total.metrics.Ir.values.new = \$ir | $filter" \
    "$template" >"$dir/summary.json"
}

# check <case> <exit code> [line]...: runs the gate on the case and checks
# its exit code and that each line appears in its output (stdout+stderr).
check() {
  local name=$1 want=$2 got=0 out line
  shift 2
  out=$("$gate" "$work/$name/base" "$work/$name/head" 2>&1) || got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $name: exit $got, expected $want"
    sed 's/^/    /' <<<"$out"
    failures=$((failures + 1))
    return
  fi
  for line in "$@"; do
    if ! grep -Fq -- "$line" <<<"$out"; then
      echo "FAIL $name: no line '$line' in"
      sed 's/^/    /' <<<"$out"
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok   $name"
}

start unchanged
put base f::g::x 1000
put head f::g::x 1000
check unchanged 0 '| `f::g::x` | 1,000 | 1,000 | 0.00 % | ok |' \
  '**Passed:** no benchmark rose by more than 10 %' '1 benchmark(s): 1 compared, 0 new, 0 removed.'

start exactly_ten_percent_passes
put base f::g::x 1000
put head f::g::x 1100
check exactly_ten_percent_passes 0 '| `f::g::x` | 1,000 | 1,100 | +10.00 % | ok |'

start above_ten_percent_fails
put base f::g::x 1000
put head f::g::x 1101
check above_ten_percent_fails 1 '| `f::g::x` | 1,000 | 1,101 | +10.10 % | **regression** |' \
  '**Failed:** 1 benchmark(s) rose by more than 10 % (ADR-0007): `f::g::x`.'

# 7e13 × 1.1 = 7.7e13 exactly; one instruction more fails. Near 2^53 a
# floating-point comparison of ratios would round these together.
start large_counts_compare_exactly
put base f::g::ok 70000000000000
put head f::g::ok 77000000000000
put base f::g::bad 70000000000000
put head f::g::bad 77000000000001
check large_counts_compare_exactly 1 '| `f::g::ok` | 70,000,000,000,000 | 77,000,000,000,000 | +10.00 % | ok |' \
  '| `f::g::bad` | 70,000,000,000,000 | 77,000,000,000,001 | +10.00 % | **regression** |'

start improvement
put base f::g::x 1000
put head f::g::x 800
check improvement 0 '| `f::g::x` | 1,000 | 800 | -20.00 % | improvement |'

start small_decrease
put base f::g::x 1000
put head f::g::x 950
check small_decrease 0 '| `f::g::x` | 1,000 | 950 | -5.00 % | ok |'

start new_benchmark_passes
put head f::g::x 1234567
check new_benchmark_passes 0 '| `f::g::x` | - | 1,234,567 | - | new |' \
  '1 benchmark(s): 0 compared, 1 new, 0 removed.'

start removed_benchmark_passes
put base f::g::x 1000
put base f::g::y 2000
put head f::g::x 1000
check removed_benchmark_passes 0 '| `f::g::y` | 2,000 | - | - | removed |' \
  '2 benchmark(s): 1 compared, 0 new, 1 removed.'

# One regression among others; -10 % exactly is not an improvement.
start one_regression_among_many
put base f::g::a 100
put base f::g::b 200
put base f::g::c 300
put head f::g::a 100
put head f::g::b 221
put head f::g::c 270
put head f::g::d 5
check one_regression_among_many 1 '| `f::g::a` | 100 | 100 | 0.00 % | ok |' \
  '| `f::g::b` | 200 | 221 | +10.50 % | **regression** |' \
  '| `f::g::c` | 300 | 270 | -10.00 % | ok |' \
  '| `f::g::d` | - | 5 | - | new |' \
  '**Failed:** 1 benchmark(s) rose by more than 10 % (ADR-0007): `f::g::b`.' \
  '4 benchmark(s): 3 compared, 1 new, 0 removed.'

start zero_base
put base f::g::same 0
put head f::g::same 0
put base f::g::more 0
put head f::g::more 1
check zero_base 1 '| `f::g::same` | 0 | 0 | 0.00 % | ok |' \
  '| `f::g::more` | 0 | 1 | n/a (base 0) | **regression** |'

start ids_are_separate_benchmarks
put base f::g::x::a 100
put base f::g::x::b 100
put head f::g::x::a 100
put head f::g::x::b 150
check ids_are_separate_benchmarks 1 '| `f::g::x::a` | 100 | 100 | 0.00 % | ok |' \
  '| `f::g::x::b` | 100 | 150 | +50.00 % | **regression** |'

start real_summary_reads
mkdir -p "$case_dir/base/r" "$case_dir/head/r"
cp "$template" "$case_dir/base/r/summary.json"
cp "$template" "$case_dir/head/r/summary.json"
check real_summary_reads 0 \
  '| `test_lib_bench_intro::fibonacci::bench_fibonacci_with_config` | 687 | 687 | 0.00 % | ok |'

# A summary written by gungraun-runner 0.20.0 for taktsim-bench in CI
# (pull request #6, unchanged), with a benchmark id; then the same summary
# with 10 % plus one instruction more: 513,491 × 1.1 = 564,840.1.
own=ci/fixtures/bench/taktsim-bench-weibull.summary.json
start own_summary_reads
mkdir -p "$case_dir/base/r" "$case_dir/head/r"
cp "$own" "$case_dir/base/r/summary.json"
jq '.profiles[0].data.total.metrics.Ir.values.new = 564841' "$own" >"$case_dir/head/r/summary.json"
check own_summary_reads 1 \
  '| `instructions_random::distributions::sample::weibull` | 513,491 | 564,841 | +10.00 % | **regression** |'
jq '.profiles[0].data.total.metrics.Ir.values.new = 564840' "$own" >"$case_dir/head/r/summary.json"
check own_summary_reads 0 \
  '| `instructions_random::distributions::sample::weibull` | 513,491 | 564,840 | +10.00 % | ok |'

start rows_are_sorted
put base f::g::b 1
put base f::g::a 1
put head f::g::b 1
put head f::g::a 1
out=$("$gate" "$work/rows_are_sorted/base" "$work/rows_are_sorted/head")
if [ "$(grep -o '`f::g::[ab]`' <<<"$out" | tr -d '\n')" = '`f::g::a``f::g::b`' ]; then
  echo "ok   rows_are_sorted"
else
  echo "FAIL rows_are_sorted:"
  sed 's/^/    /' <<<"$out"
  failures=$((failures + 1))
fi

# Input the gate cannot trust fails with exit 2, never passes.
start empty_head
put base f::g::x 1000
check empty_head 2 'no benchmark summaries in the head run'

start nothing_measured
check nothing_measured 2 'no benchmark summaries in the head run'

start unsupported_version
put base f::g::x 1000
put head f::g::x 1000 '.version = "8"'
check unsupported_version 2 'summary version "8" is not supported'

start missing_count
put base f::g::x 1000
put head f::g::x 1000 'del(.profiles[0].data.total.metrics.Ir)'
check missing_count 2 'instruction count null is not a whole number'

start no_callgrind_profile
put base f::g::x 1000
put head f::g::x 1000 '.profiles[0].tool = "DHAT"'
check no_callgrind_profile 2 'expected one Callgrind instruction count (Ir), found 0'

start two_callgrind_profiles
put base f::g::x 1000
put head f::g::x 1000 '.profiles += .profiles'
check two_callgrind_profiles 2 'expected one Callgrind instruction count (Ir), found 2'

start fractional_count
put base f::g::x 1000
put head f::g::x 1000.5
check fractional_count 2 'instruction count 1000.5 is not a whole number'

start negative_count
put base f::g::x -1
put head f::g::x 1000
check negative_count 2 'instruction count -1 is not a whole number'

start count_too_large_to_compare_exactly
put base f::g::x 1000
put head f::g::x 90000000000000
check count_too_large_to_compare_exactly 2 'is not a whole number between 0 and 81883629588554'

start string_count
put base f::g::x 1000
put head f::g::x 1000 '.profiles[0].data.total.metrics.Ir.values.new = "1000"'
check string_count 2 'instruction count "1000" is not a whole number'

start bad_id
put base f::g::x 1000
put head f::g::x 1000 '.id = 7'
check bad_id 2 'id must be a string or null'

start measured_twice
put base f::g::x 1000
put head f::g::x 1000
mkdir -p "$case_dir/head/copy"
cp "$case_dir/head/f__g__x/summary.json" "$case_dir/head/copy/summary.json"
check measured_twice 2 'benchmarks measured twice in one run: f::g::x'

start not_json
put base f::g::x 1000
put head f::g::x 1000
echo 'not json' >"$case_dir/head/f__g__x/summary.json"
check not_json 2 'cannot read the gungraun summaries'

start not_an_object
put base f::g::x 1000
mkdir -p "$case_dir/head/list"
echo '[]' >"$case_dir/head/list/summary.json"
check not_an_object 2 'not a gungraun summary'

got=0
"$gate" "$work/no-such-dir" "$work" >/dev/null 2>&1 || got=$?
if [ "$got" -eq 2 ]; then echo "ok   missing_directory"; else
  echo "FAIL missing_directory: exit $got, expected 2"
  failures=$((failures + 1))
fi

if [ "$failures" -gt 0 ]; then
  echo "error: $failures bench-compare test(s) failed" >&2
  exit 1
fi
echo "all bench-compare tests passed"
