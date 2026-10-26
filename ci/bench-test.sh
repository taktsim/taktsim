#!/usr/bin/env bash
# Tests for the instruction-count gate scripts (ADR-0016):
# ci/bench-compare.sh, ci/bench-targets.sh and ci/bench-runner-version.sh.
# The bench workflow runs them before measuring, so a broken gate fails the
# job instead of passing every pull request.
#
# The comparison cases write gungraun summaries for a base and a head run
# and check the exit code and lines of the output. The summaries are copies
# of ci/fixtures/bench/gungraun-0.20.0-example.summary.json (the schema v7
# example from the gungraun 0.20.0 guide, MIT OR Apache-2.0) with the
# benchmark name and instruction count replaced, so the gate reads the
# same structure gungraun writes; two cases use a summary gungraun-runner
# wrote for taktsim-bench in CI. Expected values are worked out by hand
# from the rule "head > base × 1.10 fails".
#
# Usage: ci/bench-test.sh   (needs bash and jq 1.6 or later)
set -euo pipefail
cd "$(dirname "$0")/.."

template=ci/fixtures/bench/gungraun-0.20.0-example.summary.json
# The gate under test; another path checks that a broken copy fails here.
gate=${BENCH_COMPARE:-ci/bench-compare.sh}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

fail() {
  echo "FAIL $1"
  failures=$((failures + 1))
}

# expect <name> <exit code> <output> <got> [line]...: checks an exit code
# and that each line appears in the output; a line written `!text` must
# not appear.
expect() {
  local name=$1 want=$2 out=$3 got=$4 line
  shift 4
  if [ "$got" -ne "$want" ]; then
    fail "$name: exit $got, expected $want"
    sed 's/^/    /' <<<"$out"
    return
  fi
  for line in "$@"; do
    if [ "${line#!}" != "$line" ]; then
      if grep -Fq -- "${line#!}" <<<"$out"; then
        fail "$name: unexpected line '${line#!}' in"
        sed 's/^/    /' <<<"$out"
        return
      fi
    elif ! grep -Fq -- "$line" <<<"$out"; then
      fail "$name: no line '$line' in"
      sed 's/^/    /' <<<"$out"
      return
    fi
  done
  echo "ok   $name"
}

# ---- ci/bench-compare.sh ----------------------------------------------

# start <case>: a new case with empty base and head runs and no removed list.
start() {
  case_dir="$work/$1"
  mkdir -p "$case_dir/base" "$case_dir/head"
  : >"$case_dir/removed.txt"
}

# put <base|head> <file::group::function[::id]> <Ir> [jq filter]: one
# summary in the current case; the filter can break it on purpose.
put() {
  local run=$1 name=$2 ir=$3 filter=${4:-.}
  local module_path id dir
  module_path=$(cut -d: -f1-5 <<<"$name")
  id=$(cut -d: -f7 <<<"$name")
  dir="$case_dir/$run/$(tr ':' '_' <<<"$name")$RANDOM"
  mkdir -p "$dir"
  jq --arg mp "$module_path" --arg id "$id" --argjson ir "$ir" \
    ".module_path = \$mp | .id = (if \$id == \"\" then null else \$id end)
     | .profiles[0].data.total.metrics.Ir.values.new = \$ir | $filter" \
    "$template" >"$dir/summary.json"
}

# accept <line>... / accept_base <line>...: lines of the case's accepted
# list, and of the base's copy of it; a case without them passes neither.
accept() { printf '%s\n' "$@" >>"$case_dir/accepted.txt"; }
accept_base() { printf '%s\n' "$@" >>"$case_dir/accepted-base.txt"; }

# check <case> <exit code> [line]...: runs the gate on the case (with its
# removed list, and its accepted lists if it has them) and checks its exit
# code and output (stdout and stderr).
check() {
  local name=$1 want=$2 got=0 out dir=$work/$1
  shift 2
  local args=(--removed "$dir/removed.txt")
  if [ -f "$dir/accepted.txt" ]; then args+=(--accepted "$dir/accepted.txt"); fi
  if [ -f "$dir/accepted-base.txt" ]; then args+=(--accepted-base "$dir/accepted-base.txt"); fi
  out=$("$gate" "${args[@]}" "$dir/base" "$dir/head" 2>&1) || got=$?
  expect "$name" "$want" "$out" "$got" "$@"
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

start new_beside_compared_passes
put base f::g::x 1000
put head f::g::x 1000
put head f::g::y 5
check new_beside_compared_passes 0 '| `f::g::y` | - | 5 | - | new |' \
  '2 benchmark(s): 1 compared, 1 new, 0 removed.'

start unlisted_removal_fails
put base f::g::x 1000
put base f::g::y 2000
put head f::g::x 1000
check unlisted_removal_fails 1 '| `f::g::y` | 2,000 | - | - | **removed, not listed** |' \
  '**Failed:** 1 benchmark(s) removed without being listed in ci/bench-removed.txt (ADR-0016): `f::g::y`.' \
  '2 benchmark(s): 1 compared, 0 new, 1 removed.'

start listed_removal_passes
put base f::g::x 1000
put base f::g::y 2000
put head f::g::x 1000
printf '# a comment\n\n  f::g::y   # renamed\nf::g::gone-long-ago\n' >"$case_dir/removed.txt"
check listed_removal_passes 0 '| `f::g::y` | 2,000 | - | - | removed (listed) |' \
  '**Passed:** no benchmark rose by more than 10 %' '2 benchmark(s): 1 compared, 0 new, 1 removed.'

start listed_but_measured_fails
put base f::g::x 1000
put head f::g::x 1000
echo 'f::g::x' >"$case_dir/removed.txt"
check listed_but_measured_fails 2 'listed as removed but measured in the head: f::g::x'

# Without this, a run that measured nothing comparable would pass with
# "0 compared".
start nothing_compared_fails
put base f::g::x 1000
put base f::g::y 1000
put head f::g::renamed 1000
echo 'f::g::x' >"$case_dir/removed.txt"
echo 'f::g::y' >>"$case_dir/removed.txt"
check nothing_compared_fails 2 "none of the base's 2 benchmarks is in the head, so nothing was compared"

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

start regression_and_unlisted_removal
put base f::g::a 100
put base f::g::b 100
put head f::g::a 200
check regression_and_unlisted_removal 1 \
  '**Failed:** 1 benchmark(s) rose by more than 10 % (ADR-0007): `f::g::a`.' \
  '**Failed:** 1 benchmark(s) removed without being listed in ci/bench-removed.txt (ADR-0016): `f::g::b`.'

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

# The package directory is not part of a benchmark's name: moving the
# crate compares the same benchmarks.
start moved_crate_is_compared
put base f::g::x 1000 '.package_dir = "crates/taktsim-bench"'
put head f::g::x 1200 '.package_dir = "benchmarks/taktsim-bench"'
check moved_crate_is_compared 1 '| `f::g::x` | 1,000 | 1,200 | +20.00 % | **regression** |' \
  '1 benchmark(s): 1 compared, 0 new, 0 removed.'

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
  fail "rows_are_sorted:"
  sed 's/^/    /' <<<"$out"
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

start numeric_id
put base f::g::x 1000
put head f::g::x 1000 '.id = 7'
check numeric_id 2 'id must be a string or null, found number'

# `false // null` is null in jq: the id is checked by its own type.
start false_id
put base f::g::x 1000
put head f::g::x 1000 '.id = false'
check false_id 2 'id must be a string or null, found boolean'

start missing_module_path
put base f::g::x 1000
put head f::g::x 1000 'del(.module_path)'
check missing_module_path 2 'module_path must be a string'

start numeric_module_path
put base f::g::x 1000
put head f::g::x 1000 '.module_path = 3'
check numeric_module_path 2 'module_path must be a string'

start missing_package_dir
put base f::g::x 1000
put head f::g::x 1000 'del(.package_dir)'
check missing_package_dir 2 'package_dir must be a string'

start list_package_dir
put base f::g::x 1000
put head f::g::x 1000 '.package_dir = ["crates"]'
check list_package_dir 2 'package_dir must be a string'

start measured_twice
put base f::g::x 1000
put head f::g::x 1000
put head f::g::x 1000
check measured_twice 2 'benchmarks measured twice in one run: f::g::x'

start not_json
put base f::g::x 1000
mkdir -p "$case_dir/head/bad"
echo 'not json' >"$case_dir/head/bad/summary.json"
check not_json 2 'cannot read the gungraun summaries'

start not_an_object
put base f::g::x 1000
mkdir -p "$case_dir/head/list"
echo '[]' >"$case_dir/head/list/summary.json"
check not_an_object 2 'not a gungraun summary'

got=0
out=$("$gate" "$work/no-such-dir" "$work" 2>&1) || got=$?
expect missing_directory 2 "$out" "$got" 'usage: ci/bench-compare.sh'

got=0
out=$("$gate" --removed "$work/no-such-list" "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect missing_removed_list 2 "$out" "$got" 'removed list'

# A jq older than 1.6 is refused, not half run: a shim reports 1.5.
mkdir -p "$work/old-jq"
real_jq=$(command -v jq)
printf '#!/bin/sh\nif [ "$1" = --version ]; then echo jq-1.5; exit 0; fi\nexec "%s" "$@"\n' "$real_jq" >"$work/old-jq/jq"
chmod +x "$work/old-jq/jq"
got=0
out=$(PATH="$work/old-jq:$PATH" "$gate" "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect old_jq_refused 2 "$out" "$got" "needs jq 1.6 or later, found 'jq-1.5'"

# ---- ci/bench-compare.sh, accepted regressions ------------------------
# An entry `<benchmark> +<N>% <reason>` lets that one benchmark rise by up
# to N % in the run whose list adds it (ADR-0016, owner's answer 1).

start accepted_within_limit
put base f::g::x 1000
put head f::g::x 1500
accept 'f::g::x +60% every event now checks the invariants'
check accepted_within_limit 0 '| `f::g::x` | 1,000 | 1,500 | +50.00 % | accepted (limit +60 %) |' \
  '**Accepted:** 1 benchmark(s) rose by more than 10 %, within the limit of a new entry in' \
  '- `f::g::x`: +50.00 %, limit +60 % (line 1). Reason: every event now checks the invariants' \
  '**Passed:** no benchmark rose by more than 10 % (ADR-0007), except the accepted ones above.' \
  '1 benchmark(s): 1 compared, 0 new, 0 removed.' '!**Stale:**'

# 1000 × 1.6 = 1600 exactly passes; one instruction more fails.
start accepted_exactly_at_limit
put base f::g::x 1000
put head f::g::x 1600
accept 'f::g::x +60% reason'
check accepted_exactly_at_limit 0 '| `f::g::x` | 1,000 | 1,600 | +60.00 % | accepted (limit +60 %) |'

start accepted_above_limit_fails
put base f::g::x 1000
put head f::g::x 1601
accept 'f::g::x +60% reason'
check accepted_above_limit_fails 1 '| `f::g::x` | 1,000 | 1,601 | +60.10 % | **regression above the accepted +60 %** |' \
  '**Failed:** 1 benchmark(s) rose by more than the limit of their entry in' \
  '`f::g::x` (+60.10 %, limit +60 %).' '!**Passed:**' '!**Accepted:**'

# 6e13 × 1.2 = 7.2e13 exactly; one more fails. The product 7.2e15 is below
# 2^53, so the comparison is exact.
start accepted_large_counts_exact
put base f::g::ok 60000000000000
put head f::g::ok 72000000000000
put base f::g::bad 60000000000000
put head f::g::bad 72000000000001
accept 'f::g::ok +20% reason' 'f::g::bad +20% reason'
check accepted_large_counts_exact 1 \
  '| `f::g::ok` | 60,000,000,000,000 | 72,000,000,000,000 | +20.00 % | accepted (limit +20 %) |' \
  '| `f::g::bad` | 60,000,000,000,000 | 72,000,000,000,001 | +20.00 % | **regression above the accepted +20 %** |'

# 8e13 × 150 = 1.2e16 > 2^53: not compared inexactly.
start accepted_product_too_large
put base f::g::x 80000000000000
put head f::g::x 80000000000000
accept 'f::g::x +50% reason'
check accepted_product_too_large 2 '`f::g::x` (line 1): the base count times the limit is too large to compare exactly'

# An entry covers its one benchmark: not another, not an id under it.
start accepted_covers_one_benchmark
put base f::g::a 1000
put base f::g::b 1000
put head f::g::a 1500
put head f::g::b 1500
accept 'f::g::a +60% reason'
check accepted_covers_one_benchmark 1 '| `f::g::a` | 1,000 | 1,500 | +50.00 % | accepted (limit +60 %) |' \
  '| `f::g::b` | 1,000 | 1,500 | +50.00 % | **regression** |' \
  '**Failed:** 1 benchmark(s) rose by more than 10 % (ADR-0007): `f::g::b`.' '!**Passed:**'

start accepted_name_is_not_a_prefix
put base f::g::x::a 1000
put head f::g::x::a 1500
accept 'f::g::x +60% reason'
check accepted_name_is_not_a_prefix 2 '`f::g::x` (line 1): not measured in both the base and the head'

start accepted_id_is_exact
put base f::g::x::a 1000
put base f::g::x::b 1000
put head f::g::x::a 1500
put head f::g::x::b 1500
accept 'f::g::x::a +60% reason'
check accepted_id_is_exact 1 '| `f::g::x::a` | 1,000 | 1,500 | +50.00 % | accepted (limit +60 %) |' \
  '| `f::g::x::b` | 1,000 | 1,500 | +50.00 % | **regression** |'

# Comments and blank lines are skipped and counted for line numbers; white
# space is free; a `#` after the first word is part of the reason.
start accepted_comments_and_spacing
put base f::g::x 1000
put head f::g::x 1500
accept '# a comment' '' '   # an indented comment' $'  f::g::x \t +60%   see   pull request #42  '
check accepted_comments_and_spacing 0 \
  '- `f::g::x`: +50.00 %, limit +60 % (line 4). Reason: see pull request #42'

start accepted_crlf
put base f::g::x 1000
put head f::g::x 1500
printf 'f::g::x +60%% reason\r\n' >"$case_dir/accepted.txt"
check accepted_crlf 0 'Reason: reason' '| `f::g::x` | 1,000 | 1,500 | +50.00 % | accepted (limit +60 %) |'

# A new entry whose benchmark did not rise by more than 10 % fails: it
# would otherwise be merged unused and stay as an open allowance.
start accepted_not_needed_fails
put base f::g::x 1000
put head f::g::x 1100
accept 'f::g::x +60% reason'
check accepted_not_needed_fails 1 '| `f::g::x` | 1,000 | 1,100 | +10.00 % | **accepted entry not needed** |' \
  '**Failed:** 1 entr(ies) in' 'not needed, the benchmark rose by 10 % or less; delete them: `f::g::x` (+10.00 %).' \
  '!**Passed:**'

start accepted_not_needed_on_improvement
put base f::g::x 1000
put head f::g::x 500
accept 'f::g::x +60% reason'
check accepted_not_needed_on_improvement 1 '| `f::g::x` | 1,000 | 500 | -50.00 % | **accepted entry not needed** |'

start accepted_zero_base_fails
put base f::g::x 0
put head f::g::x 1
accept 'f::g::x +1000% reason'
check accepted_zero_base_fails 1 '| `f::g::x` | 0 | 1 | n/a (base 0) | **regression above the accepted +1000 %** |'

start accepted_highest_limit
put base f::g::x 1000
put head f::g::x 11000
accept 'f::g::x +1000% reason'
check accepted_highest_limit 0 '| `f::g::x` | 1,000 | 11,000 | +1000.00 % | accepted (limit +1000 %) |'

# An empty list (comments only, like the committed one) changes nothing.
start accepted_empty_list
put base f::g::x 1000
put head f::g::x 1000
accept '# nothing accepted'
check accepted_empty_list 0 '| `f::g::x` | 1,000 | 1,000 | 0.00 % | ok |' \
  '**Passed:** no benchmark rose by more than 10 % (ADR-0007).' '!**Accepted:**' '!**Stale:**'

# Malformed entries fail with exit 2, every bad line named, and an example.
# accepted_bad <case> <entry> <message>
accepted_bad() {
  start "$1"
  put base f::g::x 1000
  put head f::g::x 1500
  accept "$2"
  check "$1" 2 "accepted.txt:1: $3" 'expected one entry per line, `<benchmark> +<N>% <reason>`, e.g.'
}
accepted_bad accepted_pattern_refused 'f::g::* +60% reason' '`f::g::*` is not a benchmark name'
accepted_bad accepted_short_name_refused 'x +60% reason' '`x` is not a benchmark name'
accepted_bad accepted_two_part_name_refused 'f::g +60% reason' '`f::g` is not a benchmark name'
accepted_bad accepted_no_limit_refused 'f::g::x' 'no limit and no reason'
accepted_bad accepted_no_reason_refused 'f::g::x +60%' 'no reason; say why the regression is intended'
accepted_bad accepted_blank_reason_refused $'f::g::x +60% \t ' 'no reason'
accepted_bad accepted_limit_without_plus_refused 'f::g::x 60% reason' 'the limit `60%` is not written +N%'
accepted_bad accepted_limit_without_percent_refused 'f::g::x +60 reason' 'the limit `+60` is not written +N%'
accepted_bad accepted_limit_spaced_refused 'f::g::x +60 % reason' 'the limit `+60` is not written +N%'
accepted_bad accepted_limit_fraction_refused 'f::g::x +60.5% reason' 'the limit `+60.5%` is not written +N%'
accepted_bad accepted_limit_leading_zero_refused 'f::g::x +060% reason' 'the limit `+060%` is not written +N%'
accepted_bad accepted_limit_word_refused 'f::g::x any reason' 'the limit `any` is not written +N%'
accepted_bad accepted_limit_negative_refused 'f::g::x -60% reason' 'the limit `-60%` is not written +N%'
accepted_bad accepted_limit_ten_refused 'f::g::x +10% reason' 'the limit +10% is not between +11% and +1000% (up to +10% needs no entry)'
accepted_bad accepted_limit_above_most_refused 'f::g::x +1001% reason' 'the limit +1001% is not between +11% and +1000%'
accepted_bad accepted_limit_huge_refused 'f::g::x +99999999999999999999% reason' 'the limit +99999999999999999999% is not between +11% and +1000%'

start accepted_every_bad_line_named
put base f::g::x 1000
put head f::g::x 1500
accept '# header' 'f::g::x +60%' 'f::g::* +60% reason'
check accepted_every_bad_line_named 2 'accepted.txt:2: no reason' 'accepted.txt:3: `f::g::*` is not a benchmark name' \
  'found `f::g::* +60% reason`'

start accepted_duplicate_refused
put base f::g::x 1000
put head f::g::x 1500
accept 'f::g::x +60% reason' 'f::g::x +20% another reason'
check accepted_duplicate_refused 2 '`f::g::x` has 2 entries (lines 1, 2); keep one'

start accepted_unknown_refused
put base f::g::x 1000
put head f::g::x 1500
accept 'f::g::y +60% reason'
check accepted_unknown_refused 2 '`f::g::y` (line 1): not measured in both the base and the head'

start accepted_new_benchmark_refused
put base f::g::x 1000
put head f::g::x 1000
put head f::g::y 1000
accept 'f::g::y +60% reason'
check accepted_new_benchmark_refused 2 '`f::g::y` (line 1): not measured in both the base and the head'

start accepted_removed_benchmark_refused
put base f::g::x 1000
put base f::g::y 1000
put head f::g::x 1000
echo 'f::g::y' >"$case_dir/removed.txt"
accept 'f::g::y +60% reason'
check accepted_removed_benchmark_refused 2 '`f::g::y` (line 1): not measured in both the base and the head'

# After the merge the entry is in the base's list too: stale. Its
# regression is in the base count, and it never applies again.
start stale_entry_warns
put base f::g::x 1500
put head f::g::x 1500
accept 'f::g::x +60% reason'
accept_base 'f::g::x +60% reason'
check stale_entry_warns 0 '| `f::g::x` | 1,500 | 1,500 | 0.00 % | ok |' \
  '**Stale:** 1 entr(ies) in' "also in the base's list, so already merged and not applied: \`f::g::x\` (line 1)" \
  '**Passed:** no benchmark rose by more than 10 % (ADR-0007).' '!**Accepted:**'

start stale_entry_never_applies
put base f::g::x 1000
put head f::g::x 1500
accept 'f::g::x +60% reason'
accept_base 'f::g::x +60% reason'
check stale_entry_never_applies 1 '| `f::g::x` | 1,000 | 1,500 | +50.00 % | **regression** |' \
  '**Failed:** 1 benchmark(s) rose by more than 10 % (ADR-0007): `f::g::x`.' '**Stale:**'

# A stale entry is not checked against the runs: a later pull request may
# remove its benchmark without touching the list.
start stale_entry_of_removed_benchmark
put base f::g::x 1000
put base f::g::y 1000
put head f::g::x 1000
echo 'f::g::y' >"$case_dir/removed.txt"
accept 'f::g::y +60% reason'
accept_base 'f::g::y +60% reason'
check stale_entry_of_removed_benchmark 0 '**Stale:**' '| `f::g::y` | 1,000 | - | - | removed (listed) |'

# A pull request that changes the list deletes the stale entries, so the
# list holds at most the entries of the last one that changed it.
start stale_left_beside_new_entry_fails
put base f::g::x 1500
put base f::g::y 1000
put head f::g::x 1500
put head f::g::y 1500
accept 'f::g::x +60% reason' 'f::g::y +60% reason'
accept_base 'f::g::x +60% reason'
check stale_left_beside_new_entry_fails 2 \
  "\`f::g::x\` (line 1): also in the base's list, so already merged and never applied again (stale); a pull request that changes the list deletes them"

start stale_deleted_beside_new_entry_passes
put base f::g::x 1500
put base f::g::y 1000
put head f::g::x 1500
put head f::g::y 1500
accept '# header' 'f::g::y +60% reason'
accept_base '# header' 'f::g::x +60% reason'
check stale_deleted_beside_new_entry_passes 0 '| `f::g::y` | 1,000 | 1,500 | +50.00 % | accepted (limit +60 %) |' \
  '- `f::g::y`: +50.00 %, limit +60 % (line 2).' '!**Stale:**'

start stale_deleted_alone_passes
put base f::g::x 1500
put head f::g::x 1500
accept '# header'
accept_base '# header' 'f::g::x +60% reason'
check stale_deleted_alone_passes 0 '**Passed:** no benchmark rose by more than 10 % (ADR-0007).' '!**Stale:**'

# Only a comment changed: the list changed, so the stale entry must go.
start stale_left_when_comment_changes_fails
put base f::g::x 1500
put head f::g::x 1500
accept '# new header' 'f::g::x +60% reason'
accept_base '# header' 'f::g::x +60% reason'
check stale_left_when_comment_changes_fails 2 'already merged and never applied again (stale)'

# Re-spacing an entry does not make it new: it stays stale and does not
# accept the head's regression.
start stale_respaced_is_still_stale
put base f::g::x 1000
put head f::g::x 1500
accept 'f::g::x   +60%   reason'
accept_base 'f::g::x +60% reason'
check stale_respaced_is_still_stale 2 '`f::g::x` (line 1): also in the base'"'"'s list'

# Changing an entry's limit or reason makes it a new entry, which applies
# (and must be needed) like any other the owner reviews.
start edited_entry_is_new
put base f::g::x 1000
put head f::g::x 1500
accept 'f::g::x +60% the new reason'
accept_base 'f::g::x +60% the old reason'
check edited_entry_is_new 0 '| `f::g::x` | 1,000 | 1,500 | +50.00 % | accepted (limit +60 %) |' '!**Stale:**'

start edited_entry_not_needed_fails
put base f::g::x 1500
put head f::g::x 1500
accept 'f::g::x +70% reason'
accept_base 'f::g::x +60% reason'
check edited_entry_not_needed_fails 1 '**accepted entry not needed**'

# The base's copy is read leniently (only to find stale entries): a bad line
# there does not block the pull request that fixes it.
start bad_base_list_can_be_fixed
put base f::g::x 1000
put head f::g::x 1000
accept '# fixed'
accept_base 'f::g::x +60%'
check bad_base_list_can_be_fixed 0 '**Passed:**'

# The committed list parses (read as its own base, so its entries, if any,
# are stale and not checked against these runs).
got=0
out=$("$gate" --accepted ci/bench-accepted.txt --accepted-base ci/bench-accepted.txt \
  "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect committed_accepted_list_parses 0 "$out" "$got" '**Passed:**'

got=0
out=$("$gate" --accepted "$work/no-such-list" "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect missing_accepted_list 2 "$out" "$got" "accepted list $work/no-such-list does not exist or cannot be read"

got=0
out=$("$gate" --accepted ci/bench-accepted.txt --accepted-base "$work/no-such-list" \
  "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect missing_accepted_base_list 2 "$out" "$got" "accepted list $work/no-such-list does not exist or cannot be read"

got=0
out=$("$gate" --accepted "$work" "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect accepted_list_is_a_directory 2 "$out" "$got" 'does not exist or cannot be read'

got=0
out=$("$gate" --accepted-base ci/bench-accepted.txt "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect accepted_base_alone_refused 2 "$out" "$got" 'usage: ci/bench-compare.sh'

got=0
out=$("$gate" --accepted ci/bench-accepted.txt --accepted "$work/accepted_within_limit/accepted.txt" \
  "$work/accepted_within_limit/base" "$work/accepted_within_limit/head" 2>&1) || got=$?
expect accepted_twice_refused 2 "$out" "$got" 'usage: ci/bench-compare.sh'

got=0
out=$("$gate" --accepted "" "$work/unchanged/base" "$work/unchanged/head" 2>&1) || got=$?
expect accepted_empty_path_refused 2 "$out" "$got" 'usage: ci/bench-compare.sh'

# The lists may come from a pipe, e.g. `--accepted-base <(git show main:…)`.
got=0
out=$("$gate" --accepted "$work/accepted_within_limit/accepted.txt" --accepted-base <(printf '# empty\n') \
  "$work/accepted_within_limit/base" "$work/accepted_within_limit/head" 2>&1) || got=$?
expect accepted_base_from_pipe 0 "$out" "$got" 'accepted (limit +60 %)'

# Options in any order.
got=0
out=$("$gate" --accepted "$work/accepted_within_limit/accepted.txt" --removed "$work/accepted_within_limit/removed.txt" \
  "$work/accepted_within_limit/base" "$work/accepted_within_limit/head" 2>&1) || got=$?
expect options_in_any_order 0 "$out" "$got" 'accepted (limit +60 %)'

# ---- ci/bench-targets.sh ----------------------------------------------

# metadata <package:target:kind>...: minimal `cargo metadata` JSON.
metadata() {
  local spec
  for spec in "$@"; do
    printf '%s\n' "$spec"
  done | jq -R -s '
    split("\n") | map(select(length > 0) | split(":"))
    | group_by(.[0])
    | {packages: map({name: .[0][0], targets: map({name: .[1], kind: [.[2]]})})}'
}

# targets <name> <exit code> <metadata> [--strict] [line]...
targets() {
  local name=$1 want=$2 json=$3 got=0 out
  shift 3
  local args=()
  if [ "${1:-}" = --strict ]; then
    args=(--strict)
    shift
  fi
  out=$(ci/bench-targets.sh "${args[@]+"${args[@]}"}" <<<"$json" 2>&1) || got=$?
  expect "$name" "$want" "$out" "$got" "$@"
}

ws=$(metadata taktsim-bench:taktsim_bench:lib taktsim-bench:instructions_random:bench \
  taktsim-bench:instructions_units:bench taktsim-bench:wall_random:bench taktsim-bench:workloads:test \
  taktsim-core:taktsim_core:lib)
targets targets_gated_only 0 "$ws" instructions_random instructions_units
out=$(ci/bench-targets.sh <<<"$ws")
if [ "$out" = $'instructions_random\ninstructions_units' ]; then echo "ok   targets_exactly_the_gated"; else
  fail "targets_exactly_the_gated: got '$out'"
fi
targets targets_strict_ok 0 "$ws" --strict instructions_random

bad_name=$(metadata taktsim-bench:instructions_random:bench taktsim-bench:queue:bench)
targets targets_strict_bad_name 1 "$bad_name" --strict 'taktsim-bench queue'
targets targets_lax_bad_name 0 "$bad_name" instructions_random

elsewhere=$(metadata taktsim-bench:instructions_random:bench taktsim-core:instructions_queue:bench)
targets targets_strict_other_package 1 "$elsewhere" --strict 'taktsim-core instructions_queue'
out=$(ci/bench-targets.sh <<<"$elsewhere")
if [ "$out" = instructions_random ]; then echo "ok   targets_other_package_not_run"; else
  fail "targets_other_package_not_run: got '$out'"
fi

wall_only=$(metadata taktsim-bench:wall_random:bench taktsim-bench:taktsim_bench:lib)
targets targets_strict_none 1 "$wall_only" --strict 'no instruction-count benchmarks'
out=$(ci/bench-targets.sh <<<"$wall_only")
if [ -z "$out" ]; then echo "ok   targets_none_lax_empty"; else
  fail "targets_none_lax_empty: got '$out'"
fi

# ---- ci/bench-runner-version.sh ---------------------------------------

# lock <name> <exit code> <Cargo.lock text> [line]...
lock() {
  local name=$1 want=$2 text=$3 got=0 out
  shift 3
  printf '%s\n' "$text" >"$work/$name.lock"
  out=$(ci/bench-runner-version.sh "$work/$name.lock" 2>&1) || got=$?
  expect "$name" "$want" "$out" "$got" "$@"
}

package() {
  printf '[[package]]\nname = "%s"\nversion = "%s"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n' "$1" "$2"
}

lock lock_one_version 0 "$(package gungraun-macros 0.10.0; package gungraun 0.20.0; package gungraun-runner 0.20.0)" 0.20.0
out=$(ci/bench-runner-version.sh "$work/lock_one_version.lock")
if [ "$out" = 0.20.0 ]; then echo "ok   lock_prints_only_the_version"; else
  fail "lock_prints_only_the_version: got '$out'"
fi
lock lock_runner_alone_is_not_the_library 0 "$(package gungraun-runner 0.20.0; package syn 2.0.119)"
out=$(ci/bench-runner-version.sh "$work/lock_runner_alone_is_not_the_library.lock")
if [ -z "$out" ]; then echo "ok   lock_none_prints_nothing"; else
  fail "lock_none_prints_nothing: got '$out'"
fi
lock lock_two_versions 1 "$(package gungraun 0.19.4; package gungraun 0.20.0)" 'several gungraun versions'
got=0
out=$(ci/bench-runner-version.sh "$work/no-such.lock" 2>&1) || got=$?
expect lock_missing_file 1 "$out" "$got" 'does not exist'
# The repository's own lockfile resolves exactly one version.
out=$(ci/bench-runner-version.sh)
if grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+' <<<"$out"; then echo "ok   lock_repository ($out)"; else
  fail "lock_repository: got '$out'"
fi

if [ "$failures" -gt 0 ]; then
  echo "error: $failures bench gate test(s) failed" >&2
  exit 1
fi
echo "all bench gate tests passed"
