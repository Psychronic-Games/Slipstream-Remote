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
from their download: the station appends a trailer to the executable
(`[JSON][u32 length]["SLIPCFG1"]`, see `slipstream_core::provision`) or adds
`station.json` to the macOS bundle.

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
the host.

## Session protocol (ALPN `slipstream/remote/1`, `PROTOCOL_VERSION` 3)

* **Control stream** (bi-directional, length-prefixed `postcard`): auth,
  session info (displays and capabilities), input events, frame acks,
  clipboard, chat, display/quality selection.
* **Video stream** (host → viewer, uni-directional): encoded H.264 frames.
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
fewer than *N* (default 2) frames are unacknowledged. Frames are skipped
*before* encoding, never after, so the H.264 reference chain is never broken
and stale frames never pile up in buffers — the picture is always as fresh
as the network allows.

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
  number (git commit count) and SHA-256 per platform. Apps ask with
  `update_check` 20 s after start and every 6 h. They download through
  `download.php` (which appends the station trailer), verify the hash of the
  app without the trailer, and swap themselves in: rename over on Unix,
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
