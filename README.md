# Slipstream Remote

**Fast, free, open-source remote desktop.** Give someone your 9-digit ID and
password and they can see and control your screen — or do the same to theirs.
No accounts, no subscriptions, no telemetry. MIT licensed.

![Slipstream bridge screen](docs/screenshots/bridge.png)

## Features

- **TeamViewer-style access** — short numeric IDs, one-time passwords that
  rotate after every session, optional permanent password for unattended access.
- **End-to-end encrypted** — every link is QUIC/TLS 1.3 between the two devices.
  Passwords are verified with SPAKE2, so they never cross the network and can't
  be brute-forced offline — not even by the server.
- **Peer-to-peer and fast** — direct connections through NAT hole punching,
  with a relay fallback. H.264 video rendered on the GPU; a frame-ack pacing
  loop keeps latency low instead of letting frames queue up.
- **Full control** — mouse, wheel, keyboard (physical keys, works across
  layouts), Ctrl+Alt+Del and other special keys, multi-monitor switching.
- **Clipboard sync, file transfer (drag & drop), chat.**
- **Quality presets** — Balanced, Speed, Quality, Low bandwidth; automatic
  bitrate adaptation.
- **Runs in the tray**, starts with the system, stays reachable.
- **Self-hostable server** — drop a PHP folder on any web host, or run one
  small binary that does ID lookup *and* relaying. Apps just need a domain.
- A starship-console interface. 🖖

## Platforms

| | Share screen (host) | Control others (viewer) | Status |
|---|---|---|---|
| Linux — Wayland (KDE, GNOME, wlroots) | desktop portal + PipeWire | ✅ | in testing |
| Linux — X11 | xcap + XTest | ✅ | in testing |
| Windows 10/11 | DXGI/WGC + SendInput | ✅ | builds in CI; needs testing |
| macOS 12+ | ScreenCaptureKit + CGEvent (grant Screen Recording + Accessibility) | ✅ | builds in CI; needs testing |
| Android | planned | planned | |

## Quick start

### 1. Set up a server (once)

Upload the [`server/web/`](server/web) folder to any web host with PHP 8 —
e.g. `relay.example.com` — and open it in a browser to check it's green.
Or run the `slipstream-server` binary. See [docs/SERVER.md](docs/SERVER.md).

### 2. Install the app on each computer

**Linux** (from source):

```sh
./scripts/install-linux.sh
```

**Windows / macOS**: download the build artifacts from CI (Actions tab), or
`cargo build --release -p slipstream`.

### 3. Connect

Open Slipstream → **CONFIG** → enter your server's address (e.g.
`relay.example.com`) → **Save**.
The **BRIDGE** page now shows your ID and password. On the other computer,
enter them under **Establish link** and press **Engage**.

On Wayland the first incoming session asks you to approve screen sharing;
the choice is remembered.

## Command line

```sh
slipstream                                  # the app
slipstream --connect 482913077 --password …  # connect straight away
slipstream --minimized                      # start in the tray
slipstream host [--password …]              # headless host (servers, kiosks)
slipstream connect <ID> --password …        # headless test connection with stats
slipstream set-server relay.example.com     # configure without the GUI
slipstream set-password <password>          # unattended-access password
slipstream info                             # show ID, key and settings path
```

`SLIPSTREAM_HOME=/some/dir` keeps settings and identity elsewhere (handy for
running two instances on one machine).

## Building

Requires Rust (stable) and, on Linux, PipeWire and X11/Wayland dev headers:

```sh
# Arch / CachyOS
sudo pacman -S pipewire libxkbcommon clang
# Debian / Ubuntu
sudo apt install libpipewire-0.3-dev libspa-0.2-dev clang libxkbcommon-dev libxdo-dev libxtst-dev libdbus-1-dev

cargo build --release
cargo test --workspace
```

## How it works

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — networking, the auth
protocol, the latency control loop and the platform backends.

## Roadmap

- Hardware video encoding (NVENC, VA-API, VideoToolbox, Media Foundation)
- Audio streaming
- Android app (viewer first, then host)
- Address book with saved credentials (OS keychain)
- Windows service / pre-login access

## License

MIT — see [LICENSE](LICENSE). Bundled fonts (Antonio, Barlow Semi Condensed,
Share Tech Mono) are under the SIL Open Font License 1.1; see
`apps/slipstream/assets/fonts/`.
