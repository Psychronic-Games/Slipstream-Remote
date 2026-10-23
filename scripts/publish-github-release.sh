#!/usr/bin/env bash
# Publishes the station built by scripts/build-station.sh as a GitHub release:
# tags the commit it was built from, packages the assets
# (scripts/package-release.py) and creates the release with the CHANGELOG
# notes. Stations with automatic updates install it within a few hours.
#
#   scripts/publish-github-release.sh            # asks before publishing
#   scripts/publish-github-release.sh --yes
#
# Needs the GitHub CLI signed in (gh auth login). Push main first, so the
# tagged commit is on GitHub.
set -euo pipefail
cd "$(dirname "$0")/.."
GH=${GH:-$(command -v gh || echo "$HOME/.local/bin/gh")}
STATION=dist/slipstream-station
[ -f "$STATION/lib/version.json" ] || { echo "error: build the station first (scripts/build-station.sh)" >&2; exit 1; }
read -r VERSION BUILD COMMIT < <(python3 -c 'import json, sys
v = json.load(open(sys.argv[1]))
print(v["version"], v["build"], v["commit"])' "$STATION/lib/version.json")
TAG="v$VERSION"
REPO=$("$GH" repo view --json nameWithOwner -q .nameWithOwner)

# The tag names the commit the apps were built from (their build number is
# that commit's position in the history).
full=$(git rev-parse --verify "$COMMIT^{commit}")
[ "$(git rev-list --count "$full")" = "$BUILD" ] || { echo "error: commit $COMMIT is not build $BUILD" >&2; exit 1; }
git fetch -q origin main
git merge-base --is-ancestor "$full" origin/main || { echo "error: push main first: $COMMIT is not on GitHub" >&2; exit 1; }
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  [ "$(git rev-parse "$TAG^{commit}")" = "$full" ] || { echo "error: tag $TAG already names another commit" >&2; exit 1; }
else
  git tag -a "$TAG" -m "Slipstream Remote $VERSION" "$full"
fi

cargo build --release -p slipstream-release
python3 scripts/package-release.py --station "$STATION"
DIR="dist/github-release/$VERSION"
mapfile -t ASSETS < <(find "$DIR" -maxdepth 1 -type f ! -name notes.md | sort)

echo
echo "Release $TAG (build $BUILD, $COMMIT) to $REPO with:"
printf '  %s\n' "${ASSETS[@]##*/}"
if [ "${1:-}" != "--yes" ]; then
  read -r -p "Publish? [y/N] " answer
  [ "$answer" = y ] || [ "$answer" = Y ] || exit 1
fi
git push -q origin "$TAG"
"$GH" release create "$TAG" --repo "$REPO" --verify-tag --title "Slipstream Remote $VERSION" \
  --notes-file "$DIR/notes.md" "${ASSETS[@]}"
echo "Published: https://github.com/$REPO/releases/tag/$TAG"
