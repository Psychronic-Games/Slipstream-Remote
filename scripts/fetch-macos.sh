#!/usr/bin/env bash
# Builds the macOS app for the current commit on GitHub's Macs
# (.github/workflows/macos.yml) and downloads it to dist/slipstream-macos.zip,
# where scripts/build-station.sh picks it up. Takes about 10–15 minutes.
#
#   scripts/fetch-macos.sh --start   start the build and return; build-station.sh
#                                    then downloads it once its own builds are done
#   scripts/fetch-macos.sh           wait for that build (starting one if needed)
#                                    and download it
#
# Run after pushing the release commit.
set -euo pipefail
cd "$(dirname "$0")/.."
GH=${GH:-$(command -v gh || echo "$HOME/.local/bin/gh")}
COMMIT=$(git rev-parse HEAD)
# "RUN COMMIT" of a build started for this commit.
STARTED=dist/macos-run

start() {
  git fetch -q origin
  git merge-base --is-ancestor "$COMMIT" origin/main || { echo "error: push $COMMIT first" >&2; exit 1; }
  "$GH" workflow run macos.yml -f ref="$COMMIT"
  sleep 5
  RUN=$("$GH" run list --workflow macos.yml --limit 1 --json databaseId -q '.[0].databaseId')
  mkdir -p dist
  echo "$RUN $COMMIT" >"$STARTED"
  echo "==> Building the macOS app on GitHub (run $RUN)"
}

if [ -f "$STARTED" ] && [ "$(cut -d' ' -f2 "$STARTED")" = "$COMMIT" ]; then
  RUN=$(cut -d' ' -f1 "$STARTED")
else
  start
fi
[ "${1:-}" = --start ] && exit 0

echo "==> Waiting for the macOS app (GitHub run $RUN)…"
"$GH" run watch "$RUN" --exit-status --interval 30 >/dev/null || {
  echo "error: the macOS build failed: $GH run view $RUN --log-failed" >&2
  rm -f "$STARTED"
  exit 1
}
rm -f dist/slipstream-macos.zip
"$GH" run download "$RUN" -n slipstream-macos -D dist
rm -f "$STARTED"
echo "==> dist/slipstream-macos.zip (build $(git rev-list --count HEAD))"
