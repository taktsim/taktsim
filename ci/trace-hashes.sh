#!/usr/bin/env bash
# Cross-platform trace comparison (A4, ADR-0021): every tier-1 target runs
# the golden-trace cases with TAKTSIM_TRACE_HASHES set, which writes one
# file per case: "<case> records=<n> stream=<sha256> file=<sha256>". This
# script gets one directory per target and fails unless every target has
# the same cases with the same hashes, so a target whose traces differ
# fails CI even where the golden snapshots would not catch it.
#
# Prints a Markdown report (for the job summary) on stdout, errors on
# stderr. Needs bash 4 and sha256sum.
# Usage: ci/trace-hashes.sh <dir with one subdirectory per target> <targets expected>
set -euo pipefail
dir=$1
expected=$2

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0
err() { echo "error: $*" >&2; fail=1; }

targets=()
for target in "$dir"/*/; do
  [ -d "$target" ] && targets+=("$(basename "$target")")
done

echo "### Golden-trace hashes across targets"
echo
if [ "${#targets[@]}" -ne "$expected" ]; then
  err "expected hashes from $expected targets, found ${#targets[@]}: ${targets[*]:-none}"
fi
if [ "${#targets[@]}" -eq 0 ]; then
  exit 1
fi

# Group the targets by their sorted case lines: one group means all agree.
declare -A group_of members
variants=()
for target in "${targets[@]}"; do
  shopt -s nullglob
  files=("$dir/$target"/*.txt)
  shopt -u nullglob
  if [ "${#files[@]}" -eq 0 ]; then
    err "$target reported no golden-trace hashes"
    : >"$work/$target"
  else
    cat "${files[@]}" | sort >"$work/$target"
  fi
  key=$(sha256sum <"$work/$target" | cut -c1-16)
  if [ -z "${members[$key]:-}" ]; then
    variants+=("$key")
    cp "$work/$target" "$work/variant-$key"
  fi
  members[$key]="${members[$key]:-}${members[$key]:+, }\`$target\`"
  group_of[$target]=$key
done

echo "| target | cases | hashes |"
echo "| --- | --- | --- |"
for target in "${targets[@]}"; do
  n=0
  for i in "${!variants[@]}"; do
    [ "${variants[$i]}" = "${group_of[$target]}" ] && n=$((i + 1))
  done
  echo "| \`$target\` | $(wc -l <"$work/$target") | set $n |"
done
echo
for i in "${!variants[@]}"; do
  key=${variants[$i]}
  echo "Set $((i + 1)), from ${members[$key]}:"
  echo
  echo '```text'
  cat "$work/variant-$key"
  echo '```'
  echo
done
if [ "${#variants[@]}" -gt 1 ]; then
  err "the targets disagree: ${#variants[@]} different sets of golden-trace hashes"
  for i in "${!variants[@]}"; do
    [ "$i" -eq 0 ] && continue
    echo "  set 1 (${members[${variants[0]}]}) vs set $((i + 1)) (${members[${variants[$i]}]}):" >&2
    diff "$work/variant-${variants[0]}" "$work/variant-${variants[$i]}" | sed 's/^/    /' >&2 || true
  done
fi
exit "$fail"
