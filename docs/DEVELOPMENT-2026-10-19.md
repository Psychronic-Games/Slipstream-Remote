# Development handoff — 2026-10-19

**Continuation:** [October 20](DEVELOPMENT-2026-10-20.md) completed publication
of the verified 0.21.1 candidate and authenticated HTTPS download checks. The
deployment blocker and temporary scripts below are historical; use the
repository's `scripts/publish-station-clients.py` for future publication.

## Support-tool audit and stabilization: 0.21.1

The interrupted feature work is reconstructed from implementation commit
509c51c and its five support-tool changelog entries. All five had code in
0.21.0; packaging was interrupted and deployment remained blocked. A second
review found workflow bugs beyond the original passing tests. Corrections
are in 7e5b8bb, native-window repaint fix 9afe3aa and compact layout fix
**2d1a128**, build **106**. The verified **0.21.1** packages supersede the
unpublished 0.21.0 candidate below. Linux, Windows, Android and the signed
manifest are in `dist/slipstream-station/clients/`. Later documentation
commits do not change that packaged build.

| First-pass feature | Implemented scope | Further work from the original ideas |
|---|---|---|
| Agent as a jump host | Select a discovered TCP service, create a local loopback tunnel, copy its endpoint or open standard HTTP/HTTPS locally | Serial console bridging and services needing hostname/redirect adaptation |
| Wake-on-LAN by peer | Send through the connected workstation's selected LAN adapter, with separate wake/control/scan permissions | Automatic peer selection and saved wake targets; physical sleeping-device confirmation |
| Network diagnostics | Gateway ICMP, configured/comparison DNS IPv4 answers and the host's station API | Path/MTU/captive-portal/Wi-Fi surveys and relay throughput tests |
| Support report | Review inventory/public IP, connection/retained network results and notes; explicitly choose a screenshot; export a unique local folder | Get Help capture before connecting, historical flight recording and automatic/AI ticket summaries |
| Plain-language narration | Send a manual status to the host; Dismiss / Stop support in full and compact Windows consoles | Automatic action narration and saved walkthroughs |

### Bugs corrected in the audit

- Closing Network Scan cancels pending one-shot requests and discards late
  tunnel replies, while established tunnels remain with the session. Link
  loss cancels scanner/report work and immediately clears scanner-owned
  tunnels even when the tool window is hidden. Closing a report cancels its
  remote requests while local exports finish in their own directory.
- Compact Windows status is tracked per session. Clearing/ending one message
  preserves another technician's message; Dismiss and Stop support act on
  the displayed session. The original compact implementation displayed text
  without these controls and left stale text after the session ended.
- Retained service command senders read the current service epoch when each
  command is queued. Previously a sender created before pipe connection could
  retain epoch zero indefinitely, silently losing code rotation and actions.
  Already-queued commands keep their original epoch for restart protection.
- DNS diagnostics validate the question class and return only addresses
  belonging to the queried name's CNAME chain. Unrelated answer records and
  wrong-class addresses no longer appear as the requested lookup's result;
  alias loops fail visibly.
- The native Linux smoke check found repeated blank 800×600 "egui window"
  windows above the scanner. Video callbacks repainted a detached-session
  viewport even while its session was docked. Those callbacks now repaint a
  child viewport only while the backend reports that it exists, including
  after reconnect. The regression checks both absent and live viewports.

### Verification and remaining work

The final workspace suite passed **138 tests, zero failed, five ignored**.
Linux/Windows all-target workspace and arm64 Android Clippy passed with
`-D warnings`. Regressions cover cancellation and discarded late tunnel
replies, retained/closed listeners, screenshot PNG pixels and failed-export
cleanup, independent wake permissions, service epochs and absent/live native
viewport repaints. All final desktop hashes, Ed25519 signatures and embedded
update keys were verified, with trust continuity from 0.20.0. Windows resources
identify 0.21.1 / 106 and include pinned Mesa 26.2.4. Portable Linux starts
with an isolated profile and meets the glibc 2.31 bound. The arm64 APK is
release mode, versionName 0.21.1 / versionCode 106, signed by the existing
Android certificate.

Native Linux fixtures verified reviewed report notes, separate text-only /
screenshot exports, and retention after closing/reopening the report. After
the repaint fix, repeated scanner use retained just the intended windows,
with no blank "egui window" and no scale-factor/window-creation log flood.
Closing during diagnostics then reopening displayed a cancellation notice
with form values retained; a second request completed normally with gateway,
DNS and verified local station API results. These use private fixture hosts
and a temporary local station. No production accounts were used.

Floating-session video kept updating after the repaint fix and docking
removed its native window without introducing a placeholder. The Windows
release under an isolated Wine prefix hosted a synthetic screen to the Linux
viewer. Its compact status displayed the message, Dismiss retained the live
link, and Stop support ended it with `retryable=false`; the separate Linux
fixture session remained alive. That smoke check also found footer overlap,
fixed by increasing the compact client height to 464 pixels. The final
packaged layout was checked at 500×464 with both controls fully visible
above the footer: [compact preview](screenshots/compact-support-0.21.1.png).
This exercises the direct
desktop host, not a physical Windows machine or the SYSTEM service bridge.

The station mount is still disconnected. No production deployment occurred.
Use the final 0.21.1 artifacts after verification, with clients first and the
signed manifest last; the 0.21.0 publication scripts below are superseded.
Physical Windows service/compact behavior, actual LAN wake, real Android and
macOS remain separate runtime checks. Clone detection is an additional 0.21.0
feature; its Windows imaging checks are unchanged by this support-tool audit.

Prepared continuation scripts: `python3 /tmp/slipstream-verify-0211.py`
(passed), `python3 /tmp/slipstream-publish-0211.py`, then
`python3 /tmp/slipstream-live-check-0211.py`. Publication/live scripts were
syntax-checked only; they expect the station's prior 0.20.0 / 98 / 98e84e0
manifest and back up/stage/check clients before publishing the manifest last.
They have not been run. PHP and the relay require no update.

Temporary evidence: `/tmp/slipstream-support-audit-final-tests.log`,
`/tmp/slipstream-support-audit-final-{linux,windows,android}.log`,
`/tmp/slipstream-0.21.1-build-final.log`,
`/tmp/slipstream-0.21.1-artifact-verification.log` and fixture directories
`/tmp/slipstream-native-support-tmlb90v5` / `...-1o_0aiyv`.
The later interruption stopped the fixture processes; cleanup confirmed no
owned fixture processes remained. Their logs, exports and screenshots are
retained. The final screenshot is also committed above.

## Historical release preparation: 0.21.0 (superseded)

Implementation: **509c51c**. Release: **1dc6836**, version **0.21.0**,
build **101**. Linux, Windows and Android packages are built and verified in
`dist/slipstream-station/clients/`. Publication is pending; the last verified
live release is 0.20.0 below. The initial packaging run was interrupted;
the resumed run completed with Zig and NASM on PATH. Later documentation
commits do not change the packaged build.

### Changes in 0.21.0

- Network Scan can connect to a selected discovered TCP service through the
  workstation, retain loopback tunnels with the session, and open standard
  HTTP/HTTPS endpoints locally. Existing port-forwarding permissions apply.
- Tools → Support report collects inventory/public IP, connection and retained
  scan/diagnostic results, and technician notes into a reviewed local report.
  A remote screenshot requires explicitly selecting its checkbox. Each export
  creates a unique folder; credentials, chat and raw logs are omitted.
- Network diagnostics check gateway ICMP, configured/comparison DNS resolvers
  and the host's station API. Checks are bounded and cancellable; results do
  not establish video-relay health. Wake-on-LAN uses the selected adapter's
  broadcast and strict MAC validation, with a separate disabled-by-default
  permission and a two-second request limit.
- A plain-language support status appears on the host with Dismiss / Stop
  support. Stop ends only that session without automatic recovery. Windows
  service hosts use the authenticated desktop bridge; the compact app shows
  the status too. Incoming events wake occluded/hidden host consoles.
- Desktop profiles bind their identity to locally hashed firmware evidence.
  Moving a bound image to different hardware generates a fresh identity and
  clears cached/public IDs while preserving station, unattended and service
  configuration. Concurrent starts agree on one identity; missing evidence
  retains it. Service updates preserve the machine profile's existing identity.
  **Run the updated master app/service before imaging** to record the baseline.
  Duplicate firmware IDs and images without a baseline limit detection.

### Verification for 0.21.0

- The completed feature test run passed **131 tests, zero failed, five
  ignored**. Coverage includes clone rekeying/preserved settings/concurrent
  startup, bounded firmware parsing, support permissions and tunnel bytes,
  report folder isolation, exact wake packets, and mismatched DNS replies.
- Linux/Windows all-target workspace Clippy and arm64 Android Clippy passed
  with `-D warnings`. Windows and Android checks were completed after resuming
  the interrupted release preparation. These are compile checks, not physical
  Windows/Android runtime checks.
- Prior isolated Xvfb/X11 fixtures retained report exports and screenshots
  of the support panel, network diagnostics and tunnel controls. A host log
  and viewer log confirm that Stop support ended the session without recovery.
  [Host status preview](screenshots/support-status-0.21.0.png) uses synthetic
  hosts. Real firmware-based Windows imaging/service behavior, LAN wake and
  physical-device UI checks remain open. This Linux user cannot read usable
  firmware identifiers; clone tests inject distinct evidence.
- Session protocol remains **6**; support operations append optional stream
  variants. Ordinary sessions remain compatible with older peers, while new
  optional requests require a sufficiently recent host.
- Packaged desktop hashes, Ed25519 signatures and embedded update keys match
  the existing release key, also checked against the 0.20.0 build log.
  The Windows file/product resources identify 0.21.0 / 101 and include the
  pinned Mesa 26.2.4 libraries. Portable Linux starts with isolated settings
  and requires glibc no newer than 2.31. The arm64 APK is release mode with
  versionName 0.21.0, versionCode 101 and the same valid signing certificate
  as the previous APK. Packaging completed with nonfatal linker messages
  about Zig's deprecated optimization option and missing MSVC debug symbols.
- No authenticated live 0.21.0 offer/download verification has occurred.
  An unauthenticated HTTPS API probe returned 403 as expected for a private
  station; it cannot establish the deployed version.

### Deployment blocker and continuation

The station mount at `/home/doug/WebServer` returned **Transport endpoint is
not connected** on resumption. A batch SSH check to its existing host also
failed authentication. No station files were changed. Reconnect the mount
before backing up and atomically publishing the new clients, with the signed
manifest last; then verify authenticated HTTPS offers and full downloads.
PHP and the relay do not need updating for these client features.

Prepared temporary scripts, in order after reconnecting the mount:
`python3 /tmp/slipstream-verify-021.py`,
`python3 /tmp/slipstream-publish-021.py`, then
`python3 /tmp/slipstream-live-check-021.py`. The first already passed locally.
The publisher requires the prior manifest to identify 0.20.0 / 98 / 98e84e0,
checks its signer and client hashes, backs up existing clients, stages and
checks replacements, and publishes the signed manifest last. Publication
and live-check scripts have been syntax-checked but not run. If temporary
files are gone, reconstruct this procedure from the release checklist and
the 0.20.0 deployment notes below.

Temporary evidence (not committed): `/tmp/slipstream-support-tests-final.log`,
`/tmp/slipstream-0.21.0-clippy-linux.log`,
`/tmp/slipstream-0.21.0-clippy-windows-resumed.log`,
`/tmp/slipstream-0.21.0-clippy-android-resumed.log`,
`/tmp/slipstream-0.21.0-build-resumed.log` and
`/tmp/slipstream-0.21.0-artifact-verification.log`.

## Published release: 0.20.0

Published **0.20.0, build 98, commit 98e84e0** at
https://relay.slipstreamremote.com. Implementation commit: **85bf92f**.
Later documentation commits do not change the deployed build.

## Session tabs and windows: 0.20.0

- A strip below the title bar switches attached workstation sessions and
  returns to Bridge. Drag within it to reorder; release outside it for a
  floating window. Floating windows have their own draggable tab, which can
  be dropped back into the main strip, including when no tabs remain there.
  All moves retain the same encrypted session and GPU stream identity.
- Config → Viewing → New links selects Tabs / Separate windows, applied to
  future connections after Save configuration. Existing profiles default to
  tabs and current sessions retain their layout. Android embeds its tabs and
  does not create desktop viewports. Pop out / Dock, tab context menus and
  Links window actions remain available. Closing ends only that link.
- The shared drag coordinator stores root drop geometry and queues placement
  actions; mutation follows UI lock release. Keyboard capture is released on
  selection/drag, keeping shortcuts away from the old workstation. Targets
  use physical pixels across viewport scales. Windows/X11 sample the pointer
  during a tab drag to restore movement/release lost after winit CursorLeft.
  The destination can also accept release locally. Hidden/minimized roots
  do not accept drops. Pure Wayland/macOS cross-window dragging still needs
  runtime confirmation; explicit Dock remains available.

## Verification for 0.20.0

- Workspace tests: **122 passed, zero failed, five ignored**. Three new
  headless UI tests exercise real pointer events through the strip, including
  reorder/tear-out/Escape, different source/destination scales and mouse-up
  delivered to the destination. Settings round-trip coverage checks the
  default and persistence of the window preference.
- Linux/Windows all-target workspace Clippy and arm64 Android Clippy passed
  with `-D warnings`. No physical Windows, macOS, real mixed-DPI monitor or
  new Android runtime check was made for this feature.
- Private Xvfb/X11 sessions confirmed two workstation tabs, switching,
  reordering, tear-out beyond the source window, drag-back into an empty main
  strip and closing only one link. Saving Separate windows through the GUI
  persisted the preference; the next connection opened its own native window
  while the other link remained. These tests used synthetic hosts and private
  settings, with no production accounts. Fixture processes were stopped.
- [Tab-strip preview](screenshots/session-tabs-0.20.0.png) uses synthetic hosts.
- Packaged desktop hashes/signatures and embedded trust keys matched the
  existing release key. Portable Linux started under an isolated profile and
  retained the glibc 2.31 bound. The arm64 release APK uses versionName 0.20.0,
  versionCode 98, release mode and the existing signing certificate.
- Live authenticated HTTPS offers report 0.20.0 / 98 / 98e84e0 with valid
  signatures. Full desktop downloads match signed prefixes and provisioning
  trailers. Android retains its app filename, APK MIME type, page filename
  hint and exact signed bytes. Session protocol remains 6.

## Deployment and rollback for 0.20.0

Only clients and the signed manifest changed on the station at
`/home/doug/WebServer/home/relay/public_html/`. Prior clients were backed up;
new clients were staged and checked, ownership/modes preserved, and atomically
renamed before publishing the manifest last. Copied hashes were checked again.
PHP remains at 0.18.0. No relay binary restart or GitHub push was needed.

Rollback (contains 0.19.1 build 95 clients/manifest):
`/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.19.1-build95-to-0.20.0-build98-20261019T233944Z`

Restore the client files atomically, then the saved manifest last. Prior
release evidence and rollback paths follow. On the real Windows VM, check
reorder/tear-out/dock-back with tabs and windows mixed, keyboard focus,
Escape cancellation, the new-link default after restarting, and per-monitor
DPI. Older service/permissions/tool-window confirmation tasks still apply.

## Earlier releases today: 0.19.0–0.19.1

Published **0.19.1, build 95, commit 491f201** at
https://relay.slipstreamremote.com. Feature implementation: **404086e**;
phone/full-screen identification fix: **bd6ff9a**. The intermediate 0.19.0
release was **a81e951**, build 93. Subsequent documentation commits do not
change the deployed client build.

### Changes in 0.19.0–0.19.1

- Full and compact Windows consoles show the service host's real one-time
  code with Copy and New actions, alongside unattended access on the same ID.
  The code comes from the existing authenticated, session-checked local pipe;
  it clears on pipe loss and is omitted from service stdout. Older services
  keep unattended access until updated through Config → Windows service →
  Update. Code rotation does not disconnect existing sessions or change the
  unattended password. The host already accepted both credentials; the GUI
  now exposes the service's code instead of hiding it.
- Desktop sessions have Pop out / Dock actions and Links has Open window /
  Focus window. Several workstation windows can remain visible concurrently.
  Closing a native session ends only that link, while Bridge opens the main
  console without ending it. Docking and recovery preserve the connection
  state, display/quality choices, chat and tools. Manual Reconnect uses the
  existing retained-session recovery path.
- Shared session state is held behind per-session locks. Native viewports own
  keyboard capture and queued shortcuts; focus changes release held input.
  Background tool repaints do not change the input owner. Deferred session,
  Files and Network windows are registered even when the console is hidden.
- Workstation names sit prominently in desktop title bars. Picture quality
  sits beside FPS/latency; measured available space decides button labels,
  with a Console menu at narrow widths and a screen-selection menu for
  multiple displays. Android/full-screen sessions retain the name in the
  toolbar or Console menu because their native title bar is absent.

### Verification for 0.19.x

- `cargo test --workspace --locked`: **119 passed, zero failed, five ignored**
  after the final patch. Added end-to-end coverage accepts one-time and
  permanent passwords simultaneously, rotates the code, rejects the old one
  and retains existing access. Service cursor coverage checks old snapshot
  compatibility and code changes. A two-session encrypted GUI fixture checks
  focused-window routing, held-key release, pending shortcuts, background
  tool repaints and docking back to the root viewport.
- Linux and Windows all-target workspace Clippy, plus arm64 Android Clippy,
  passed with `-D warnings` after the final patch. Windows uses cargo-xwin;
  Android uses cargo-ndk. Physical Windows/service and macOS runtime were
  not exercised by this pass.
- Isolated Xvfb/native X11 sessions displayed two live fixture workstations
  at once. Docking retained the link, closing one native window retained the
  other, and a killed/restarted fixture host reconnected in the same window
  with its three displays. Layouts were checked at 520, 900 and 1180 pixels.
  Files/Network opened and closed with the console unmapped; System Info
  closed without a retained black region. X11 unmapping is not a real window
  manager's minimize path. Audio was not verified in this pass.
- A [session preview](screenshots/session-0.19.0.png) shows the new desktop
  layout using a synthetic workstation and no production data.
- Final packaged desktop hashes, Ed25519 signatures and embedded update trust
  keys were checked against the previous deployment. Portable Linux starts
  with an isolated profile and requires glibc no newer than 2.31. Windows
  includes Mesa 26.2.4. The arm64 release APK has versionName 0.19.1,
  versionCode 95, no debuggable flag and the existing valid signing certificate.
- Live authenticated HTTPS offers report 0.19.1 / 95 / 491f201 with valid
  signatures. Full desktop downloads match signed prefixes and provisioning
  trailers. Android has the app filename, APK MIME type, matching page hint
  and exact signed bytes. Direct access to `clients/` stays denied; tests use
  the supported API and download handler. Session protocol remains 6.

### Deployment and rollback for 0.19.x

Station root: `/home/doug/WebServer/home/relay/public_html/`.
Only the Linux, Windows and Android clients and `clients/manifest.json` were
updated for 0.19.x. PHP remains at the 0.18.0 implementation below. Each client
was backed up, staged in its destination directory, hash checked and renamed
atomically with ownership/modes retained; the manifest was published last.
No relay executable replacement/restart, GitHub push or GitHub release occurred.
Signing keys remain outside the repository.

Final patch rollback (contains 0.19.0 build 93):
`/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.19.0-build93-to-0.19.1-build95-20261019T230203Z`

Feature rollback (contains 0.18.0 build 90):
`/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.18.0-build90-to-0.19.0-build93-20261019T225608Z`

Restore the listed client files atomically, then the saved manifest last.
The earlier 0.18.0 rollback below also retains the previous PHP implementation.

### Remaining checks from 0.19.x

On the Windows VM, update the installed service as well as the GUI; verify
Copy/New in full and compact consoles, use both password types on the same
ID, and confirm background access with the GUI closed. Exercise multiple
native session windows, keyboard/shortcut focus, docking, reconnecting and
independent close. Repeatedly close/reopen Files and System Info with the
console minimized/covered. Check the toolbar at narrow widths and Android /
full-screen identification on real devices. Existing Wayland consent and
reboot-token checks below remain open.

## Earlier release today: 0.18.0

Published **0.18.0, build 90, commit ba3161e** at
https://relay.slipstreamremote.com. Implementation commit: **4b9a5f0**.
Subsequent documentation commits do not change the deployed build.

### Changes

- Detached File Transfer, transfer receipts and Network Scan hide immediately
  on close. Their parent receives a repaint request that can run while
  occluded, allowing native-window cleanup without another main-window click.
  System Information discards its closing pass and redraws without the panel.
  This targets the user's Windows black-panel report; physical VM confirmation
  remains open.
- Wayland GUI and headless hosts prepare screen/control access on startup.
  Config offers Approve now and Reset / approve again. Local preparation and
  incoming sessions share one cached backend and save desktop restore tokens.
  The grant stays usable while the process runs in the tray. Reset refuses
  active sessions; session registration holds the backend lock until the
  viewer is registered, closing the reset/authentication race.
- Windows needs no portal consent. Config explains automatic screen access
  and the background service's login/UAC access rather than showing Reset.
  Android retains explicit Start sharing consent. macOS retains OS permission
  requirements, explained in Settings.
- Station download buttons include the same filename as the HTTP attachment,
  including the APK. Ordinary ASCII attachment headers omit the unnecessary
  extended filename parameter; non-ASCII names retain UTF-8 support. Signed
  APK downloads remain byte-for-byte copies, with the APK MIME type.

### Verification

- `cargo test --workspace --locked`: **116 passed, zero failed, five ignored**.
  New coverage prepares access concurrently, connects through the prepared
  backend, refuses reset with an active viewer, then forgets/reopens access
  after disconnect. Viewer-only/disabled hosts cannot prepare incoming access.
- Linux and Windows all-target workspace Clippy passed with `-D warnings`;
  Windows used `cargo xwin clippy`. Arm64 Android Clippy passed through
  `cargo ndk`. These checks include compilation, not physical-device runtime.
- Isolated PHP station checks passed: Android button filename, HTTP filename,
  MIME type and exact bytes, plus the existing groups/accounts checks.
  Changed PHP files passed lint. Production accounts were not test fixtures.
- All desktop package hashes and Ed25519 signatures verified, with the same
  embedded release trust key as the previous deployment. The portable Linux
  CLI starts under a private temporary profile and requires glibc no newer
  than 2.31. Windows includes Mesa 26.2.4 for the software-renderer fallback.
- The arm64-only release APK has versionName 0.18.0, versionCode 90, no
  debuggable flag and the same valid signing certificate as the prior APK.
- Live authenticated HTTPS update offers report 0.18.0 / 90 / ba3161e with
  valid signatures. Full Linux/Windows downloads match signed prefixes and
  carry valid provisioning trailers. The APK response has the app filename,
  correct MIME type and exact signed bytes; the live page includes its
  download filename hint. Direct access to `clients/` remains denied as
  intended; verification uses the API and `download.php`.

### Deployment and rollback

Station root: `/home/doug/WebServer/home/relay/public_html/`.

Updated `lib/slipstream.php`, `download.php`, `index.php` and the Linux,
Windows and Android files in `clients/`. Files were backed up, staged in
their destination directories, checked, and atomically renamed with existing
ownership/modes preserved. The shared PHP helper was published before its
callers; `clients/manifest.json` was published last. Copied files were checked
again after publication.

Rollback directory:
`/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.17.1-build87-to-0.18.0-build90-20261019T222231Z`

It contains the previous PHP files and 0.17.1 clients/manifest, plus a file
list. Restore the listed PHP/binary files atomically, then restore the old
manifest last. No relay-program replacement or restart was needed. Signing
keys remain outside the repository. No GitHub push or release was performed.

### Remaining checks

On the Windows VM, repeatedly open/close Files and System Information,
including with the main app covered/minimized, and confirm no black panel
remains and tools reopen normally. Confirm background service access using
the unattended password with the GUI closed.

On Wayland, approve locally at startup, connect later while in the tray,
change the screen with Reset / approve again, and check restart/reboot token
behavior on each supported desktop. Automated coverage uses a synthetic
backend; it does not establish real portal persistence. On a real Android
browser, confirm the saved APK name as well as the verified HTTP/page hints.
