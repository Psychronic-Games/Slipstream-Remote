#!/usr/bin/env bash
# Assembles a ready-to-upload Slipstream Station in dist/slipstream-station/:
# the PHP site, the relay program (bin/) and the apps (clients/). Upload the
# folder's contents to the root of a (sub)domain, open /admin/ and follow
# the setup.
#
#   scripts/build-station.sh                     # build Linux (+ Windows with cargo-xwin) here
#   scripts/build-station.sh --from-release DIR  # use a CI release's files (signed apps)
#
# For --from-release, download every file of the GitHub release (including
# release.json) into DIR, e.g. `gh release download v0.6.0 -D DIR`.
#
# Updates are signed with a release key kept on this machine (created on the
# first run; see tools/slipstream-release). Set SLIPSTREAM_SIGNING_KEY to
# use a key file elsewhere, and SLIPSTREAM_EXTRA_UPDATE_KEYS (hex, comma
# separated) to make the apps also trust other keys, e.g. while moving to a
# new key.
set -euo pipefail
cd "$(dirname "$0")/.."

# The software renderer built into the Windows app, for machines whose
# graphics can't run the full window (Windows Server 2016…): the Mesa DLLs
# pinned in packaging/windows/softgl.txt, gzipped into $SOFTGL for build.rs.
# The archive is downloaded once and cached.
SOFTGL="${XDG_CACHE_HOME:-$HOME/.cache}/slipstream-mesa/$(awk '$1 == "version" { print $2 }' packaging/windows/softgl.txt)"
softgl() {
  local pins=packaging/windows/softgl.txt url sha archive tmp name fsha
  read -r url sha < <(awk '$1 == "archive" { print $2, $3 }' "$pins")
  archive="$(dirname "$SOFTGL")/$(basename "$url")"
  mkdir -p "$SOFTGL" || return 1
  if [ ! -f "$archive" ]; then
    echo "==> Downloading $(basename "$url")"
    curl -fsSL -o "$archive.part" "$url" && mv "$archive.part" "$archive" || return 1
  fi
  echo "$sha  $archive" | sha256sum -c --quiet || { rm -f "$archive"; return 1; }
  command -v 7z >/dev/null || { echo "7z is needed to unpack Mesa" >&2; return 1; }
  tmp=$(mktemp -d)
  while read -r name fsha; do
    [ -f "$SOFTGL/$name.gz" ] && continue
    7z e -y -o"$tmp" "$archive" "x64/$name" >/dev/null || return 1
    echo "$fsha  $tmp/$name" | sha256sum -c --quiet || return 1
    gzip -9n -c "$tmp/$name" > "$SOFTGL/$name.gz" || return 1
  done < <(awk '$1 == "file" { print $2, $3 }' "$pins")
  rm -rf "$tmp"
}


# Every station build is a release with its own version: entries still
# under "Unreleased" mean scripts/release-version.sh hasn't been run.
# ALLOW_UNRELEASED=1 builds anyway (for local tests, not for stations).
if [ "${ALLOW_UNRELEASED:-}" != 1 ]; then
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  if awk '/^## Unreleased/ { on = 1; next } /^## / { on = 0 } on && NF { found = 1 } END { exit !found }' CHANGELOG.md; then
    echo "error: CHANGELOG.md has unreleased changes. Give them a version first:" >&2
    echo "  scripts/release-version.sh patch|minor   (now $version), then commit" >&2
    exit 1
  fi
  if ! grep -q "^## $version " CHANGELOG.md; then
    echo "error: CHANGELOG.md has no section for version $version" >&2
    exit 1
  fi
  if ! git diff --quiet HEAD -- Cargo.toml Cargo.lock CHANGELOG.md; then
    echo "error: commit the version change first (git commit -am \"Release $version\")" >&2
    exit 1
  fi
fi

FROM=""
if [ "${1:-}" = "--from-release" ]; then
  FROM="${2:?usage: scripts/build-station.sh --from-release DIR}"
  [ -f "$FROM/release.json" ] || { echo "error: $FROM/release.json not found: download every file of the release" >&2; exit 1; }
fi

echo "==> Release signing key"
cargo build --release -p slipstream-release
RELEASE=target/release/slipstream-release
SLIPSTREAM_UPDATE_KEYS="$("$RELEASE" pubkey)${SLIPSTREAM_EXTRA_UPDATE_KEYS:+,$SLIPSTREAM_EXTRA_UPDATE_KEYS}"
export SLIPSTREAM_UPDATE_KEYS
echo "    apps will trust: $SLIPSTREAM_UPDATE_KEYS"

OUT=dist/slipstream-station
rm -rf "$OUT"
mkdir -p dist
cp -r server/station "$OUT"
rm -f "$OUT"/bin/README.txt "$OUT"/clients/README.txt

# The apps' build number as written into Info.plist (macOS zip).
mac_build() {
  python3 -c 'import sys, zipfile, plistlib
z = zipfile.ZipFile(sys.argv[1])
name = next(n for n in z.namelist() if n.endswith(".app/Contents/Info.plist"))
print(plistlib.loads(z.read(name)).get("CFBundleVersion", ""))' "$1" 2>/dev/null || true
}

if [ -n "$FROM" ]; then
  # ── From a CI release ───────────────────────────────────────────────────
  read -r VERSION BUILD COMMIT SIGNED < <(python3 -c 'import json, sys
r = json.load(open(sys.argv[1]))
print(r["version"], r["build"], r["commit"], ",".join(r.get("signed", [])) or "-")' "$FROM/release.json")
  [ "$SIGNED" = "-" ] && SIGNED=""
  echo "==> Release $VERSION build $BUILD ($COMMIT), signed: ${SIGNED:-none}"
  cp "$FROM/slipstream-server-linux-x86_64" "$OUT/bin/"
  for f in slipstream-linux-x86_64 slipstream-windows-x86_64.exe slipstream-macos.zip; do
    [ -f "$FROM/$f" ] && cp "$FROM/$f" "$OUT/clients/"
  done
  # The apps must trust this machine's key, or they'd refuse every update.
  KEY=$("$RELEASE" pubkey)
  for f in "$OUT"/clients/slipstream-linux-x86_64 "$OUT"/clients/slipstream-windows-x86_64.exe; do
    if [ -f "$f" ] && ! grep -qa "$KEY" "$f"; then
      echo "error: $(basename "$f") wasn't built with this machine's release key ($KEY)." >&2
      echo "       Set the repository variable SLIPSTREAM_UPDATE_KEYS to it and release again." >&2
      exit 1
    fi
  done
  if [ -f "$OUT/clients/slipstream-macos.zip" ] && [ "$(mac_build "$OUT/clients/slipstream-macos.zip")" != "$BUILD" ]; then
    echo "error: slipstream-macos.zip isn't build $BUILD" >&2
    exit 1
  fi
else
  # ── Build here ──────────────────────────────────────────────────────────
  # OpenH264 quietly falls back to plain C without nasm, which makes video
  # encoding and decoding 2-3x slower. Never ship that.
  if ! command -v nasm >/dev/null; then
    echo "error: nasm not found. Install it (e.g. pacman -S nasm / apt install nasm), or put it on PATH." >&2
    exit 1
  fi
  # OpenH264 builds cached from before nasm was installed stay plain C until
  # rebuilt: clean them. check_asm DIR verifies a finished build.
  for t in x86_64-unknown-linux-gnu x86_64-pc-windows-msvc ""; do
    dir=target/${t:+$t/}release
    if grep -qs "not using any assembly" "$dir"/build/openh264-sys2-*/output; then
      cargo clean --release -p openh264-sys2 ${t:+--target $t}
    fi
  done
  check_asm() {
    if grep -qs "not using any assembly" "$1"/build/openh264-sys2-*/output; then
      echo "error: OpenH264 in $1 was built without assembly (is nasm working?)" >&2
      exit 1
    fi
  }

  # Linux builds target glibc 2.31 (Debian 11 / Ubuntu 20.04 and newer) via
  # cargo-zigbuild, so they run on far more systems than the build machine.
  if command -v zig >/dev/null && cargo zigbuild --help >/dev/null 2>&1; then
    echo "==> Linux builds (portable, glibc 2.31+)"
    # libspa-sys asks the linker for libpipewire even though we load it at
    # runtime; give zig's linker an empty stand-in (dropped as unused).
    STUB="${XDG_CACHE_HOME:-$HOME/.cache}/slipstream-zig-stubs"
    mkdir -p "$STUB"
    [ -f "$STUB/libpipewire-0.3.so" ] || { echo "" > "$STUB/empty.c"; zig cc -target x86_64-linux-gnu.2.31 -shared -o "$STUB/libpipewire-0.3.so" "$STUB/empty.c"; }
    RUSTFLAGS="-L native=$STUB" cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p slipstream -p slipstream-server
    LINUX=target/x86_64-unknown-linux-gnu/release
  else
    echo "==> Linux builds (native — install zig + cargo-zigbuild for portable builds)"
    cargo build --release -p slipstream -p slipstream-server
    LINUX=target/release
  fi
  check_asm "$LINUX"
  cp "$LINUX/slipstream-server" "$OUT/bin/slipstream-server-linux-x86_64"
  cp "$LINUX/slipstream" "$OUT/clients/slipstream-linux-x86_64"

  # Every build listed in the manifest must come from this run (same build
  # number), or apps would loop updating to a stale binary.
  if cargo xwin --help >/dev/null 2>&1; then
    echo "==> Windows app (cross-compiled)"
    if softgl; then
      echo "    with the software renderer (Mesa $(basename "$SOFTGL"))"
      export SLIPSTREAM_SOFTGL_DIR="$SOFTGL"
    else
      echo "warning: building without the software renderer; machines like Windows Server 2016 get the compact window" >&2
    fi
    cargo xwin build --release --target x86_64-pc-windows-msvc -p slipstream
    check_asm target/x86_64-pc-windows-msvc/release
    cp target/x86_64-pc-windows-msvc/release/slipstream.exe "$OUT/clients/slipstream-windows-x86_64.exe"
  else
    echo "==> Skipping Windows app (install cargo-xwin to build it)"
  fi

  VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 0)
  COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo dev)
  SIGNED=""
  # The macOS app is built on a Mac (scripts/build-macos.sh writes
  # dist/slipstream-macos.zip). Only take it if it's from this same build.
  if [ -f dist/slipstream-macos.zip ]; then
    MAC_BUILD=$(mac_build dist/slipstream-macos.zip)
    if [ "$MAC_BUILD" = "$BUILD" ]; then
      cp dist/slipstream-macos.zip "$OUT/clients/"
    else
      echo "==> Skipping dist/slipstream-macos.zip: it's build ${MAC_BUILD:-?}, not $BUILD. Rebuild it on the Mac." >&2
    fi
  fi
fi

# Update manifest: apps compare (version, build) with their own, check the
# release signature, and verify the download against the signed checksum.
# Code-signed apps are marked, so the station serves them untouched.
"$RELEASE" manifest "$OUT/clients" --version "$VERSION" --build "$BUILD" --commit "$COMMIT" ${SIGNED:+--signed "$SIGNED"}
echo "==> Signed manifest: version $VERSION build $BUILD ($COMMIT)"

chmod 755 "$OUT"/bin/* "$OUT"/clients/slipstream-linux-x86_64 2>/dev/null || true
echo "==> Station assembled in $OUT:"
ls -la "$OUT/bin" "$OUT/clients" | grep -v "^total"
