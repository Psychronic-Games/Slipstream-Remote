#!/usr/bin/env bash
# Starts a release: raises the version in the workspace Cargo.toml and files
# the changelog's "Unreleased" entries under it. Commit afterwards, then
# build (scripts/build-station.sh refuses while "Unreleased" has entries).
#
#   scripts/release-version.sh patch    # 0.6.0 → 0.6.1: fixes only
#   scripts/release-version.sh minor    # 0.6.1 → 0.7.0: new features
#   scripts/release-version.sh major    # → 1.0.0, then breaking changes
#   scripts/release-version.sh 0.8.0    # an exact version
#
# See "Versions" in docs/BUILDING.md for what each part means.
set -euo pipefail
cd "$(dirname "$0")/.."

cur=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IFS=. read -r major minor patch <<<"${cur%%[-+]*}"
case "${1:-}" in
  patch) new="$major.$minor.$((patch + 1))" ;;
  minor) new="$major.$((minor + 1)).0" ;;
  major) new="$((major + 1)).0.0" ;;
  [0-9]*.[0-9]*.[0-9]*) new=$1 ;;
  *)
    echo "usage: scripts/release-version.sh patch|minor|major|X.Y.Z   (now $cur)" >&2
    exit 2
    ;;
esac

python3 - "$new" "$(date +%Y-%m-%d)" <<'EOF'
import re, sys
new, day = sys.argv[1], sys.argv[2]
text = open("CHANGELOG.md").read()
m = re.search(r"^## Unreleased\n(.*?)(?=^## )", text, re.S | re.M)
if not m or not m.group(1).strip():
    sys.exit('CHANGELOG.md: nothing under "## Unreleased" to release')
if re.search(rf"^## {re.escape(new)}\b", text, re.M):
    sys.exit(f"CHANGELOG.md already has a {new} section")
body = m.group(1).strip("\n")
text = text[: m.start()] + f"## Unreleased\n\n## {new} — {day}\n\n{body}\n\n" + text[m.end():]
open("CHANGELOG.md", "w").write(text)
EOF

sed -i "0,/^version = \"$cur\"/s//version = \"$new\"/" Cargo.toml
cargo update --workspace --offline -q

echo "Version $cur → $new. Check CHANGELOG.md, then:"
echo "  git commit -am \"Release $new\" && scripts/build-station.sh"
