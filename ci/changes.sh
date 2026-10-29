#!/usr/bin/env bash
# Decides whether a change can affect the Rust and Python gates (ci.yml) and
# the instruction-count gate (bench.yml). Prints `rust=true` or `rust=false`
# for $GITHUB_OUTPUT on stdout, and the reason on stderr.
#
# The gates are skipped only if every changed file is on the list in
# skippable() below: files that no build, test, lint or benchmark reads. Any
# other file, or a range that cannot be diffed, runs everything. Renames
# count as a deletion and an addition, so moving a file out of crates/ runs
# the gates too.
#
# Usage: ci/changes.sh <base> <head>
set -euo pipefail
base=$1 head=$2

# README.md is python/pyproject.toml's readme; maturin copies it into the
# package metadata and never fails on its content.
skippable() {
  case "$1" in
    docs/* | website/*) return 0 ;;
    README.md | CONTRIBUTING.md | CLAUDE.md | CODE_OF_CONDUCT.md | SECURITY.md) return 0 ;;
    LICENSE-APACHE | LICENSE-MIT | .nvmrc | cliff.toml) return 0 ;;
    .github/ISSUE_TEMPLATE/* | .github/pull_request_template.md) return 0 ;;
    .github/CODEOWNERS | .github/dependabot.yml) return 0 ;;
    .github/workflows/site.yml | .github/workflows/pr.yml) return 0 ;;
    *) return 1 ;;
  esac
}

run_all() {
  echo "$1: the Rust and Python gates run" >&2
  echo "rust=true"
  exit 0
}

if ! git cat-file -e "$base^{commit}" 2>/dev/null; then
  run_all "base $base is not a commit here (first push of a branch?)"
fi
if ! files=$(git diff --name-only --no-renames "$base...$head"); then
  run_all "cannot diff $base...$head"
fi

while IFS= read -r f; do
  [ -z "$f" ] && continue
  if ! skippable "$f"; then
    run_all "$f changed"
  fi
done <<<"$files"

echo "only documentation, site or repository files changed: the Rust and Python gates are skipped" >&2
echo "rust=false"
