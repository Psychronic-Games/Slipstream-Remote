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
  `http://` drops the `Upgrade` header. Clients fall back to the public n0
  relays only if the station allows it.
* **Wrong clocks:** a computer whose clock is off by weeks would reject every
  TLS certificate as "expired" or "not yet valid". All TLS verification
  (directory and relays) goes through `clock::ClockedVerifier`. It checks
  certificates against *corrected* time: system time plus an offset learned
  from the `Date` header of every station response, and never earlier than
  the build time. On a time error the client reads the station's `Date`
  once without verifying, and retries.

## Stations and provisioning

Each organisation runs its own station; there is no central server. A
private station requires its access key on every directory request, and its
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

The directory server is *not* trusted. A viewer types an ID + password; the
password never leaves the machine:

1. Viewer → Host: `Hello` + SPAKE2 message A, bound to (viewer EndpointId,
   host EndpointId as resolved).
2. Host → Viewer: one SPAKE2 message B per accepted password (temporary
   one-time password and, optionally, the permanent unattended password).
3. Viewer → Host: HMAC key confirmations for each B.
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

## Session protocol (ALPN `slipstream/remote/1`, `PROTOCOL_VERSION` 4)

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
  path, or `UnderCursor`, which on Windows asks Explorer which folder window
  is under the pointer (used for drag and drop).
* **Capabilities** tell the viewer what the host allows (input, clipboard,
  files, chat) and whether its video already shows the mouse pointer
  (`cursor_in_video`). The viewer hides its own pointer only in that case.

`postcard` has no optional fields, so any change to a message bumps
`PROTOCOL_VERSION`. Mismatched peers refuse each other with a "please
update" message.

### Latency control

The viewer acks every decoded frame. The host only encodes a new frame when
fewer than *N* (default 2) frames are unacknowledged. *N* grows with the
link's base round-trip time (the minimum seen, not the smoothed one, which
includes queueing). Frames are skipped *before* encoding, never after, so
the H.264 reference chain is never broken and stale frames never pile up in
buffers — the picture is always as fresh as the network allows.

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
* **Buffers:** captured frames are copied into recycled buffers (the frame
  slot keeps a small pool), so 4K capture doesn't allocate 33 MB a frame.
* **Display:** the viewer presents with a one-frame swap chain queue.

## Platform backends

| | Capture | Input |
|---|---|---|
| Linux (Wayland) | xdg-desktop-portal ScreenCast + PipeWire, pointer embedded by the compositor | RemoteDesktop portal |
| Linux (X11) | MIT-SHM (GetImage fallback) via pure-Rust `x11rb`, RandR monitors, pointer composited from XFixes | XTest via `enigo` |
| Windows | DXGI Desktop Duplication via `xcap`, pointer composited by us (`win_cursor.rs`) | `SendInput` via `enigo` |
| macOS | ScreenCaptureKit via `xcap` | CGEvent via `enigo` |
| Android (later) | MediaProjection | AccessibilityService |

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

## Stations, updates and the app

* **Self-update:** the station's `clients/manifest.json` lists version, build
  number (git commit count) and, per platform, size, SHA-256 and an Ed25519
  release signature over platform, version, build, size and hash
  (`slipstream_core::release`). The signing key stays on the build machine;
  its public key is compiled into the apps (`SLIPSTREAM_UPDATE_KEYS`), so a
  compromised station can't push its own build. Apps ask with
  `update_check` 20 s after start and every 6 h, and only offer a newer
  build whose signature checks out. They download through `download.php`
  (which appends the station trailer; the trailer only holds station
  settings, not code), verify the hash of the app without the trailer, and
  swap themselves in: rename over on Unix,
  move the running `.exe` aside on Windows. Then they restart with
  `SLIPSTREAM_RESTARTED=1`, so the single-instance guard waits for the old
  copy to exit.
* **Single instance:** a loopback TCP port derived from the settings folder.
  A second launch asks the first to show its window. `SLIPSTREAM_HOME` gives
  an independent instance.
* **Desktop integration:** on Linux the app installs its own `.desktop` entry
  and icons (app id `slipstream-remote`, which Wayland needs to match the
  window to its icon). Autostart uses XDG autostart, the Windows `Run` key,
  or a macOS LaunchAgent.

## UI

egui on wgpu, styled with a custom sci-fi theme: a starship-console layout
with a neon-on-dark palette, glow strokes and HUD corner brackets. Video
frames are converted from YUV to RGB on the GPU (a WGSL shader in an
egui_wgpu paint callback).

The UI redraws at the display's refresh rate while visible (60 fps when
unfocused) and slows to one tick a second when hidden or minimized. The
host-side chat is a separate native window (an egui *deferred* viewport;
immediate viewports don't map on KDE Wayland).

egui also targets Android, so the same UI can ship on mobile later.
