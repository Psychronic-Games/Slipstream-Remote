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

echo "==> Relay program (Linux x86_64)"
cargo build --release -p slipstream-server
cp target/release/slipstream-server "$OUT/bin/slipstream-server-linux-x86_64"

echo "==> Linux app"
cargo build --release -p slipstream
cp target/release/slipstream "$OUT/clients/slipstream-linux-x86_64"

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

chmod 755 "$OUT"/bin/* 2>/dev/null || true
echo "==> Station assembled in $OUT:"
ls -la "$OUT/bin" "$OUT/clients" | grep -v "^total"
