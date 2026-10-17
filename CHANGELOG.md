# Changelog

Build numbers are the git commit count. The app shows them as
`0.5.0 (build N · commit)`.

## Unreleased

### Sound
- The remote computer's sound can be played on the viewer: a **Sound**
  toggle in the session bar, and a default in *Config → Viewing* (off
  unless you switch it on). The host can refuse it under *Permissions*.
- Opus at 96 kbit/s in 20 ms QUIC datagrams (a lost packet is concealed,
  never waited for), with a 60 ms jitter buffer. libopus is vendored and
  built with `cc`, like OpenH264.
- Hosts: Windows (WASAPI loopback) and Linux (PulseAudio, also on PipeWire
  desktops; loaded at runtime, so the binary still needs no audio
  libraries). Mac hosts can't share sound yet; Macs can play it.
- `PROTOCOL_VERSION` 4: both computers need this version.

### Session bar
- The screen buttons are labelled *SCREEN*, with each screen's name and
  size in the tooltip.
- The Bridge button shows the Slipstream delta pointing back, not "◀".
- The bar adapts to the window width instead of overlapping: compact stats
  first, then tighter buttons, then a screen menu.

### Windows
- Windows Server 2016 (and other machines without a usable GPU): the app
  now falls back to Windows' software renderer instead of exiting without
  a word, and any start-up failure shows a message box.
- The C runtime is linked in: no Visual C++ redistributable needed.

### Security
- Updates are signed. The station build script signs every app in the
  manifest with a release key that stays on the build machine, and apps
  install only updates signed by the key they were built with. Before
  this, anyone who got into the web host could have pushed their own app
  to every client.
- Password guesses sent over many connections at once are now counted one
  by one, so they hit the lockout like any others. Before, parallel
  connections could get several extra guesses in before it started. A
  viewer that doesn't answer the password challenge within 10 s is
  dropped.

### Performance
- OpenH264 is now built with its SIMD assembly (builds need `nasm`; the
  build script refuses to run without it). Encoding is 2.2x faster, decoding
  about 1.3x (4K: 27 → 12.5 ms per frame).
- A fully moving picture streams at up to 60 fps instead of about 17: the
  encoder keeps every frame while the link keeps up, and only drops frames
  once the connection is congested or a fixed bitrate was chosen.
- Bitrate changes no longer rebuild the encoder or force a keyframe.
- The in-flight frame limit follows the link's base round trip, not one
  inflated by queueing.
- Control and input go before video, and video before file transfers.
- Capture reuses frame buffers instead of allocating one per frame, and the
  X11 host skips unchanged frames.
- PipeWire is asked for no more frames than the session uses.
- The viewer shows frames one display refresh sooner.

### Fixes
- Sessions could break when a large message (clipboard text) arrived over a
  slow link while video was flowing: a half-read message was dropped.
- Screens bigger than 4K (5120×1440 ultrawides, 5K) couldn't be shared;
  they are now scaled to fit.
- A frame the encoder skipped could leave the viewer with a stale picture
  once the screen stopped changing.
- Raising the frame rate (e.g. *Low bandwidth* → *Balanced*) didn't speed
  up capture; it now restarts at the new rate.
- On a Windows PC in light mode, the app used egui's default (small) text
  sizes.
- A normal disconnect showed as "connection lost: closed by peer". The
  goodbye message now arrives first, and either side says who ended it.
- "Waiting for the remote side to allow screen sharing" only appears when
  there is a prompt.
- `slipstream update` and `connect` close their connection on errors
  (no more "Endpoint dropped" error lines).

### Downloads and setup
- Signed apps and the Mac app are served exactly as released (one file for
  everyone, so signatures stay valid and browsers stop flagging every
  download as "uncommon"). They connect to their station by themselves,
  from the download address the browser recorded on the file. Failing that,
  there's a **Connect this app** button (a `slipstream://` link, registered
  on Windows and Linux) and a **setup code** on the download page. Codes
  last 30 days and are cancelled by renewing the access key. Unsigned builds
  still carry the settings inside the download.
- New on the Bridge when no station is set: a *Connect to your station* form.
  Config has a *Setup code* field.
- macOS: `scripts/build-macos.sh` builds a universal, ad-hoc signed
  `Slipstream.app`. The app compiles for macOS for the first time, after a
  fix to the capture thread. Updates replace the bundle's contents instead
  of writing a zip over the executable.
- Releases: `.github/workflows/release.yml` builds all three platforms from
  a tag and has SignPath sign the Windows app. `build-station.sh
  --from-release DIR` assembles a station from such a release, checking the
  apps trust this machine's release key.

### Interface
- The local computer is "This device" ("station" now always means the
  organisation's server). Network settings explain themselves without
  developer jargon.
- Station pages use the app's body font, and themed checkboxes.

### Tools
- `tools/slipstream-release` creates the release key and writes the
  signed manifest; `build-station.sh` uses it.
- `slipstream info` shows which release keys the app trusts.

## 0.5.0 — 2026-10-16

The first working release. It was built from scratch in a day and tested
between Linux (KDE Wayland) and Windows 11.

### Remote control
- TeamViewer-style access: permanent 9-digit IDs, one-time passwords that
  change after every session, optional unattended password.
- End-to-end encrypted QUIC (iroh) with NAT hole punching and relay
  fallback. Passwords are checked with SPAKE2 bound to both device keys, so
  they never cross the network and the server can't learn them.
- H.264 (OpenH264, multi-threaded) with GPU colour conversion, ack-paced
  frames (no queueing lag) and adaptive bitrate. Presets: Balanced, Speed,
  Quality, Low bandwidth.
- Mouse, wheel and keyboard by physical key (works across layouts); special
  keys menu (Ctrl+Alt+Del, Alt+Tab, Windows key, …); multi-monitor
  switching; full screen.
- The host's mouse pointer is shown in the video on every platform. On
  Windows it is drawn in by Slipstream, because Windows capture leaves it
  out.

### Files, clipboard, chat
- Two-pane file manager: browse both computers, send and receive files and
  whole folders, make folders, progress and cancel.
- Drag files from your desktop onto the remote screen; they land in the
  folder under the pointer (Explorer windows on Windows hosts).
- Clipboard text sync both ways; chat both ways. On the controlled
  computer, chat opens in its own window.

### Stations (self-hosted service)
- A station is one PHP web folder per organisation. It has a download page
  with preconfigured apps, a private device directory, its own relay
  (started by PHP, routed through `.htaccess`) and an admin page with
  setup, health checks, devices, settings and access-key rotation.
- Runs on ordinary hosting (Virtualmin, cPanel) without Docker, root or a
  database server. Also available as a standalone `slipstream-server`
  binary.
- Private stations: apps carry the station's access key, and outsiders
  can't use the directory or relay.
- Downloads are named after the organisation
  ("Slipstream Remote - Acme IT.exe").

### Updates
- Apps update themselves from their own station: they check 20 s after
  start and every 6 h, verify the SHA-256, swap themselves in and restart,
  keeping their ID and settings. `slipstream update` does the same from the
  command line.

### Platforms and robustness
- Linux: one portable binary (glibc 2.31+) with no required system
  libraries. Wayland through the desktop portal (permission remembered),
  X11 through MIT-SHM and XTest.
- Windows 10/11: single .exe with icon and version info. SmartScreen warns
  because it isn't signed yet.
- Works with a wrong system clock: certificates are checked against the
  station's time.
- Single instance per profile. Closing the window quits; staying in the
  tray is opt-in (and turned on with *Start with system*). Log file in the
  settings folder.

### Interface
- Starship-console theme with its own logo. Animates at the display's
  refresh rate; readouts roll smoothly; live session timer.
