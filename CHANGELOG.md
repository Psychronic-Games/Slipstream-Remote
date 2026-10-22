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

- Repair enabled Linux autostart entries when a moved or newly downloaded app
  starts, and refresh the executable when saving Config. Respect entries disabled
  by the desktop and leave side-profile startup integration alone.
- Add search to Bridge's Saved devices across private/shared lists and stations,
  matching name, ID, group and station. Open matching groups during search and
  share filtering with the New link picker.
- Detect a viewer stopping its video stream even when the encoder has exhausted
  frame credit, ending the session instead of leaving a frozen link. Strengthen
  the existing regression to reproduce cancellation after the last allowed frame.

## 0.23.2 — 2026-10-21

### Meeting previews and tab groups

- Show refreshed, local-only screen thumbnails in the sharing chooser and
  desktop arrangement map. Preview one screen at a time at low frame rate;
  stop preview work when the chooser closes and publish only after confirmation.
- Exclude Linux webcam metadata endpoints from camera choices and repair a saved
  invalid selection when one usable camera remains. Negotiate advertised camera
  modes instead of requiring YUYV at 640×480; try supported alternatives.
- Allow every native session window to contain a group of tabs. Drag tabs into
  another floating group or the main console, reorder them, or tear them out.
  Keep the connections alive and send remote input only to the selected tab.
- Keep new links in the window where + was clicked, including saved-password
  prompts, even when the separate-windows preference is enabled. Close an empty
  source window after moving its last tab; retain siblings when one tab closes.
- Isolate native QA clients on a private D-Bus session as well as Xvfb, preventing
  fixture icons from appearing in the user's real system tray. Explicitly shut
  down the Linux tray service when its owner is dropped.

## 0.23.1 — 2026-10-21

### Meeting usability

- Split meetings into Live meeting, Chat, Participants and Devices tabs, with
  media controls always accessible. Show unread chat and waiting participants;
  support Enter to send and Shift+Enter for a new line.
- Replace the empty microphone field and numeric camera index with named
  device pickers, the current system-default microphone and remembered choices.
  Refresh device lists without starting capture; apply device changes to active
  media. Discover Linux microphones through runtime-loaded PulseAudio, excluding
  loopback monitors, and use stable microphone identities on Windows/macOS/Android.
- Ask which screen to share before starting capture, with names, resolution,
  main-display status and a desktop arrangement map. Cancel preserves current
  sharing; Change screen uses the explicitly selected display without fallback.
- Show Cancel call only while dialing/ringing and remove the invitation status
  after Answer. Leave meeting and End meeting control the ongoing room.
  Confirm organizer end/removal actions before disconnecting participants.
- Replace Lock meeting with an explained Allow new join requests setting in
  Participants. Closing admission blocks new arrivals and keeps current members
  connected; existing waiting guests can still be admitted.

## 0.23.0 — 2026-10-21

### Group meetings and mobile Show me

- Add participant-hosted meetings with a default capacity of 20 (configurable
  to 32), organizer admission, lock/remove controls, invitations, shared
  Subspace chat and independent remote-support tabs. Forward encoded media
  through the organizer with up to four selected remote video sources.
- Add a Meeting action for saved systems, authenticated incoming calls with
  Answer/Decline, cancellation, call preferences and the Windows service bridge.
  Answer starts muted; camera and screen sharing require a separate action.
- Add microphone voice audio with echo cancellation, noise suppression,
  level control and mixed playback. Pause support loopback capture while
  meeting audio plays to avoid recapturing the conversation.
- Add desktop camera/screen sharing and Android Show me with rear/front camera,
  local preview, torch where supported, runtime permissions and a foreground
  notification that can stop camera and microphone capture.
- Give meeting endpoints expiring relay-only guest leases. Invitations never
  carry the station access key. Keep remote-control protocol 6 unchanged.
  Meetings last up to four hours and end when their organizer leaves.

## 0.22.1 — 2026-10-20

### LCARS interface polish and automatic device recognition
- Place the new-link + after the last tab, or immediately beside Bridge when
  there are no sessions. Remove the empty-strip description and keep overflow
  scrolling within the tab rail.
- Give New Link the full LCARS header, left rail and footer used by styled
  panels. Show private/shared groups and empty groups, with searchable system
  cards, saved-password indicators and an expandable manual link form.
- Integrate the device editor's group chooser into the group field; align its
  menu with the field. Use consistent frames for connection/tray dialogs,
  clip long headings and keep device actions visible beside long names.
  Keep settings fields and station sign-in controls within compact panels;
  draw file-browser refresh icons without platform-dependent glyphs.
- Rename the settings status to Unique device detection and remove imaging
  instructions from the interface. On Linux, recognize permanent built-in PCI
  network hardware when firmware files are restricted to root; preserve
  existing identities when learning this fallback or adding/removing adapters.

### Build cache housekeeping

- Run cache housekeeping before and after station/Android builds and commands
  run through `scripts/check.sh`: 12 GiB for combined development/test caches,
  32 GiB for combined release caches, and two generated APK versions per variant.
  These are cleanup thresholds; a running command may exceed them until it ends.
- Disable incremental compilation, reduce development debug information and
  run Gradle without a persistent daemon, with two workers. Preserve sources,
  signing keys, profiles, assembled station clients and deployed rollback backups.
  See [BUILDING](docs/BUILDING.md#disk-space) for inspection and cleanup commands.

## 0.22.0 — 2026-10-20

### Faster linking and compact desktop tools
- Keep the animated menu-bar pulse inside its rounded segments and gaps.
- Add + Group on each signed-in station's Saved devices panel, including
  empty groups synced to the technician account, Add device within a group,
  and removal of empty groups. Older clients retain the new group metadata.
- Add a fixed + button to console/floating tabs with a searchable picker for
  recent links, station-saved/shared devices and manual IDs. Reuse saved
  passwords and station routes; retain separate recents for the same numeric
  ID on different stations.
- Tighten desktop menus, panel gutters and dialog spacing; allow a smaller
  console and file browser with full-width path fields. Replace transfer
  arrows and recent-link favourite stars with font-independent vector art.
- Prevent Windows native captions from flashing over custom tool headers,
  register native tools once, and ignore late file/preview/progress repaints
  for closed windows. Covers the browser and transfer-progress window.

### Linux screen sharing and file icons
- Detect KDE portals still using an old Qt version after a system update
  and refresh them automatically only when the portal reports no active
  sessions or requests. Preserve saved approvals and leave other sessions
  alone; show a useful error when safe automatic recovery is unavailable.
- Bound portal startup/approval waits, close abandoned sessions, provide
  Cancel/Retry controls and invalidate cached access when the desktop revokes
  it, so future connections can prepare fresh access.
- Identify Linux executables with a stable ELF note and install their MIME
  icon on first normal launch. Renamed and station-provisioned copies retain
  the logo in supporting file managers; unrelated executables keep their icons.

## 0.21.1 — 2026-10-19

### Support tool reliability
- Repaint floating session windows only while they exist, preventing video
  updates from repeatedly creating blank native windows over the console/tools.
- Cancel pending scanner and report requests when their window closes or the
  link drops, discard late tunnel replies, and release scanner-owned tunnels
  immediately on interruption while retaining established tunnels on window close.
- Give compact Windows hosts session-specific Dismiss / Stop support controls;
  clear ended/cleared messages without hiding another technician's status.
- Keep retained Windows service command senders in step with service restarts,
  fixing compact-window code rotation and support actions created before sign-in.
- Limit DNS diagnostic results to the queried name's IPv4 answers and CNAME
  chain; reject wrong-class replies and alias loops.
- Add regressions for request cancellation, tunnel lifetime, screenshot PNGs
  and failed exports, independent wake permissions, and service command epochs.

## 0.21.0 — 2026-10-19

### Support tools
- Connect to selected discovered TCP services through the remote workstation;
  retain loopback tunnels with the session and open standard HTTP/HTTPS locally.
- Review/export a support report with inventory, public IP, connection/network
  results, technician notes and an explicitly selected remote screenshot.
- Send peer-assisted LAN wake packets with a separate opt-in host permission,
  adapter-derived broadcasts, strict MAC checks and a bounded request rate.
- Check gateway ICMP, configured/comparison DNS answers and the host's station
  API with bounded, cancellable diagnostics and copyable results.
- Show a plain-language host status with Dismiss / Stop support, including the
  Windows service bridge and compact app. Wake occluded/hidden host consoles
  for incoming events so status does not wait for a click.

### Automatic cloned-device identities
- Bind desktop profiles to locally hashed firmware UUID/serial evidence.
  An imaged profile starting on different hardware generates a fresh random
  key and station ID while retaining station/unattended/service configuration.
- Clear stale cached/public IDs, serialize concurrent starts, retain identity
  on missing evidence, and show firmware availability in Config. An updated
  master app/service must run once before capture to establish the baseline.
- Keep existing service identity during updates/reinstalls, including when a
  clone's user and machine profiles have independently rekeyed.

## 0.20.0 — 2026-10-19

### Session tabs and floating windows
- Keep desktop links in a switchable tab strip, reorder tabs by dragging, or
  drag a tab out into its own native window. Drag the floating tab back into
  the main strip to dock without reconnecting. Tabs and floating sessions can
  coexist, with Pop out / Dock and a right-click menu as alternate controls.
- Save a Tabs / Separate windows default for new connections under Config →
  Viewing. Existing profiles default to tabs and existing links keep their
  layout. Closing a tab/window ends only that link.
- Track native pointer movement/release during Windows/X11 tab drags so drops
  outside the source window work; normalize cross-window targets for display
  scale and keep drag gestures from sending remote keyboard input.

## 0.19.1 — 2026-10-19

### Session identification
- Keep the workstation label in the toolbar when Android or full screen hides
  the native title bar. Compact Console menus also show the full workstation
  name, including when the toolbar has too little room for it.

## 0.19.0 — 2026-10-19

### One-time codes, session windows and toolbar layout
- Show the Windows service's actual one-time code alongside unattended access,
  with Copy and New controls in the full and compact apps. Both passwords
  authenticate the same host ID. Codes rotate after sessions, clear when the
  local service disconnects, and travel only through its authenticated,
  session-checked local pipe; service stdout no longer prints them. Older
  services retain unattended access until updated.
- Pop a desktop session into its own native window, open/focus windows from
  Links, and dock them back without reconnecting. Several workstations can
  remain visible at once. Closing a session window ends only that link;
  Bridge opens the console while the link continues. Reconnection preserves
  the window, quality/display choices and tools.
- Route keyboard capture and pending shortcuts by viewport, release held input
  on focus loss, and keep background tool repaints from stealing control.
  Files and Network Scan can open while the console is hidden.
- Put the workstation name in the top title bar beside the app name/window
  controls. Place FPS/latency next to picture quality. Measure the remaining
  toolbar space before showing labels; retain compact buttons at medium
  widths and a Console menu in very small windows. Screen selection uses a
  menu so multiple monitors do not crowd out the other controls.

## 0.18.0 — 2026-10-19

### Screen access and tool cleanup
- Hide detached File Transfer, transfer receipts and Network Scan immediately
  when closed, and repaint an occluded parent to finish native-window cleanup.
  Redraw System Information's closing frame without the panel so it cannot
  leave a black rectangle waiting for another click.
- Prepare Wayland screen/control permission when the desktop app or headless
  host starts, keeping the approved session available while running in the tray.
  Config adds Approve now and Reset / approve again; desktop restore tokens
  are saved when supplied, and resetting refuses to interrupt incoming viewers.
- Show platform-specific screen-access guidance: Windows needs no portal
  approval and its installed service provides access in the background;
  Android retains its explicit Start sharing approval.
- Give station download buttons an explicit app filename, including the APK,
  and simplify ASCII attachment headers for older mobile downloaders. Keep
  signed APK bytes unchanged and retain UTF-8 names for non-ASCII station names.

## 0.17.1 — 2026-10-18

### Audit: transfer and session reliability
- Give each upload/download a private staging file, remove it on failure or
  cancellation, and publish without overwriting concurrent/existing files.
  Require the declared bytes and clean end of stream before reporting success.
- Reject non-regular transfer sources, bound reads to the advertised length,
  stop clipboard/remote symlink cycles, and preserve Windows UNC share paths.
- Release held input after controller disconnects, observe video-writer errors,
  interrupt full encoder queues, bound decoder buffering, and reject future
  frame acknowledgements. Support non-contiguous display IDs.
- Close active connections with their tunnel, preserve normal half-closes,
  fix active-count races and IPv6 targets, and own/bound file-request tasks.
- Publish one identity across concurrent first starts and atomically save private
  settings with unique staging names and serialized snapshots.

### Audit: responsiveness and bounds
- Move local folder listings/places off the UI thread, bound listing workers,
  ignore stale navigation replies, and register detached Files windows while
  the parent is hidden. Cache lowercase directory-sort keys.
- Stream update hashes instead of rereading the whole executable, bound
  provisioning trailers and response waits, clean failed update staging, and
  use a unique non-destructive write probe.
- Retain Android's final pending screen image at the capture interval and close
  replaced image readers on their acquisition worker. Disable Android UDP
  segmentation offload for direct-link compatibility; other platforms can
  request the same behavior with SLIPSTREAM_DISABLE_GSO=1.
- Bound audio-device startup/shutdown waits, pair frame serials with pixels,
  and invoke repaint callbacks outside their lock.
- Validate foreign PipeWire/raw-frame bounds without integer overflow. Reject
  extreme signed timestamps safely and ignore spoofed forwarding prefixes
  behind the Rust server's immediate local reverse proxy.
- Add regression coverage and document findings and remaining limits in
  docs/AUDIT-2026-10-18.md. Repair outdated Windows cursor test coverage.

## 0.17.0 — 2026-10-18

### Android hosting and device ID
- Register Android as a persistent host, show its ID and password on the home
  console, and name new phones from the manufacturer/model instead of leaving
  Android in viewer-only mode with no ID.
- Add Start sharing / Stop sharing controls, Android's screen-capture consent
  flow, and a media-projection foreground service with a visible Stop action.
  Capture survives backgrounding, handles rotation without reusing consent
  tokens, and ends connected sessions when the phone revokes sharing.
- Add an explicitly enabled AccessibilityService for remote taps, held-touch
  drags/long presses, wheel swipes, basic typing/editing and Android navigation.
  Guide sideloaded-phone users through Accessibility/App info setup, retain a
  view-only mode, and add Android Back/Home/Recents actions to the PC Keys menu.
- Bound capture to 1280 pixels on the long edge and 30 fps, copy padded RGBA
  planes into reusable latest-frame slots and use the existing H.264 encoder.
  Seed new viewers from the latest still image and reject revoked cached
  capture sessions until the phone approves sharing again.
  Hardware encoding, Android host audio/clipboard/files and unattended capture
  without on-device approval remain future work.

### Drop-transfer receipts
- Automatically open a compact, independent LCARS progress window for file
  transfers, including desktop drops onto the remote screen or a folder pane.
- Show file/batch names, resolved destinations, byte counts, progress and
  preparing/sending/confirming states. Mark Complete only after remote save
  receipts; retain completed, failed and cancelled results until dismissed.
- Let users cancel active work, clear finished history or open the full file
  browser. Dismissing the panel leaves transfers running and a new transfer
  opens it again. Preserve cancelled/interrupted outcomes against late replies.

## 0.16.0 — 2026-10-18

### File previews and sorting
- Show image thumbnails in both File Transfer panes, with a larger hover
  preview; support PNG, JPEG, GIF (first frame), WebP, BMP, ICO and TIFF.
- Preview plain-text notes, logs and source files as passive page icons and
  readable hover text. Unsupported, corrupt or oversized items keep LCARS icons.
- Load only visible-row previews in the background with bounded concurrency
  and a 128-entry cache per pane. Generate remote previews on the host, sending
  at most a 128-pixel RGBA image or 8 KB of text instead of the original file.
- Sort each pane independently by Name or Modified, click again to reverse,
  keep folders first and retain sort choices across navigation and reconnects.
  Selection and transfers continue to reference the original listing entries.
- Clear preview caches on refresh/navigation and reject stale asynchronous
  results. Older hosts keep normal browsing and generic icons; previews use
  the existing file-browsing permission over an optional protocol-6 stream.

## 0.15.0 — 2026-10-18

### Automatic link recovery
- Detect an unresponsive remote host with bounded performance-probe requests,
  independently of video frame rate, and show a reconnecting overlay over the
  last picture instead of silently waiting for a transport timeout.
- Announce host updates before replacing the program, using a compatible
  restart reason and close code so graceful service updates also recover.
- Retry interrupted desktop links for up to two minutes with fresh device
  lookups and bounded, backed-off attempts. Cancel reconnect and End Link stop
  recovery; explicit session endings and authentication rejections never loop.
- Restore the same tab, chat, display, quality and tool windows after the host
  returns. Release held input, clear old video textures and require a click to
  regain control. Interrupted transfers and port forwards are not replayed.

### Detached file transfer
- Give File Transfer its own movable, resizable LCARS desktop window, with
  the same curved rails and separated window controls as Network Scan.
- Draw LCARS folder/file symbols as vectors rather than platform emoji, with
  segmented rails and theme colors that scale cleanly with the interface.
- Keep transfers running when that window is closed. Support desktop file and
  folder drops onto its remote pane, including when the main window is hidden,
  and keep drop coordinates scoped to the correct native viewport.
- Preserve transfer history across reconnects, mark interrupted jobs failed,
  and reload remote folders through the new authenticated session. Android
  embeds the tool on backends without native secondary windows.

## 0.14.1 — 2026-10-18

### Remote control focus
- Background repaints of the detached scanner no longer release remote
  control. Taking focus in a tool still releases held remote input, and
  tool keyboard events stay local. This prevents control-prompt flicker
  around remote browser launches and scan/name updates.

### System Information
- Use a red Close button to distinguish it from the yellow header rail.

### LCARS frame corners
- Smooth the lower-left outer rail/footer joins with full-size corner curves
  in Network Scan, System Information/shared dialogs and the main session.
- Give native Minimize, Maximize/Restore and Close controls more separation
  so their surrounding outlines do not touch.

## 0.14.0 — 2026-10-18

### Seamless network scanner frame
- Replace the stacked scanner headers with one cyan title rail flowing into
  the left frame through a curved elbow. Inset symbolic window controls,
  a contrasting peach Close action and a blue footer keep the corners clear.
- Keep the frame fixed while the scanner body scrolls at compact sizes;
  preserve detached-window dragging, resizing and maximize/restore.

### Public IP in System Information
- Show the remote computer's public internet address near the top of System
  Information, with Copy IP and inclusion in Copy all. Lookup runs on the
  host over HTTPS, independently of hardware inventory and live statistics.
- Prefer IPv4 with dual-stack fallback using ipify. Share a one-minute cache
  across viewers; Refresh can recheck after five seconds. Failed lookups
  show Unavailable, back off briefly, and leave system details usable.
- Use an optional appended protocol-6 stream; existing hardware inventory
  and sessions stay compatible with older clients/hosts.

## 0.13.0 — 2026-10-18

### Remote web links from network scans
- Highlight open HTTP (80) and HTTPS (443) services in an outlined LCARS
  section, with blue HTTP and green HTTPS actions. Select a topology node
  to use the same links in its device details.
- Clicking opens that IP's HTTP/HTTPS address in the remote computer's
  default browser, including the signed-in user's browser for Windows
  service hosts. Show pending, acknowledged and failed launch requests.
- Keep browsing and scan traffic remote. The optional protocol-6 browser
  stream requires an updated host and existing network-scan/remote-control
  permission; it accepts only typed unicast IPv4 addresses and ports 80/443.

## 0.12.0 — 2026-10-18

### Desktop drag and drop
- Receive Linux desktop file/folder drops using X11/Xwayland when available,
  including the actual pointer location during a drag. The remote capture
  backend continues to use the Wayland portal when appropriate.
- Drop over a Windows Explorer window to send into its open folder, or
  elsewhere on the remote picture to send to the signed-in user's Desktop,
  including redirected Desktops and service-hosted sessions. Resolve the
  destination once per batch so later pointer movement cannot scatter files.
- Preserve empty directories and nested folder structure, avoid following
  symlink cycles, and show progress/errors in Files. Drops on local toolbar
  buttons, letterboxing and dialogs do not start an unintended transfer.
- The optional drop-folder stream requires an updated remote app. Existing
  protocol-6 sessions and file browsing/transfers remain compatible.

### Saved devices and dialog layout
- Organize private and shared saved devices into collapsible named groups.
  Type a new group or choose an existing one in Add/Edit; an empty group
  means Ungrouped. Groups sync through the station, and updates from older
  clients preserve them.
- Add this device fills in the local host ID and computer name, using the
  Windows service's host ID when applicable. Existing entries keep their details.
- Notes wrap across multiple lines and grow while typing; longer forms scroll
  while Save stays accessible. Give Group and Notes more breathing room.
- Use contrasting peach Close and green Save actions beside the violet device editor rail.
  The dialog elbow correction also keeps the bottom System Information rows,
  including DNS servers, clear of the decorative curve.

## 0.11.4 — 2026-10-18

### Modal frame correction
- Keep LCARS elbow cutouts inside their decorative rail area. The footer
  curve could paint over the lower-left corner of Save in the device
  editor, and over nearby controls in other floating dialogs.

## 0.11.3 — 2026-10-18

### Menus and communications window
- Align console dropdown frames beneath their toolbar segments, with a
  narrow gap, matching rounded corners and consistent menu-row padding.
  Selected choices use blue on a dark blue background instead of brown.
- Rename Privacy mode to Blank remote screen, align it with the other Tools
  entries, and explain its purpose and platform requirements in its tooltip.
  It remains optional and unavailable on systems without capture exclusion.
- Give incoming Subspace chat a lavender LCARS title strip, contrasting peach channel band,
  swept left rail and blue footer. Both ends use framed messages and a
  composer anchored at the bottom with a rectangular Send segment.
- Remove the one-pixel native shadow strip above borderless Windows content,
  including the separate chat window, while preserving maximized work-area
  handling and the app's own resize/title controls.

## 0.11.2 — 2026-10-18

### Remote session frame corrections
- Show the computer-name section's rounded right corners; the control-row
  clip previously cut them off on the full toolbar.
- Keep the slim left rail visible in windowed desktop sessions at every
  video aspect ratio and display scale, including while zoomed or waiting
  for a picture. Reserve frame space outside the video and its input area.
- Extend the blue footer across the window below the remote taskbar, and
  leave a black break between the right rail and End Link.
- Outline the dark FPS/latency panel in subtle blue, including the compact
  toolbar, so the connection figures have their own instrument section.

## 0.11.1 — 2026-10-18

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
