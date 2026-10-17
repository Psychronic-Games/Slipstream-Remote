#!/usr/bin/env bash
# Files every docs/issues/*.md as a GitHub issue (title and labels from the
# front matter, the rest as the body). Skips issues whose title already
# exists, so it's safe to run again. Needs the GitHub CLI (`gh auth login`)
# and must run inside the repository's clone.
#
#   scripts/file-issues.sh            # file them
#   scripts/file-issues.sh --dry-run  # just show what would be filed
set -euo pipefail
cd "$(dirname "$0")/.."
DRY=${1:-}

existing=""
[ "$DRY" = "--dry-run" ] || existing=$(gh issue list --state all --limit 500 --json title --jq '.[].title')

for f in docs/issues/[0-9]*.md; do
  title=$(sed -n 's/^title: *//p' "$f" | head -1 | sed 's/^"\(.*\)"$/\1/')
  labels=$(sed -n 's/^labels: *\[\(.*\)\]/\1/p' "$f" | head -1 | tr -d ' ')
  body=$(awk 'n>=2 {print} /^---$/ {n++}' "$f")
  if grep -Fxq "$title" <<< "$existing"; then
    echo "skip  $title (already filed)"
    continue
  fi
  if [ "$DRY" = "--dry-run" ]; then
    echo "would file  $title  [$labels]"
    continue
  fi
  args=()
  IFS=, read -ra list <<< "$labels"
  for l in "${list[@]}"; do
    [ -n "$l" ] || continue
    gh label create "$l" --force >/dev/null 2>&1 || true
    args+=(--label "$l")
  done
  gh issue create --title "$title" --body "$body" "${args[@]}"
done
