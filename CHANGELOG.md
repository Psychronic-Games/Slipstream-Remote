# Changelog

Versions follow [semantic versioning](https://semver.org). Until 1.0, a
minor release (0.6 → 0.7) brings new features and a patch release
(0.6.0 → 0.6.1) only fixes. 1.0.0 is the feature-complete, stable
release. After it, the major number goes up for changes that don't work
with older apps or stations. Every station build is a release.
`scripts/release-version.sh` raises the version and files the
"Unreleased" entries under it.

Build numbers are the git commit count. They order builds for updates and
show in small print: `0.6.0 (build N · commit)`.

## Unreleased

### Remote session frame
- Join the title and session controls to an LCARS frame. A slim side rail
  (up to 44 px) occupies existing letterboxing, with a separate swept LINK
  elbow leading into the shifted toolbar. The video keeps its available
  width; the toolbar is shorter, at 36 px. Full screen keeps its auto-hide.
- Flat colored toolbar segments with rounded rectangular corners, narrow
  seams and scalable vector symbols for chat, files, keys, sound, tools,
  picture quality, full screen and disconnect. Narrow windows group actions
  in Console. Tooltips, menus and keyboard shortcuts remain available.
- Round the actual outer window corners: native window regions on Windows
  (including the compact fallback), native shapes on X11, and transparent
  corners on Wayland. Maximized/full-screen windows use square bounds.
- The demo/test pattern waits for pointer input before drawing a crosshair,
  avoiding an initial white line along the picture's top and left edges.

## 0.11.0 — 2026-10-18

### Network scan improvements
- Discover device names from the remote computer's DNS, NetBIOS node status
  and unicast mDNS, with the name and lookup source in the table and export.
  Names remain best effort; unknown names do not delay port results.
- Scan any manually entered, reachable IPv4 subnet or VLAN from the selected
  remote adapter, retaining host/probe limits. Routing/firewalls can prevent
  replies. The topology displays the gateway once, with its results on that node.
- The scanner is an independent desktop window: move, resize or maximize it
  beyond the remote desktop/main window, and keep it open on other app pages.
  Closing it stops outstanding work. Its Close segment contrasts with the rail.

### Desktop and Windows host improvements
- Rounded LCARS title strips replace OS decorations on Linux and Windows:
  drag, double-click maximize/restore, minimize, close and border resizing.
  Controls use down/up delta symbols, overlapping restore panels and a close
  cross, with descriptive tooltips. The scanner and incoming chat use the same
  controls; full screen hides the
  strip. The compact Windows fallback also has LCARS title controls.
- System Information includes present peripherals with reported model/maker,
  installed printer queues and scanners, saved network drive mappings in
  loaded Windows user profiles, and configured shared folders. The SYSTEM
  host can see signed-in users' saved mappings. Queue/mapping availability
  is labeled explicitly. Linux adds USB/CUPS inventory and network mounts;
  inventory is cached for 30 seconds, and URI credentials are omitted.
- Restore incoming Windows chat when the service hosts the session. A local,
  session-checked named pipe forwards messages and replies to the user's GUI,
  recovers bounded history on reconnect, and launches the app as the signed-in
  user when necessary. The GUI verifies the machine identity before sending
  commands; chat is not written to disk or the public service log. The compact
  fallback can display received service-chat messages.

## 0.10.0 — 2026-10-18

### Sharp video on slow hosts
- Balanced and Quality preserve full picture resolution during movement.
  Adaptive resolution had applied to every preset, shrinking the desktop
  on slow encoders (especially server VMs) and sharpening only when still.
  Speed and Low bandwidth retain adaptive sizing. Changing presets resets
  the sizing state immediately; the stats tooltip shows the encoded size.

### Remote network scan
- **Tools → Network scan…** surveys the remote computer's connected IPv4
  network, using its adapter address, subnet mask and reported gateway.
  Choose an adapter, narrow the subnet if needed, and scan common or custom
  TCP ports (comma-separated ports and ranges).
- LCARS table and logical topology views, cyan active selectors with dark
  text and lavender outlined inactive selectors, live progress, Stop, filtering,
  per-device open ports and conventional service names, MAC addresses when
  available, and copying results as text. Topology shows subnet membership
  and the reported gateway; it does not claim to discover physical links.
- TCP and ICMP answers identify devices; neighbor-cache-only entries are
  marked unconfirmed. Windows uses native ICMP; Linux/macOS use system ping
  when available. Silent devices and untested ports may not be discovered.
- Runs on the authenticated host over a separate low-priority QUIC stream.
  Host permissions include Network scan (enabled by default), with subnet,
  probe-count and concurrency limits. Closing the tool or ending the
  session cancels outstanding scanning.
- Session protocol **6**: update both host and viewer together.

## 0.9.0 — 2026-10-18

### The Android app on a phone
- A handheld console on phones held upright: the title and a sensor sweep
  at the top, the sections as a row of buttons at the bottom within reach
  of a thumb, and pages whose forms stack instead of running off the
  right edge. Short windows (a phone on its side) get slimmer elbows, so
  every section button still fits.
- Touch on the remote screen: tap to click, long-press to right-click,
  drag with one finger, pinch to zoom around your fingers and drag two
  fingers to look around the zoomed picture (or, not zoomed, to scroll).
  Before, the first finger of a pinch already clicked on the remote
  computer, and there was no zoom. Touchscreen laptops get the same.
- In a session on a phone, the bar is the back button, a **Console** menu
  (screens, fit to screen, full screen, keys, chat, files, sound, tools,
  quality), the frame rate and End. Before, most buttons were cut off,
  and the screen menu's "▾" was all that showed of it. Chat opens under
  the picture.
- The soft keyboard no longer flickers open and shut: as it slid up it
  covered the field for a moment, and the app took that as the field
  letting go of it.
- The Android app no longer offers an ID and password: it can't be
  controlled from a computer yet, so trying only failed. Its Bridge says
  so, and Config leaves out the incoming access settings.

## 0.8.0 — 2026-10-18

### Android, first steps
- The app builds for Android and runs there: the same console, through a
  small Java activity (`apps/slipstream-android`,
  `scripts/build-android.sh`). It keeps clear of the status bar, the
  camera cutout and the keyboard, types through the soft keyboard, opens
  `slipstream://` station links and plays remote sound. On the emulator
  it goes online and holds a session at 60 fps. Not for users yet: the
  layout is still the desktop's, and touch control of the remote screen
  comes next (issue 09).
- The station's download page offers the Android app (and picks it for
  phones), with how to install it: Android asks once to let the browser
  install apps. The APK is served exactly as built, so its signature holds;
  *Connect this app* on the page links it to the station. It's signed with
  a release key of its own, made on the first build (kept beside the update
  key; back it up, since Android only updates an app signed with the same
  key). The Android app isn't in the update manifest yet: it can't install
  updates itself.
- `scripts/clean.sh` also clears cross-compiled development builds
  (Windows tests, macOS checks), which it used to leave behind.

## 0.7.1 — 2026-10-17

### Windows capture stays on DXGI
- Only the first session after the host started got DXGI duplication.
  xcap's recorder kept a background thread, still holding its
  duplication, after a session ended, so later sessions couldn't
  duplicate the screen and fell back to GDI. On the Server 2016 VM that
  was 35–50 ms a frame, about 20 fps. Windows now captures with its own
  DXGI duplication (`win_dxgi.rs`), created and released by each session.
  It keeps one staging texture, copies once out of it, and hands BGRA
  straight to the encoder: xcap made about four copies plus a
  colour-swap pass per frame. That's part of issue 15.
- Without the service too: if duplication isn't available, GDI fills
  in, and duplication is tried again after 2 s (after five failures, once
  a minute).
- The host's figures in the viewer start with its version
  ("Slipstream 0.7.1 (build …)"), so the log shows which build they come
  from.

## 0.7.0 — 2026-10-17

### 60 frames a second on slow computers
- The video encoder now uses OpenH264's camera mode rather than its
  screen-content mode. Measured on 2 cores at 1280x800 with scrolling
  text, it takes 1.2 ms a frame instead of 3.2, and it uses extra cores,
  which screen mode ignored. It's a little softer while text scrolls
  (29.8 vs 32.3 dB) and much sharper once it's still (49 vs 36 dB). On
  the slow virtual machine that prompted this, encoding (23 ms a frame)
  was what held video at about 35 frames a second.
- Adaptive resolution: if encoding still can't keep up with the frame
  rate, moving pictures are encoded smaller (down to half size), and the
  size climbs back as soon as there's room. A third of a second after the
  picture stops, a full-size frame follows, so text at rest is always
  sharp. The host logs each change ("encoding moving pictures at 60%
  size").
- `crates/slipstream-codec/examples/encode_bench.rs` compares encoder
  settings for speed and quality.

## 0.6.2 — 2026-10-17

### Seeing where video time goes
- The viewer asks the host how fast its video stages run: screen
  capture (and how: DXGI or GDI), encoding, waiting for a new screen
  frame, and waiting for the viewer. The stats tooltip shows them, and
  every 30 seconds the viewer's log gets one line with the link's frame
  rate, bitrate, round trip, path and decode time plus the host's
  figures. `slipstream connect` prints them too. Older hosts just don't
  answer.
- The toolbar shows the bitrate in kbit/s below 1 Mbit/s. A still screen
  streams at a few dozen kbit/s, which read as "0.0 MBPS".

## 0.6.1 — 2026-10-17

### Faster video from the Windows service
- With the service, the host captured the screen as a full GDI screenshot
  every frame, through a library call that made a new screen-sized bitmap
  each time and swapped every pixel's colour bytes. On a machine without
  a graphics card that capped video at about 20 frames a second. Now:
  - DXGI Desktop Duplication on the normal desktop, as the app already
    uses without the service. It's fast, and costs nothing while the
    screen is still.
  - On the login screen and UAC prompts, which duplication can't follow,
    a GDI capturer that keeps one bitmap and hands frames on as they are.
  - The host switches between them as the desktop changes.
- The host logs its pace every 30 seconds (frames a second and
  milliseconds per frame, for capture and for encoding), so a slow stage
  shows in the log. *Config → Service log* opens it.

## 0.6.0 — 2026-10-17

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
- Progress bars (update download, file transfers) are console bars whose
  text reads black over the fill and light over the track. Before, it was
  light orange on bright green. The update bar shows percent and size.
- Config → Windows service shows the service's real state (Running,
  Starting…, Installed but not running, Not installed), kept current.
  Before, a stopped service read "Not installed". A stopped one offers
  *Start* and *Remove*. Install now checks that the service keeps
  running, and if it doesn't, says so with the end of the service's
  log. An app started while the service is still starting no longer
  hosts beside it, where the two would push each other off the station.
- A computer no longer gets a new ID when its identity file can't be read
  for a moment (Windows antivirus scanning the copy the service install
  just made). Before, any failed read made a new identity, so the
  service turned up on the station under a different ID from the app's,
  and the app's ID read offline. Now only a missing file makes a new one;
  an unreadable one is retried and then reported. Install also checks
  its copy matches.
- While the service hosts, the Bridge shows the ID the *service* is
  registered under (it publishes it, readable by every user, in
  `ProgramData\Slipstream Remote\public`). If that differs from the
  app's own ID, the Bridge says so and how to make them match. Before,
  it showed the app's ID, which partners then found offline.
- A failed link is written to the log, with its reason.
- One host per computer: once the service is installed it is the host,
  whether or not it's running at that moment. Before, an app started
  while the service was stopped hosted too, under the same key. The two
  pushed each other off the relay, and links failed with both saying
  they were connected. Quitting such an app also marked the computer
  offline. Apps that only connect out (the app while the service
  hosts, `slipstream connect` and other command-line tools) use a
  throwaway key, so they never collide with the computer's host.
- The app keeps an installed service running: if it's stopped, the app
  starts it (no administrator prompt) and logs that. The Bridge shows
  the service's live state with *Start the service* when it's down, and
  Config's buttons follow the live state too.
- The service logs to `ProgramData\Slipstream Remote\public`, readable
  by every user. *Config → Service log* opens it (or the old,
  administrators-only log from before build 34).
- The app no longer starts the service while it's being installed,
  started or removed. An install stops the service to replace the
  program, and starting it in between ran the old program and failed the
  install's own start.
- Reinstalling the service failed with "Access is denied" on the
  identity, and older builds could give the computer a new ID. Each
  install set the profile's permissions file by file (`icacls /T`). On a
  reinstall that left the files already there with no permissions at
  all, so the install couldn't replace the identity and the service
  couldn't read it. The folder's permissions are now set on the folder
  alone and everything inside is reset to inherit them. The service runs
  that repair whenever it starts, so damaged installs heal. The identity
  is now swapped in as a fresh file instead of copied over the old one.
- Versions: semantic versioning from here on (see the top of this file).
  Each station build is a release with its own version.
  `scripts/release-version.sh` raises it, and the station build refuses
  unreleased changes. The Windows .exe's version info now comes from
  Cargo.toml, with the build number as its fourth part.
- When the service won't install or start, Config (and the Bridge) shows
  Windows' own explanation, selectable and with *Copy*: the service's
  state and exit code, its command line, and the Service Control
  Manager's messages about it from the System event log. Console tools
  the service code runs no longer flash a window.
- The service may be started by signed-in users (not stopped or changed),
  so *Start* on a stopped service works without the administrator
  prompt, falling back to reinstalling if needed. After an install the
  app also checks that the service is running and starts it if not.
  `slipstream service start` does the same from a command line. The
  service log records its build when it starts and why it stopped.

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
