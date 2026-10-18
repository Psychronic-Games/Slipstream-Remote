# Changelog

Build numbers are the git commit count. The app shows them as
`0.5.0 (build N · commit)`.

## Unreleased

### Service install, the remote pointer, lighter software drawing
- Installing the service over an older one no longer fails when the
  installed program is still running (the tray icon at sign-in, or the
  old service's host): it's moved aside and replaced, and the leftover
  is removed next time. When an install or removal does fail, the app
  now shows the actual reason instead of an exit code.
- The remote pointer no longer vanishes on a computer with no mouse
  plugged in (a server or VM reached only remotely). Windows hides the
  pointer there unless another remote tool's virtual mouse is present,
  so it disappeared once TeamViewer had left. It's now drawn as the arrow.
- On the software renderer (Mesa llvmpipe), the console animates at 8
  frames a second (2 when not focused) instead of the display's rate.
  Clicks, typing and remote video still redraw at once. Before, even
  an idle window kept the processor busy.
- The log reads better: local time, no module paths, an identical
  repeating line written once with "… repeated N more times". Windows
  refusing remote input (an administrator window in front, which only
  the service can control) is reported once, with that explanation,
  instead of an error per mouse move.
- System information: each adapter's IPv4 address, subnet mask, IPv6
  addresses (link-local marked) and gateway on lines of their own under
  it. A virtual machine shows as one (VMware, Hyper-V, VirtualBox,
  QEMU/KVM, Xen, Parallels): its "motherboard" is the hypervisor's
  made-up one (VMware's Intel 440BX), not the host's.
- Scrollbars are wider and solid (lavender, orange on hover or drag).

### Windows, file transfer and the console look
- With the Slipstream service installed, the app starts hidden in the
  tray at every user's sign-in and stays there when its window is closed;
  *Config → Tray icon* turns that off per user. Removing the service
  removes the sign-in entry.
- About → *Check for updates* shows that it's checking, and a found
  update right there (with *Install & restart*): before, the banner only
  appeared on the Bridge, so from About nothing seemed to happen.
- System information: subnet masks, default gateways and DNS servers
  (Windows adapter table; Linux routing tables and the resolver's real
  upstream servers; macOS `route`/`scutil`).
- The file transfer window no longer grows back to the bottom of the
  screen: a fraction of a pixel of overshoot each frame (the panes'
  footer and text heights) fed back into its size. It's laid out with
  panels and keeps slack now.
- Dialogs have a console frame: header band with the title and a Close
  segment, a rail down the left joined by elbows, and a footer band.
  Scrollbars are a solid lavender bar on a dark track (orange while
  dragged).

### Technician accounts, saved devices, several stations
- Stations have technician accounts (admin page → **Technicians**). In the
  app, *Config → Stations* signs in to any number of stations, also other
  organisations', whose access key the sign-in hands over.
- **Saved devices** on the Bridge: each technician's list per station,
  kept on the station (follows you to any computer), with online lamps,
  notes, and optionally the device's password for one-click connecting.
  Saved passwords are sealed in the app (Argon2id key from the sign-in
  password, XChaCha20-Poly1305, bound to the device ID): the station
  stores them without being able to read them. ★ on a recent link saves
  it. Station admins manage a **shared** list, from the admin page or the
  app.
- Connecting goes through the device's own station, several at once;
  Reconnect remembers which.
- New API ops `login`, `account`, `device_save`, `device_delete`,
  `shared_save`, `shared_delete`, `logout` (before the access-key check;
  tokens stored hashed; rate-limited). CLI: `sign-in`, `sign-out`,
  `saved`, `save`, `connect --station`.
- Dialogs have console headers (colour cap, title, rule, Close pill)
  instead of a grey title bar, and text sits centred in input fields.

### System information
- *Tools ▾ → System information…* shows the remote computer's name, make
  and model, motherboard, BIOS, operating system and build, processor
  (model, cores, speed, use), memory and page file, graphics cards (with
  their memory and driver), drives and free space, temperatures and
  network adapters. It updates every 3 s while open; *Copy all* puts it
  on the clipboard as text, e.g. for a ticket. Windows usually shows
  temperatures only to administrators and the Slipstream service.
- Collected by the host with `sysinfo` (MIT) plus the registry on Windows,
  `/sys` (and `lspci`, `nvidia-smi` when installed) on Linux, and
  `system_profiler` on macOS. A new `StreamOpen::SystemInfo` request,
  appended to the enum, so no protocol bump: older hosts just don't
  answer it.

### From testing build 19
- **The full window on Windows Server 2016** and other machines whose
  graphics can't run it, with nothing to download or press: the Windows
  app carries Mesa's CPU OpenGL driver (llvmpipe; 23 MB more in the .exe)
  and uses it by itself when there's no Direct3D 12, Vulkan or OpenGL it
  can use. The system's OpenGL is tested in a separate process, so older
  graphics cards still draw in hardware. If the software renderer crashes
  or hangs, the next start uses the compact window, with a *Full window*
  button to try again. `SLIPSTREAM_SOFTWARE_RENDERER=1` forces it. The DLLs
  are pinned by checksum (`packaging/windows/softgl.txt`), and OpenGL is
  delay-loaded so Mesa's copy is used rather than System32's.
- Closing Slipstream on Windows ends the process. Before, it could stay in
  Task Manager without a window: the compact window's Quit button, a
  relaunch after an update, or closing during a session left its message
  loop running (the "destroyed" message arrived while its handler was
  busy), and exiting could hang in a DLL's cleanup or while the async
  runtime waited for blocking work.
- When the software renderer can't draw from the app where it's installed
  (seen on a real Windows Server 2016: Windows then takes System32's
  OpenGL for the window's pixel format), the app copies itself next to
  the Mesa files, Mesa's supported layout, and restarts from there. If
  even that fails, the compact window shows OpenGL's own error.
- Restarting after an update from the compact window no longer depends on
  the old copy finishing its shutdown, which could leave a windowless
  process behind; shutdown itself now gives up after 3 s.
- **Full screen**: the toolbar's menus no longer vanish as you move into
  them; Ctrl+Alt+Enter leaves (or enters) full screen and is never sent
  to the remote computer; leaving the session for the Bridge or another
  page brings the window back, instead of a full-screen Bridge with no way
  out.
- When the remote computer is at its lock screen or a UAC prompt, or
  Ctrl+Alt+Del is sent, and it doesn't run the Slipstream service, the
  viewer is told why the screen is black or nothing happened, and what
  fixes it (Windows hides those screens from normal apps).
- The compact window's ID and password can be selected and copied.
- The compact window's title fits long organisation names on one line
  ("SLIPSTREAM REMOTE · SECTION 31 - TECH" lost its last word), and the
  window says why the full window couldn't start.

### Session tools
- **Copy and paste files** between the computers, like text. Copy files
  in Explorer, Finder or a Linux file manager, click into the remote
  screen and press Ctrl+V: they're uploaded onto the remote clipboard and
  pasted there. Files copied on the remote computer are fetched onto this
  computer's clipboard (up to 64 MB / 500 files at a time; more than that
  is left to *Files*). Needs *Clipboard* and *Files* allowed on the host.
- **Privacy mode** (*Tools ▾* in the session bar): blacks out the remote
  computer's own screens while you work, with a note saying who is
  connected. The black screen is hidden from capture, so you still see the
  desktop. Its keyboard and mouse are blocked too when the host runs as
  the Slipstream service. Windows 10 2004 or later; it switches off when
  the session ends.
- **Port forwarding** (*Tools ▾ → Port forwarding…*, or
  `slipstream connect --forward 13389:localhost:3389`): a port on this
  computer that reaches a service on the remote computer or its network,
  such as Remote Desktop or a router's admin page. Each connection gets a
  QUIC stream of its own. Off unless the host allows it under *Config →
  Permissions → Port forwarding*.
- `PROTOCOL_VERSION` 5: every computer needs this version.

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
- **Slipstream service** (*Config → Incoming access → Windows service*, or
  `slipstream service install`): unattended access that includes UAC
  prompts, administrator windows and the login screen, from boot. A
  LocalSystem supervisor keeps a host running in the console session as
  SYSTEM; the host follows the input desktop, and Ctrl+Alt+Del is generated
  for real (`SendSAS`). Same ID as before; settings in a profile only SYSTEM
  and administrators can read; updates install themselves.
- Windows Server 2016 and other machines wgpu can't draw on (its Direct3D
  12 backend needs Shader Model 6, which Windows 10 1607 doesn't have)
  get a compact window instead of nothing: built from plain Win32 controls
  in the console style (the app's fonts, pill buttons, colour strips). It
  shows the ID, password and link status, takes *Connect this app* links
  and installs updates; the computer can be controlled as usual.
  `SLIPSTREAM_LITE=1` forces it.
- Elsewhere the app falls back to a software renderer when there's no GPU,
  and any start-up failure shows a message box.
- The C runtime is linked in (no Visual C++ redistributable needed), and
  the .exe has a manifest (themed controls, per-monitor DPI, Windows 10/11).

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
