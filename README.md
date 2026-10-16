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
- **Your own station, not a central service** — an IT firm or company runs
  one self-contained web folder: a download page with apps preconfigured for
  it, a private directory of its clients' devices, its own relay and an admin
  page. Works on ordinary PHP web hosting (Virtualmin, cPanel, …).
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

### 1. Set up your station (once)

Run `scripts/build-station.sh`, upload `dist/slipstream-station/` to the root
of a (sub)domain with PHP 8 and HTTPS — e.g. `support.example.com` — and open
`/admin/` to name it and set an admin password. See
[docs/SERVER.md](docs/SERVER.md).

### 2. Get the app

Download it from your station's page (`https://support.example.com/`) on
every computer — it arrives already configured for your station. Any OS can
control any other.

### 3. Connect

The **BRIDGE** page shows the computer's ID and password. On the other
computer, enter them under **Establish link** and press **Engage**.

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
