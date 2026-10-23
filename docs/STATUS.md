# Project status

*Updated 2026-10-23. Releases: [GitHub](https://github.com/Psychronic-Games/Slipstream-Remote/releases);
changes: [CHANGELOG.md](../CHANGELOG.md).*

## Where things stand

Slipstream Remote is usable day to day between Linux (KDE Wayland) and
Windows machines, through the free **Slipstream Public** station or a
self-hosted one. Both of the project's stations run current releases:
`public.slipstreamremote.com` (keyless, default for raw downloads) and a
private station. The main features (sessions, files, clipboard, sound, chat,
support tools, meetings, technician accounts, the Windows service, the
Android viewer/host, signed self-updates) are implemented and covered by
automated end-to-end tests: 175+ workspace tests, plus Wine checks for the
Windows build and native UI audits.

What remains is mostly **verification on real hardware** rather than new
features: the Windows service at UAC prompts and the login screen, real
Android phones, a native macOS build, and sound/clipboard files between two
physical machines. See [Next steps](#next-steps).

**0.27** adds scam protection (one-time-password links on Slipstream Public
wait for **Allow**) and stations that update themselves from signed GitHub
releases.

## What's verified

| Area | Status |
|---|---|
| Windows 11 → Linux (KDE Wayland) control, 3 monitors | ✅ tested by hand |
| Linux → Windows 11 control | ✅ tested by hand (pointer fix in 0.5.0 still to be confirmed on the laptop) |
| Station on Virtualmin: setup, checks, provisioned downloads, relay through Apache | ✅ live |
| Relay-only session through the station | ✅ |
| Wrong clock (a month behind, a year ahead) | ✅ against the live station |
| Self-update: offer, download, checksum, swap, restart; tampered file rejected | ✅ |
| Signed updates (release key, unsigned/foreign builds refused) | ✅ signature tests and deployed manifest/HTTPS download verification |
| Parallel password guessing held to the lockout | ✅ end-to-end test |
| Station on the server's PHP 8.4 with its standard extensions, including sodium: setup, sign-in, settings, download password, devices, key renewal, update offer | ✅ against a local copy; all PHP lint-clean |
| Signed update installs; tampered, unsigned and foreign manifests and a swapped binary are refused | ✅ against a local station |
| Windows build under Wine: host at 1440p/60 fps, GUI | ✅ |
| Compact window (no usable GPU, e.g. Windows Server 2016): ID/password/status, being controlled | ✅ under Wine (forced); on real Server 2016 by the user |
| Full window through the built-in software renderer (Mesa llvmpipe) | ✅ on real Windows Server 2016 (VMware VM), through the copy of the app beside Mesa (loading Mesa in-process found no adapter there). Idle CPU use cut by slower animations on llvmpipe (0.6.0); not re-measured |
| Linux → Windows Server 2016 VM over the internet | ✅ direct path, 20–50 ms round trip. Earlier measurements: DXGI capture 1.8 ms/frame, encoding 23 ms/frame, about 34 fps. Current camera-mode encoding needs remeasurement there; Speed/Low bandwidth can reduce moving-picture resolution, while Balanced/Quality preserve resolution. |
| File manager, chat, clipboard | ✅ end-to-end tests; real KDE clipboard; Windows host under Wine |
| Copy and paste files (both ways) | ⚠️ end-to-end test; file lists on the real KDE clipboard both ways; not yet pasted in Explorer or Dolphin during a session |
| Technician accounts, saved devices, several stations | ✅ against a local PHP station: sign-in (wrong password, unknown user), sealed passwords stored unreadable, live presence, connecting through another station with the saved password, admin-only shared list, password reset signing out and clearing secrets; GUI checked by screenshots. Not yet on the live station |
| Scanner HTTP/HTTPS actions | ✅ Linux GUI to host launcher; exact URLs, failure reporting and host permission/target checks. Windows service launch needs real-VM confirmation |
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
| macOS | ⚠️ type-checks for Apple Silicon (cross-checked from Linux); universal packaging script prepared; native building and runtime checks pending |
| Android viewer and host | ⚠️ Android 16 emulator: persistent ID, consent/foreground capture, PC control, rotation and revocation tested; real phones still need confirmation. Software host encoding; audio/clipboard/files pending. |
| Setup codes and download-origin linking | ✅ against a local station (origin attribute on Linux, `slipstream://` handed to a running app, bad codes and rate limit) |

## Known gaps and limits

- **Meetings**: the organizer must stay connected; no host migration, automatic
  active-speaker selection or adaptive meeting bitrate yet. Invitations/rooms
  last four hours. Android incoming calls require a running, reachable app.
  See [meeting scope](issues/17-group-meetings-and-show-me.md) for remaining work.
- **macOS**: `scripts/build-macos.sh` is prepared to package a universal app;
  native building and runtime verification remain pending.
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
- **Windows signing**: the release workflow includes a conditional SignPath
  step. Repository publication, application/acceptance and signing credentials
  remain pending. The published .exe is not Authenticode-signed and may
  trigger SmartScreen warnings; desktop update signatures are already active.
- **Hardware encoding is NVIDIA only so far** (NVENC, 0.28, issue 06):
  other computers encode with OpenH264 on the processor (SIMD; builds need
  `nasm`), where slow processors drop resolution while the picture moves.
  Intel/AMD (Media Foundation, VA-API) and macOS are still to come. NVENC
  needs confirming on real Windows hardware and under the Windows service.
  Decoding is OpenH264 everywhere.
- **Windows capture** uses its own DXGI duplication since 0.7.1: one
  copy per frame, BGRA straight to the encoder, released by each session
  (xcap's recorder blocked later sessions). Not yet measured at 1440p and
  4K on real hardware (issue 15).
- **Sound**: Windows and Linux hosts share it; Mac hosts can't yet
  (ScreenCaptureKit audio). Not yet heard on real speakers between two
  machines: end-to-end test with a test tone; real devices open on Linux.
- **Desktop/station ARM builds**: Linux and Windows apps and the station relay
  are currently distributed for x86_64 only. Android releases are arm64;
  the unverified macOS packaging script targets Apple Silicon and Intel.
- **Wayland unattended access**: depends on the portal's restore token
  surviving. A reboot may bring the permission prompt back on some desktops.
- **File transfer**: no resume for interrupted large transfers.
- **CI** (`.github/workflows/ci.yml`) is new on GitHub (October 2026) and
  still being brought to green on all three systems.
- **Link approval** needs a window that can ask: headless `slipstream host`
  machines and the Windows login screen decline one-time-password links on
  Slipstream Public. Use the unattended password there. Android shows the
  prompt only while the app is open.

## Next steps

Each step has an issue with tasks and pointers into the code in
[issues/](issues/README.md).

1. **Real-machine pass** (issues 01, 04, 16): the Windows service at UAC
   prompts, elevated windows and the login screen; the new link approval on
   Windows (full window, compact window and service) and Linux; privacy mode;
   copying files in Explorer/Dolphin; sound between two machines.
2. **Android on real phones** (issue 09): persistent ID, sharing consent,
   accessibility setup, PC input, direct and relayed links, rotation, Stop,
   link approval. Include an older supported Android version.
3. **CI green on GitHub** (issue 02) for Linux, Windows and macOS.
4. **macOS** (issue 03): run `scripts/build-macos.sh` on the Mac; test install,
   Open Anyway, permissions, capture, input, setup code and an update.
5. **Code signing** (issue 05): apply to SignPath now that the repository is
   public; Apple notarisation once the Developer Program is joined.
6. **Hardware encoding** (issue 06), Android host audio/clipboard/files and
   self-updates, sound from Mac hosts.

## Picking the work back up

- Build and test: [BUILDING.md](BUILDING.md), including the release checklist
  (build, publish to GitHub; stations then update themselves).
- How it fits together: [ARCHITECTURE.md](ARCHITECTURE.md).
- User-facing behaviour: [GETTING-STARTED.md](GETTING-STARTED.md) and
  [STATION-SETUP.md](STATION-SETUP.md).
- Audit findings and limits: [AUDIT-2026-10-18.md](AUDIT-2026-10-18.md).
