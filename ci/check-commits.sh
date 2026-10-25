#!/usr/bin/env bash
# Checks every commit in BASE..HEAD (CLAUDE.md, Commits and PRs):
# - Conventional Commits subject
# - DCO: a Signed-off-by line matching the author (ADR-0006)
# - a commit that changes taktsim-core also changes a test, or names one in a
#   "Test:" trailer (spec §10)
# Usage: ci/check-commits.sh <base> <head>
set -euo pipefail
base=$1 head=$2

fail=0
err() { echo "error: $1: $2" >&2; fail=1; }
subject_re='^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-z0-9._-]+\))?!?: .+'

for c in $(git rev-list --no-merges "$base..$head"); do
  short=$(git rev-parse --short "$c")
  subject=$(git log -1 --format=%s "$c")
  body=$(git log -1 --format=%B "$c")
  author=$(git log -1 --format='%an <%ae>' "$c")

  grep -Eq "$subject_re" <<<"$subject" ||
    err "$short" "subject is not a Conventional Commit: '$subject' (e.g. 'feat(core): add event queue')"

  # GitHub bots (Dependabot) cannot sign off; their PRs still run every gate.
  if [[ $author != *"[bot]@users.noreply.github.com>" ]]; then
    grep -Fqx "Signed-off-by: $author" <<<"$body" ||
      err "$short" "missing 'Signed-off-by: $author' (commit with git commit -s)"
  fi

  if git diff-tree --no-commit-id --name-only -r "$c" | grep -q '^crates/taktsim-core/'; then
    if ! git diff-tree --no-commit-id --name-only -r "$c" | grep -q '^crates/taktsim-core/tests/' &&
       ! git show --format= "$c" -- crates/taktsim-core | grep -Eq '^\+.*(#\[test\]|proptest!|#\[rstest)' &&
       ! grep -Eq '^Test: .+' <<<"$body"; then
      err "$short" "changes taktsim-core without changing a test; add or change one, or name it in a 'Test: <name>' trailer"
    fi
  fi
done

exit "$fail"
