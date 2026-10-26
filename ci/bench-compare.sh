#!/usr/bin/env bash
# Instruction-count regression gate (spec §7, ADR-0007, ADR-0016).
#
# Compares the Callgrind instruction count (Ir) of every gungraun benchmark
# between two runs on the same machine and fails if one rose by more than
# 10 %, unless a new entry in the accepted-regressions list allows that
# benchmark more, up to the entry's limit. A benchmark only in the head is
# reported as new and passes. A benchmark only in the base fails unless the
# removed list names it.
#
# Usage: ci/bench-compare.sh [--removed <file>]
#          [--accepted <file> [--accepted-base <file>]] <base-dir> <head-dir>
#   Each directory holds the summary.json files of one run of
#   ci/bench-run.sh (searched recursively). The base directory may be empty:
#   the base had no benchmarks yet.
#   --removed: benchmarks that may be missing from the head, one name per
#   line as in the table (`#` starts a comment); the bench workflow passes
#   the head's ci/bench-removed.txt.
#   --accepted: accepted regressions, one per line, `<benchmark> +<N>%
#   <reason>` (a line whose first word starts with `#` is a comment; the
#   format and rules are in ci/bench-accepted.txt); the bench workflow
#   passes the head's ci/bench-accepted.txt.
#   --accepted-base: the same list as the base has it (an empty file if the
#   base has none). An entry that is in both lists is stale: its regression
#   is already in the base, and it never applies again. Without this option
#   every entry is new.
# Output: a Markdown table on stdout (benchmark, base, head, change,
#   result), sorted by benchmark; errors on stderr.
# Exit: 0 passed; 1 a regression (above 10 %, or above the limit of the
#   benchmark's new accepted entry), a new accepted entry that was not
#   needed, or an unlisted removal; 2 the input cannot be trusted: an
#   unreadable, unsupported or incomplete summary, a benchmark measured
#   twice, no benchmark in the head, a base with benchmarks of which none is
#   in the head, a listed removal that the head still measures, a malformed
#   or duplicate accepted entry, a new entry naming a benchmark that is not
#   measured in both runs, or stale entries left in an accepted list that
#   differs from the base's. The gate never passes on input it cannot read.
#
# The bench workflow runs this script from the base commit when the base
# has it, so a pull request is judged by the gate it would change: keep the
# arguments and exit codes stable.
# Needs bash and jq 1.6 or later. Tests: ci/bench-test.sh.
set -euo pipefail

# ADR-0007: "> 10 % regression fails". Exactly +10 % passes.
max_increase_pct=10
# The highest limit an accepted entry may state (ADR-0016): the count may
# at most become eleven times the base's. A change that costs more is a
# different workload: a new benchmark, with the old one listed as removed.
max_accepted_pct=1000
# gungraun summary schema versions this script reads (summary.v7 is
# gungraun >= 0.20). A new version fails with exit 2 until it is checked
# and added here.
supported_versions='["7"]'

usage() {
  echo "usage: ci/bench-compare.sh [--removed <file>] [--accepted <file> [--accepted-base <file>]] <base-dir> <head-dir>" >&2
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

removed_file= accepted_file= accepted_base_file=
while [ $# -gt 0 ]; do
  case $1 in
    --removed | --accepted | --accepted-base)
      # Each option at most once, with a non-empty value.
      if [ $# -lt 2 ] || [ -z "$2" ]; then usage; fi
      case $1 in
        --removed) [ -z "$removed_file" ] || usage; removed_file=$2 ;;
        --accepted) [ -z "$accepted_file" ] || usage; accepted_file=$2 ;;
        --accepted-base) [ -z "$accepted_base_file" ] || usage; accepted_base_file=$2 ;;
      esac
      shift 2
      ;;
    *) break ;;
  esac
done
if [ -n "$removed_file" ] && [ ! -f "$removed_file" ]; then
  echo "error: removed list $removed_file does not exist" >&2
  exit 2
fi
if [ -n "$accepted_base_file" ] && [ -z "$accepted_file" ]; then
  usage
fi
# Readable and not a directory; a pipe such as `<(git show …)` is fine.
for list in "$accepted_file" "$accepted_base_file"; do
  if [ -n "$list" ] && { [ ! -r "$list" ] || [ -d "$list" ]; }; then
    echo "error: accepted list $list does not exist or cannot be read" >&2
    exit 2
  fi
done
if [ $# -ne 2 ] || [ ! -d "$1" ] || [ ! -d "$2" ]; then
  usage
fi

# The removed list as a JSON array of names, comments and blanks dropped.
removed='[]'
if [ -n "$removed_file" ]; then
  removed=$(sed -e 's/#.*//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$removed_file" |
    jq -R -s 'split("\n") | map(select(length > 0))')
fi

# The accepted regressions as {new, stale, changed}: the entries of the
# list that the base's list does not have (only these apply), the ones it
# has (stale, never applied), and whether the two lists differ. Exits 2,
# listing every bad line, if an entry is malformed or a benchmark has two.
accepted='{"new": [], "stale": [], "changed": false}'
if [ -n "$accepted_file" ]; then
  accepted_text=$(cat -- "$accepted_file")
  accepted_base_text=
  if [ -n "$accepted_base_file" ]; then
    accepted_base_text=$(cat -- "$accepted_base_file")
  fi
  if ! accepted=$(jq -n --arg file "$accepted_file" --arg text "$accepted_text" \
    --arg base "$accepted_base_text" --argjson max "$max_increase_pct" \
    --argjson most "$max_accepted_pct" '
    # The words of a line, split at white space.
    def tokens: [splits("\\s+")] | map(select(length > 0));
    # {n: line number, t: words} of each line that is neither blank nor a
    # comment (first word starting with #); a later # belongs to the reason.
    def entry_lines:
      split("\n") | . as $l
      | range(0; length) | {n: (. + 1), t: ($l[.] | tokens)}
      | select((.t | length) > 0 and (.t[0] | startswith("#") | not));
    # An entry is identified by its words: white space does not matter, but
    # any change of benchmark, limit or reason makes it a new entry.
    [$base | entry_lines | .t | join(" ")] as $base_keys
    | [$text | entry_lines
       | .n as $n | .t as $t | ($t | join(" ")) as $key
       | if ($t[0] | test("^[A-Za-z0-9_]+(::[A-Za-z0-9_]+){2,}$") | not) then
           {n: $n, key: $key, problem: "`\($t[0])` is not a benchmark name as the table shows it (file::group::function or file::group::function::id, of letters, digits and _)"}
         elif ($t | length) < 2 then
           {n: $n, key: $key, problem: "no limit and no reason"}
         elif ($t[1] | test("^\\+[1-9][0-9]*%$") | not) then
           {n: $n, key: $key, problem: "the limit `\($t[1])` is not written +N% with N a whole number"}
         elif ($t | length) < 3 then
           {n: $n, key: $key, problem: "no reason; say why the regression is intended"}
         else
           ($t[1] | ltrimstr("+") | rtrimstr("%")) as $d
           | if ($d | tonumber) <= $max or ($d | tonumber) > $most then
               {n: $n, key: $key, problem: "the limit \($t[1]) is not between +\($max + 1)% and +\($most)% (up to +\($max)% needs no entry)"}
             else
               {n: $n, key: $key, name: $t[0], limit: ($d | tonumber), reason: ($t[2:] | join(" "))}
             end
         end] as $lines
    | [$lines[] | select(has("problem")) | "\($file):\(.n): \(.problem); found `\(.key)`"]
      + ([$lines[] | select(has("name"))] | group_by(.name) | map(select(length > 1))
         | map("\($file): `\(.[0].name)` has \(length) entries (lines \(map(.n | tostring) | join(", "))); keep one"))
    | if length > 0 then
        (map("error: " + .) | join("\n"))
        + "\nexpected one entry per line, `<benchmark> +<N>% <reason>`, e.g.\n"
        + "  instructions_random::stream::next_u64::draws +25% every draw now checks the stream'"'"'s position (ADR-0007)\n"
        | halt_error(2)
      else . end
    | [$lines[] | select(has("name"))]
    | {new: map(select(.key as $k | $base_keys | any(. == $k) | not)),
       stale: map(select(.key as $k | $base_keys | any(. == $k))),
       changed: ($text != $base)}'); then
    exit 2
  fi
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
  --argjson accepted "$accepted" --arg accepted_file "${accepted_file:-the accepted list}" \
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
  # Accepted entries as "`name` (line n)".
  def entries: map("`\(.name)` (line \(.n))") | join(", ");
  def failed: .kind as $k | ["regression", "above", "unneeded", "gone"] | any(. == $k);
  def compared: .kind as $k | ["ok", "improvement", "regression", "accepted", "above", "unneeded"] | any(. == $k);

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
  # An accepted entry names one benchmark that both runs measure: a new or
  # removed benchmark has no regression to accept.
  | [$accepted.new[] | .name as $n | select((($b | has($n)) and ($h | has($n))) | not)] as $unknown
  | if ($unknown | length) > 0 then
      "error: \($accepted_file): \($unknown | entries): not measured in both the base and the head; an entry names one benchmark that both runs measure, as the table shows it\n" | halt_error(2)
    else . end
  | if ($accepted.stale | length) > 0 and $accepted.changed then
      "error: \($accepted_file): \($accepted.stale | entries): also in the base'"'"'s list, so already merged and never applied again (stale); a pull request that changes the list deletes them\n" | halt_error(2)
    else . end
  # head × 100 > base × (100 + limit), exact while the product stays below
  # 2^53 (the counts themselves are checked against 2^53 / 110).
  | [$accepted.new[] | select($b[.name].ir * (100 + .limit) >= pow(2; 53))] as $inexact
  | if ($inexact | length) > 0 then
      "error: \($accepted_file): \($inexact | entries): the base count times the limit is too large to compare exactly\n" | halt_error(2)
    else . end
  | ($accepted.new | by_name) as $a
  | [($b + $h) | keys[] as $k
     | if ($b | has($k)) and ($h | has($k)) then
         $b[$k].ir as $old | $h[$k].ir as $new | $a[$k] as $e
         | {name: $k, base: ($old | thousands), head: ($new | thousands), entry: $e,
            change: (if $old == 0 then (if $new == 0 then "0.00 %" else "n/a (base 0)" end)
                     else (($new - $old) / $old * 100 | percent) end)}
         + (if $new * 100 > $old * (100 + $max) then
              (if $e == null then {kind: "regression", result: "regression"}
               elif $new * 100 > $old * (100 + $e.limit) then
                 {kind: "above", result: "regression above the accepted +\($e.limit) %"}
               else {kind: "accepted", result: "accepted (limit +\($e.limit) %)"} end)
            elif $e != null then {kind: "unneeded", result: "accepted entry not needed"}
            elif $new * 100 < $old * (100 - $max) then {kind: "improvement", result: "improvement"}
            else {kind: "ok", result: "ok"} end)
       elif $h | has($k) then
         {name: $k, base: "-", head: ($h[$k].ir | thousands), change: "-", kind: "new", result: "new"}
       elif ($removed | any(. == $k)) then
         {name: $k, base: ($b[$k].ir | thousands), head: "-", change: "-", kind: "removed", result: "removed (listed)"}
       else
         {name: $k, base: ($b[$k].ir | thousands), head: "-", change: "-", kind: "gone", result: "removed, not listed"}
       end]
  # One row per name; `keys` is sorted, so the rows are sorted by name.
  | . as $rows
  | ([$rows[] | select(compared)] | length) as $compared
  | if ($base | length) > 0 and $compared == 0 then
      "error: none of the base'"'"'s \($base | length) benchmarks is in the head, so nothing was compared; add new benchmarks and remove old ones in separate pull requests\n" | halt_error(2)
    else . end
  | [$rows[] | select(.kind == "regression")] as $bad
  | [$rows[] | select(.kind == "above")] as $above
  | [$rows[] | select(.kind == "unneeded")] as $unneeded
  | [$rows[] | select(.kind == "gone")] as $gone
  | [$rows[] | select(.kind == "accepted")] as $ok_accepted
  | ([$rows[] | select(failed)] | length) as $failures
  | ([$rows[] | select(.kind == "new")] | length) as $new
  | ([$rows[] | select(.kind == "removed" or .kind == "gone")] | length) as $dropped
  | "### Instruction counts (Callgrind Ir), base and head on the same runner",
    "",
    "| Benchmark | Base | Head | Change | Result |",
    "| --- | ---: | ---: | ---: | --- |",
    ($rows[] | "| `\(.name)` | \(.base) | \(.head) | \(.change) | \(if failed then "**\(.result)**" else .result end) |"),
    "",
    (if ($bad | length) > 0 then
       "**Failed:** \($bad | length) benchmark(s) rose by more than \($max) % (ADR-0007): \($bad | names)."
     else empty end),
    (if ($above | length) > 0 then
       "**Failed:** \($above | length) benchmark(s) rose by more than the limit of their entry in \($accepted_file) (ADR-0016): \($above | map("`\(.name)` (\(.change), limit +\(.entry.limit) %)") | join(", "))."
     else empty end),
    (if ($unneeded | length) > 0 then
       "**Failed:** \($unneeded | length) entr(ies) in \($accepted_file) not needed, the benchmark rose by \($max) % or less; delete them: \($unneeded | map("`\(.name)` (\(.change))") | join(", "))."
     else empty end),
    (if ($gone | length) > 0 then
       "**Failed:** \($gone | length) benchmark(s) removed without being listed in ci/bench-removed.txt (ADR-0016): \($gone | names)."
     else empty end),
    (if ($ok_accepted | length) > 0 then
       # A blank line after the failure lines, if any, so the list stands apart.
       (if $failures > 0 then "" else empty end),
       "**Accepted:** \($ok_accepted | length) benchmark(s) rose by more than \($max) %, within the limit of a new entry in \($accepted_file) (ADR-0016):",
       "",
       ($ok_accepted[] | "- `\(.name)`: \(.change), limit +\(.entry.limit) % (line \(.entry.n)). Reason: \(.entry.reason)"),
       ""
     else empty end),
    (if ($accepted.stale | length) > 0 then
       "**Stale:** \($accepted.stale | length) entr(ies) in \($accepted_file) also in the base'"'"'s list, so already merged and not applied: \($accepted.stale | entries); delete them when the list next changes (ADR-0016)."
     else empty end),
    (if $failures == 0 then
       (if ($ok_accepted | length) == 0 then
          "**Passed:** no benchmark rose by more than \($max) % (ADR-0007)."
        else
          "**Passed:** no benchmark rose by more than \($max) % (ADR-0007), except the accepted ones above."
        end)
     else empty end),
    "\($rows | length) benchmark(s): \($compared) compared, \($new) new, \($dropped) removed.",
    (if $failures > 0 then "" | halt_error(1) else empty end)
'
