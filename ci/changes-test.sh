#!/usr/bin/env bash
# Tests for ci/changes.sh. ci.yml and bench.yml run them before deciding, so
# a broken decision fails the job instead of skipping the gates.
#
# Each case commits files to a scratch repository and checks the decision.
# Usage: ci/changes-test.sh
set -euo pipefail
script=$(cd "$(dirname "$0")" && pwd)/changes.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

cd "$work"
git init -q -b main
git config user.name test
git config user.email test@example.com
git config commit.gpgsign false
mkdir -p crates/core/src docs website
echo 'fn main() {}' >crates/core/src/lib.rs
echo '# Spec' >docs/spec.md
echo '# Read me' >crates/core/README.md
echo 'root' >README.md
git add -A
git commit -q -m base
base=$(git rev-parse HEAD)

# change <expected> <description> <command...>: runs the command on a branch
# from base, commits, and checks the decision for base...HEAD.
change() {
  local expected=$1 what=$2
  shift 2
  git checkout -q -B case "$base"
  "$@"
  git add -A
  git commit -q --allow-empty -m case
  local got
  got=$("$script" "$base" HEAD 2>/dev/null)
  if [ "$got" = "rust=$expected" ]; then
    echo "ok   $what"
  else
    echo "FAIL $what: expected rust=$expected, got $got"
    failures=$((failures + 1))
  fi
}

change false "docs only" sh -c 'echo more >>docs/spec.md'
change false "site only" sh -c 'echo x >website/book.toml'
change false "top-level Markdown" sh -c 'echo more >>README.md; echo x >CONTRIBUTING.md'
change false "site and PR workflows" sh -c 'mkdir -p .github/workflows; echo x >.github/workflows/site.yml; echo x >.github/workflows/pr.yml'
change false "no change at all" true
change true "Rust source" sh -c 'echo "// x" >>crates/core/src/lib.rs'
change true "docs and Rust together" sh -c 'echo more >>docs/spec.md; echo "// x" >>crates/core/src/lib.rs'
change true "Markdown inside a crate" sh -c 'echo more >>crates/core/README.md'
change true "the ci workflow" sh -c 'mkdir -p .github/workflows; echo x >.github/workflows/ci.yml'
change true "the bench workflow" sh -c 'mkdir -p .github/workflows; echo x >.github/workflows/bench.yml'
change true "a ci script" sh -c 'mkdir -p ci; echo x >ci/changes.sh'
change true "a file not on the list" sh -c 'echo x >Cargo.lock'
change true "a file moved from a crate to docs" git mv crates/core/src/lib.rs docs/lib.rs
change true "a deleted crate file" git rm -q crates/core/src/lib.rs

# A base the repository does not have, as on the first push of a branch.
got=$("$script" 0000000000000000000000000000000000000000 HEAD 2>/dev/null)
if [ "$got" = "rust=true" ]; then
  echo "ok   unknown base"
else
  echo "FAIL unknown base: expected rust=true, got $got"
  failures=$((failures + 1))
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures case(s) failed"
  exit 1
fi
echo "all cases passed"
