# Development handoff — 2026-10-18

This records the day's work through **0.17.1 (build 87, commit a8e5e25)**,
using America/Los_Angeles dates. The latest clients are published at
https://relay.slipstreamremote.com. The release commits and implementation
commits are already in Git; this handoff is a subsequent documentation change.
A documentation commit increases the repository's commit count but does not
change the version/build of the clients already published.

See the [changelog](../CHANGELOG.md) for individual release changes,
[status](STATUS.md) for the ongoing handoff, and the
[audit report](AUDIT-2026-10-18.md) for findings and regression evidence.

## Release record

The day began with 0.7.1 as the previous release. Housekeeping removed a stray
object file, expanded cross-target development-build cleanup, and refreshed
status/issues before Android work. Sixteen releases followed:

| Release | Release commit | Main result |
| --- | --- | --- |
| 0.8.0 | a8bc7c0 | First running Android app; station APK download and release signing |
| 0.9.0 | d0aafcf | Handheld layout, touch gestures, zoom and stable soft keyboard |
| 0.10.0 | 60e126d | Sharp Balanced/Quality video and remote network scanning |
| 0.11.0 | 305549c | Device names, manual subnets, detached scanner, inventory, Windows chat and title bars |
| 0.11.1 | 641a71f | Integrated LCARS session rails, toolbar symbols and native rounded windows |
| 0.11.2 | b31ece0 | Visible side rail, continuous footer and outlined connection statistics |
| 0.11.3 | 5326ca7 | Aligned menus, clearer screen-blanking label and LCARS chat |
| 0.11.4 | cee430b | Modal elbow no longer cuts into Save and nearby controls |
| 0.12.0 | 2ef5faa | Desktop file drops, saved-device groups and improved device editor |
| 0.13.0 | bd8cc36 | Scanner HTTP/HTTPS actions open the remote browser |
| 0.14.0 | 7024bd5 | Public IP inventory and unified scanner title rail |
| 0.14.1 | 68a5e85 | Contrasting System Information Close action; subsequent focus/corner refinements |
| 0.15.0 | 4de42c0 | Automatic link recovery and detached File Transfer |
| 0.16.0 | e3a772c | Image/text previews and independent file-pane sorting |
| 0.17.0 | 9e4ca97 | Android hosting/control and detached transfer receipts |
| 0.17.1 | a8e5e25 | Deep-audit reliability, bounds and Android direct-link fixes |

Some refinements landed between release commits: the computer-name rounding
after 0.11.2, scanner focus and frame/control spacing after 0.14.1, and detached
tool repainting/vector file icons after 0.15.0. They are included in subsequent
packages and in the current 0.17.1 deployment.

## Desktop video and connection recovery

The moving-picture blur reported from Linux into the Windows Server VM came
from adaptive resolution being applied to every quality preset. Balanced and
Quality now preserve native picture resolution during movement. Speed and Low
bandwidth retain adaptive sizing; switching presets immediately resets sizing
state. The performance tooltip exposes encoded dimensions for diagnosis.
High FPS alone does not establish that the picture is full resolution.
The sharpness change still needs confirmation on the actual Windows VM.

Interrupted desktop links now detect an unresponsive host through bounded
performance probes, independently of video activity. Recovery retries fresh
lookups with backoff for up to two minutes. Host updates announce a restart
before replacement. Reconnection restores the same tab, chat, selected display,
quality and tool windows; it releases input and requires a click to resume
control. End Link, Cancel reconnect, explicit session endings and rejected
passwords stop recovery. Transfers fail visibly and forwards close rather than
being replayed. An abruptly killed/restarted Linux host was checked; an actual
Windows service update remains a separate runtime check.

## Network scanner and system information

- Tools → Network scan runs from the authenticated remote host, initially
  using its adapter/address/mask/gateway. A manual IPv4 subnet can target
  another routed network or VLAN; unreachable targets can simply return no
  results. Port lists/ranges, host/probe/concurrency bounds, Stop, filtering,
  copy/export, table view and logical topology are supported.
- Names come from DNS, NetBIOS and unicast mDNS on a best-effort basis, with
  lookup sources shown. Missing names do not hold up port results. MACs are
  shown when available; neighbor-cache-only devices are marked unconfirmed.
  The gateway appears once, with its scan results attached to that node.
  Topology represents subnet membership rather than measured physical links.
- The scanner owns a movable/resizable/maximizable desktop window beyond the
  main session's resolution. Closing it stops outstanding work. Contrasting
  view selectors and Close controls replace the brown/purple combination.
- Highlighted port 80/443 actions open the numeric HTTP/HTTPS URL in the
  remote user's browser and report launch errors. Windows service hosts use
  the signed-in user's browser association. The host checks both scan and
  control permission. Actual service-host browser launching needs VM checks.
- System Information adds present peripherals, maker/model details where
  reported, printer queues/scanners, network-drive mappings and shared folders.
  Windows service collection includes saved mappings in loaded user profiles;
  Linux includes USB/CUPS inventory and network mounts. Availability is labeled,
  URI credentials are omitted, and inventory is cached for 30 seconds.
- Public IP is a separate host-side HTTPS lookup through ipify, preferring
  IPv4 with dual-stack fallback. Copy IP and Copy all include it. A shared
  one-minute cache, five-second refresh gate and failure backoff avoid repeated
  requests; unavailable internet does not prevent local inventory from loading.

Protocol advanced to **6** for the initial scanner. Later optional streams
extend protocol 6 without another version change. Update both endpoints for
new features; older protocol-6 hosts retain their existing capabilities.

## LCARS frame, menus, chat and saved devices

Linux and Windows use custom rounded title bars with draggable areas,
double-click maximize/restore, border resizing and symbolic controls. Delta
symbols indicate minimize/maximize, overlapping panels indicate restore, and
a cross closes. Native corner regions/masks cover Windows and X11; Wayland
uses transparency. Maximized/full-screen windows keep rectangular bounds.

The title and remote controls form the LCARS frame, with a slimmer visible
left rail, a curved LINK elbow easing into the shifted toolbar, vector symbols,
rounded rectangular segments, and a blue footer below the remote taskbar.
The right rail leaves a gap below End Link; the dark FPS/latency display has
its own outline. Computer-name corners, outer/lower-left curves, top-right
Close placement, control spacing, stray strips and corner clipping were refined.
Input mapping uses the same inset video area as rendering.

Dropdowns align below their toolbar segments and use blue selections. Privacy
mode is labeled **Blank remote screen**, aligned with the other Tools entries,
with platform/permission explanations. It covers the remote screen during work;
Windows capture exclusion requires Windows 10 2004+, and blocking local input
requires the elevated service. Real-machine privacy checks are still pending.

Windows service chat again reaches the signed-in user's GUI through a
session-checked local named pipe, launching it if needed and recovering bounded
history. Chat is kept out of disk/public service logs. Incoming chat uses a
lavender title rail, contrasting peach channel band, swept side rail, blue
footer and bottom composer. The white native strip above Windows chat was
removed. Scanner background repaints no longer toggle remote control off;
focused tools still release remote input and keep tool keystrokes local.

Saved devices support collapsible private/shared groups synced by the station,
including preservation when older clients edit entries. Add this device fills
the local ID/name, including the Windows service identity when applicable.
Notes wrap and grow, Group/Notes have more space, long forms scroll, Close is
peach and Save green. The modal rail cutout no longer erases Save's lower-left
corner or the final DNS rows in System Information. Account permissions and
encrypted saved passwords remain enforced.

## File workflows

Linux desktop file/folder drops use native X11/Xwayland drop coordinates.
For Windows targets, dropping over Explorer resolves that folder; otherwise
the signed-in user's Desktop is used, including service sessions and redirected
Desktops. Destination resolution happens once per batch. Nested/empty folders
survive, and drops outside the video or onto local controls do not send files.
Windows Explorer/service destination selection still needs real-VM confirmation.

File Transfer has a detached LCARS window, vector folder/file icons, independent
repainting and desktop drops into its remote pane. Closing it leaves transfers
running. Reconnects retain history and rebind browsing to the new session.
Backends without native secondary windows embed the tool.

Each pane independently sorts by Name or Modified, reverses direction on a
second click and keeps folders first. Sort preferences survive navigation and
reconnects without changing selected-file identity. Visible image rows receive
thumbnails and larger hover previews; text/source files get passive text
previews. Host-generated previews are bounded to 128-pixel RGBA or 8 KB text,
with two outstanding requests per pane and 128 cache entries. Navigation and
refresh invalidate stale results. Unsupported/oversized/corrupt files and older
hosts keep generic icons. PDF, video and office-document rendering is pending.

Transfers automatically open an independent LCARS receipt panel with filenames,
resolved destination, bytes, progress and preparing/sending/receiving/confirming
states. Complete requires the remote save acknowledgement. Failed, cancelled
and complete results stay visible until cleared; late replies cannot overwrite
terminal failure/cancellation. Closing hides the panel without cancelling work,
new work reopens it, and Cancel/Clear finished/Files expose the expected actions.

## Android progression and current operation

The day started with the first Android build: Java GameActivity/JNI integration,
private identity/settings storage, keyboard/inset handling, station links,
remote audio playback and station-hosted APK download. Phone layout followed:
bottom navigation, stacked forms, compact Console menu, tap, long-press,
one-finger drag, pinch zoom and two-finger pan/scroll. The soft keyboard stopped
flickering when its animation briefly obscured a field.

The initial viewer-only restriction explains the missing host ID in 0.9.x;
0.17.0 replaces that limitation with a persistent ID/password and a default
manufacturer/model name. To host now:

1. Open the Android app and choose Start sharing; approve Android's capture
   prompt. MediaProjection runs in a foreground service with a Stop action.
2. Enable Slipstream's optional accessibility service for PC control. Sideloaded
   apps may need App info → Allow restricted settings first. Without that
   service, the authenticated connection is view-only.
3. Connect from the PC using the phone's ID/password. Taps, held-touch drags,
   long presses, wheel swipes, basic typing/editing and Keys → Back/Home/Recents
   are supported. Input requires active full-display capture and a live viewer.
4. Stop sharing in the app/notification to revoke capture and disconnect viewers.
   A new sharing session requires fresh approval and retains the same device ID.

Capture uses reusable bounded RGBA buffers and software H.264, limited to a
1280-pixel long edge and 30 fps. Rotation resizes the existing display rather
than reusing a consent token. Capture survives backgrounding, late viewers get
the latest still image, and revoked cached capture cannot be reused.

The audit reproduced a cold direct-link stall after authentication/first large
send; relay worked. Disabling UDP segmentation offload on Android passed three
fresh direct starts. Android now uses individual datagrams by default; desktop
hosts can use `SLIPSTREAM_DISABLE_GSO=1` for the same compatibility workaround.
The final pending still frame is also retained by the capture rate limiter.

Android host audio, clipboard, files, hardware encoding and self-update remain
pending. Unattended screen approval is not supported. Viewer keyboard polish,
real phone/tablet testing and older Android runtime checks remain on issue 09.
Station APKs retain the existing Android signing certificate and are served
unchanged; they are not offered as desktop-style self-updates.

## Audit and verification

The [audit report](AUDIT-2026-10-18.md) records the detailed findings. Fixes
cover unique private transfer staging/no-overwrite publication, exact stream
lengths and cleanup, regular-file validation, UNC paths, symlink cycles, owned
and bounded file/tunnel tasks, held-input release, video failure propagation,
interruptible encoder shutdown and a two-frame decoder queue. Other fixes
cover atomic identity/settings writes, asynchronous/stale-safe file listings,
incremental bounded updater hashing, bounded audio waits, frame/repaint locking,
checked foreign-buffer arithmetic and Rust-server proxy/timestamp handling.

Final verification for 0.17.1:

- `cargo test --workspace --locked`: **115 passed, zero failed, five opt-in
  tests ignored**. Regressions include concurrent uploads/publication, different
  extension downloads, incomplete/reset/oversized payloads, symlink cycles,
  held modifiers on disconnect, video stream failure, IPv6/tunnel teardown,
  settings races, updater bodies, stale folder results and buffer bounds.
- Linux and Windows Clippy passed with `--workspace --all-targets --locked`
  and `-D warnings`; arm64 Android Clippy also passed. The Windows check
  repaired an outdated cursor-test call. These are compile/static checks,
  not claims of equivalent physical-device runtime coverage.
- Private native X11 checks verified exact dropped bytes, visible progress,
  cancellation/error receipts and retention, independent file sorting/previews,
  and selected-file Send/Receive bytes after sorting. Final audit checks also
  opened Files from its receipt while the parent was hidden and navigated both
  asynchronous panes. Hiding/unmapping was checked; this was not a window-manager
  minimize test. Earlier scanner/menu/frame/chat checks are recorded in status.
- Android 16 emulator checks covered visible/persistent ID, view-only denial,
  PC-decoded video, taps/navigation/typing/backspace/held drag, background capture,
  rotation, notification revocation, idle revoke/regrant and the cold direct/relay
  investigation. No physical phone was used for these host checks.
- Private profiles and fixtures isolated these tests from production settings.
  Owned test servers, GUI helpers and emulator were stopped; emulator files and
  changed settings were restored, temporary root access reverted and the emulator
  closed without saving the test snapshot.

## Deployment and rollback

Current mounted deployment:

- Station: `https://relay.slipstreamremote.com`
- Client directory: `/home/doug/WebServer/home/relay/public_html/clients/`
- Manifest: **0.17.1 / build 87 / a8e5e25**
- Files: `slipstream-linux-x86_64`, `slipstream-windows-x86_64.exe`,
  `slipstream-android.apk`, then `manifest.json` published last.
- Prior package backup:
  `/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.17.0-build85-to-0.17.1-build87-20261019T031836Z`
  (the UTC directory timestamp is October 19; the release was October 18 locally).

Release checks verified desktop checksums, Ed25519 signatures and embedded trust
keys, plus Android release mode, arm64-only packaging, versionCode 87,
versionName 0.17.1 and the existing signing certificate. Live HTTPS signed update
offers and full Linux/Windows downloads matched, including valid provisioning
trailers; the APK download matched the signed artifact exactly. Binaries were
published before the manifest. Signing material stays outside the repository.

Earlier saved-device work updated the two account API PHP files for groups.
Neither 0.17.0 nor 0.17.1 required a live PHP/station-program change. The standalone
Rust server fixes are in source/package; the live PHP station remains distinct.
No Git push or GitHub release was performed. No package rebuild is needed for
this documentation-only handoff.

## Next checks and remaining limits

1. On the real Windows Server VM, confirm sharp Balanced/Quality motion, service
   update/reconnect, incoming chat, inventory/mappings, Explorer/Desktop drop
   destinations, scanner browser launching and LCARS native corners/rails.
2. On a physical Android phone, confirm ID persistence, accessibility setup,
   capture approval/control, direct and relayed cold starts, rotation and Stop.
   Exercise older supported Android versions as well as Android 16.
3. Continue existing real-machine checks: UAC/login screen/privacy mode, audio
   between machines, clipboard files, Debian portability and macOS runtime.
4. Address remaining engineering work: hardware video, Android host integrations,
   resumable transfers, GitHub/CI and platform signing/notarization.

Simultaneous controllers still share backend input ownership; ReleaseAll is
global. A permanently stuck audio driver can retain a detached worker until
it returns or the process exits. Abrupt process termination can leave hidden
staging data, although handled failures/cancellation remove it. The audit does
not replace fuzzing, load testing or dependency-advisory review. These limits and
the physical-device gaps remain open rather than being marked verified.
