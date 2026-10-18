#!/usr/bin/env bash
# Builds the Android app: the Rust library (apps/slipstream-android) for
# each ABI with cargo-ndk, then the APK with Gradle, into dist/android/.
#
#   scripts/build-android.sh                 # phones and tablets (arm64) and the emulator (x86_64)
#   ABIS=x86_64 scripts/build-android.sh     # just the emulator, quicker
#   scripts/build-android.sh --install       # then install it on the device or emulator adb sees
#   scripts/build-android.sh --debuggable    # an APK adb can look inside (run-as): settings, logs
#
# Needs the Android SDK with the NDK named in app/build.gradle.kts
# (ANDROID_HOME, default ~/Android/Sdk), JDK 17+, cargo-ndk and the Rust
# targets: see docs/BUILDING.md, "Android".
set -euo pipefail
cd "$(dirname "$0")/.."

install=
variant=release
for arg in "$@"; do
  case "$arg" in
    --install) install=1 ;;
    --debuggable) variant=debug ;;
    *) echo "unknown option $arg" >&2; exit 2 ;;
  esac
done

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
ndk=$(sed -n 's/^ *ndkVersion = "\(.*\)"/\1/p' apps/slipstream-android/app/build.gradle.kts)
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$ndk"
[ -d "$ANDROID_NDK_HOME" ] || { echo "error: NDK $ndk isn't installed: sdkmanager \"ndk;$ndk\"" >&2; exit 1; }
export PATH="$HOME/.cargo/bin:$HOME/.local/share/nasm:$PATH"
command -v cargo-ndk >/dev/null || { echo "error: cargo-ndk is missing: cargo install cargo-ndk" >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
build=$(git rev-list --count HEAD)
abis=${ABIS:-arm64-v8a x86_64}

app=apps/slipstream-android
jni="$app/app/src/main/jniLibs"
# Build output stays out of Dropbox, as target/ does.
mkdir -p "$jni" "$app/.gradle" "$app/build" "$app/app/build" dist/android
for d in "$jni" "$app/.gradle" "$app/build" "$app/app/build" dist; do
  setfattr -n user.com.dropbox.ignored -v 1 "$d" 2>/dev/null || true
done
rm -rf "${jni:?}"/*

echo "==> Rust library ($abis), Slipstream $version (build $build)"
targets=()
for abi in $abis; do targets+=(-t "$abi"); done
# Android 8 (API 26) is the oldest the app supports (minSdk).
# OpenH264 is C++, hence libc++_shared.so beside the app's library.
cargo ndk "${targets[@]}" -P 26 -o "$jni" --link-libcxx-shared build --release -p slipstream-android
# cargo-ndk copies every library in the target folder, iroh's FFI ones too.
find "$jni" -name '*.so' ! -name libslipstream_android.so ! -name libc++_shared.so -delete

echo "==> APK"
# The Rust library is a release build either way.
task=assemble${variant^}
(cd "$app" && ./gradlew --quiet "$task" -PslipstreamVersion="$version" -PslipstreamBuild="$build")
apk="dist/android/slipstream-remote-$version.apk"
[ "$variant" = release ] || apk="dist/android/slipstream-remote-$version-$variant.apk"
cp "$app/app/build/outputs/apk/$variant/app-$variant.apk" "$apk"
echo "    $apk ($(du -h "$apk" | cut -f1))"

if [ -n "$install" ]; then
  echo "==> Installing"
  "$ANDROID_HOME/platform-tools/adb" install -r "$apk"
fi
