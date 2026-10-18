# Project status

*Updated 2026-10-18; latest release 0.9.0, with current unreleased work
below. Read this first when picking the work back up.*

**Current work (after 0.9.0, unreleased):** full-resolution Balanced/Quality
video and remote Network scan with table/topology views (session protocol 6).
Local tests cover slow-encoder sizing/preset changes and a scan over an
actual authenticated QUIC session to this machine's own address, including
open ports, range validation and permission denial. Windows cross-checks
cover the scanner and native ICMP. The server VM still needs the updated
host and viewer installed for a real-machine sharpness and LAN-scan check.
The verification table below records earlier releases; Android progress is
newer in CHANGELOG.md and issue 09.


## What's verified

| Area | Status |
|---|---|
| Windows 11 → Linux (KDE Wayland) control, 3 monitors | ✅ tested by hand |
| Linux → Windows 11 control | ✅ tested by hand (pointer fix in 0.5.0 still to be confirmed on the laptop) |
| Station on Virtualmin: setup, checks, provisioned downloads, relay through Apache | ✅ live |
| Relay-only session through the station | ✅ |
| Wrong clock (a month behind, a year ahead) | ✅ against the live station |
| Self-update: offer, download, checksum, swap, restart; tampered file rejected | ✅ |
| Signed updates (release key, unsigned/foreign builds refused) | ⚠️ unit tests; first signed release not deployed yet |
| Parallel password guessing held to the lockout | ✅ end-to-end test |
| Station on the server's PHP 8.4 (no extensions): setup, sign-in, settings, download password, devices, key renewal, update offer | ✅ against a local copy; all PHP lint-clean |
| Signed update installs; tampered, unsigned and foreign manifests and a swapped binary are refused | ✅ against a local station |
| Windows build under Wine: host at 1440p/60 fps, GUI | ✅ |
| Compact window (no usable GPU, e.g. Windows Server 2016): ID/password/status, being controlled | ✅ under Wine (forced); on real Server 2016 by the user |
| Full window through the built-in software renderer (Mesa llvmpipe) | ✅ on real Windows Server 2016 (VMware VM), through the copy of the app beside Mesa (loading Mesa in-process found no adapter there). Idle CPU use cut by slower animations on llvmpipe (0.6.0); not re-measured |
| Linux → Windows Server 2016 VM over the internet | ✅ direct path, 20–50 ms round trip. Service capture uses DXGI duplication there (1.8 ms a frame). Encoding took 23 ms a frame, so about 34 fps; 0.7.0's camera mode and adaptive resolution should reach 60 but haven't been measured there yet |
| File manager, chat, clipboard | ✅ end-to-end tests; real KDE clipboard; Windows host under Wine |
| Copy and paste files (both ways) | ⚠️ end-to-end test; file lists on the real KDE clipboard both ways; not yet pasted in Explorer or Dolphin during a session |
| Technician accounts, saved devices, several stations | ✅ against a local PHP station: sign-in (wrong password, unknown user), sealed passwords stored unreadable, live presence, connecting through another station with the saved password, admin-only shared list, password reset signing out and clearing secrets; GUI checked by screenshots. Not yet on the live station |
| System information window (live) | ✅ end-to-end test; Linux and Windows (under Wine) collectors checked by hand; macOS type-checks only |
| Port forwarding | ✅ end-to-end test (echo through the tunnel, unreachable target, refused when not allowed) |
| Privacy mode | ⚠️ builds for Windows; needs a real Windows 10 2004+ machine (capture exclusion doesn't exist under Wine) |
| Windows service | ⚠️ installs, reinstalls over an older one, starts without the administrator prompt, and hosts under the app's ID on real Server 2016 (from 0.6.0). Install, start, a session and the published ID also checked under Wine. UAC, login screen and Ctrl+Alt+Del still to try for real (issue 04). Once, after *Update*, the service ended up stopped for reasons not yet known; the app now restarts it and the readable service log records why it stops |
| Portable Linux binary (glibc 2.31, no PipeWire link) | ⚠️ needs only libc, libm, libpthread and libdl (glibc symbols ≤ 2.30); the Debian server that failed before hasn't re-tried it |
| Host chat as its own window (KDE Wayland) | ✅ |
| Windows pointer drawing | ✅ unit tests under Wine; on the Server 2016 VM, which has no mouse, the arrow is drawn when Windows hides the pointer (0.6.0, to confirm) |
| Adaptive resolution and idle sharpening | ✅ pipeline test with a simulated slow encoder (shrinks to fit 60 fps, full-size frame once still) and a local session at 30 ms a simulated frame |
| Host pace in the viewer (stats tooltip, 30 s log line) | ✅ local sessions and the live Server 2016 VM |
| X11 host | ⚠️ implemented, lightly tested |
| macOS | ⚠️ type-checks for Apple Silicon (cross-checked from Linux); `scripts/build-macos.sh` packages it; not yet run on a Mac |
| Android app (phase 0) | ⚠️ emulator only (Android 16): starts, goes online, soft-keyboard typing, `slipstream://` links (refused bad code from the live station), a 60 fps session to a test-pattern host. Desktop layout, no touch control yet; not on a real phone |
| Setup codes and download-origin linking | ✅ against a local station (origin attribute on Linux, `slipstream://` handed to a running app, bad codes and rate limit) |

`cargo test --workspace` passes: unit tests plus in-process sessions
covering video and input, files, chat, clipboard text and files, sound
and port forwarding.

## Known gaps and limits

- **macOS**: packaged by `scripts/build-macos.sh` but never run on a Mac.
  Not notarised (Apple Developer Program not joined yet), so users choose
  *Open Anyway*, and permissions may need granting again after updates. Its
  capture may or may not show the pointer (`cursor_in_video` is false, so
  the viewer keeps its own pointer visible). `slipstream://` links don't
  reach the app on macOS (winit doesn't pass URL events on); setup codes do.
- **Windows elevation**: without the Slipstream service the app runs as
  the user, so it can't control UAC prompts, elevated windows or the login
  screen. The service does (it runs on real Server 2016), but UAC and the
  login screen haven't been tried through it yet.
- **Privacy mode** needs Windows 10 2004 or later on the remote side, and
  blocks the local keyboard and mouse only under the service (Windows
  allows `BlockInput` only to elevated programs).
- **Windows signing**: set up for SignPath (free for open source) in the
  release workflow, which needs the repository public on GitHub. Until then
  the .exe is unsigned and SmartScreen warns about it.
- **Software video only**: OpenH264 on the CPU (with its SIMD code: builds
  need `nasm`), in camera mode since 0.7.0. The pipeline figures (60 fps up
  to 1080p, about 56 at 1440p and 37 at 4K for a fully moving picture) were
  measured before that and are worth re-measuring. Slow processors now
  drop resolution while the picture moves, rather than frame rate.
  Hardware encoding is issue 06.
- **Windows capture** uses its own DXGI duplication since 0.7.1: one
  copy per frame, BGRA straight to the encoder, released by each session
  (xcap's recorder blocked later sessions). Not yet measured at 1440p and
  4K on real hardware (issue 15).
- **Sound**: Windows and Linux hosts share it; Mac hosts can't yet
  (ScreenCaptureKit audio). Not yet heard on real speakers between two
  machines: end-to-end test with a test tone; real devices open on Linux.
- **x86_64 only**: no ARM Linux or Windows-on-ARM builds yet.
- **Wayland unattended access**: depends on the portal's restore token
  surviving. A reboot may bring the permission prompt back on some desktops.
- **File transfer**: no resume for interrupted large transfers.
- **CI** (`.github/workflows/ci.yml`) has never run: there's no GitHub remote
  yet. The plan is to push after more testing.

## Next steps (suggested order)

Each step is written up as an issue with tasks and pointers into the code
in [issues/](issues/README.md).

**In progress: Android** (issue 09), in phases: 0 build and run the app
on Android, 1 control PCs from a phone or tablet, 2 control an Android
device from a PC, 3 sound, clipboard, files and updates. The steps below
carry on alongside it.

1. **Measure 0.7.x on the Server 2016 VM**: the viewer's 30-second log
   line shows frame rate, round trip, decode time and the host's capture
   and encode times. Confirm 60 fps, and how often adaptive resolution
   kicks in. Also confirm the pointer on the mouseless VM, and that the
   service stays running after *Update*.
2. **Confirm 0.5.0 on real machines**: Windows pointer while controlling the
   laptop, the separate chat window on Windows, smooth animations, and the
   Linux app on the Debian server.
3. **Push to GitHub** (tags `v0.5.0` to `v0.7.1` are local only), get CI
   green on all three OSes, and attach release builds.
4. **macOS**: run `scripts/build-macos.sh` on the Mac; test install, Open
   Anyway, permissions, capture, input, setup code and an update (issue 03).
5. **Try the new session tools on real machines** (issue 16): UAC and the
   login screen through the service, privacy mode, copying files both
   ways, port forwarding to Remote Desktop.
6. **Hardware encoding**: NVENC, VA-API, Media Foundation, VideoToolbox.
7. **Code signing**: apply to SignPath once the repo is public (issue 05);
   Apple notarisation when the Developer Program is joined.
8. **Sound from Mac hosts** (ScreenCaptureKit audio).

## Picking the work back up

- Build and test: [BUILDING.md](BUILDING.md). The release checklist is
  there: commit, `scripts/release-version.sh patch|minor`, commit, run
  `scripts/build-station.sh`, upload the apps, then the manifest.
- How it fits together: [ARCHITECTURE.md](ARCHITECTURE.md).
- User-facing behaviour: [GETTING-STARTED.md](GETTING-STARTED.md).
- What changed: [CHANGELOG.md](../CHANGELOG.md).
