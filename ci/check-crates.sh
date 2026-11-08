#!/usr/bin/env bash
# Every crate under crates/ (and python/) is a workspace member, uses the workspace lints and
# license, and declares forbid(unsafe_code) and deny(missing_docs) at its root
# (spec §7 Layer 1). Workspace crates depend in one direction only, dev
# dependencies included (ADR-0022 §2, ADR-0024 §1): units <- core <- model, trace
# <- components <- run <- cli, python.
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

# allowed <crate> <workspace crates it may depend on>
allowed() {
  local crate=$1 may=$2 dep
  # Fails the script through `fail` if the crate is gone: a renamed crate must
  # not drop out of this check.
  jq -e --arg c "$crate" '.packages[] | select(.name == $c)' <<<"$metadata" >/dev/null ||
    err "$crate: not found in cargo metadata; update ci/check-crates.sh"
  for dep in $(jq -r --arg c "$crate" \
    '.packages[] | select(.name == $c) | .dependencies[].name | select(startswith("taktsim-"))' \
    <<<"$metadata" | sort -u); do
    case " $may " in
      *" $dep "*) ;;
      *) err "$crate depends on $dep, against the direction of ADR-0022 §2 (allowed: ${may:-none})" ;;
    esac
  done
}

metadata=$(cargo metadata --no-deps --format-version 1)
allowed taktsim-units ""
allowed taktsim-core "taktsim-units"
# Pure numbers: core only as a dev-dependency, for seeded test data (ADR-0014, ADR-0025 §1).
allowed taktsim-stats "taktsim-units taktsim-core"
allowed taktsim-model "taktsim-units taktsim-core"
allowed taktsim-trace "taktsim-units taktsim-core"
allowed taktsim-components "taktsim-units taktsim-core taktsim-model taktsim-trace"
allowed taktsim-run "taktsim-units taktsim-core taktsim-model taktsim-trace taktsim-components taktsim-stats"
allowed taktsim-cli "taktsim-units taktsim-model taktsim-components taktsim-run"

exit "$fail"
