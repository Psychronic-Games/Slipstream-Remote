# Building and releasing Slipstream Remote

For running a station and using the apps, see
[GETTING-STARTED.md](GETTING-STARTED.md). This page is for building from
source and publishing new versions.

## Development build

You need Rust (stable, edition 2024) and, on Linux, the headers used at
build time:

```sh
# Arch / CachyOS
sudo pacman -S pipewire libxkbcommon clang
# Debian / Ubuntu
sudo apt install libpipewire-0.3-dev libspa-0.2-dev clang libxkbcommon-dev libxdo-dev libxtst-dev libdbus-1-dev
```

The finished app does **not** need these libraries at runtime. PipeWire is
loaded on demand (`dlopen`) for Wayland screen sharing, and X11 is spoken
in pure Rust.

```sh
cargo build                     # debug build of everything
cargo test --workspace          # unit + end-to-end session tests
cargo clippy --workspace
cargo run -p slipstream         # the app
scripts/try-local.sh            # one-machine trial: local server + host + viewer
```

`SLIPSTREAM_HOME=/some/dir` gives an instance its own settings, identity and
single-instance lock. Use it to run a second copy next to your normal one.
Such side instances leave your desktop launcher alone.

Some tests touch real hardware or the network and are `#[ignore]`d by
default (for example the real clipboard round-trip). Run them with
`cargo test -- --ignored`.

## Release builds (the station package)

`scripts/build-station.sh` builds everything a station serves and assembles
`dist/slipstream-station/`:

| Output | Built with | Notes |
|---|---|---|
| `clients/slipstream-linux-x86_64` | `cargo zigbuild` for glibc 2.31 | Runs on Debian 11 / Ubuntu 20.04 and newer. Falls back to a native build (newer glibc only) without zig. |
| `bin/slipstream-server-linux-x86_64` | same | The relay the station runs. |
| `clients/slipstream-windows-x86_64.exe` | `cargo xwin` (MSVC target, cross-compiled from Linux) | Skipped when cargo-xwin isn't installed. |
| `clients/slipstream-macos.zip` | — | Copied in if you put it at `dist/slipstream-macos.zip` (build `Slipstream.app` on a Mac). |
| `clients/manifest.json` | the script | Version, build number, commit, and size + SHA-256 of each app, for self-update. |

One-time tool setup:

```sh
# Zig (any recent version) somewhere on PATH, e.g. ~/.local/share/zig
cargo install cargo-zigbuild cargo-xwin
rustup target add x86_64-pc-windows-msvc
```

The script makes an empty stand-in `libpipewire-0.3.so` in
`~/.cache/slipstream-zig-stubs`, because libspa-sys asks the linker for it.
The path must not contain spaces.

### Version and build numbers

- `version` in the workspace `Cargo.toml` is the human version (0.5.0).
  To change it, also update `packaging/windows/slipstream.rc` (the .exe's
  version info) and add a section to `CHANGELOG.md`.
- The **build number** is the git commit count (`git rev-list --count HEAD`),
  baked into every binary along with the commit hash. Apps update when the
  station's `(version, build)` is higher than their own.

So **commit before you build a release**, or the new apps carry the same
build number as the old ones and nobody gets offered the update.

**Build every platform in the same run.** If `manifest.json` listed a
Windows app from an older build, Windows clients would update, find they
are still "old", and update again in a loop. The script therefore always
rebuilds every platform it lists.

### Release checklist

1. Commit your changes (`git commit`). This raises the build number.
2. `scripts/build-station.sh`
3. Upload to the station's web root:
   - `clients/` apps first, `clients/manifest.json` **last**,
   - `bin/` and the PHP files if they changed.

   On a shared host, copy each file to a temporary name and rename it into
   place, so nobody downloads a half-written file.
4. If `bin/` changed, press **Restart relay** on the admin page.
5. Check the update is offered:

   ```sh
   curl -s -X POST https://support.example.com/api/ -H 'Content-Type: application/json' \
     -d '{"op":"update_check","platform":"windows-x86_64","key":"<access key>"}'
   ```

Running apps pick the update up within 6 hours, or about 20 seconds after
they next start.

### Protocol changes

`PROTOCOL_VERSION` in `crates/slipstream-core/src/proto.rs` must go up
whenever a session message changes shape. Messages are `postcard`-encoded,
which has no optional fields: an added field breaks older peers. Hosts and
viewers with different versions refuse each other with a "please update"
message, so a protocol change means updating every app (the station does
that for you).

## Testing the Windows build on Linux

The Windows app runs under Wine well enough to test file transfer, chat and
the host side with the screenshot capture fallback:

```sh
export WINEPREFIX=/tmp/slip-wine WINEDEBUG=-all
wine target/x86_64-pc-windows-msvc/release/slipstream.exe host --password test1234
```

Windows-only unit tests (for example the pointer drawing) run under Wine too:

```sh
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine \
  cargo xwin test --target x86_64-pc-windows-msvc -p slipstream-platform
```

Real DXGI capture, SmartScreen and input on the secure desktop need a real
Windows machine.

## Repository layout

```
apps/slipstream/           desktop app (egui) + CLI
apps/slipstream-server/    standalone directory + relay server
crates/slipstream-core/    protocol, auth, directory client, config, clock
crates/slipstream-codec/   H.264 encode/decode
crates/slipstream-platform/ screen capture, input, clipboard per OS
crates/slipstream-session/ host/viewer sessions, files, updates
server/station/            the PHP station (download page, API, admin)
packaging/                 desktop entry, icons, Windows resources, server configs
scripts/                   build-station.sh, try-local.sh, install-linux.sh
docs/                      these documents
```
