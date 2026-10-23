#!/usr/bin/env bash
# Builds the macOS app for the current commit on GitHub's Macs
# (.github/workflows/macos.yml) and downloads it to dist/slipstream-macos.zip,
# where scripts/build-station.sh picks it up. Run after pushing the release
# commit, before build-station.sh. Takes about 10–15 minutes.
set -euo pipefail
cd "$(dirname "$0")/.."
GH=${GH:-$(command -v gh || echo "$HOME/.local/bin/gh")}
COMMIT=$(git rev-parse HEAD)
git fetch -q origin
git merge-base --is-ancestor "$COMMIT" origin/main || { echo "error: push $COMMIT first" >&2; exit 1; }
"$GH" workflow run macos.yml -f ref="$COMMIT"
sleep 5
RUN=$("$GH" run list --workflow macos.yml --limit 1 --json databaseId -q '.[0].databaseId')
echo "==> Building on GitHub (run $RUN)…"
"$GH" run watch "$RUN" --exit-status --interval 30 >/dev/null
rm -f dist/slipstream-macos.zip
"$GH" run download "$RUN" -n slipstream-macos -D dist
echo "==> dist/slipstream-macos.zip (build $(git rev-list --count HEAD))"
