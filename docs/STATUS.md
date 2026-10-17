# Project status

*Updated 2026-10-16, at version 0.5.0. Read this first when picking the work
back up.*

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
| Full window through the built-in software renderer (Mesa llvmpipe) | ⚠️ under Wine: the full app and a 60 fps session draw on llvmpipe; unpacking, the failed-start fallback and the retry button work. Real Server 2016 pending (Wine's Direct3D loads its own OpenGL first, so loading Mesa's copy can only be proven on Windows) |
| File manager, chat, clipboard | ✅ end-to-end tests; real KDE clipboard; Windows host under Wine |
| Copy and paste files (both ways) | ⚠️ end-to-end test; file lists on the real KDE clipboard both ways; not yet pasted in Explorer or Dolphin during a session |
| System information window (live) | ✅ end-to-end test; Linux and Windows (under Wine) collectors checked by hand; macOS type-checks only |
| Port forwarding | ✅ end-to-end test (echo through the tunnel, unreachable target, refused when not allowed) |
| Privacy mode | ⚠️ builds for Windows; needs a real Windows 10 2004+ machine (capture exclusion doesn't exist under Wine) |
| Windows service | ⚠️ install, a session at 1440p/60 and uninstall under Wine; UAC, login screen and Ctrl+Alt+Del need real Windows (issue 04) |
| Portable Linux binary (glibc 2.31, no PipeWire link) | ⚠️ needs only libc, libm, libpthread and libdl (glibc symbols ≤ 2.30); the Debian server that failed before hasn't re-tried it |
| Host chat as its own window (KDE Wayland) | ✅ |
| Windows pointer drawing | ✅ unit tests under Wine; real hardware pending |
| X11 host | ⚠️ implemented, lightly tested |
| macOS | ⚠️ type-checks for Apple Silicon (cross-checked from Linux); `scripts/build-macos.sh` packages it; not yet run on a Mac |
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
  screen. The service does, but hasn't been tried on real Windows yet.
- **Privacy mode** needs Windows 10 2004 or later on the remote side, and
  blocks the local keyboard and mouse only under the service (Windows
  allows `BlockInput` only to elevated programs).
- **Windows signing**: set up for SignPath (free for open source) in the
  release workflow, which needs the repository public on GitHub. Until then
  the .exe is unsigned and SmartScreen warns about it.
- **Software video only**: OpenH264 on the CPU (with its SIMD code: builds
  need `nasm`). On the dev machine a fully moving picture runs at 60 fps up
  to 1080p, about 56 at 1440p and 37 at 4K. Static and typing content is far
  cheaper. Hardware encoding is issue 06.
- **Windows capture** makes several full-frame copies inside xcap (issue 15).
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

1. **Confirm 0.5.0 on real machines**: Windows pointer while controlling the
   laptop, the separate chat window on Windows, smooth animations, and the
   Linux app on the Debian server.
2. **Push to GitHub**, get CI green on all three OSes, and attach release
   builds.
3. **macOS**: run `scripts/build-macos.sh` on the Mac; test install, Open
   Anyway, permissions, capture, input, setup code and an update (issue 03).
4. **Try the new session tools on real machines** (issue 16): the
   Windows service, privacy mode, copying files both ways, port
   forwarding to Remote Desktop.
5. **Hardware encoding**: NVENC, VA-API, Media Foundation, VideoToolbox.
6. **Code signing**: apply to SignPath once the repo is public (issue 05);
   Apple notarisation when the Developer Program is joined.
7. **Sound from Mac hosts** (ScreenCaptureKit audio).
8. **Address book** with saved devices and credentials (OS keychain).
9. **Android**: viewer first, then host (MediaProjection +
   AccessibilityService). The egui UI already targets Android.

## Picking the work back up

- Build and test: [BUILDING.md](BUILDING.md). The release checklist is
  there: commit, run `scripts/build-station.sh`, upload the apps, then the
  manifest.
- How it fits together: [ARCHITECTURE.md](ARCHITECTURE.md).
- User-facing behaviour: [GETTING-STARTED.md](GETTING-STARTED.md).
- What changed: [CHANGELOG.md](../CHANGELOG.md).
