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
| `crates/slipstream-core` | Wire protocol, framing, password auth (SPAKE2), directory client, iroh endpoint setup, host & viewer session logic |
| `crates/slipstream-codec` | Video encoder/decoder abstraction (OpenH264 today; hardware encoders later) |
| `crates/slipstream-platform` | Per-OS screen capture, input injection, clipboard |
| `apps/slipstream` | The desktop app (egui, sci-fi theme) plus a headless CLI |
| `apps/slipstream-server` | ID directory + embedded iroh relay, one binary on one HTTP port |
| `server/web` | The same directory API as a drop-in PHP folder (files or MariaDB) |

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
  binary. Clients also fall back to the public n0 relays.

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

## Session protocol (ALPN `slipstream/remote/1`)

* **Control stream** (bi-directional, length-prefixed `postcard`): auth,
  session info, input events, frame acks, clipboard, chat, display/quality
  selection.
* **Video stream** (host → viewer, uni-directional): encoded H.264 frames.
* **File transfer:** one bi-stream per transfer.

### Latency control

The viewer acks every decoded frame. The host only encodes a new frame when
fewer than *N* (default 2) frames are unacknowledged. Frames are skipped
*before* encoding, never after, so the H.264 reference chain is never broken
and stale frames never pile up in buffers — the picture is always as fresh
as the network allows.

## Platform backends

| | Capture | Input |
|---|---|---|
| Linux (Wayland) | xdg-desktop-portal ScreenCast + PipeWire | RemoteDesktop portal |
| Linux (X11) | XShm via `xcap` | XTest via `enigo` |
| Windows | DXGI Desktop Duplication | `SendInput` via `enigo` |
| macOS | ScreenCaptureKit | CGEvent via `enigo` |
| Android (later) | MediaProjection | AccessibilityService |

Keyboard events travel as physical key codes (W3C `code` names) and are
mapped to evdev / Windows scancodes / macOS keycodes on the host.

## UI

egui on wgpu, styled with a custom sci-fi theme (neon-on-dark palette, glow
strokes, HUD corner brackets, animated grid). egui also targets Android, so
the same UI can ship on mobile later.
