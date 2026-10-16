#!/usr/bin/env bash
# Assembles a ready-to-upload Slipstream Station in dist/slipstream-station/:
# the PHP site, the relay program (bin/) and whatever client builds exist
# (clients/). Upload the folder's contents to the root of a (sub)domain,
# open /admin/ and follow the setup.
#
#   scripts/build-station.sh            # build what this machine can build
#   WINDOWS=1 scripts/build-station.sh  # also cross-compile the Windows app (needs cargo-xwin)
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=dist/slipstream-station
rm -rf "$OUT"
mkdir -p dist
cp -r server/station "$OUT"
rm -f "$OUT"/bin/README.txt "$OUT"/clients/README.txt

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
cp "$LINUX/slipstream-server" "$OUT/bin/slipstream-server-linux-x86_64"
cp "$LINUX/slipstream" "$OUT/clients/slipstream-linux-x86_64"

if [ "${WINDOWS:-0}" = 1 ]; then
  echo "==> Windows app (cross-compiled)"
  cargo xwin build --release --target x86_64-pc-windows-msvc -p slipstream
fi
if [ -f target/x86_64-pc-windows-msvc/release/slipstream.exe ]; then
  cp target/x86_64-pc-windows-msvc/release/slipstream.exe "$OUT/clients/slipstream-windows-x86_64.exe"
fi
# macOS builds come from CI or a Mac: drop slipstream-macos.zip (containing
# Slipstream.app) into clients/.
[ -f dist/slipstream-macos.zip ] && cp dist/slipstream-macos.zip "$OUT/clients/"

# Update manifest: apps compare (version, build) with their own and verify
# the download against these checksums.
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 0)
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo dev)
{
  printf '{\n  "version": "%s",\n  "build": %s,\n  "commit": "%s",\n  "built": %s,\n  "platforms": {' "$VERSION" "$BUILD" "$COMMIT" "$(date +%s)"
  sep=""
  for spec in "linux-x86_64:linux:slipstream-linux-x86_64" "windows-x86_64:windows:slipstream-windows-x86_64.exe" "macos-aarch64:macos:slipstream-macos.zip"; do
    IFS=: read -r platform os file <<< "$spec"
    f="$OUT/clients/$file"
    [ -f "$f" ] || continue
    printf '%s\n    "%s": {"os": "%s", "file": "%s", "size": %s, "sha256": "%s"}' "$sep" "$platform" "$os" "$file" "$(stat -c %s "$f")" "$(sha256sum "$f" | cut -d' ' -f1)"
    sep=","
  done
  printf '\n  }\n}\n'
} > "$OUT/clients/manifest.json"
echo "==> Manifest: version $VERSION build $BUILD ($COMMIT)"

chmod 755 "$OUT"/bin/* 2>/dev/null || true
echo "==> Station assembled in $OUT:"
ls -la "$OUT/bin" "$OUT/clients" | grep -v "^total"
