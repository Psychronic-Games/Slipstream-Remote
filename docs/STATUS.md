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
| File manager, chat, clipboard | ✅ end-to-end tests; real KDE clipboard; Windows host under Wine |
| Portable Linux binary (glibc 2.31, no PipeWire link) | ⚠️ needs only libc, libm, libpthread and libdl (glibc symbols ≤ 2.30); the Debian server that failed before hasn't re-tried it |
| Host chat as its own window (KDE Wayland) | ✅ |
| Windows pointer drawing | ✅ unit tests under Wine; real hardware pending |
| X11 host | ⚠️ implemented, lightly tested |
| macOS | ❌ never built (the CI job exists but has not run), packaged or run |

`cargo test --workspace` passes: unit tests plus in-process sessions
covering video and input, files, chat and clipboard.

## Known gaps and limits

- **macOS**: no `.app` packaging in `build-station.sh`; never run. Its
  capture may or may not show the pointer (`cursor_in_video` is false, so
  the viewer keeps its own pointer visible).
- **Windows elevation**: the app runs as the user, so it can't control UAC
  prompts, elevated windows or the login screen. That needs a Windows
  service running as SYSTEM.
- **Windows signing**: the .exe is unsigned, so SmartScreen warns about it.
- **Software video only**: OpenH264 on the CPU. Fine at 1080p; heavy at 4K.
- **No audio.**
- **x86_64 only**: no ARM Linux or Windows-on-ARM builds yet.
- **Wayland unattended access**: depends on the portal's restore token
  surviving. A reboot may bring the permission prompt back on some desktops.
- **File transfer**: no resume for interrupted large transfers.
- **CI** (`.github/workflows/ci.yml`) has never run: there's no GitHub remote
  yet. The plan is to push after more testing.

## Next steps (suggested order)

1. **Confirm 0.5.0 on real machines**: Windows pointer while controlling the
   laptop, the separate chat window on Windows, smooth animations, and the
   Linux app on the Debian server.
2. **Push to GitHub**, get CI green on all three OSes, and attach release
   builds.
3. **macOS**: package `Slipstream.app` (bundle, `station.json`, icon), test
   capture, input and permissions on a real Mac, and add it to
   `build-station.sh` / CI.
4. **Windows service** for UAC, elevated apps and the login screen
   (unattended access to servers).
5. **Hardware encoding**: NVENC, VA-API, Media Foundation, VideoToolbox.
6. **Code signing** for Windows (and notarisation for macOS).
7. **Audio streaming.**
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
