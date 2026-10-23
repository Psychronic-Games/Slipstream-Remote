# Slipstream Remote — Architecture

Slipstream is a free, MIT-licensed remote desktop app: TeamViewer-style
"give me your ID and password" access, but fast, self-hostable, and open.

## Components

```
┌──────────────┐     directory (HTTPS/JSON)    ┌────────────────────┐
│ Host  (PC A) │ ─── signed heartbeats ──────▶ │  server            │
└──────┬───────┘                               │  • ID directory    │
       │  ▲                                    │  • relay (optional)│
       │  │ end-to-end encrypted QUIC          └─────────▲──────────┘
       │  │ (direct via hole punching,                   │ lookup ID
       │  │  relay fallback)                             │
       ▼  │                                    ┌─────────┴──────────┐
       └──┴──────────────────────────────────▶ │ Viewer (PC B)      │
                                               └────────────────────┘
```

| Crate | Purpose |
|---|---|
| `crates/slipstream-core` | Wire protocol, framing, password auth (SPAKE2), directory client, config, provisioning, clock correction, iroh endpoint setup |
| `crates/slipstream-codec` | Video encoder/decoder abstraction (OpenH264 today; hardware encoders later) |
| `crates/slipstream-audio` | Remote sound: vendored libopus (built with `cc`), capture (WASAPI loopback, PulseAudio monitor) and playback with a jitter buffer |
| `crates/slipstream-platform` | Per-OS screen capture, input injection, clipboard |
| `crates/slipstream-session` | The long-lived `Client`: directory heartbeat, accepting and opening sessions, host & viewer session logic, file transfer, self-update |
| `apps/slipstream` | The desktop app (egui, sci-fi theme) plus a headless CLI |
| `apps/slipstream-server` | ID directory + embedded iroh relay, one binary on one HTTP port |
| `server/station` | The **station**: a self-contained PHP web folder with the directory API, a download page serving preconfigured apps, an admin page, and a supervisor that runs the relay program and routes `/relay` to it via `.htaccess` |

## Networking

* **Transport:** [iroh](https://iroh.computer) — QUIC with NAT hole punching
  and relay fallback. Every device has an Ed25519 keypair; its public key is
  its *EndpointId*. All traffic is TLS 1.3 encrypted end to end; relays only
  forward ciphertext.
* **Directory:** JSON over HTTPS (`POST {server}/api/`), implemented both in
  PHP and in the server binary. Hosts send a heartbeat every 30 s, signed with
  their Ed25519 endpoint key; the server assigns each key a stable 9-digit ID
  and answers `lookup(id) → EndpointAddr` while the host is fresh (75 s).
  Clients configure only a domain, so servers can move without breaking them;
  the directory also tells clients which relays to use.
* **Relay:** iroh's relay is a WebSocket at `/relay`, so it can live behind
  Apache/Nginx on the same domain as the PHP directory, or inside the server
  binary. The PHP station starts it (`slipstream-server --relay-only`) on a
  private loopback port and forwards `/relay` with an `.htaccess`
  `RewriteRule … ws://127.0.0.1:PORT/relay [P]`. It has to be `ws://`:
  `http://` drops the `Upgrade` header. The optional backup is Slipstream's
  own public relay, allowed only when both client and station permit it.
  No n0-operated default relay map is used.
* **Wrong clocks:** a computer whose clock is off by weeks would reject every
  TLS certificate as "expired" or "not yet valid". All TLS verification
  (directory and relays) goes through `clock::ClockedVerifier`. It checks
  certificates against *corrected* time: system time plus an offset learned
  from the `Date` header of every station response, and never earlier than
  the build time. On a time error the client reads the station's `Date`
  once without verifying, and retries.

## Stations and provisioning

Raw downloads use `https://public.slipstreamremote.com`. Configured private
stations remain the default, with a Public tab above the ID. Each registry
allocates its own numeric IDs. The public listener uses a domain-separated
key derived from the persistent identity; it never advertises private relay
credentials. Both listeners share the host backend, passwords, permissions,
authentication throttle and session state. Public registration has independent
status and cached ID. Test clients opt in explicitly with an isolated station,
so default fixtures never contact the production public service. Connections,
recents and reconnecting tabs retain their station choice; Windows gets the
service's public status over its authenticated local pipe.

Each organisation can run its own station. A private station requires its access key on every directory request, and its
relay admits only clients presenting the key (iroh passes it as
`?token=` on the relay URL). Apps learn the station's address, key and name
from their download (`slipstream_core::provision`):

* **Unsigned builds** carry them: the station appends a trailer to the
  executable (`[JSON][u32 length]["SLIPCFG1"]`).
* **Signed builds and the Mac app** must stay byte-for-byte as released (a
  changed file breaks the signature and is "uncommon" to every browser), so
  they get a **link** instead: the station address plus a setup code. Codes
  are an HMAC of the access key and the day, valid 30 days, so the station
  stores nothing and renewing the key cancels them. The API's `link` op
  trades a code for the config. A link arrives as the download URL
  (`download.php?…&c=CODE`), which browsers record on the file (Windows
  `Zone.Identifier`, Linux `user.xdg.origin.url`, macOS
  `kMDItemWhereFroms`); as a `slipstream://link?s=…&c=…` URL from the
  download page's button (registered per user on Windows and Linux; a
  second launch hands it to the running app); or typed in.

## Authentication

Remote-session authentication does not trust the directory server. A viewer
types an ID and remote-session password; that password is proved through
SPAKE2 without sending it to the host or directory:

1. Viewer → Host: `Hello` with protocol version and viewer name.
2. Host → Viewer: `AuthChallenge` with one SPAKE2 side-A message per accepted
   password (temporary one-time password and, optionally, the permanent
   unattended password), bound to the host and viewer EndpointIds.
3. Viewer → Host: `AuthResponse` with a SPAKE2 side-B message and a
   domain-separated key confirmation for each challenge, using the host
   EndpointId resolved by the viewer and its own EndpointId.
4. Host → Viewer: `AuthOk` + host confirmation (mutual authentication).

Registrations are signed, so nobody else can claim or move a device's ID.
A malicious directory that substitutes its own key ends up with mismatched
identities in the SPAKE2 transcript, so the keys never agree. Offline
dictionary attacks are impossible (PAKE); online guessing is rate limited by
the host. After three wrong passwords each attempt waits longer (2 s
doubling, up to 5 min). The host checks one password at a time and keeps
the throttle locked until the attempt is counted, so opening many
connections at once gains no extra guesses. A viewer that doesn't answer
the challenge within 10 s is dropped.

## Session protocol (ALPN `slipstream/remote/1`, `PROTOCOL_VERSION` 6)

* **Control stream** (bi-directional, length-prefixed `postcard`): auth,
  session info (displays and capabilities), input events, frame acks,
  clipboard, chat, display/quality selection.
* **Video stream** (host → viewer, uni-directional): encoded H.264 frames.
* **Sound** (host → viewer, QUIC datagrams, after `SetAudio(true)`):
  `[0xA1][u16 sequence][Opus packet]`, 20 ms of 48 kHz stereo each at
  96 kbit/s. A lost datagram is concealed by Opus rather than resent; the
  viewer keeps ~60 ms buffered and drops audio beyond 200 ms, so delay
  can't build up. Capture is WASAPI loopback on Windows and the PulseAudio
  monitor on Linux (dlopen, no link dependency); playback is WASAPI /
  CoreAudio (cpal) or PulseAudio (`crates/slipstream-audio`).
* **File streams:** every later bi-stream starts with a `StreamOpen` request
  (`List`, `Places`, `Download`, `Upload`, `MakeDir`) and gets a
  `StreamReply`. An upload names a target: the default folder, a folder
  path, `UnderCursor`, which on Windows asks Explorer which folder window
  is under the pointer (used for drag and drop), or `Clipboard { batch }`.
* **Copied files:** the clipboard holds text or a file list. A host's
  file list goes to the viewer as a summary (`ClipboardData::Files`: names,
  total size and count) and the viewer downloads it, if small enough, into
  a temporary paste folder that it puts on its own clipboard. The other
  way, Ctrl+V in the viewer uploads the files with
  `UploadTarget::Clipboard { batch }`, then `StreamOpen::ClipboardFiles {
  batch }` puts that batch's folder contents on the host's clipboard
  before the paste keystroke is sent. Paste folders older than a day are
  removed.
* **Tunnels:** `StreamOpen::Tunnel { host, port }` asks the host to
  connect to `host:port` (10 s timeout); after `Ready` the stream carries
  that TCP connection's bytes both ways, half-closes included. The viewer
  listens on 127.0.0.1 and opens one stream per accepted connection
  (`crates/slipstream-session/src/tunnel.rs`). Hosts refuse unless
  *Port forwarding* is allowed.
* **Privacy mode** (`ViewerMsg::SetPrivacy`): on Windows the host shows a
  topmost, click-through black window excluded from capture
  (`WDA_EXCLUDEFROMCAPTURE`, build 19041 and later), and the input thread
  calls `BlockInput`, which still lets its own injected input through. The
  host turns it off when the session ends however it ends.
* **Capabilities** tell the viewer what the host allows (input, clipboard,
  files, chat, sound, privacy mode, tunnels) and whether its video already
  shows the mouse pointer (`cursor_in_video`). The viewer hides its own
  pointer only in that case.

* **Technician accounts** (station API, not the session protocol):
  `login` returns a token, the station's config (access key included) and
  the account's vault salt; `account`/`device_*`/`shared_*` take the token.
  These run before the access-key check, so an app can sign in to a
  station it didn't come from. Saved passwords are sealed client-side
  (`crates/slipstream-core/src/vault.rs`). The app looks a device up on
  the station it's saved on and connects as usual
  (`crates/slipstream-session/src/accounts.rs`). Each saved device carries
  a group string (empty means Ungrouped), defaulting to empty when reading
  older JSON/TOML. PHP preserves the existing group when an older client
  omits it and accepts an explicit empty string to clear it. Group names
  are trimmed and limited to 64 characters. The editor offers existing
  groups and new names, with separate collapsible private/shared lists.
  Add this device uses this station's host ID and computer name (the service
  ID on Windows); it preserves an existing entry's details. Multiline notes
  grow inside a bounded scrolling form with the Save controls outside it.
* **Desktop file drops:** Linux windows use X11/Xwayland where available,
  because winit 0.30 emits no native Wayland drop events. During a drag the
  viewer samples the X11 pointer and translates it into its viewport; normal
  mouse motion is grabbed by the source. A point on the fitted/zoomed remote
  picture becomes a physical coordinate on the selected host display.
  Appended `DropTarget` stream/reply variants resolve the destination once
  before upload planning, independent of the input stream. Windows queries
  the Explorer window at that point or the user's known Desktop folder;
  service hosts query the user in their own session, never SYSTEM's Desktop.
  Explorer lookup runs with the user's token, bounded time/output and limited
  handle inheritance. Failures are reported rather than silently writing into
  another folder. Upload plans retain empty directories and stop symlink loops.
  Native Wayland without Xwayland cannot receive desktop drops with this
  window library; remote capture/input still uses the Wayland portal.
* **Network web actions:** `StreamOpen::OpenNetworkWeb { address, port }`
  is appended to protocol 6 and answers `Done` or `Error`. The host requires
  network scanning and remote input, constructs only `http://IPv4/` (80) or
  `https://IPv4/` (443), and rejects unspecified/multicast/broadcast addresses.
  Scan names and arbitrary URLs/commands never enter the launcher. The GUI
  uses buttons rather than local hyperlinks and reports the launch request
  outcome; it does not claim the page loaded or passed TLS validation.
  The shared desktop shell module selects the signed-in user's token in the
  Windows service's controlled session. A bounded user PowerShell process
  uses that user's URL association; normal Windows hosts use ShellExecute.
  Linux/macOS hosts use their desktop URL opener. Older hosts fail just this
  optional stream and show an update hint; other scan functions still work.
* **System information:** `StreamOpen::SystemInfo` → `StreamReply::SystemInfo`,
  titled sections of (label, value) rows already worded on the host
  (`crates/slipstream-platform/src/machine.rs`), so new facts need no
  protocol change. Public IP uses a separate optional `PublicIp { refresh }`
  stream/reply so internet lookup cannot hold up inventory. The host's HTTPS
  request goes to [ipify](https://www.ipify.org/), preferring its IPv4 endpoint
  with dual-stack fallback; no station keys, device IDs or account data are
  included. Replies are limited to 64 bytes and validated as public numeric
  addresses. Each provider has a four-second timeout. A per-process cache
  serializes concurrent lookups: success lasts 60 seconds, failure 15 seconds,
  and manual Refresh has a five-second minimum interval. The viewer requests
  while System Information is open, shows status/Copy IP above the inventory,
  and includes the value in Copy all. VPN/proxy/NAT egress may be what is seen.

`postcard` has no optional fields, so any change to a message bumps
`PROTOCOL_VERSION`. The exception is a new variant appended to
`StreamOpen`/`StreamReply`: older peers fail only that one stream. Mismatched peers refuse each other with a "please
update" message.

### Network discovery

`StreamOpen::NetworkInterfaces` returns structured IPv4 adapter addresses,
prefixes, MACs and default gateways, collected with the same platform code
as System information. `StreamOpen::NetworkScan` opens a streamed survey
(`Started`, `Device`, `Progress`, `Finished`). The host validates the range
using its current connected source adapter; manually entered routed subnets
and VLANs are allowed. Scanning has its own host permission
and requires session authentication. A scan accepts up to 4096 addresses,
1024 TCP ports, and 262144 total probes, with one scan per session and 96
TCP/ICMP probes in flight across viewers. TCP sockets bind to the selected
adapter address. Probe timeout is 400 ms; native Windows ICMP waits 500 ms.
Linux/macOS optionally invoke system ping. Neighbor entries supplement
positive TCP/ICMP responses, explicitly marked unconfirmed when cached only.

The viewer dropping the scan stops its response stream; the host cancels
its async jobs and subprocesses. Native Windows ICMP calls already running
finish within their 500 ms timeout. Session disconnection also cancels the
request. The graph represents the reported gateway and subnet membership,
not detected physical links. UI: `gui/network.rs`; host and viewer transport:
`slipstream-session/src/network.rs`; data and validation: `slipstream-core/src/network.rs`.

`StreamOpen::NetworkNames` / `StreamReply::NetworkNames` are appended optional
variants (protocol 6 remains unchanged), with batches of at most 16 discovered
addresses. Older hosts reject only this auxiliary stream. Lookups race the
remote system's configured DNS resolvers against unicast mDNS and NetBIOS
node status, with 850 ms UDP timeouts and at most 16 device lookups globally.
No public fallback DNS or broadcast sweep is used. DNS compression is parsed
by `simple-dns`; NetBIOS node-status parsing bounds each record and ignores
group names. References: [DNS](https://www.rfc-editor.org/rfc/rfc1035.html),
[NetBIOS](https://www.rfc-editor.org/rfc/rfc1002.html).
The scanner owns a deferred native viewport and releases remote keyboard
capture whenever its input receives focus. Its state belongs to the session;
closing the tool or session cancels its tasks.

### Link recovery and detached tools

`viewer.rs` distinguishes interrupted connections from deliberate host/viewer
session ends. Existing optional `StreamOpen::Pace` requests double as bounded
health checks: a six-second deadline and three-second interval detect silent
host death without treating a low frame rate or static desktop as a lost link.
Hosts declining the optional probe retain normal transport-close detection.
Before installing an update, a hosting client sends the existing Bye message
with the reserved restart reason and closes with restart code 1. Either
signal requests recovery; deliberate endings still use code 0. No wire
variants change; protocol remains 6.

`Client::reconnect` in `reconnect.rs` performs fresh station/account lookups
for the same device, retrying for at most two minutes with 1/2/4/8-second gaps
and a twenty-second bound around each complete lookup/connect/auth attempt.
Authentication refusals are typed terminal errors. Dropping the future stops
retries. Each GUI tab owns its recovery task, so End Link or closing the tab
aborts it; Cancel reconnect leaves the manual reconnect action available.
Recovery runs during the app logic tick, including while minimized.

The tab keeps its last picture under an inactive reconnecting overlay, and
preserves chat, display, quality and window identities. On success it replaces
transport, frame and event handles, clears cached video textures, reloads
system details, and rebinds file/scanner state. Held input is released and
never replayed; taking control requires another click. Transfers are marked
interrupted and port forwards close. Privacy mode resets with the new session.

File Transfer, like Network Scan, owns a deferred native viewport per tab;
Android uses egui's embedded fallback. `FileManager` is shared behind a mutex
and retains job history when rebinding. Tools read live transport status
inside their own viewport callbacks, including across minimized-parent
recovery. Closing the tool only hides it; tab
cleanup cancels active jobs. Async work repaints its captured viewport and
the root toolbar, so progress/listings do not depend on root-window rendering.
Drop hit testing includes the viewport ID as well as the pane rect/layer;
Linux's grabbed-pointer sampling also uses the event's own viewport and scale.

Listings run on bounded blocking workers with generation-tagged replies;
cancelled or stale navigation cannot replace a newer folder. Incoming files
use unique private staging, require the advertised bytes and clean stream end,
and publish without replacing existing files. Publication retries name races.
Request task sets belong to their session, and active tunnel connections belong
to their listener. Closing either owner cancels its children.

### File preview and sort state

Each file pane owns its sort choice, cached display-index order and preview
cache. Sorting never mutates a listing: selected rows and shift anchors keep
original entry indices so upload/download actions cannot change targets.
Folders sort before files, unknown timestamps follow known ones, and names
break equal-date ties. New Modified sorting defaults to newest first.

Optional appended `StreamOpen::FilePreview` / `StreamReply::FilePreview`
variants preserve protocol-6 interoperability. Hosts require `FilePolicy.browse`;
unsupported/unreadable items return no preview. Older-host RPC failures disable
further preview attempts until reconnect, without affecting file browsing.
A preview is either at most 128 × 128 RGBA pixels or 8192 UTF-8 bytes, validated
before texture allocation. Raster files are limited to 32 MiB, 8192 pixels
per dimension, 16 million pixels total and a 96 MiB decoder allocation limit.
EXIF orientation is applied after resizing. Text is truncated safely at UTF-8
boundaries, rejects binary control characters, and is never interpreted as HTML,
SVG or executable code. PDFs, office documents and videos use generic icons.

Only visible rows request previews: at most two tasks per pane, low-priority
QUIC streams, a twelve-second remote request deadline and a 128-entry LRU
texture/text cache per pane. All local/host decoders share a two-worker process
semaphore; its permit stays with a blocking job after cancellation. Refresh
and navigation advance a generation, abort old requests and drop textures;
late results cannot populate another folder's cache. Reconnect replaces the
remote cache/transport while preserving its sort choice.

### Windows service chat and device inventory

The SYSTEM helper owns incoming sessions; the user GUI has an ephemeral
viewer endpoint. `service_chat.rs` bridges their chat over a local-only named
pipe per console session. SYSTEM/admins own pipe instances; interactive users
receive data rights, excluding create-instance. The server checks the client's
session ID. Before commands, the GUI verifies a fresh nonce signature against
the service's public identity in the protected ProgramData public directory.
Length-framed JSON is bounded; epoch-stamped commands prevent replies to
sessions from a prior helper lifetime. Snapshot history is bounded to 128
messages and never persisted. On incoming chat without a GUI connection,
WTSQueryUserToken/CreateProcessAsUser launch the GUI using the signed-in
user's environment, never the helper's SYSTEM identity/profile.

`machine_inventory.rs` adds 30-second cached inventory to System Information's
existing rows, requiring no protocol change. Windows uses SetupAPI for present
peripherals, installed printer registry data, loaded users' drive/printer
mappings (including from the SYSTEM host) and LanmanServer shares. Saved
mappings and installed queues do not imply current reachability. Linux uses
USB sysfs, readable CUPS configuration and network mountinfo; macOS collects
system_profiler and mount output. Device-URI credentials are omitted.

### Latency control

The viewer acks every decoded frame. The host only encodes a new frame when
fewer than *N* (default 2) frames are unacknowledged. *N* grows with the
link's base round-trip time (the minimum seen, not the smoothed one, which
includes queueing). Frames are skipped *before* encoding, never after, so
the H.264 reference chain is never broken and stale frames never pile up in
buffers — the picture is always as fresh as the network allows.
The decode queue also holds at most two pending frames. Encoder sends observe
shutdown while backpressured, and the session observes video-writer failures.
Frame acknowledgements cannot advance past the last issued sequence.

* **Bitrate** adapts AIMD-style when frame round trips stretch. Changes are
  applied to the running encoder, without a keyframe.
* **Frame skipping** by the encoder's rate control is off while the link
  keeps up, so motion stays at full frame rate (quality dips instead). It
  switches on once the adaptive bitrate has backed off, or when the viewer
  chose a fixed bitrate, because big frames on a congested link mean lag.
  A frame it skips is re-sent when the screen goes still.
* **Priorities:** control messages (input, acks) go before video, and video
  before file transfers, so a big copy can't freeze the picture.
* **Big screens:** OpenH264 takes at most 4K by area, so larger screens
  (5120×1440, 5K) are scaled down to fit before encoding.
* **Encoder mode:** OpenH264 runs in its camera mode, on up to 8 threads.
  Its screen-content mode was about 3× slower on desktop content, ignored
  extra threads, and was no sharper once the picture settles.
  `crates/slipstream-codec/examples/encode_bench.rs` measures modes for
  speed and PSNR.
* **Adaptive resolution** (Speed and Low bandwidth presets only; Balanced
  and Quality keep full resolution): (`Speed` in `video.rs`): when encoding takes more
  than about 90% of the frame budget (1/max fps), moving pictures are
  encoded smaller (down to half size, from the square root of the
  overshoot), and the size climbs back after 3 s with room to spare. About
  0.3 s after the picture stops changing, the last frame goes out again at
  full size, so text at rest is sharp. Every size change starts with a
  keyframe; the viewer scales whatever size arrives.
* **Pace reporting** (`slipstream_platform::pace`): each stage (capture
  and its method, encoding, waiting for a new screen frame, waiting for
  the viewer) logs its frames a second and ms a frame every 30 s, and
  keeps a live 2-second figure. The viewer fetches those with
  `StreamOpen::Pace` every 3 s, shows them in the stats tooltip, and logs
  one line with the link's figures plus the host's every 30 s.
* **Buffers:** captured frames are copied into recycled buffers (the frame
  slot keeps a small pool), so 4K capture doesn't allocate 33 MB a frame.
* **Display:** the viewer presents with a one-frame swap chain queue.

## Platform backends

The Wayland GUI and headless host prepare the cached backend at startup,
before an incoming viewer needs it. Config can prepare or reset/reapprove
it locally. Preparation and authenticated sessions share the backend mutex,
so they reuse one portal grant. Reset drops the cached portal and stored
restore token only when no incoming session is active. Tokens returned by
the desktop persist to settings; a desktop that supplies none only grants
access for the current running process. Windows service hosts use their
existing active-console-session backend and require no portal approval.

| | Capture | Input |
|---|---|---|
| Linux (Wayland) | xdg-desktop-portal ScreenCast + PipeWire, pointer embedded by the compositor | RemoteDesktop portal |
| Linux (X11) | MIT-SHM (GetImage fallback) via pure-Rust `x11rb`, RandR monitors, pointer composited from XFixes | XTest via `enigo` |
| Windows | DXGI Desktop Duplication of our own (`win_dxgi.rs`, one copy per frame, created per session); under the service, DXGI on the normal desktop and a reusable GDI capturer (`win_gdi.rs`) on the login screen and UAC's secure desktop. Pointer composited by us (`win_cursor.rs`) | `SendInput` via `enigo` |
| macOS | ScreenCaptureKit via `xcap` | CGEvent via `enigo` |
| Android (implemented; emulator-tested, issue 09) | MediaProjection + bounded ImageReader frames, software H.264 | AccessibilityService with active full-display sharing and explicit enablement |

Keyboard events travel as physical key codes (W3C `code` names) and are
mapped to evdev / Windows scancodes / macOS keycodes on the host.

**Portable Linux binary.** The app links no optional system libraries.
PipeWire is `dlopen`ed only when a Wayland session shares its screen
(`pw.rs`), and X11 needs no Xlib. Release builds target glibc 2.31 with
cargo-zigbuild, so one binary runs on distributions from 2020 on.

**Windows pointer.** DXGI frames contain no pointer, and the recorder only
delivers frames when the picture changes. The capture loop therefore keeps
the last clean frame. It draws the current pointer into a copy (colour,
alpha and monochrome cursors, in physical pixels), and sends it again when
only the pointer moved.

**Windows pointer when Windows hides it.** On a computer with no mouse
plugged in (a server or VM reached only remotely), Windows reports no
visible pointer unless some program provides a mouse (another remote
tool's virtual one, say). The host then draws the standard arrow, so the
person controlling it still sees where they are.

## The Android app

The desktop app's GUI runs unchanged on Android: `apps/slipstream` is a
library as well as the desktop program, and `apps/slipstream-android` is
the shared library (`libslipstream_android.so`) a GameActivity loads. Its
`android_main` points `SLIPSTREAM_HOME` at the app's private storage and
starts `gui::run`. The Java side (`MainActivity.java`) is kept small and
calls into Rust over JNI:

* **Insets.** Android 15 and later draw every app edge to edge, and winit
  doesn't report what covers the screen. The activity reports the status
  and navigation bars, the camera cutout and the soft keyboard
  (`onInsets`). The GUI hands them to egui as its safe area and lays out
  in `Context::content_rect`. The surface stays full screen: resizing it
  instead made winit scale the old frame, since its window size on Android
  is the display's.
* **Typing.** With GameActivity the keyboard edits a copy of the text and
  hands over the whole state, which winit drops. `gui/android.rs` compares
  it with what it passed on and sends egui the difference as text and
  backspaces (suggestions rewrite the end, so it erases back to where the
  two still agree). Enter or Done arrives through `onEditorAction`. The app
  asks for the keyboard outright when a text field takes focus (winit's
  implicit request can be ignored), and scrolls the field into view above
  it.
* **Links.** `slipstream://link` opens the app (an intent filter;
  `singleTask` hands it to the running app). Links reach the GUI through
  an in-process inbox (`single::inbox`), not the desktop's loopback port,
  which any other app on the device could connect to.
* **Logging** goes to logcat (tag `Slipstream`) as well as the log file.
* **Sound** plays through cpal's AAudio backend.

The Android build needs `libc++_shared.so` beside the app's library, since
OpenH264 is C++. Backups are off: restoring the device identity onto
another phone would put two devices on the station with one key.

### Android hosting

Android starts `Client` with incoming sessions enabled and its stored identity,
so directory registration produces a stable ID. `android_main` initializes the
JNI bridge from the actual activity class/VM, names default identities from
`Build.MANUFACTURER`/`MODEL`, and keeps identity/settings in private storage.
The home console shows ID/password before the setup controls, and Config
exposes incoming-access/control/password preferences.

`MainActivity` launches the capture consent prompt from a visible user action.
Only its successful result starts the non-exported `ScreenShareService`:
foreground notification first, then `getMediaProjection`, registered callbacks
and one virtual display. Only after the foreground service/display are ready
does the activity move its task to the background, revealing the shared phone
and avoiding an initially stalled/self-referential app view. Its ImageReader emits RGBA planes through a bounded
JNI handoff into `CaptureHub`; row/pixel strides are checked and copied while
the Java image remains open. A process-wide latest image seeds new viewers;
up to four weak frame slots receive fresh frames with recycled buffers.
The existing software H.264 pipeline consumes these slots. Capture is limited
to an even-sized 1280-pixel long edge and at most 30 fps. Hardware MediaCodec
encoding remains a later optimization.

Android 14+ requests the default full display. Resize callbacks replace the
ImageReader surface and resize the existing virtual display rather than
reusing a consent token. Older Android uses display-change notifications.
When the system captures a partial app instead of the physical display, input
is disabled to avoid applying cropped coordinates to the whole device.
Stop/revocation clears the latest image, fails all slots, releases held input
and shuts down the foreground service. Capture failure follows the normal
host Bye path, so deliberate revocation does not auto-reconnect. Tokens are
never persisted; app/phone restarts require fresh approval.

`RemoteInputService` is exported only under Android's
`BIND_ACCESSIBILITY_SERVICE` permission and requires explicit user enablement.
The Rust backend forwards authenticated input only with an active full-screen
capture, a live viewer and the accessibility service connected. The Java main
handler rechecks these conditions before applying queued events. Continued
strokes keep a touch down for drags/long presses, but stationary continuations
are skipped: they generate no events and Android cancels them. Release emits
a final up stroke. Pointer coordinates use the current physical display size;
wheel events create short swipes. Keys use accessibility input connections on
API 33+ and editable-node actions on older versions. Desktop Keys menus expose
Android navigation without changing protocol 6. Android host file/clipboard
and network-scan capabilities are withheld until those backends are ready.

### Transfer receipts

Each `FileManager` owns a separate progress viewport per remote tab, shared by
its job list and native file browser. New work opens this compact LCARS panel;
dismissal hides it without cancelling, while later work opens it again.
The callback reads live atomic byte counts/job state independently of the
parent's rendering and repaints itself during transfer. Desktop root logic
registers it even when a folder-pane drop occurs with the parent minimized.
Destinations are
recorded once remote drop resolution completes. Sending all bytes remains
Running/Confirming until each remote Saved reply arrives; terminal results
are retained until cleared. Begin/finish transitions cannot overwrite a
cancelled or disconnected state. Ending the tab closes both file viewports.

## The Windows service

The service (`apps/slipstream/src/service.rs`) runs as LocalSystem in
session 0. It starts a helper, `slipstream service host`, in the active
console session as SYSTEM, and that helper captures, injects input and
follows the input desktop (`windesk.rs`), which is what reaches UAC
prompts and the login screen. Task Manager therefore shows two processes,
plus the tray app if it's on.

* **One host per computer.** Once the service is installed it is the
  host, running or not. The app never hosts beside it: two endpoints with
  one key push each other off the relay. Clients that don't host (the app
  in service mode, `slipstream connect` and the other command-line tools)
  bind a throwaway key, so the computer's identity is only ever used by
  its one host.
* **Identity handover.** Install copies the app's `identity.key` into the
  machine profile (`%ProgramData%\Slipstream Remote`, SYSTEM and
  administrators only). It writes a fresh file and renames it in, and
  checks the copy matches. The ID therefore stays the app's.
  `Store::identity` creates a key when none exists, or when recorded hardware
  evidence proves a clone/change as described below. An unreadable key is
  retried and then an error, never replaced just because reading failed.
* **Permissions.** The profile's ACL is set on the folder alone, and
  everything inside is reset to inherit it (`icacls … /reset`); the service
  repeats that repair at every start. An `icacls /T` grant used to leave
  files from an earlier install with no usable permissions.
* **Public folder** (`…\public`, readable by all users): the service
  writes `device-id.txt` (the ID it registered as, which the app's Bridge
  shows in service mode) and its log, `slipstream.log`.
* **Starting.** The service's DACL lets interactive users start it (not
  stop or change it), so the app keeps an installed service running
  (`keep_running`, paused while the app installs, starts or removes it)
  and *Start* needs no administrator prompt.
* **Diagnostics.** Install and start failures carry Windows' own account
  (`diagnose`: `sc queryex`/`qc` and the Service Control Manager's System
  log messages), written by the elevated process to a `--report` file and
  shown, copyable, in Config.
* **Install over a running copy.** The installed `.exe` is usually still
  running (the tray app at sign-in), so it's renamed aside and replaced,
  and the leftover is removed at the next install.
* **App and service updates.** Full/compact GUI and CLI updates replace the
  app and an existing service together. A new app also checks for an older
  service on startup, covering the first upgrade from an older updater. That
  catch-up fetches only signed metadata matching the installed app, reuses the
  app's local bytes and updates only the service, leaving the app open. A newer
  station release remains a normal user-triggered update of both programs.
  Downloading happens without elevation. A UAC helper copies the candidate
  into the protected Program Files directory and verifies its release
  signature, bounded size and checksum again before changing either program.
  It reads the service executable from SCM, refuses downgrades, stages copies
  beside each target, stops a running service, replaces both programs and
  verifies the restarted service stays running. Failed replacement/restart
  restores the previous executables; failed recovery preserves their backups.
  A stopped service stays stopped during replacement, and no service is
  installed implicitly. The machine profile is never imported from the GUI
  during a program update. Service run/host and the update helper ignore
  unsigned station provisioning trailers. **Apply settings** remains the
  explicit operation for station/unattended-settings changes.
* **Update coordination.** A protected exclusive marker coordinates app,
  service and watchdog updates. Elevated update errors use a random nonce
  under a fixed protected report directory, never a caller-selected output
  path. Successful `.old` leftovers can be cleaned on startup; failed-recovery
  backups are retained.

## Stations, updates and the app

* **Versions:** semantic versioning (pre-1.0 minor = features, patch =
  fixes). Every station build is a release: `scripts/release-version.sh`
  raises the workspace version and files the changelog's "Unreleased"
  entries under it, and `scripts/build-station.sh` refuses unreleased
  changes. The Windows `.exe`'s version info comes from the same version
  plus the build number.

* **Self-update:** the station's `clients/manifest.json` lists version, build
  number (git commit count) and, per platform, size, SHA-256 and an Ed25519
  release signature over platform, version, build, size and hash
  (`slipstream_core::release`). The signing key stays on the build machine;
  its public key is compiled into the apps (`SLIPSTREAM_UPDATE_KEYS`), so a
  compromised station can't push its own build. Apps ask with
  `update_check` 20 s after start and every 6 h, and only offer a newer
  build whose signature checks out. They download through `download.php`
  (which appends a station trailer to unsigned desktop executables and
  preserves code-signed files and Mac bundles unchanged), hash the signed app
  prefix incrementally, validate the
  bounded trailer and clean stream end with response/idle deadlines, and
  swap themselves in: rename over on Unix,
  move the running `.exe` aside on Windows. Then they restart with
  `SLIPSTREAM_RESTARTED=1`, so the single-instance guard waits for the old
  copy to exit.
  Before downloading, the updater refreshes the signed offer and adds its
  hash to the download URL to distinguish cached releases. If publication
  races a download, it retries once only when a separately verified newer
  signed offer exists. Unchanged damaged files still fail verification.
* **Single instance:** a loopback TCP port derived from the settings folder.
  A second launch asks the first to show its window. `SLIPSTREAM_HOME` gives
  an independent instance.
* **Desktop integration:** on Linux the app installs its own `.desktop` entry
  and icons (app id `slipstream-remote`, which Wayland needs to match the
  window to its icon). Autostart uses XDG autostart, the Windows `Run` key,
  or a macOS LaunchAgent.

## Meetings and calls

Meetings use separate version-1 ALPNs and endpoint-bound membership; remote
support stays on protocol 6. The organizer admits guests and forwards selected
encoded H.264/Opus sources with bounded queues. Pending guests receive no room
content. Private stations issue scoped, expiring relay-only guest leases through
`meeting_relay`; invitations never contain station access keys. Call offers
prove station access and bind both endpoints to a one-use invitation. Windows
service signaling crosses the authenticated local bridge while the interactive
app owns microphone, camera and speaker output.

`crates/slipstream-session/src/meetings/` owns room/call/media lifecycles;
`crates/slipstream-core/src/meeting.rs` defines shared types. Device selection
is remembered. Screen thumbnails use a bounded local worker, one capture at a
time; closing the chooser cancels it. Only explicit screen confirmation starts
a meeting publisher. Android Show me uses Camera2 independently of remote-screen
capture and accessibility. See [usage and limits](MEETINGS.md) and the
[October 21 handoff](DEVELOPMENT-2026-10-21.md) for verification and deployment.

## UI

The [desktop session preview](screenshots/session-0.19.0.png) shows the
workstation in the title bar and the performance/quality controls together.

Outgoing sessions are shared `Arc<Mutex<RemoteTab>>` objects owned by the
console. A session belongs to the root tab group or a floating group rendered
in a deferred native viewport. Moving a tab between groups changes the
presentation, not the connection or GPU stream ID. Deferred callbacks send
lifecycle actions back to the root; the root polls sessions/reconnects and registers session,
Files and Network Scan windows even while hidden. Each session queues its
own captured keys; only the selected tab in a group consumes remote input,
so full-screen commands and shortcuts target the right session. Focus changes
release other sessions; unfocused tool repaints do not. Ending a link explicitly disconnects
it before dropping shared callbacks, and texture retention includes detached
sessions. Manual reconnect now uses the same retained-session recovery path.

`gui/session_tabs.rs` adds a shared tab strip and drag coordinator. Root tabs
snapshot attached sessions; each floating group has its own selectable strip.
Drops queue placement actions so session locks are released before reordering,
merging groups or creating a viewport. Closing the last member or moving it
away releases an empty floating window. New-link actions and password prompts
carry their source viewport into the connection attempt; + adds to that group
regardless of the global separate-windows preference. Targets and pointer
positions use physical screen pixels across viewport scale factors. During a tab drag only, Windows
and X11 sample the native pointer and synthesize missing pointer-up events:
winit can drop them after CursorLeft. The destination may also accept release
locally when a compositor withholds global positions. Hidden/minimized root
windows are not drop targets. Drag cancellation/close clears pending ownership.
`viewer.separate_windows` persists the new-link default; its absent-field
value is false for existing profiles. Android embeds tabs and ignores the
native-window preference. No session protocol changes are involved.


The Windows local service pipe additionally publishes the host's current
one-time code in its authenticated snapshots and accepts regeneration requests
bound to the current host epoch. Missing optional fields preserve older local
service compatibility. The code is never written to public files or service
stdout, and the GUI clears it when the pipe disconnects. The compact GUI
refreshes the authoritative service ID instead of pairing a service code with
the viewer process's ID. Network authentication still accepts the temporary
and permanent password alternatives; session protocol remains version 6.

egui on wgpu, styled with a custom sci-fi theme: a starship-console layout
with a neon-on-dark palette, glow strokes and HUD corner brackets. Video
frames are converted from YUV to RGB on the GPU (a WGSL shader in an
egui_wgpu paint callback).

The UI redraws at the display's refresh rate while visible (60 fps when
unfocused) and slows to one tick a second when hidden or minimized. On a
CPU renderer (Mesa llvmpipe, detected from the adapter) animations run at 8
frames a second, 2 when unfocused; input and new video still redraw at
once.

**No usable GPU** (Windows Server 2016, VMs without a display driver):
release builds embed Mesa's llvmpipe OpenGL driver (`gui/softgl.rs`). It's
unpacked per user and loaded in-process, or, where that fails, a copy of
the app runs from beside Mesa's DLLs. A start marker falls back to the
native compact window (`gui/lite.rs`) after a crash. The
host-side chat is a separate native window (an egui *deferred* viewport;
immediate viewports don't map on KDE Wayland).

Android already uses the shared egui interface, with phone layouts, touch
controls and a Java activity for keyboard/insets and sharing consent. Desktop
tool layouts and viewer keyboard behavior still need phone-specific work.

### Desktop session frame

The title strip and remote controls use connected LCARS color segments.
`theme::session_body` reserves 80 points on the left, 16 on the right and
16 below the content in windowed desktop sessions. The video fits inside
this body (with room for chat); input normalization uses its actual fitted
or zoomed video rectangle and interaction is clipped to the video area.
`theme::session_rails` paints a 44-point left rail and curved elbows outside
that body, with a full-width blue footer below the remote taskbar and a
12-point break below End Link on the right. Rails no longer depend on
letterboxing, picture arrival, zoom or platform display scaling. The
36-point toolbar uses vector symbols with rounded rectangular controls,
a bordered dark stats panel, and Console when space is tight. Full screen
and Android omit the reserved frame; full-screen controls still auto-hide.

`gui/window_shape.rs` gives borderless windows actual rounded bounds rather
than painting rounded panels on rectangular black surfaces. Windows uses
cached, DPI-scaled native regions compatible with the VM software renderer.
X11 uses SHAPE rectangles (the Vulkan surface may only support opaque alpha);
Wayland uses an alpha surface with a rounded opaque background. Native
masks are cleared to rectangular bounds while maximized/full screen. Native
window state/size changes invalidate the mask cache. On Windows, a subclass
of the app's caption-styled native windows gives non-maximized client areas
the whole window rectangle. This bypasses egui-winit's one-pixel shadow
strip; maximized bounds and other messages still go through winit. Native
compact popup windows do not need this subclass.

`theme::console_menu` anchors dropdown outer frames to the toolbar segments
with a four-point seam and six-point corners. Menu rows share padding and
blue selection colors, including inherited submenus. Blank remote screen
uses a selected button, so its text aligns with the other Tools actions.

Incoming chat uses a separate native viewport with `theme::chat_body` rails
around its content. The shared chat UI fills a bounded scrolling log with
framed messages, leaving space for the composer at the bottom. Service-chat
messages still wait for the service echo; unsuccessful sends retain the
input. Closing that viewport only closes chat, not the host session.


### Support tools and device binding (0.21.0)

Three appended protocol-6 request variants add WakeOnLan, NetworkDiagnostics
and SupportStatus, plus an appended diagnostic reply. Host file-stream policy
enforces existing network/control permissions and a new opt-in wake setting.
Requests have deadlines; scans/diagnostics share a session semaphore, DNS UDP
replies are bounded and matched to the request, and closing a request cancels
pending async work. Wake targets are adapter-derived subnet broadcasts, with
strict unicast MAC validation and a host-wide two-second throttle. A loopback
UDP test checks all three exact packets without waking a real machine.

Scanner-created TCP tunnels preflight the target using the existing host tunnel
policy, bind locally to loopback, and live with the session's scanner state.
They survive closing the scanner UI and close with interruption/disconnection.
The UI only constructs numeric loopback HTTP/HTTPS URLs; scanned names never
become executable commands. The support report copies an allowlist of inventory,
network and viewer statistics with explicit notes. Local exports use unique
private temporary directories retained only on success, with optional PNG
conversion off the UI thread. No settings/credential object or raw log is
serialized into a report.

Host status emits an in-memory session event, bridged to full/compact Windows
apps through an optional snapshot field on the authenticated local pipe. The
full app registers a deferred status viewport independently of chat, with a
local disconnect action. Client events and pipe updates request root paint even
while hidden; otherwise an occluded host can defer the notification until click.
CLI hosts record only a generic status event, not the potentially sensitive text.

Store::identity reads firmware evidence once per process. Windows uses the
bounded RawSMBIOSData API ([Microsoft documentation](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemfirmwaretable));
SMBIOS 2.6+ UUID bytes are normalized to canonical order. Linux/macOS use their
firmware UUID and serial sources. Linux also records permanent built-in PCI
network-adapter evidence for a fallback when firmware is unreadable; virtual,
randomized, user-assigned and removable USB addresses are excluded. Learning
the fallback preserves an existing identity. Firmware comparisons take priority;
adapter additions/removals preserve identity while a known adapter remains.
Only context-separated hashes are persisted
in device-binding.json. An exclusive OS file lock serializes binding/identity
updates across processes. UUID comparison takes priority; serial and then
Linux adapter evidence provide fallbacks.
A proven change creates a fresh random key, clears cached/public IDs and saves
the new binding after configuration cleanup. Missing evidence and damaged or
unknown binding metadata never silently replace the key. Client startup binds
before loading settings so stale cached IDs cannot be republished. Viewer
network endpoints remain ephemeral even though their persistent profiles are
checked. Windows installation retains an existing machine key instead of
copying an independently rekeyed user profile over it. First installation still
adopts the user's key and establishes a machine binding. Service registration
and unattended settings are preserved; actual cloned Windows hardware remains
a runtime confirmation task.
