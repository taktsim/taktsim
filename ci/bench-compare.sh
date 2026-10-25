#!/usr/bin/env bash
# Instruction-count regression gate (spec §7, ADR-0007, ADR-0016).
#
# Compares the Callgrind instruction count (Ir) of every gungraun benchmark
# between two runs on the same machine and fails if one rose by more than
# 10 %. A benchmark only in the head is reported as new and passes. A
# benchmark only in the base fails unless the removed list names it.
#
# Usage: ci/bench-compare.sh [--removed <file>] <base-dir> <head-dir>
#   Each directory holds the summary.json files of one run of
#   ci/bench-run.sh (searched recursively). The base directory may be empty:
#   the base had no benchmarks yet.
#   --removed: benchmarks that may be missing from the head, one name per
#   line as in the table (`#` starts a comment); the bench workflow passes
#   the head's ci/bench-removed.txt.
# Output: a Markdown table on stdout (benchmark, base, head, change,
#   result), sorted by benchmark; errors on stderr.
# Exit: 0 passed; 1 a regression or an unlisted removal; 2 the input cannot
#   be trusted: an unreadable, unsupported or incomplete summary, a
#   benchmark measured twice, no benchmark in the head, a base with
#   benchmarks of which none is in the head, or a listed removal that the
#   head still measures. The gate never passes on input it cannot read.
#
# The bench workflow runs this script from the base commit when the base
# has it, so a pull request is judged by the gate it would change: keep the
# arguments and exit codes stable.
# Needs bash and jq 1.6 or later. Tests: ci/bench-test.sh.
set -euo pipefail

# ADR-0007: "> 10 % regression fails". Exactly +10 % passes.
max_increase_pct=10
# gungraun summary schema versions this script reads (summary.v7 is
# gungraun >= 0.20). A new version fails with exit 2 until it is checked
# and added here.
supported_versions='["7"]'

usage() {
  echo "usage: ci/bench-compare.sh [--removed <file>] <base-dir> <head-dir>" >&2
  exit 2
}

# jq 1.6 is the oldest version these programs are written for (no
# `if` without `else`, nothing added in 1.7).
jq_version=$(jq --version 2>/dev/null || true)
case "$jq_version" in
  jq-1.[6-9]* | jq-1.[1-9][0-9]* | jq-[2-9]*) ;;
  *)
    echo "error: needs jq 1.6 or later, found '${jq_version:-no jq}'" >&2
    exit 2
    ;;
esac

removed_file=
if [ "${1:-}" = --removed ]; then
  [ $# -ge 2 ] || usage
  removed_file=$2
  shift 2
  if [ ! -f "$removed_file" ]; then
    echo "error: removed list $removed_file does not exist" >&2
    exit 2
  fi
fi
if [ $# -ne 2 ] || [ ! -d "$1" ] || [ ! -d "$2" ]; then
  usage
fi

# The removed list as a JSON array of names, comments and blanks dropped.
removed='[]'
if [ -n "$removed_file" ]; then
  removed=$(sed -e 's/#.*//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$removed_file" |
    jq -R -s 'split("\n") | map(select(length > 0))')
fi

# Reads every summary.json under $1 into a JSON array of {name, ir};
# exits 2 with the file name if one is unusable.
read_runs() {
  local dir=$1 files=() file
  while IFS= read -r -d '' file; do
    files+=("$file")
  done < <(find "$dir" -type f -name summary.json -print0)
  if [ "${#files[@]}" -eq 0 ]; then
    echo '[]'
    return
  fi
  # Counts are compared as head × 100 > base × 110 in jq's doubles, which
  # is exact while the products stay below 2^53, i.e. for counts up to
  # 2^53 / 110 (about 8 × 10^13 instructions, hours under Callgrind).
  # Larger counts are rejected rather than compared inexactly.
  if ! jq -n --argjson supported "$supported_versions" --argjson max "$max_increase_pct" '
    def fail($msg): error("\(input_filename): \($msg)");
    (pow(2; 53) / (100 + $max) | floor) as $max_exact
    | [inputs
       | if type != "object" then fail("not a gungraun summary (expected a JSON object)") else . end
       | . as $s
       | ($s.version | tostring) as $v
       | if ($supported | any(. == $v)) | not then
           fail("summary version \($s.version | tojson) is not supported (supported: \($supported | join(", "))); check the new schema and update ci/bench-compare.sh")
         else . end
       | if ($s.module_path | type) != "string" then fail("module_path must be a string") else . end
       | if ($s.package_dir | type) != "string" then fail("package_dir must be a string") else . end
       | ($s.id | type) as $t
       | if $t != "string" and $t != "null" then fail("id must be a string or null, found \($t)") else . end
       | [$s.profiles[]? | select(.tool == "Callgrind") | .data.total.metrics.Ir.values.new]
       | if length != 1 then fail("expected one Callgrind instruction count (Ir), found \(length)") else . end
       | .[0] as $ir
       | if ($ir | type) != "number" or $ir < 0 or $ir != ($ir | floor) or $ir > $max_exact then
           fail("instruction count \($ir | tojson) is not a whole number between 0 and \($max_exact)")
         else . end
       # The name gungraun identifies a benchmark by (file::group::function
       # and the id); not the package directory, so moving the crate does
       # not turn every benchmark into a removed and a new one.
       | {name: ($s.module_path + (if $t == "string" then "::" + $s.id else "" end)), ir: $ir}
      ]' "${files[@]}"; then
    echo "error: cannot read the gungraun summaries in $dir" >&2
    exit 2
  fi
}

base=$(read_runs "$1")
head=$(read_runs "$2")

jq -n -r --argjson base "$base" --argjson head "$head" --argjson removed "$removed" \
  --argjson max "$max_increase_pct" '
  def duplicates: group_by(.name) | map(select(length > 1) | .[0].name);
  def by_name: map({(.name): .}) | add // {};
  # 1234567 -> "1,234,567"
  def thousands: tostring | [scan(".{1,3}(?=(?:.{3})*$)")] | join(",");
  # A ratio in percent, with sign and two decimals: "+12.35 %".
  def percent:
    (. * 100 | round) as $c | ($c | fabs) as $a
    | (if $c > 0 then "+" elif $c < 0 then "-" else "" end)
      + "\(($a / 100) | floor).\($a % 100 | tostring | if length < 2 then "0" + . else . end) %";
  def names: map("`\(.name)`") | join(", ");

  if ($head | length) == 0 then
    "error: no benchmark summaries in the head run; it must have at least one instruction-count benchmark\n" | halt_error(2)
  else . end
  | (($base | duplicates) + ($head | duplicates)) as $dup
  | if ($dup | length) > 0 then
      "error: benchmarks measured twice in one run: \($dup | join(", "))\n" | halt_error(2)
    else . end
  | ($base | by_name) as $b
  | ($head | by_name) as $h
  | [$removed[] | . as $n | select($h | has($n))] as $still
  | if ($still | length) > 0 then
      "error: listed as removed but measured in the head: \($still | join(", ")); delete them from the removed list\n" | halt_error(2)
    else . end
  | [($b + $h) | keys[] as $k
     | if ($b | has($k)) and ($h | has($k)) then
         $b[$k].ir as $old | $h[$k].ir as $new
         | {name: $k, base: ($old | thousands), head: ($new | thousands),
            change: (if $old == 0 then (if $new == 0 then "0.00 %" else "n/a (base 0)" end)
                     else (($new - $old) / $old * 100 | percent) end),
            result: (if $new * 100 > $old * (100 + $max) then "regression"
                     elif $new * 100 < $old * (100 - $max) then "improvement"
                     else "ok" end)}
       elif $h | has($k) then
         {name: $k, base: "-", head: ($h[$k].ir | thousands), change: "-", result: "new"}
       elif ($removed | any(. == $k)) then
         {name: $k, base: ($b[$k].ir | thousands), head: "-", change: "-", result: "removed (listed)"}
       else
         {name: $k, base: ($b[$k].ir | thousands), head: "-", change: "-", result: "removed, not listed"}
       end]
  # One row per name; `keys` is sorted, so the rows are sorted by name.
  | . as $rows
  | ([$rows[] | select(.result == "ok" or .result == "improvement" or .result == "regression")] | length) as $compared
  | if ($base | length) > 0 and $compared == 0 then
      "error: none of the base'"'"'s \($base | length) benchmarks is in the head, so nothing was compared; add new benchmarks and remove old ones in separate pull requests\n" | halt_error(2)
    else . end
  | [$rows[] | select(.result == "regression")] as $bad
  | [$rows[] | select(.result == "removed, not listed")] as $gone
  | ([$rows[] | select(.result == "new")] | length) as $new
  | ([$rows[] | select(.result | startswith("removed"))] | length) as $dropped
  | "### Instruction counts (Callgrind Ir), base and head on the same runner",
    "",
    "| Benchmark | Base | Head | Change | Result |",
    "| --- | ---: | ---: | ---: | --- |",
    ($rows[] | "| `\(.name)` | \(.base) | \(.head) | \(.change) | \(if .result == "regression" or .result == "removed, not listed" then "**\(.result)**" else .result end) |"),
    "",
    (if ($bad | length) > 0 then
       "**Failed:** \($bad | length) benchmark(s) rose by more than \($max) % (ADR-0007): \($bad | names)."
     else empty end),
    (if ($gone | length) > 0 then
       "**Failed:** \($gone | length) benchmark(s) removed without being listed in ci/bench-removed.txt (ADR-0016): \($gone | names)."
     else empty end),
    (if ($bad | length) + ($gone | length) == 0 then
       "**Passed:** no benchmark rose by more than \($max) % (ADR-0007)."
     else empty end),
    "\($rows | length) benchmark(s): \($compared) compared, \($new) new, \($dropped) removed.",
    (if ($bad | length) + ($gone | length) > 0 then "" | halt_error(1) else empty end)
'
