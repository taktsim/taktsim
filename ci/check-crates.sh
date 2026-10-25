#!/usr/bin/env bash
# Every crate under crates/ (and python/) is a workspace member, uses the workspace lints and
# license, and declares forbid(unsafe_code) and deny(missing_docs) at its root
# (spec §7 Layer 1).
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
err() { echo "error: $*" >&2; fail=1; }

members=$(cargo metadata --no-deps --format-version 1 | jq -r '.workspace_members[]')

for dir in crates/*/ python/; do
  dir=${dir%/}
  name=$(basename "$dir")
  manifest="$dir/Cargo.toml"
  grep -q "/$dir#\|/$dir)" <<<"$members" || err "$dir is not a workspace member (Cargo.toml [workspace] members)"
  awk 'prev == "[lints]" && $0 == "workspace = true" {ok = 1} {prev = $0} END {exit !ok}' "$manifest" || err "$manifest: missing [lints] workspace = true"
  grep -q '^license.workspace = true' "$manifest" || err "$manifest: missing license.workspace = true"
  for root in "$dir/src/lib.rs" "$dir/src/main.rs"; do
    [ -f "$root" ] || continue
    grep -q '^#!\[forbid(unsafe_code)\]' "$root" || err "$root: missing #![forbid(unsafe_code)]"
    grep -q '^#!\[deny(missing_docs)\]' "$root" || err "$root: missing #![deny(missing_docs)]"
  done
done

exit "$fail"
