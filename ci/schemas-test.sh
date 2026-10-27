#!/usr/bin/env bash
# Tests for ci/check-schemas.sh: it passes on the committed schemas and fails
# on a stale copy (a changed byte, an extra file, a missing file), so a broken
# freshness check fails CI instead of passing every pull request.
#
# Usage: ci/schemas-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."

check=ci/check-schemas.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

# expect <name> <exit code> <schemas directory>
expect() {
  local name=$1 want=$2 dir=$3 got=0 out
  out=$("$check" "$dir" 2>&1) || got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $name: exit $got, expected $want"
    sed 's/^/    /' <<<"$out"
    failures=$((failures + 1))
  else
    echo "ok   $name"
  fi
}

# copy <case>: a fresh copy of the committed schemas.
copy() {
  rm -rf "${work:?}/$1"
  cp -R schemas "$work/$1"
  echo "$work/$1"
}

expect "the committed schemas are fresh" 0 schemas

dir=$(copy changed)
file=$(find "$dir/model" -type f -name '*.json' | sort | head -n 1)
sed -i.bak 's/"title": "taktsim model/"title": "stale model/' "$file" && rm -f "$file.bak"
if cmp -s "$file" "schemas/model/$(basename "$file")"; then
  echo "FAIL setup: the edit did not change $file"
  failures=$((failures + 1))
fi
expect "a changed schema fails" 1 "$dir"

dir=$(copy extra)
echo '{}' >"$dir/model/0.9.json"
expect "an extra schema fails" 1 "$dir"

dir=$(copy missing)
find "$dir/model" -type f -name '*.json' -delete
expect "a missing schema fails" 1 "$dir"

dir=$(copy empty)
rm -rf "${dir:?}/model"
expect "a missing directory fails" 1 "$dir"

if [ "$failures" -gt 0 ]; then
  echo "$failures case(s) failed" >&2
  exit 1
fi
echo "all cases passed"
