# Building and releasing Slipstream Remote

For running a station and using the apps, see
[GETTING-STARTED.md](GETTING-STARTED.md). This page is for building from
source and publishing new versions.

## Development build

You need Rust (stable, edition 2024) and, on Linux, the headers used at
build time:

```sh
# Arch / CachyOS
sudo pacman -S pipewire libxkbcommon clang nasm
# Debian / Ubuntu
sudo apt install libpipewire-0.3-dev libspa-0.2-dev clang nasm libxkbcommon-dev libxdo-dev libxtst-dev libdbus-1-dev
```

The finished app does **not** need these libraries at runtime. PipeWire is
loaded on demand (`dlopen`) for Wayland screen sharing, and X11 is spoken
in pure Rust.

```sh
scripts/check.sh cargo build --workspace --locked
scripts/check.sh                # unit + end-to-end session tests
scripts/check.sh cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/check.sh cargo run -p slipstream         # the app
scripts/check.sh scripts/try-local.sh # one-machine trial: local server + host + viewer
```

`SLIPSTREAM_HOME=/some/dir` gives an instance its own settings, identity and
single-instance lock. Use it to run a second copy next to your normal one.
Such side instances leave your desktop launcher alone.

The station account API has an isolated integration check (Python 3 and PHP
CLI required). It copies the PHP site into a temporary directory and uses
fixture accounts, without touching a running station or real settings:

```sh
scripts/check.sh python3 scripts/test-station-accounts.py --client target/debug/slipstream
# Set SLIPSTREAM_TEST_PHP=/path/to/php if PHP is not on PATH.
```

Some tests touch real hardware or the network and are `#[ignore]`d by
default (for example the real clipboard round-trip). Run them with
`scripts/check.sh cargo test -- --ignored`.

Linux desktop file drops need X11/Xwayland: the app selects X11 for its
windows when `DISPLAY` is available, including inside a Wayland session.
Capture/input still selects the native Wayland portal from `WAYLAND_DISPLAY`.
Native Wayland file drops are unavailable in [winit 0.30's feature matrix](https://docs.rs/crate/winit/0.30.13/source/FEATURES.md).

### Native meeting and tab checks

The native audit uses temporary profiles, a PHP fixture station, synthetic
camera frames and Xvfb. It re-executes under `dbus-run-session` to isolate tray
registrations and desktop portals. Xvfb alone still exposes the real desktop's
D-Bus services; preserve the private-bus setup when adding fixtures. It requires
Pillow, tesseract, Xvfb, xdotool, dbus-run-session and PHP CLI with sodium.
Run the modes sequentially through the resource wrapper:

```sh
scripts/check.sh python3 scripts/test-meetings-ui.py --client target/debug/slipstream
scripts/check.sh python3 scripts/test-meetings-ui.py --client target/debug/slipstream --tabs
```

The script prints a temporary evidence directory containing logs/screenshots.
See the [latest handoff](DEVELOPMENT-2026-10-21.md) for completed release checks.

### Disk space

Cargo keeps compiled dependencies and previous build outputs in `target/`.
Development builds use line-number debug information and disable incremental
compilation by default: the old incremental snapshots alone grew to 85 GiB.
Dependencies still omit debug information.

Station and standalone Android builds run cache housekeeping before and after
packaging. For development, tests, Clippy and native checks, use
`scripts/check.sh COMMAND [ARG ...]` (no arguments runs workspace tests).
It uses the workstation's `codex-check` resource/serialization wrapper when
available, then enforces the same budgets before and after the command.
Direct ad-hoc Cargo commands bypass housekeeping.

- All development/test caches combined: **12 GiB**. Crossing the budget
  clears those caches on the next housekeeping pass.
- All release caches combined: **32 GiB**. Crossing that budget clears the
  release caches after packaging, or before the next build. Subsequent builds
  recompile dependencies; ready-to-download apps remain in `dist/`.
- `dist/android/`: retain the **two newest versions per release/debug variant**.
  Assembled station clients, unknown files and deployed rollback backups remain.

These are cleanup thresholds, not filesystem quotas: one running command may
exceed them until it finishes. Override them with positive whole-GiB values in
`SLIPSTREAM_DEV_CACHE_GIB` and `SLIPSTREAM_RELEASE_CACHE_GIB` if necessary.
Housekeeping runs between builds under the same check lock. During station
assembly, the nested Android build defers housekeeping to the outer build so
it cannot remove the still-needed release-signing tool.

`scripts/clean.sh --status` reports sizes without changes.
`scripts/clean.sh` explicitly clears development/test output;
`scripts/clean.sh --all` clears all local Cargo caches. Only the workspace's
own `target/` is cleaned; symlinked target roots are refused. Source, settings,
signing keys and deployed backups are preserved. Both cleanup modes also
prune old generated APKs. Everything removed can be rebuilt.

## Release builds (the station package)

`scripts/build-station.sh` builds everything a station serves and assembles
`dist/slipstream-station/`:

| Output | Built with | Notes |
|---|---|---|
| `clients/slipstream-linux-x86_64` | `cargo zigbuild` for glibc 2.31 | Runs on Debian 11 / Ubuntu 20.04 and newer. Falls back to a native build (newer glibc only) without zig. |
| `bin/slipstream-server-linux-x86_64` | same | The relay the station runs. |
| `clients/slipstream-windows-x86_64.exe` | `cargo xwin` (MSVC target, cross-compiled from Linux) | Skipped when cargo-xwin isn't installed. Carries Mesa's CPU OpenGL driver for machines whose graphics can't run the full window: [mesa-dist-win](https://github.com/pal1000/mesa-dist-win), pinned in `packaging/windows/softgl.txt`, downloaded once into `~/.cache/slipstream-mesa` (needs `7z`) and handed to `build.rs` as `SLIPSTREAM_SOFTGL_DIR`. Without it the app builds without the renderer, with a warning. |
| `clients/slipstream-macos.zip` | `scripts/build-macos.sh` on a Mac | Universal (Apple Silicon + Intel), ad-hoc signed. Taken from `dist/slipstream-macos.zip` if it's from the same build. |
| `clients/slipstream-android.apk` | `scripts/build-android.sh` (arm64 only: phones and tablets) | Skipped without the Android SDK and cargo-ndk (see *Android* below). Signed with the Android release key; not in the update manifest, since the app can't update itself yet. |
| `clients/manifest.json` | `tools/slipstream-release` | Version, build number, commit, and size, SHA-256 and release signature of each app, for self-update. |

One-time tool setup:

```sh
# nasm: OpenH264's SIMD code. Without it video encoding is 2-3x slower,
# so build-station.sh refuses to run.
sudo pacman -S nasm        # or: sudo apt install nasm / brew install nasm
# 7z: unpacks the Mesa archive for the software renderer.
sudo pacman -S 7zip        # or: sudo apt install 7zip
# Zig (any recent version) somewhere on PATH, e.g. ~/.local/share/zig
cargo install cargo-zigbuild cargo-xwin
rustup target add x86_64-pc-windows-msvc
```

The script makes an empty stand-in `libpipewire-0.3.so` in
`~/.cache/slipstream-zig-stubs`, because libspa-sys asks the linker for it.
The path must not contain spaces.

### The release signing key

Apps only install updates signed with the release key they were built
with. The manifest's checksums alone would not protect them: whoever got
into the web host could replace an app and its checksum together.

- On the first run, `build-station.sh` creates the key at
  `~/.config/slipstream-release/update-signing.key`, outside the project.
  It never goes to the station. Set `SLIPSTREAM_SIGNING_KEY` to keep it
  somewhere else.
- **Back the key up.** Apps built with it accept updates only from it.
  Without it, every installed app has to be downloaded again by hand.
- The script builds the key's public half into every app
  (`SLIPSTREAM_UPDATE_KEYS`), then signs each app's platform, version,
  build, size and SHA-256 into `manifest.json`.
- To move to a new key, build one release with the old key and
  `SLIPSTREAM_EXTRA_UPDATE_KEYS=<new public key>`. Once the apps have
  updated to it, switch to the new key.
- `slipstream info` shows which keys an app trusts. A plain `cargo build`
  has none, so it never installs updates.

### Versions

Slipstream uses [semantic versioning](https://semver.org):

- **Before 1.0** (`0.MINOR.PATCH`): a **minor** release (0.6 → 0.7) adds
  features; a **patch** release (0.6.0 → 0.6.1) only fixes things.
- **1.0.0** is the feature-complete release, with no known bugs in what it
  offers.
- **After 1.0**: **major** for changes that don't work with older apps or
  stations (protocol, station API), **minor** for features, **patch** for
  fixes.

Every station build is a release with its own version.
`scripts/release-version.sh patch|minor|major|X.Y.Z` raises `version` in
the workspace `Cargo.toml` (the one place it's written; the Windows .exe
gets it through `apps/slipstream/build.rs`) and turns the changelog's
"Unreleased" entries into that version's section.
`scripts/build-station.sh` refuses to run while "Unreleased" has entries
or the version change isn't committed. Use `ALLOW_UNRELEASED=1` for local
test builds.

### Build numbers

The **build number** is the git commit count (`git rev-list --count HEAD`),
baked into every binary along with the commit hash (and into the .exe's
version as the fourth part). Apps update when the station's
`(version, build)` is higher than their own.

So **commit before you build a release**, or the new apps carry the same
build number as the old ones and nobody gets offered the update.

**Build every platform in the same run.** If `manifest.json` listed a
Windows app from an older build, Windows clients would update, find they
are still "old", and update again in a loop. The script therefore always
rebuilds every platform it lists.

### Release checklist

1. Commit your changes, with their entries under "Unreleased" in
   `CHANGELOG.md`.
2. `scripts/release-version.sh patch` (fixes only) or `minor` (new
   features), check the changelog, and commit
   (`git commit -am "Release X.Y.Z"`). This also raises the build number.
3. `scripts/build-station.sh`
4. Upload to the station's web root:
   - `clients/` apps first, `clients/manifest.json` **last**,
   - `bin/` and the PHP files if they changed.

   On a shared host, copy each file to a temporary name and rename it into
   place, so nobody downloads a half-written file.
5. If `bin/` changed, press **Restart relay** on the admin page.
6. Check the update is offered and carries a `sig`:

   ```sh
   curl -s -X POST https://support.example.com/api/ -H 'Content-Type: application/json' \
     -d '{"op":"update_check","platform":"windows-x86_64","key":"<access key>"}'
   ```

Running apps pick the update up within 6 hours, or about 20 seconds after
they next start.

### Publishing clients through a mounted station

`scripts/publish-station-clients.py` preserves the release procedure in the
repository. It requires Python 3.11+ and the `cryptography` package. Use the
previously trusted public release key, rather than trusting a key from a new
manifest. These commands use an already built release; they do not rebuild
clients or update PHP/the relay:

```sh
python3 scripts/publish-station-clients.py --trusted-key PUBLIC_KEY verify
python3 scripts/publish-station-clients.py --trusted-key PUBLIC_KEY publish \
  --root /path/to/station/public_html --expect VERSION/BUILD/COMMIT
python3 scripts/publish-station-clients.py --trusted-key PUBLIC_KEY check-live
```

Use the workstation's `codex-check` wrapper for these commands where local
resource rules require it. `verify` checks desktop sizes, SHA-256 hashes and
Ed25519 signatures, and records the APK checksum. Verify the Android signing
certificate separately with the SDK's `apksigner verify --print-certs` before
publishing an APK; it is outside the desktop update manifest.

`publish` verifies the previous release and requires its exact
`version/build/commit`. It refuses an unavailable station root, an older
candidate, or a candidate missing deployed platforms. It creates a rollback
backup under the station account's existing private `tmp/` directory, stages
and checks every replacement, preserves ownership/modes, replaces clients
atomically, then publishes the manifest last. Ordinary replacement failures
restore the prior files. If the mount disconnects during publication, the
printed backup remains available for manual recovery. Restore its saved
clients using temporary filenames and atomic renames, preserving deployed
ownership/modes; restore `manifest.json` last. `rollback.json` records saved
hashes and any new files to remove.

`check-live` reads the station URL/access key from the local
`~/.config/slipstream/settings.toml` without printing credentials. Use
`--settings PATH` for another profile. It checks authenticated HTTPS update
offers and complete downloads, including desktop provisioning trailers and
unchanged APK bytes, MIME type and filename. Redirects are refused so the
access key is not forwarded elsewhere.

The isolated publication regression check uses generated keys and temporary
files, without touching a real station:

```sh
scripts/check.sh python3 scripts/test-publish-station-clients.py
```

### The macOS app

On a Mac with the Xcode command line tools, Rust and `brew install nasm`:

```sh
SLIPSTREAM_UPDATE_KEYS=PUBLIC_KEY scripts/check.sh scripts/build-macos.sh
```

`target/release/slipstream-release pubkey` on the build machine prints the
key. The script writes `dist/slipstream-macos.zip`. If the project folder
syncs between the machines (Dropbox), `build-station.sh` finds it there;
otherwise copy it over. Build both from the same commit, or the script
leaves the Mac app out (a mismatched build number would make Macs update in
a loop).

### Releases from CI (signed apps)

The CI/release workflows are prepared but have not run; the repository has
no GitHub remote configured. Current published clients are built locally.

Once the repository is published, `.github/workflows/release.yml` is configured
to run on a `v*` version tag. It builds Linux, Windows and macOS and publishes
the artifacts plus `release.json` as a draft GitHub release. Windows signing
is conditional on the SignPath configuration; without it the executable stays
unsigned. The workflow does not assemble a station zip or build Android.
On the machine with the release key, download all artifacts from the chosen
tag into a directory, then assemble the station:

```sh
gh release download vX.Y.Z -D /tmp/rel
scripts/build-station.sh --from-release /tmp/rel
```

Assembly signs a new update manifest locally; CI does not receive the private
update key or produce that manifest. Apps marked as code-signed in
`release.json`, and Mac bundles, are served untouched by the station.
Before the first CI release, set the repository
variable `SLIPSTREAM_UPDATE_KEYS` to your release public key; the script
refuses a release built with a different one. The SignPath variables are
described at the top of the workflow, and the signing policy is in
[CODE-SIGNING.md](CODE-SIGNING.md).

### Protocol changes

`PROTOCOL_VERSION` in `crates/slipstream-core/src/proto.rs` must go up
whenever a session message changes shape. The current version is **6**.
Messages are `postcard`-encoded,
which has no optional fields: an added field breaks older peers. Hosts and
viewers with different versions refuse each other with a "please update"
message, so a protocol change means updating both peers; desktop apps offer
the station's update, while Android requires installing its new APK.

An appended `StreamOpen`/`StreamReply` variant is an exception: an older peer
rejects only that optional request stream. Existing variant layouts and
discriminants must remain unchanged. See [ARCHITECTURE.md](ARCHITECTURE.md#session-protocol-alpn-slipstreamremote1-protocol_version-6).

## Testing the Windows build on Linux

The Windows app runs under Wine well enough to test file transfer, chat and
the host side with the screenshot capture fallback:

```sh
export WINEPREFIX=/tmp/slip-wine WINEDEBUG=-all
scripts/check.sh wine target/x86_64-pc-windows-msvc/release/slipstream.exe host --password test1234
```

Windows-only unit tests (for example the pointer drawing) run under Wine too:

```sh
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine \
  scripts/check.sh cargo xwin test --target x86_64-pc-windows-msvc -p slipstream-platform
```

Real DXGI capture, SmartScreen and input on the secure desktop need a real
Windows machine.

## Android

The Android app is the same Rust GUI as the desktop app, built as a
library (`apps/slipstream-android`) that a small Java activity loads. The
activity is a GameActivity, which has proper soft-keyboard support. The
Gradle project that packages it into an APK lives in the same folder.

One-time setup (the SDK lives outside Dropbox, in `~/Android/Sdk`):

```sh
# Android command-line tools from developer.android.com/studio#command-tools,
# unpacked to ~/Android/Sdk/cmdline-tools/latest, then:
export ANDROID_HOME=~/Android/Sdk
sdkmanager "ndk;29.0.14206865" "platforms;android-36" "platform-tools"
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
# JDK 17 or later; Gradle downloads itself (apps/slipstream-android/gradlew).
```

```sh
scripts/build-android.sh                   # dist/android/slipstream-remote-VERSION.apk (arm64 + x86_64)
ABIS=x86_64 scripts/build-android.sh       # emulator only, quicker
scripts/build-android.sh --install         # and install it with adb
scripts/build-android.sh --debuggable      # an APK adb can look inside
```

The NDK version is pinned in `app/build.gradle.kts`; the script reads it
from there. The APK's version is the workspace version and its version
code is the build number, as on the desktop.

**Testing on the emulator.** Create one with an x86_64 image (`sdkmanager
"emulator" "system-images;android-36;google_apis;x86_64"`, then `avdmanager
create avd -n slipstream-test -k "system-images;android-36;google_apis;x86_64"
-d pixel_7`) and run it headless:

```sh
scripts/check.sh emulator -avd slipstream-test -no-window -no-audio -gpu swiftshader_indirect
```

Keep the emulator in the foreground in its own terminal/tool session. In a
second terminal, use lightweight inspection commands:

```sh
adb exec-out screencap -p > shot.png       # see the screen
adb logcat -s Slipstream                   # the app's log
```

`-gpu host` is much faster but crashed after a few app restarts without a
window; `swiftshader_indirect` is slow but stable. The emulator reaches
this computer as `10.0.2.2`: run a local `slipstream-server --listen
127.0.0.1:21217` and a `slipstream host --test-pattern`, then point the app
at `http://10.0.2.2:21217`. With a `--debuggable` APK that can be written
straight into its settings:

```sh
printf '[server]\nurl = "http://10.0.2.2:21217"\n' > /tmp/s.toml
adb shell "run-as com.slipstreamremote.app sh -c 'cat > files/settings.toml'" < /tmp/s.toml
```

Escape `&` when opening a link through `adb shell am start -d '…'`: the
device's shell splits the command there otherwise.

**Signing.** Published APKs use the existing Android release key.
`scripts/build-android.sh` creates it on first use at
`~/.config/slipstream-release/android-release.p12` (or
`$XDG_CONFIG_HOME/slipstream-release/android-release.p12`), with its password
in the adjacent `.pass` file. `SLIPSTREAM_ANDROID_KEYSTORE` selects another
path. Back up both files: Android accepts an update only when its signing
certificate matches the installed app. `--debuggable` uses the debug key;
a direct Gradle build without release-key configuration also falls back to
the debug key. Verify a release with `apksigner verify --print-certs` and
compare its certificate with the [published handoff](DEVELOPMENT-2026-10-21.md).
Android updates are installed manually from the station and are outside the
desktop update manifest.

## Repository layout

```
apps/slipstream/           desktop app (egui) + CLI, also a library for Android
apps/slipstream-android/   Android: the app's library for the activity, and the Gradle project
apps/slipstream-server/    standalone directory + relay server
crates/slipstream-core/    protocol, auth, directory client, config, clock
crates/slipstream-codec/   H.264 encode/decode
crates/slipstream-audio/   sound: vendored libopus, capture, playback
crates/slipstream-platform/ screen capture, input, clipboard per OS
crates/slipstream-session/ host/viewer sessions, files, updates
tools/slipstream-release/  release key and signed update manifest (build-station.sh uses it)
server/station/            the PHP station (download page, API, admin)
packaging/                 desktop entry, icons, Windows resources, server configs
scripts/                   build-station.sh, build-android.sh, try-local.sh, install-linux.sh, file-issues.sh
docs/                      these documents; docs/issues/ holds the open issues
```
