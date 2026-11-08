# Project status

*Updated 2026-11-08. Releases: [GitHub](https://github.com/Psychronic-Games/Slipstream-Remote/releases);
changes: [CHANGELOG.md](../CHANGELOG.md).*

## Where things stand

Slipstream Remote is **in production use** on several Linux and Windows
systems, through the free **Slipstream Public** station and a
self-hosted one, and works well day to day. Both of the project's stations run
current releases: `public.slipstreamremote.com` (keyless, default for raw
downloads) and a private station. The main features (sessions, files,
clipboard, sound, chat, support tools, meetings, technician accounts, the
Windows service, the Android viewer/host, signed self-updates) are implemented
and covered by automated end-to-end tests: 214 workspace tests, station
PHP/Node regressions, Wine checks for the Windows build and native UI audits.
CI builds and tests every push on Linux, Windows and macOS.

Work now follows **usability and UI polish** found in daily use. As of
November 8 the user has confirmed on real machines the paths that were long
listed as open: the Windows service at UAC prompts and the login screen,
privacy mode, NVENC on Windows, Windows 8.x, copying files in
Explorer/Dolphin, sound between machines, chat-only links, meeting recording,
Android phones, sharing and control from a Mac, the X11 host, Wayland
unattended access after a reboot and remote-management restarts. Wake-on-LAN
through a gateway and management controllers (BMC/vPro consoles, Redfish)
haven't been tried on real hardware yet. See [Next steps](#next-steps).

Version **0.34** adds **Subspace Relay**: a computer online through an
Android phone's mobile data (the phone's Wi-Fi Direct network, then a
tunnel through Slipstream on the phone; [issue 19](issues/19-subspace-relay.md)).
Chat-only links no longer open the workstation's screen.

Version **0.33** adds chat-only links with Subspace chat tabs, remote
management (restarts, Wake-on-LAN through a gateway, BMC/vPro consoles and
Redfish power), one Windows build meant to start on Windows 7 SP1/8.x as well,
and about 40 fixes from a second code audit. Stations need 0.33 to save
remote management settings.

Version **0.32** adds named administrator sign-in, a wider responsive console,
searchable checkbox pickers, multiple group memberships, compact device editing
with a themed notes bar, and a wrapped/searchable relay activity report. Clients
send full last-reported System information to private stations every 15 minutes
and report motherboard model/serial information for custom PCs when firmware
supplies it. Existing clients remain compatible with station group projections.

The private station web changes were deployed and verified over HTTPS on
October 29, with private rollback backups and existing technician accounts
preserved. API/legacy-client checks and interactive browser audits at 1680, 1024
and 390 pixels pass. Full inventory and motherboard fields require 0.32 clients.

**0.33.0** in detail:
- **Chat-only links:** **Chat** beside a saved device messages the person there
  without sharing their screen. Chats open as tabs in a Subspace chat panel
  beside the device list, and **Connect** turns a chat into a session in place.
- **Bridge rows:** device actions stay beside the names on wide windows.
- **Remote management:** **Manage** on saved devices restarts them
  (normally, into Safe Mode with Networking, or into the BIOS setup). It also
  wakes them through a gateway computer on their network, and reaches their
  management controller (BMC or Intel vPro console, plus Redfish power)
  through that gateway.
- **Passwords:** saved passwords, including your own for shared devices, can be
  changed from **Edit**, and a refused one asks for the current one.
- **Chat labels:** messages carry Slipstream device names.
- **Audit fixes:** about 40 fixes from a second whole-codebase audit
  ([AUDIT-2026-11-05.md](AUDIT-2026-11-05.md)), covering meeting recordings,
  sign-in throttling, chat-only enforcement on the host, station link
  confirmation, station abuse limits and per-frame performance.
- **Station console:** device search and sorting, an LCARS-styled device
  register, full relay-log paging and export, and saving group visibility
  from the picker.

Recent releases:

- **0.34.2**: the Android app installs its own updates; Subspace Relay's
  band back to automatic, DNS lookups no longer inflate its connection count.
- **0.34.1**: Subspace Relay runs in its own process on the phone, uses
  2.4 GHz by default (5 GHz made Linux computers cut their transmit power),
  and computers connect by themselves to phones they've used.
- **0.34.0**: Subspace Relay; chat-only links open the screen only when
  asked to view.
- **0.33.1**: the Subspace chat panel stays in view beside long device lists
  (and comes first on narrow windows).
- **0.33.0**: chat-only links and Subspace chat tabs, remote management,
  Windows 7 SP1/8.x support in the one Windows build, second audit fixes.
- **0.32.1**: independent batched online-status polling and connection lookups;
  signed heartbeat/inventory budgets per device so fleets behind one router
  do not consume one another's allowance. Older stations retain compatibility.
- **0.32**: full station System information reports, motherboard identification,
  named web administrator accounts, searchable group/technician/workstation
  pickers, multiple group memberships and readable relay activity.
- **0.31**: private station enrollment, hardware/serial summaries, station-side
  name/group editing, batch technician assignment and personal credentials for
  shared devices.
- **0.27**: scam protection (one-time-password links on Slipstream Public
  wait for **Allow**); stations update themselves from signed GitHub releases.
- **0.28**: NVIDIA hardware encoding (NVENC); Balanced and Quality run at
  the chosen maximum frame rate (up to 144 fps).
- **0.29**: VA-API encoding (AMD and Intel on Linux), an opt-in Media
  Foundation encoder for Windows; meeting tabs, speaker/gallery layouts,
  chat reactions and the *Incoming transmission* call; password prompts for
  recent links.
- **0.30**: meeting recording (MP4/MKV), colour reactions, the macOS app
  built on GitHub's Macs, `slipstream://` links on Macs and **Link to my
  organisation** on Bridge.

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
| Copy and paste files (both ways) | ✅ end-to-end test; pasted in Explorer and Dolphin between real machines (user, 2026-11-08) |
| Technician accounts, saved devices, several stations | ✅ against a local PHP station: sign-in (wrong password, unknown user), sealed passwords stored unreadable, live presence, connecting through another station with the saved password, admin-only shared list, password reset signing out and clearing secrets; GUI checked by screenshots. In production use on the private station since its October 29 deploy; issue 08's two-computer and revocation checklist isn't recorded item by item |
| Subspace Relay (0.34) | ✅ loopback QUIC unit tests and the real Linux tunnel between network namespaces (`scripts/test-relay-tunnel.sh`). ⚠️ Not yet on a real phone or on Windows/macOS computers ([issue 19](issues/19-subspace-relay.md)) |
| Remote management (restarts, wake, controllers) | ✅ end-to-end tests: Redfish power state, graceful restart with authentication and refusals against a local test controller through a real gateway link and port forward; the console forward; restarts refused without Control. Station storage, validation and older-app preservation in the account suite. Restarts (normal, Safe Mode with Networking, BIOS setup) confirmed on real machines (user, 2026-11-08). ⚠️ Wake-on-LAN through a gateway and real Supermicro, iDRAC, iLO or vPro controllers not yet tried ([issue 18](issues/18-remote-management.md)) |
| Chat-only links and Subspace chat tabs | ✅ end-to-end test: chat both ways with no picture or clipboard either way, the host's viewing event only after Connect, then video. Bridge layout checked in native screenshots at 1680 and 1000 px. Used between real machines (user, 2026-11-08) |
| Private station automatic enrollment and hardware summaries | ✅ isolated PHP API/admin checks: default names/Ungrouped, preserved edits and removals, legacy shared-list migration, signed metadata, admin access and public exclusion; batch technician assignment before sign-in and while offline, partial failures and retained credentials verified. Rust registration/authentication suites pass. Native Windows/macOS hardware summaries still need verification |
| Scanner HTTP/HTTPS actions | ✅ Linux GUI to host launcher; exact URLs, failure reporting and host permission/target checks. Windows service launch needs real-VM confirmation |
| System information window (live) | ✅ end-to-end test; Linux and Windows (under Wine) collectors checked by hand; macOS type-checks only |
| Port forwarding | ✅ end-to-end test (echo through the tunnel, unreachable target, refused when not allowed) |
| Privacy mode | ✅ on a real Windows 10 2004+/11 machine (user, 2026-11-08); capture exclusion doesn't exist under Wine |
| Windows service | ✅ installs, reinstalls over an older one, starts without the administrator prompt, and hosts under the app's ID (real Server 2016 from 0.6.0). UAC prompts, the login screen and Ctrl+Alt+Del work through it on real Windows (user, 2026-11-08). Once, after *Update*, the service ended up stopped for reasons not yet known; the app now restarts it and the readable service log records why it stops |
| Windows 7 SP1 / 8.x (the one Windows build) | ✅ 0.33.1 on a real Windows 7 machine (user, 2026-11-05) and on Windows 8.x (user, 2026-11-08); imports checked at build time by `scripts/check-windows-imports.py` |
| Sound between two machines | ✅ end-to-end test with a test tone; heard on real speakers between machines (user, 2026-11-08) |
| Wayland unattended access after a reboot | ✅ portal restore token survives a reboot without a new prompt (user, 2026-11-08); other desktops' portals may differ |
| Portable Linux binary (glibc 2.31, no PipeWire link) | ⚠️ needs only libc, libm, libpthread and libdl (glibc symbols ≤ 2.30); the Debian server that failed before hasn't re-tried it |
| Host chat as its own window (KDE Wayland) | ✅ |
| Windows pointer drawing | ✅ unit tests under Wine; on the Server 2016 VM, which has no mouse, the arrow is drawn when Windows hides the pointer (0.6.0, to confirm) |
| Adaptive resolution and idle sharpening | ✅ pipeline test with a simulated slow encoder (shrinks to fit 60 fps, full-size frame once still) and a local session at 30 ms a simulated frame |
| Host pace in the viewer (stats tooltip, 30 s log line) | ✅ local sessions and the live Server 2016 VM |
| X11 host | ✅ hosting from a real X11 session (user, 2026-11-08) |
| macOS | ✅ universal app (Apple Silicon and Intel) built by `.github/workflows/macos.yml` and served by both stations since 0.30; sharing the screen and being controlled work on a real Mac (user, 2026-11-08) |
| Hardware encoding | ✅ NVENC on Linux (RTX 5070: 1.4 ms a 1080p frame) and VA-API on AMD (radeonsi: 4.5 ms), both decoded and checked by `slipstream encode-test`; NVENC on real Windows (user, 2026-11-08). ⚠️ Media Foundation only tested against Wine's software encoder, so it stays opt-in (`SLIPSTREAM_HWENC=mf`) |
| Meeting recording | ✅ native UI audit records MP4 and MKV, which ffprobe reads (H.264 and Opus frames); saved through the desktop's real save dialog (user, 2026-11-08) |
| Android viewer and host | ✅ Android 16 emulator: persistent ID, consent/foreground capture, PC control, rotation and revocation tested; works on a real phone (user, 2026-11-08). Software host encoding; audio/clipboard/files pending. |
| Setup codes and download-origin linking | ✅ against a local station (origin attribute on Linux, `slipstream://` handed to a running app, bad codes and rate limit) |

## Known gaps and limits

- **Audit open items:** the
  [2026-11-05 audit](AUDIT-2026-11-05.md#open) lists findings that need design
  work or real hardware. The largest is that saved passwords are sealed with
  a key derived from the sign-in password, which the station sees at sign-in.
  (The Windows service's unrestricted SYSTEM file access is by design.)
- **Windows 7 SP1 / 8.x:** the one Windows build links YY-Thunks, so it
  imports nothing Windows 7 lacks (checked at build time by
  `scripts/check-windows-imports.py`; see
  [BUILDING.md](BUILDING.md#older-windows-7-sp1-8x)). It works on real
  Windows 7 (0.33.1, 2026-11-05) and Windows 8.x (2026-11-08) machines.
  There it uses GDI capture and the OpenGL/software/compact window
  fallbacks, and has no privacy mode.
- **Meetings**: the organizer must stay connected; no host migration or
  adaptive meeting bitrate yet. Invitations/rooms
  last four hours. Android incoming calls require a running, reachable app.
  See [meeting scope](issues/17-group-meetings-and-show-me.md) for remaining work.
- **macOS**: the universal app is built on GitHub's Macs for each release
  (`scripts/fetch-macos.sh`); sharing and control work on a real Mac.
  Not notarised (Apple Developer Program not joined yet), so users choose
  *Open Anyway*, and permissions may need granting again after updates. Its
  capture may or may not show the pointer (`cursor_in_video` is false, so
  the viewer keeps its own pointer visible). `slipstream://` links reach the
  app on macOS through its own Apple Event handler (0.30); setup codes work
  too.
- **Windows elevation**: without the Slipstream service the app runs as
  the user, so it can't control UAC prompts, elevated windows or the login
  screen. The service does: UAC prompts, the login screen and Ctrl+Alt+Del
  work through it on real Windows.
- **Privacy mode** needs Windows 10 2004 or later on the remote side, and
  blocks the local keyboard and mouse only under the service (Windows
  allows `BlockInput` only to elevated programs).
- **Windows signing**: the release workflow includes a conditional SignPath
  step. Repository publication, application/acceptance and signing credentials
  remain pending. The published .exe is not Authenticode-signed and may
  trigger SmartScreen warnings; desktop update signatures are already active.
- **Hardware encoding** (issue 06): NVENC (Windows and Linux) and VA-API
  (AMD and Intel on Linux) are used automatically; Media Foundation (Intel
  and AMD on Windows) is opt-in until proven on real hardware, and Macs and
  Android have none yet. Other computers encode with OpenH264 on the
  processor (SIMD; builds need `nasm`), where slow processors drop
  resolution while the picture moves. Decoding is OpenH264 everywhere.
- **Windows capture** uses its own DXGI duplication since 0.7.1: one
  copy per frame, BGRA straight to the encoder, released by each session
  (xcap's recorder blocked later sessions). Not yet measured at 1440p and
  4K on real hardware (issue 15).
- **Sound**: Windows and Linux hosts share it; Mac hosts can't yet
  (ScreenCaptureKit audio).
- **Desktop/station ARM builds**: Linux and Windows apps and the station relay
  are currently distributed for x86_64 only. Android releases are arm64;
  the macOS app is universal (Apple Silicon and Intel).
- **Wayland unattended access**: depends on the portal's restore token
  surviving. It survives a reboot on the user's machines; other desktops'
  portals may still ask again.
- **File transfer**: no resume for interrupted large transfers.
- **CI** (`.github/workflows/ci.yml`) builds and tests on Linux, Windows and
  macOS for every push; green on all three since October 23, 2026.
- **Link approval** needs a window that can ask: headless `slipstream host`
  machines and the Windows login screen decline one-time-password links on
  Slipstream Public. Use the unattended password there. Android shows the
  prompt only while the app is open.

## Next steps

Each step has an issue with tasks and pointers into the code in
[issues/](issues/README.md).

0. **Usability polish from production use** (ongoing), starting with the
   open audit items that show in daily use: under the Windows service the
   Files window's default places and pasted files should resolve to the
   signed-in user's folders. (Chat-only links no longer open the screen
   since 0.34.)
1. **Subspace Relay on real devices** (issue 19): the phone's Wi-Fi Direct
   group and relay, and Windows, macOS and Linux computers through it.
2. **Saved-password vault** (audit, high): derive separate sign-in and vault
   keys on the client so the station never sees what opens saved passwords;
   needs a station/app migration, best done before 1.0.
3. **Remote management on real hardware** (issue 18): Wake-on-LAN through a
   gateway, and Supermicro/iDRAC/iLO/vPro consoles and Redfish power.
4. **Code signing** (issue 05): apply to SignPath now that the repository is
   public; Apple notarisation once the Developer Program is joined.
5. **Hardware encoding** (issue 06): `slipstream encode-test` on AMD/Intel
   Windows machines to turn Media Foundation on by default; then VideoToolbox
   on Macs.
6. **Meetings robustness** (issue 17 and the audit's meeting items).
7. Android host audio/clipboard/files and self-updates, sound from Mac hosts.

## Picking the work back up

- Build and test: [BUILDING.md](BUILDING.md), including the release checklist
  (build, publish to GitHub; stations then update themselves).
- How it fits together: [ARCHITECTURE.md](ARCHITECTURE.md).
- User-facing behaviour: [GETTING-STARTED.md](GETTING-STARTED.md) and
  [STATION-SETUP.md](STATION-SETUP.md).
- Audit findings and limits: [AUDIT-2026-10-18.md](AUDIT-2026-10-18.md) and
  [AUDIT-2026-11-05.md](AUDIT-2026-11-05.md) (with open items).
