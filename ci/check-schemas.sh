#!/usr/bin/env bash
# Fails if a committed JSON Schema differs from what the Rust types generate
# (spec §7 Layer 1: "JSON Schema ... generated from them and committed; CI
# fails if ... stale"; ADR-0020 §11). Checks schemas/model/: every file the
# generator writes must be committed unchanged, and no other file may be
# there.
#
# Usage: ci/check-schemas.sh [schemas directory]   (default: schemas)
# To regenerate: cargo run -p taktsim-model --example schemas -- schemas
set -euo pipefail
cd "$(dirname "$0")/.."

committed=${1:-schemas}
fresh=$(mktemp -d)
trap 'rm -rf "$fresh"' EXIT

cargo run --quiet --locked -p taktsim-model --example schemas -- "$fresh"

if ! diff -r -u "$committed/model" "$fresh/model"; then
  echo "error: $committed/model is stale; regenerate it with" >&2
  echo "  cargo run -p taktsim-model --example schemas -- schemas" >&2
  echo "and commit the result in the same change as the types (CLAUDE.md)." >&2
  exit 1
fi
echo "schemas fresh: $(cd "$fresh" && find model -type f | sort | tr '\n' ' ')"
