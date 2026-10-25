#!/usr/bin/env bash
# Instruction-count regression gate (spec §7, ADR-0007, ADR-0016).
#
# Compares the Callgrind instruction count (Ir) of every gungraun benchmark
# between two runs on the same machine and fails if one rose by more than
# 10 %. A benchmark only in the head is reported as new, one only in the
# base as removed; neither fails the gate.
#
# Usage: ci/bench-compare.sh <base-dir> <head-dir>
#   Each directory holds the summary.json files of one run of
#   ci/bench-run.sh (searched recursively). The base directory may be empty:
#   the base had no benchmarks yet.
# Output: a Markdown table on stdout (benchmark, base, head, change,
#   result), sorted by benchmark; errors on stderr.
# Exit: 0 no regression; 1 at least one regression; 2 the input cannot be
#   trusted (unreadable, unsupported or incomplete summary, a benchmark
#   twice, no benchmark in the head). The gate never passes on input it
#   cannot read.
# Tests: ci/bench-compare-test.sh.
set -euo pipefail

# ADR-0007: "> 10 % regression fails". Exactly +10 % passes.
max_increase_pct=10
# gungraun summary schema versions this script reads (summary.v7 is
# gungraun >= 0.20). A new version fails with exit 2 until it is checked
# and added here.
supported_versions='["7"]'

if [ $# -ne 2 ] || [ ! -d "$1" ] || [ ! -d "$2" ]; then
  echo "usage: ci/bench-compare.sh <base-dir> <head-dir> (both existing directories)" >&2
  exit 2
fi

# Reads every summary.json under $1 into a JSON array of
# {key, name, ir}; exits 2 with the file name if one is unusable.
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
     | if type != "object" then fail("not a gungraun summary (expected a JSON object)") end
     | . as $s
     | ($s.version | tostring) as $v
     | if ($supported | any(. == $v)) | not then
         fail("summary version \($s.version | tojson) is not supported (supported: \($supported | join(", "))); check the new schema and update ci/bench-compare.sh")
       end
     | if ($s.module_path | type) != "string" or ($s.package_dir | type) != "string" then
         fail("module_path and package_dir must be strings")
       end
     | if (($s.id // null) | type) as $t | ($t != "string" and $t != "null") then
         fail("id must be a string or null")
       end
     | [$s.profiles[]? | select(.tool == "Callgrind") | .data.total.metrics.Ir.values.new]
     | if length != 1 then fail("expected one Callgrind instruction count (Ir), found \(length)") end
     | .[0] as $ir
     | if ($ir | type) != "number" or $ir < 0 or $ir != ($ir | floor) or $ir > $max_exact then
         fail("instruction count \($ir | tojson) is not a whole number between 0 and \($max_exact)")
       end
     | ($s.module_path + (if $s.id then "::" + $s.id else "" end)) as $name
     | {key: ($s.package_dir + " " + $name), name: $name, ir: $ir}
    ]' "${files[@]}"; then
    echo "error: cannot read the gungraun summaries in $dir" >&2
    exit 2
  fi
}

base=$(read_runs "$1")
head=$(read_runs "$2")

jq -n -r --argjson base "$base" --argjson head "$head" --argjson max "$max_increase_pct" '
  def duplicates: group_by(.key) | map(select(length > 1) | .[0].name);
  def by_key: map({(.key): .}) | add // {};
  # 1234567 -> "1,234,567"
  def thousands: tostring | [scan(".{1,3}(?=(?:.{3})*$)")] | join(",");
  # A ratio in percent, with sign and two decimals: "+12.35 %".
  def percent:
    (. * 100 | round) as $c | ($c | fabs) as $a
    | (if $c > 0 then "+" elif $c < 0 then "-" else "" end)
      + "\(($a / 100) | floor).\($a % 100 | tostring | if length < 2 then "0" + . else . end) %";

  if ($head | length) == 0 then
    "error: no benchmark summaries in the head run; it must have at least one instruction-count benchmark\n" | halt_error(2)
  end
  | (($base | duplicates) + ($head | duplicates)) as $dup
  | if ($dup | length) > 0 then
      "error: benchmarks measured twice in one run: \($dup | join(", "))\n" | halt_error(2)
    end
  | ($base | by_key) as $b
  | ($head | by_key) as $h
  | [($b + $h) | keys[] as $k
     | if ($b | has($k)) and ($h | has($k)) then
         $b[$k].ir as $old | $h[$k].ir as $new
         | {name: $h[$k].name, base: ($old | thousands), head: ($new | thousands),
            change: (if $old == 0 then (if $new == 0 then "0.00 %" else "n/a (base 0)" end)
                     else (($new - $old) / $old * 100 | percent) end),
            result: (if $new * 100 > $old * (100 + $max) then "regression"
                     elif $new * 100 < $old * (100 - $max) then "improvement"
                     else "ok" end)}
       elif $h | has($k) then
         {name: $h[$k].name, base: "-", head: ($h[$k].ir | thousands), change: "-", result: "new"}
       else
         {name: $b[$k].name, base: ($b[$k].ir | thousands), head: "-", change: "-", result: "removed"}
       end]
  | sort_by(.name) as $rows
  | [$rows[] | select(.result == "regression")] as $bad
  | ([$rows[] | select(.result == "new")] | length) as $new
  | ([$rows[] | select(.result == "removed")] | length) as $removed
  | "### Instruction counts (Callgrind Ir), base and head on the same runner",
    "",
    "| Benchmark | Base | Head | Change | Result |",
    "| --- | ---: | ---: | ---: | --- |",
    ($rows[] | "| `\(.name)` | \(.base) | \(.head) | \(.change) | \(if .result == "regression" then "**regression**" else .result end) |"),
    "",
    (if ($bad | length) > 0 then
       "**Failed:** \($bad | length) benchmark(s) rose by more than \($max) % (ADR-0007): \($bad | map("`\(.name)`") | join(", "))."
     else
       "**Passed:** no benchmark rose by more than \($max) % (ADR-0007)."
     end),
    "\($rows | length) benchmark(s): \(($rows | length) - $new - $removed) compared, \($new) new, \($removed) removed.",
    (if ($bad | length) > 0 then "" | halt_error(1) else empty end)
'
