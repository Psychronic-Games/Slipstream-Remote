# Changelog

Build numbers are the git commit count. The app shows them as
`0.5.0 (build N · commit)`.

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
