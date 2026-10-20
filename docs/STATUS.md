# Project status

*Updated 2026-10-20; latest published release 0.21.1.
Read this first when picking the work back up.*

The latest [development and deployment handoff](DEVELOPMENT-2026-10-20.md)
covers completed 0.21.1 publication and durable release tooling. The
[October 19 handoff](DEVELOPMENT-2026-10-19.md) covers the support-tool audit,
cloned-device identity detection, plus
draggable tabs, configurable windows, service one-time codes, toolbar layout,
tool-window cleanup, proactive screen approval and APK naming.
**Published clients: 0.21.1, build 106, commit 2d1a128**, on
https://relay.slipstreamremote.com. Later documentation commits do not change
that deployed build. The mounted client directory and rollback backups are
recorded in the handoff.

**0.21.1 published:** final implementation 2d1a128, build 106. Audits the
five support-tool features below. Fixes pending requests surviving window
close/link loss, late tunnel replies, compact-host status cleanup and missing
Dismiss / Stop controls, stale Windows service command epochs and unrelated
DNS answers. Video repaints no longer create blank native windows for docked
sessions. Compact controls have enough room above the footer.

138 workspace tests pass (five ignored); Linux/Windows all-target and arm64
Android Clippy pass. Native Linux checks verify report exports, cancelled
diagnostics/reopening, live floating video and docking with no placeholder
windows. The Windows release under Wine verifies compact status, Dismiss
retaining the link and Stop ending it without recovery. The final packaged
compact layout has both controls above the footer ([preview](screenshots/compact-support-0.21.1.png)).
Linux/Windows/Android artifacts are in `dist/slipstream-station/clients/`;
hashes/signatures, update trust continuity, Windows resources/Mesa, Linux
startup/glibc and Android version/certificate checks passed. The mount was
reconnected October 20: clients were backed up and published with the manifest
last. Authenticated live offers and full Linux/Windows/Android HTTPS downloads
passed. Today's workspace run again passed 138 tests; Linux Clippy and six
isolated publication regressions passed. See the latest handoff for rollback.
Physical Windows service, LAN wake and real
Android/macOS checks remain open. The handoff maps each first-pass feature
to its implemented scope and the larger ideas still on the roadmap.

**0.21.0 (superseded unpublished candidate):** 1dc6836, build 101. Adds reviewed support
reports, host status with Stop support, discovered-service tunnels, peer-assisted
LAN wake and bounded gateway/DNS/station diagnostics. Desktop hardware bindings
give imaged profiles a fresh identity on different firmware while retaining
station/unattended settings; run the updated master app/service before imaging.
Missing or duplicate firmware evidence limits detection. The feature test run
passed 131 tests (five ignored); Linux/Windows all-target and arm64 Android
Clippy passed in its initial preparation. Its artifacts were checked before
the subsequent workflow audit found the bugs corrected in 0.21.1. This
intermediate candidate was never published; 0.21.1 is now live.

**0.20.0:** a session strip switches workstations and returns to Bridge.
Drag tabs to reorder, release outside the strip to create a floating window,
or drag a floating session's tab back to the main strip to dock the same link.
Tabbed and floating links can coexist. Config → Viewing → New links saves
Tabs / Separate windows as the default for future connections; existing
profiles default to tabs and existing links retain their layout. Pop out /
Dock and tab context menus remain available. Closing a tab/window ends only
its link. Windows/X11 drags sample native pointer release outside the source;
physical-pixel targets account for per-window scaling.

122 workspace tests pass (five ignored); Linux/Windows all-target and arm64
Android Clippy pass. New headless UI checks cover reordering, tear-out, Escape,
cross-scale docking and release delivered to the destination. Isolated native
X11 checks confirm two tabs, reordering, tear-out beyond the source window,
docking into an empty strip, retaining another link on close, and saving /
using the separate-window default. [Preview](screenshots/session-tabs-0.20.0.png).
Physical Windows drag and real mixed-DPI monitors remain runtime checks;
macOS and pure Wayland cross-window dragging were not exercised. Explicit
Pop out / Dock actions provide an alternative. Release and live download
verification are recorded in the handoff.

**0.19.0–0.19.1:** the full and compact Windows apps show the running
service's actual one-time code alongside its unattended password. Copy and
New code use the authenticated local pipe; passwords are omitted from service
stdout. Update the installed service to expose these controls with an older
installation. Both passwords authenticate the same ID, and regenerating the
code leaves unattended access and existing sessions intact.

Desktop sessions can **Pop out** into independent native windows, opened or
focused from **Links**, then **Dock** without reconnecting. Each window owns
its input focus and shortcuts. Closing it ends only its link; Bridge opens
the console while the session continues. Reconnection retains the same
window and display/quality choices. Native title bars show the workstation
name prominently, and FPS/latency sit beside picture quality. Toolbar labels
adapt to measured space, with a Console menu at small widths and a screen
selection menu. Android and full-screen sessions retain the workstation name
in the toolbar/menu when the native title bar is absent.

119 workspace tests pass (five opt-in tests ignored), including simultaneous
one-time/permanent authentication, service code updates and two-session input
routing. Linux/Windows all-target Clippy and arm64 Android Clippy pass.
Isolated native X11 checks cover two live workstation windows, docking,
closing only one link, reconnecting in place, three-screen menus, narrow
layouts and tools with the console hidden. The final phone/full-screen label
patch compiled for all three targets. Physical Windows service/UI and actual
window-manager minimization remain confirmation tasks; hidden-console checks
used X11 unmapping. Release and live HTTPS checks are recorded in the handoff.

**0.18.0:** detached Files, transfer receipts and Network Scan hide immediately
on close and force their parent to repaint even while occluded. System
Information discards its closing frame and redraws without the panel. These
changes target Windows black rectangles left until the main app is clicked;
the actual Windows VM still needs visual confirmation.

Wayland GUI/headless hosts prepare screen/control access at startup, before
any viewer connects. Config adds **Approve now** and **Reset / approve again**.
The approved backend stays ready in the tray; desktop restore tokens are
saved when supplied. Reset refuses active viewers, and incoming session
registration is serialized with reset. Reboot persistence remains desktop
dependent. Windows Settings explains its automatic screen access and background
service instead of showing the inapplicable portal Reset button. Android keeps
explicit Start sharing consent. Station download links now include the app
filename, including `.apk`, with simple ASCII attachment headers and UTF-8
fallbacks; APK bytes remain unchanged by the station.

116 workspace tests pass (five opt-in tests ignored), including local
preapproval reuse, concurrent preparation and reset protection during a live
encrypted session. Linux/Windows all-target Clippy and arm64 Android Clippy
pass. Isolated PHP checks verify Android naming/MIME/exact bytes and account
behavior. Release signatures, desktop trust keys, portable Linux startup,
Android signing/version/ABI and live HTTPS update/download checks passed.
No physical Windows UI or fresh Wayland consent/reboot runtime check was made.

The previous [October 18 handoff](DEVELOPMENT-2026-10-18.md) covers releases
0.8.0–0.17.1 and their broader runtime evidence.

**0.17.1:** a [deep code audit](AUDIT-2026-10-18.md) fixes concurrent transfer
collisions/overwrites, abandoned staging on ordinary failures, held input after
connection loss, video queue shutdown and unbounded decode buffering, tunnel
lifetime/IPv6 issues, concurrent identity/settings writes, UNC paths and symlink
cycles. Folder browsing runs off the UI thread and ignores stale replies.
Updates hash incrementally with bounded trailing data and idle deadlines.
Android retains its final pending capture image and uses individual UDP packets
for compatibility: a cold direct-link stall reproduced with offload enabled,
while the relayed connection and three cold direct starts with offload disabled
passed. Other hosts can set SLIPSTREAM_DISABLE_GSO=1 for the same workaround.
Audio driver waits and foreign-frame bounds are now bounded/checked. Rust server
rate limiting ignores spoofed forwarding prefixes behind a single local proxy.

115 workspace tests pass (five opt-in tests ignored); Linux and
Windows Clippy include all targets/test code, and arm64 Android checks pass.
Isolated native X11 checks confirm exact drop bytes, detached Files opening from
its receipt with the parent hidden, and asynchronous navigation on both panes.
Android 16 emulator checks cover cold direct/relay startup, revoking/restarting
sharing with no viewer, PC navigation/typing/dragging, rotation and notification
revocation. Physical Windows/macOS/Android runtime confirmation
remains useful. The audit records shared input ownership, blocked-driver worker
retention and abrupt-process-kill staging as remaining limits.

**0.17.0:** Android now registers a stable host ID and shows its ID/password
on the main console, with a manufacturer/model name for default identities.
Start sharing obtains Android consent and runs MediaProjection in a foreground
service with a Stop notification. The optional accessibility service enables
PC taps, held-touch drags, wheel swipes, typing/editing and Back/Home/Recents;
without it, sessions are view-only. Input is gated by an active full-display
projection, an enabled accessibility service and a live authenticated capture.
Rotation resizes the existing display/surface; revocation closes capture slots
and disconnects viewers. Initial capture uses the existing software encoder,
a 1280-pixel long-edge bound and a 30 fps cap. Android host audio, clipboard,
files, hardware encoding and capture without fresh approval are not included.

Desktop drops and other transfers open a small native LCARS receipt window.
It shows the target, bytes and active progress; Complete follows the remote
save acknowledgement, while cancelled/failed results stay visible. Closing
hides the panel without cancelling; new work reopens it. Clear finished and
Files actions expose history/browser controls.

Android 16 emulator checks exercise the visible ID, view-only input denial,
PC-decoded capture, Home/Back/Recents, mouse taps, keyboard input/backspace,
held-touch dragging, background capture, rotation and Stop notification
revocation, including stopping while no viewer is connected and starting a
fresh approved capture under the same ID. A real phone still needs confirmation. Isolated Linux native-drop
checks verify saved bytes, live progress, cancellation, source permission
errors and receipt retention. Physical Windows/Android hardware remains
outside these local runtime checks.

**0.16.0:** File Transfer has independent Name/Modified sorting in each
pane, reversible with another click and with folders always first. Choices
survive navigation and reconnects; raw listing indices preserve selection
and transfer identity. Common raster images show thumbnails and larger hover
previews; text/source files show passive page previews. Visible rows load in
the background, with two requests per pane and 128 cached entries. Hosts
return bounded previews, never the original image, over an optional
file-browsing-authorized stream. Refresh/navigation clear previews and ignore
late results. Older hosts retain generic icons. Update both ends for remote
previews. PDFs, video and office-document rendering are not implemented.

**0.15.0:** desktop viewers detect an unresponsive host using the existing
performance probe (six-second request deadline, three-second polling gap),
then automatically retry interrupted links for up to two minutes. Recovery
uses fresh lookups, bounded attempts and backoff; rejected passwords, explicit
session ends, Cancel reconnect and End Link stop recovery. The same tab,
chat, display/quality and tool windows survive; input capture is released.
Transfers fail visibly and port forwards close rather than being replayed.
File Transfer now owns a native LCARS viewport, larger than a small remote
window, with vector folder/file icons, desktop drops into its remote pane
and independent repainting/live connection state while the parent is minimized.
Actual Windows service-update recovery still benefits from a real VM check;
Linux runtime checks use an abruptly killed and restarted host process.

**0.14.1:** System Information uses a contrasting red Close button on its
yellow title rail. Background scanner repaints preserve remote control;
focused tools still release held remote input and keep their keyboard
events local. This fixes control-prompt flicker during browser/scan updates.
Lower-left outer rail/footer joins also have wider curves in the scanner,
shared dialogs and the main session frame.
Native window controls also have a small gap between their outlines.

**0.14.0:** a single cyan scanner title rail with a curved left elbow,
inset symbolic controls, peach Close and a fixed blue footer. Compact
scanner contents scroll inside the frame. Also adds a public-IP row and Copy IP action near the top of System
Information. The host queries ipify over HTTPS, with IPv4/dual-stack fallback,
a shared one-minute cache and a separate request from hardware inventory.
Refresh rechecks after five seconds; blocked internet/older hosts show
Unavailable while the rest of the window remains usable. Copy all includes
public IP too. The browser links from 0.13.0 are included in this live update.

**0.13.0:** highlighted HTTP/HTTPS port actions in the network table and
selected-device details. Clicking sends the numeric URL to the remote
computer's default browser, with request status/errors in the scanner.
Windows service hosts use the controlled session's signed-in user and their
browser association. The optional stream preserves existing protocol-6 scans;
both network-scan and remote-control permissions are required. Windows
service browser launching still needs confirmation on the real VM.

**0.12.0:** Linux desktop file/folder drops use X11/Xwayland and actual drop
coordinates; Windows resolves the open Explorer folder or the signed-in
user's Desktop once per batch, including service hosts. Empty directories
survive. The Windows service/Explorer path needs confirmation on the real VM.
Also adds collapsible saved-device groups synced by the station, an
Add this device shortcut, growing multiline notes, roomier editor fields,
and contrasting peach Close / green Save actions. Station updates preserve
groups when older clients edit entries; private/shared permissions and encrypted passwords
remain enforced. The shared elbow correction also protects the bottom DNS
rows in System Information.

**0.11.4:** clip floating-dialog elbow cutouts to their own rail geometry,
so the frame no longer erases the Save button's lower-left corner in the
Save a device / Edit device modal. The shared frame fix protects other
dialog controls too.

**0.11.3:** aligned console dropdowns and Tools entries, a blue menu
selection palette, clearer Blank remote screen wording, and an LCARS
communications window with framed messages and a bottom composer. Windows
borderless client areas cover the native shadow strip above chat. Linux
runtime checks cover both chat directions and the new menus; Windows native
code is cross-checked, with actual server confirmation still useful.

**0.11.2:** the windowed desktop frame reserves space for its left rail
at every aspect ratio/display scale. The blue footer spans below the remote
picture and taskbar, the right rail has a break below End Link, and the dark
FPS/latency panel has a subtle blue outline. Video fitting and input share
the inset content area; full screen and Android retain their full picture.
Linux runtime checks cover wide, near-aspect and compact windows, 150%
display scaling, chat, menus and full-screen transitions.

**0.11.1:** remote-session LCARS rails in existing side letterboxing,
a swept LINK elbow, full-height toolbar segments with vector icons, and
rounded native window bounds. Native corner masks are used on Windows and
X11; Wayland uses alpha compositing. Full-screen/maximized windows remain
rectangular. Linux checks cover wide/compact layouts, chat, popup menus,
full screen and native shapes. Windows native code is cross-checked; real-VM
confirmation remains useful.

**0.11.0:** device names, routed subnet/VLAN scans, a detached scanner,
peripheral/printer/network-drive inventory, restored Windows service chat,
and LCARS desktop title bars. Protocol remains 6: optional name lookup uses
appended stream variants, preserving 0.10 scan interoperability.

115 workspace tests pass (5 opt-in tests ignored). Isolated Linux
GUI checks also cover image/text thumbnails, hover previews, independent
Name/Modified sorting and direction toggles, selected-file Send/Receive
bytes after reordering, and native file/folder drops. Linux GUI checks
confirm the scanner is larger than its main window, finds this machine's
name/ports, switches views, maximizes/restores, and closes without ending the
session. Linux and Windows Clippy and Android checks pass. Windows service
chat and the expanded native inventory compile; their behavior on the actual
Windows VM still needs confirmation. The previous 0.10 sharpness fix also
needs real-VM confirmation. Latest clients are deployed to the live relay;
the two account API PHP files are updated for group persistence. The relay
program needs no update. Isolated PHP and Rust CLI integration checks cover
group changes, older-client updates, multiline notes, passwords and permissions.
The verification table below also records earlier releases.


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
| Station on the server's PHP 8.4 (no extensions): setup, sign-in, settings, download password, devices, key renewal, update offer | ✅ against a local copy; all PHP lint-clean |
| Signed update installs; tampered, unsigned and foreign manifests and a swapped binary are refused | ✅ against a local station |
| Windows build under Wine: host at 1440p/60 fps, GUI | ✅ |
| Compact window (no usable GPU, e.g. Windows Server 2016): ID/password/status, being controlled | ✅ under Wine (forced); on real Server 2016 by the user |
| Full window through the built-in software renderer (Mesa llvmpipe) | ✅ on real Windows Server 2016 (VMware VM), through the copy of the app beside Mesa (loading Mesa in-process found no adapter there). Idle CPU use cut by slower animations on llvmpipe (0.6.0); not re-measured |
| Linux → Windows Server 2016 VM over the internet | ✅ direct path, 20–50 ms round trip. Service capture uses DXGI duplication there (1.8 ms a frame). Encoding took 23 ms a frame, so about 34 fps; 0.7.0's camera mode and adaptive resolution should reach 60 but haven't been measured there yet |
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
| macOS | ⚠️ type-checks for Apple Silicon (cross-checked from Linux); `scripts/build-macos.sh` packages it; not yet run on a Mac |
| Android viewer and host | ⚠️ Android 16 emulator: persistent ID, consent/foreground capture, PC control, rotation and revocation tested; real phones still need confirmation. Software host encoding; audio/clipboard/files pending. |
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

**In progress: Android** (issue 09). Initial PC-to-phone hosting is now
implemented and emulator-tested, alongside the touch viewer. Remaining work
includes real-phone checks, viewer keyboard polish, hardware encoding and
host audio/clipboard/files/updates. The steps below carry on alongside it.

1. **Confirm 0.21.1 on real machines**: publication and signed HTTPS checks
   are complete. Confirm clone/service identity on
   differently identified Windows hardware; exercise reports, status/Stop,
   discovered-service tunnels, permitted LAN wake and network diagnostics.
   **Confirm 0.20.0 on the Windows Server VM**: tab reorder/tear-out/dock-back
   and the saved new-link default, service one-time codes beside
   unattended access, independent workstation windows, closing tools without
   black remnants, sharp Balanced/Quality motion,
   the pointer on the mouseless VM, service-update recovery, incoming chat,
   peripheral/network-drive inventory, Explorer/Desktop drop destinations,
   scanner browser launching and native frame corners/rails. Use the stats
   tooltip and 30-second log line to measure capture/encode/decode and picture
   dimensions rather than assuming a high frame rate means full resolution.
2. **Confirm Android hosting on real phones**: persistent ID, sharing consent,
   accessibility setup, PC input, cold direct/relay links, rotation and Stop.
   Include older supported Android versions; current host runtime evidence is
   from an Android 16 emulator.
3. **Finish earlier real-machine checks**: Windows laptop pointer, Debian
   portability, clipboard files and sound between machines. Existing issue
   checklists retain the original version numbers for traceability.
4. **Push to GitHub**, get CI green on all three OSes, and attach release builds.
   Local commits/releases have not been pushed.
5. **macOS**: run `scripts/build-macos.sh` on the Mac; test install, Open
   Anyway, permissions, capture, input, setup code and an update (issue 03).
6. **Try the session tools on real machines** (issue 16): UAC and the
   login screen through the service, privacy mode, copying files both
   ways, port forwarding to Remote Desktop.
7. **Hardware encoding**: NVENC, VA-API, Media Foundation, VideoToolbox, Android
   MediaCodec; continue Android host audio/clipboard/files and update support.
8. **Code signing**: apply to SignPath once the repo is public (issue 05);
   Apple notarisation when the Developer Program is joined.
9. **Sound from Mac hosts** (ScreenCaptureKit audio).

## Picking the work back up

- Build and test: [BUILDING.md](BUILDING.md). The release checklist is
  there: commit, `scripts/release-version.sh patch|minor`, commit, run
  `scripts/build-station.sh`, upload the apps, then the manifest.
- How it fits together: [ARCHITECTURE.md](ARCHITECTURE.md).
- User-facing behaviour: [GETTING-STARTED.md](GETTING-STARTED.md).
- What changed: [CHANGELOG.md](../CHANGELOG.md).
- Latest handoff and deployed build: [DEVELOPMENT-2026-10-20.md](DEVELOPMENT-2026-10-20.md).
- Audit findings, regression evidence and limits: [AUDIT-2026-10-18.md](AUDIT-2026-10-18.md).
