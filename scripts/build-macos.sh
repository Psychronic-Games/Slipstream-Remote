#!/usr/bin/env bash
# Builds Slipstream.app on a Mac (one universal app for Apple Silicon and
# Intel) and zips it into dist/slipstream-macos.zip, where
# scripts/build-station.sh on the Linux build machine picks it up.
#
#   SLIPSTREAM_UPDATE_KEYS=<hex public key> scripts/build-macos.sh
#
# Needs the Xcode command line tools (xcode-select --install), Rust (rustup)
# and nasm (brew install nasm). SLIPSTREAM_UPDATE_KEYS is the release public
# key: `target/release/slipstream-release pubkey` prints it on the build
# machine. Without it, this build can't install updates.
#
# Build from the same commit as the Linux and Windows apps, so all of them
# carry the same build number.
set -euo pipefail
cd "$(dirname "$0")/.."

[ "$(uname)" = Darwin ] || { echo "error: run this on a Mac" >&2; exit 1; }
command -v nasm >/dev/null || { echo "error: nasm not found (brew install nasm); video would be 2-3x slower" >&2; exit 1; }
if [ -z "${SLIPSTREAM_UPDATE_KEYS:-}" ]; then
  echo "warning: SLIPSTREAM_UPDATE_KEYS is not set, so this build won't install updates" >&2
fi
# Keep build files out of the (synced) project folder.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/Library/Caches/slipstream-target}"
export MACOSX_DEPLOYMENT_TARGET=12.0

echo "==> Building for Apple Silicon and Intel"
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --target "$t" -p slipstream
  # openh264-sys2 has assembly for x86 only; Apple Silicon builds plain C.
  if [ "$t" = x86_64-apple-darwin ] && grep -qs "not using any assembly" "$CARGO_TARGET_DIR/$t/release/build/"openh264-sys2-*/output; then
    echo "error: OpenH264 for $t was built without assembly — cargo clean --release --target $t -p openh264-sys2" >&2
    exit 1
  fi
done

OUT=dist/macos
APP="$OUT/Slipstream.app"
rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/slipstream" \
  "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/slipstream" \
  "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/slipstream"

echo "==> Icon"
ICONSET="$OUT/slipstream.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  "$APP/Contents/MacOS/slipstream" export-icon "$ICONSET/icon_${s}x${s}.png" --size "$s"
  "$APP/Contents/MacOS/slipstream" export-icon "$ICONSET/icon_${s}x${s}@2x.png" --size $((s * 2))
done
iconutil -c icns -o "$APP/Contents/Resources/slipstream.icns" "$ICONSET"
rm -rf "$ICONSET"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 0)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Slipstream</string>
  <key>CFBundleDisplayName</key><string>Slipstream Remote</string>
  <key>CFBundleIdentifier</key><string>dev.slipstream.remote</string>
  <key>CFBundleExecutable</key><string>slipstream</string>
  <key>CFBundleIconFile</key><string>slipstream</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUsageDescription</key>
  <string>Slipstream uses your camera when you choose to share it in a meeting.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>Slipstream uses your microphone when you unmute in a meeting.</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>Slipstream shares your screen with the technician you gave your ID and password to.</string>
  <key>NSAppleEventsUsageDescription</key>
  <string>Slipstream sends keyboard and mouse input from the technician you let in.</string>
</dict>
</plist>
PLIST

echo "==> Signing (ad hoc; Developer ID and notarisation come later)"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

ditto -c -k --keepParent "$APP" dist/slipstream-macos.zip
echo "==> dist/slipstream-macos.zip (version $VERSION build $BUILD)"
echo "    Next, on the Linux build machine: scripts/build-station.sh"
