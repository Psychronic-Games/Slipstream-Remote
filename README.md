# Slipstream Remote

**Fast, free, open-source remote desktop.** Give someone your 9-digit ID and
password and they can see and control your screen — or do the same to theirs.
No account required for remote sessions, no subscriptions, no telemetry.
MIT licensed.

![Slipstream bridge screen](docs/screenshots/bridge.png)

## Features

- **TeamViewer-style access** — short numeric IDs, one-time passwords that
  rotate after every session, optional permanent password for unattended access.
- **End-to-end encrypted** — every link is QUIC/TLS 1.3 between the two devices.
  Remote-session passwords are verified with SPAKE2, so they never cross the
  network and can't be brute-forced offline — not even by the server.
- **Peer-to-peer and fast** — direct connections through NAT hole punching,
  with a relay fallback. H.264 video rendered on the GPU; a frame-ack pacing
  loop keeps latency low instead of letting frames queue up.
- **Full control** — mouse, wheel, keyboard (physical keys, works across
  layouts), Ctrl+Alt+Del and other special keys, multi-monitor switching.
- **Support tools** — connect to discovered TCP services through a workstation,
  export reviewed diagnostic reports, send LAN wake packets, compare DNS and
  gateway/station checks, and show the user a plain-language support status.
- **Cloned-device detection** — locally hashed desktop hardware evidence gives an
  imaged machine a fresh identity when it starts on different hardware, while
  retaining station and unattended settings. Run the updated master/service
  before imaging. Firmware UUID/serial identifiers take priority; Linux can
  fall back to permanent built-in PCI network adapters. Missing or duplicated
  evidence limits detection. See [imaging](docs/GETTING-STARTED.md#imaging-and-automatic-device-ids).
- **Several workstations at once** — group tabs in the console or floating
  windows, drag them between groups or tear them out without reconnecting.
  Each group's **+** opens recent or station-saved/shared devices in that window.
- **Group meetings and Show me** — authorized clients join an organizer-hosted
  room with voice, camera, chosen-screen sharing and Subspace chat. Android's
  rear camera can show equipment to a technician. The default room size is 20;
  usable video capacity depends on the organizer's upload connection.
  See the [meeting guide](docs/MEETINGS.md).
- **Clipboard sync, chat, and a two-pane file manager** — copy and paste
  text and files between the computers, browse both, send and receive
  folders, or drag files from your desktop straight onto the remote screen.
- **Technician sign-in** — keep your list of devices (with their passwords
  encrypted in the app before station storage) on your station, and sign in
  to several stations at once, other organisations' too.
- **System information** — the remote computer's hardware, drives,
  temperatures and network, live, plus peripherals, printers/scanners and
  network drives.
- **Sound** from the remote computer, when you want it.
- **Unattended Windows servers** — the Slipstream service hosts from boot,
  with support for UAC prompts, administrator windows and the login screen.
  Secure-desktop control still needs [real-Windows verification](docs/issues/04-windows-service.md).
- **Privacy mode** blacks out the remote screen while you work, and
  **port forwarding** reaches services on the remote network (its Remote
  Desktop, a router's admin page).
- **Network scan** discovers devices and open TCP ports on the remote
  computer's IPv4 network or a manually entered subnet/VLAN, with device
  names, LCARS table/topology views, and its own movable desktop window.
- **Quality presets** — Balanced, Speed, Quality, Low bandwidth; automatic
  bitrate adaptation.
- **Runs in the tray**, starts with the system, stays reachable.
- **Desktop apps update themselves** from your station — upload a new build,
  and they offer *Install & restart* and keep their settings. Android updates
  are installed from the station's APK.
- **Works immediately, with self-hosting available** — raw downloads use
  `public.slipstreamremote.com`. A configured self-hosted station stays the
  default; shared tabs above Your ID and Establish link switch both panels
  between that station and Slipstream Public.
- **Your own station** — an IT firm or company runs
  one self-contained web folder: a download page with apps preconfigured for
  it, a private directory of its clients' devices, its own relay and an admin
  page. Works on ordinary PHP web hosting (Virtualmin, cPanel, …).
- A starship-console interface. 🖖

## Platforms

| | Share screen (host) | Control others (viewer) | Status |
|---|---|---|---|
| Linux — Wayland (KDE, GNOME, wlroots) | desktop portal + PipeWire | ✅ | tested (KDE) |
| Linux — X11 | MIT-SHM + XTest | ✅ | in testing |
| Windows 10/11 | DXGI + SendInput | ✅ | tested |
| macOS 12+ | ScreenCaptureKit + CGEvent (grant Screen Recording + Accessibility) | implemented | Apple Silicon cross-type-checks; native build and real-Mac testing pending |
| Android 8+ | MediaProjection + accessibility (on-device approval) | touch viewer | Android 16 emulator tested; real phones next |

The portable Linux app is a single x86_64 file targeting glibc 2.31+
(Debian 11 / Ubuntu 20.04 and newer), with no optional capture libraries
required at runtime. The older Debian-server startup check remains open.
See [status](docs/STATUS.md) for platform verification and limits.

## Quick start

**New here? Read [docs/GETTING-STARTED.md](docs/GETTING-STARTED.md)**: a
step-by-step guide from an empty web server to your first session, including
client installs, unattended access and troubleshooting.

For public access, get the app from
[Slipstream Public](https://public.slipstreamremote.com/) and connect using
the Public tab on both computers. No station setup is needed. The steps below
set up your own station, which gets its own device directory and IDs; its tab
becomes the default without removing your separate public ID.

### 1. Set up your station (once)

Run `scripts/build-station.sh`, upload the contents of
`dist/slipstream-station/` to the root of a (sub)domain with PHP 8 and HTTPS
(e.g. `support.example.com`), and open `/admin/` to name it and set an admin
password. See [docs/GETTING-STARTED.md](docs/GETTING-STARTED.md#1-set-up-your-station).

### 2. Get the app

Download it from your station's page (`https://support.example.com/`) on
every computer — it connects to your station by itself (or with the setup
code shown on the page). The same session protocol connects the supported
platforms; Android hosting requires approval on the phone, and macOS still
needs native verification.

### 3. Connect

The **BRIDGE** page shows the computer's ID and password. On the other
computer, enter them under **Establish link** and press **Engage**.

The one-time code and an unattended password both work with the same ID,
including through the Windows service. Share the rotating code when you do
not want to share the unattended password. Update an older installed service
to make its code available in the app.

On Wayland, approve screen sharing when Slipstream starts, or choose
**Config → Screen permission → Approve now** before someone connects.
Access stays ready while it runs in the tray; the desktop decides whether
approval can be remembered across restarts. Windows needs no screen-sharing
approval, including when its background service hosts the computer.

## Command line

```sh
slipstream                                  # the app
slipstream --connect 482913077 --password …  # connect straight away
slipstream --minimized                      # start in the tray
slipstream host [--password …]              # headless host (servers, kiosks)
slipstream connect <ID> --password …        # headless test connection with stats
slipstream set-server support.example.com --key …  # configure without the GUI
slipstream set-password <password>          # unattended-access password
slipstream info                             # show ID, station and settings path
slipstream update                           # install the newest build from your station
```

`SLIPSTREAM_HOME=/some/dir` keeps settings and identity elsewhere (handy for
running two instances on one machine).

## Building

```sh
scripts/check.sh cargo build --release --locked
scripts/check.sh                # workspace tests, with resource/cache limits
scripts/build-station.sh        # portable Linux + Windows builds, ready to upload
```

Build dependencies, cross-compiling and the release checklist are in
[docs/BUILDING.md](docs/BUILDING.md).

## Documentation

| | |
|---|---|
| [Getting started](docs/GETTING-STARTED.md) | Station setup, installing clients, connecting, unattended access, troubleshooting |
| [Running a station](docs/SERVER.md) | Station reference: requirements, data, relay, access control, Nginx, standalone server |
| [Building and releasing](docs/BUILDING.md) | Toolchain, release builds, publishing updates |
| [Architecture](docs/ARCHITECTURE.md) | Networking, authentication, protocol, platform backends |
| [Status](docs/STATUS.md) | What's verified, known gaps, next steps |
| [Meetings and Show me](docs/MEETINGS.md) | Calls, admission, devices, screen previews and mobile camera sharing |
| [Latest development handoff](docs/DEVELOPMENT-2026-10-22.md) | Published 0.25.0, public/private stations, verification and rollback |
| [2026-10-20 handoff](docs/DEVELOPMENT-2026-10-20.md) | Earlier 0.22.1 publication and verification |
| [2026-10-19 handoff](docs/DEVELOPMENT-2026-10-19.md) | Support features, audit and remaining device checks |
| [2026-10-18 handoff](docs/DEVELOPMENT-2026-10-18.md) | Earlier features, verification and deployment history |
| [2026-10-18 audit](docs/AUDIT-2026-10-18.md) | Fixed reliability/performance findings, regression evidence and limits |
| [Open issues](docs/issues/README.md) | The work queue, ready to file on GitHub |
| [Changelog](CHANGELOG.md) | What's in each version |
| [Code signing policy](docs/CODE-SIGNING.md) | What is signed, by whom, and privacy |

## How it works

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — networking, the auth
protocol, the latency control loop and the platform backends.

## Roadmap

- Hardware video encoding (NVENC, VA-API, VideoToolbox, Media Foundation)
- Android real-phone verification, hardware encoding, host audio/clipboard/files and updates

## License

MIT — see [LICENSE](LICENSE). Bundled fonts (Antonio, Barlow Semi Condensed,
Share Tech Mono) are under the SIL Open Font License 1.1; see
`apps/slipstream/assets/fonts/`.
