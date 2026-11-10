#!/usr/bin/env bash
# Fails if a committed JSON Schema differs from what the Rust types generate
# (spec §7 Layer 1: "JSON Schema ... generated from them and committed; CI
# fails if ... stale"; ADR-0020 §11, ADR-0024 §10). Checks schemas/model/ and
# schemas/cli/: every file the generators write must be committed unchanged, and
# no other file may be there.
#
# Usage: ci/check-schemas.sh [schemas directory]   (default: schemas)
# To regenerate:
#   cargo run -p taktsim-components --example schemas -- schemas
#   cargo run -p taktsim-cli -- describe > schemas/cli/describe.json
set -euo pipefail
cd "$(dirname "$0")/.."

committed=${1:-schemas}
fresh=$(mktemp -d)
trap 'rm -rf "$fresh"' EXIT

cargo run --quiet --locked -p taktsim-components --example schemas -- "$fresh"

if ! diff -r -u "$committed/model" "$fresh/model"; then
  echo "error: $committed/model is stale; regenerate it with" >&2
  echo "  cargo run -p taktsim-components --example schemas -- schemas" >&2
  echo "and commit the result in the same change as the types (CLAUDE.md)." >&2
  exit 1
fi

mkdir -p "$fresh/cli"
cargo run --quiet --locked -p taktsim-cli -- describe >"$fresh/cli/describe.json"

if ! diff -r -u "$committed/cli" "$fresh/cli"; then
  echo "error: $committed/cli is stale; regenerate it with" >&2
  echo "  cargo run -p taktsim-cli -- describe > schemas/cli/describe.json" >&2
  echo "and commit the result in the same change as the CLI (ADR-0024 §10)." >&2
  exit 1
fi
echo "schemas fresh: $(cd "$fresh" && find model cli -type f | sort | tr '\n' ' ')"
