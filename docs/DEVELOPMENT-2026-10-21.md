# Development handoff — 2026-10-21

## Group meetings and Show me

Implements the requested participant-hosted group meetings, default 20 and
configurable up to 32, without a separate cloud media service. See
[meeting usage](MEETINGS.md) and [scope/status](issues/17-group-meetings-and-show-me.md).
The organizer coordinates admission and forwards encrypted-transport H.264
and Opus; each sender uploads one copy. Up to four remote video sources are
selected. Camera/screen sources use 15 fps, 800 kbit/s and a 1280-pixel long edge.
Frame acknowledgements bound in-flight video to two frames per source and
subscriber, with keyframe recovery and control priority.

Meetings and incoming calls have separate ALPNs, version 1. Remote-control
protocol 6 remains unchanged. Pending lobby guests receive no roster, chat or
media. Membership binds to the authenticated endpoint; one-use call tickets
expire in two minutes. Rooms/general invitations expire after four hours.
Organizer departure ends the room. Admission lock, removal and bounded queues
are enforced by the forwarding host, not just the interface.

Private stations issue HMAC relay-only guest leases through authenticated
meeting_relay. Meeting endpoints use only guest leases before binding, so
transport address exchange cannot leak the master station key. The directory
never accepts a guest lease as a station access key. Calls prove station
access with a nonce bound to both endpoint identities and the invitation.
Windows service commands remain authenticated and epoch-bound; media stays
in the interactive app. Compact hosts return unavailable.

Microphone capture uses Opus VOIP, Sonora's pure-Rust WebRTC AEC3/noise
suppression/AGC, and the actual speaker render reference. Playback mixes admitted
sources with bounded jitter queues. Support loopback capture pauses during
call playback to avoid feeding the room back into remote support audio.
Camera capture uses native desktop backends and Android Camera2. Android adds
camera/microphone foreground-service types, runtime permission handling,
rear/front selection, torch, preview and local notification stop. Incoming
notifications open the app to Answer or can Decline directly; there is no
force-stopped-app push wake-up.

## Validation

- Full workspace suite: 155 passed, zero failures, five existing ignored checks.
- Linux all-target workspace Clippy and Windows all-target client Clippy pass
  with warnings denied. Android arm64 Clippy also passes with warnings denied.
- Twenty real iroh participants exercise admission/lobby/capacity, chat,
  identity rewriting, Opus packets, H.264 encode/decode, removal and end.
  Call checks cover authorization, answer, decline, cancellation, single-use
  tickets and remote-control independence. Native two-client PHP/Xvfb evidence
  under /tmp/slipstream-meetings-ui-u74a_44v shows incoming Answer, preview,
  decoded peer camera, group chat, camera stop and organizer teardown.
- Android x86_64 debug APK runs in a read-only emulator with disposable app
  data. Notification/camera/microphone permission prompts, real Camera2 local
  preview, unmute, and notification Stop camera and microphone were exercised.
  The UI returns to muted/no camera while the room remains active; dumpsys
  confirms no capture service remains. Evidence:
  /tmp/slipstream-meetings-android-mq_hb3od. All fixture processes stopped.
- An isolated private Rust station passed the meeting smoke with
  SLIPSTREAM_RELAY_ONLY=1 and public relays disabled: guest lease, admission,
  group chat, real Opus/H.264 encode/decode and end. Evidence:
  /tmp/slipstream-meetings-relay-local-nhd67r8x. This also validates waiting
  for the new endpoint to select its relay before creating an invitation.
- All substantial checks use scripts/check.sh serially with the project
  memory, job and cache limits. Fixture profiles do not touch real identities.

A 20-member synthetic transport check is not a physical 20-camera benchmark.
Automatic active-speaker selection, adaptive bitrate, invitation rotation,
room renewal and host migration are follow-up work.

## Release and publication

Published **0.23.0 / build 119 / 9c7ee24** to
https://relay.slipstreamremote.com through the existing mounted station.
Feature source is b43ca63; ebdff85 waits for initial relay selection;
aeba54b filters invitation public routes according to both stations' policy;
9c7ee24 preserves Windows startup without optional Media Foundation DLLs.
Later documentation commits do not change the published client metadata.

The final meeting checks pass (four tests), including the 20-member room,
frame-credit replenishment and public-route policy regression. Windows Clippy
was rerun after the optional-camera-component fix with warnings denied.

Package verification passed:

- Signed desktop manifest, file hashes and sizes use the unchanged release key.
  Linux starts as 0.23.0 / 119 / 9c7ee24. Both ELF programs have a maximum
  referenced glibc version of 2.30 and no GLIBC_ABI_DT_RELR requirement; the
  portable build targets glibc 2.31.
- Windows version resources are 0.23.0.119 and bundled Mesa remains present.
  Media Foundation DLLs are delay imports. The packaged client starts under
  isolated Wine with mf.dll, mfplat.dll and mfreadwrite.dll explicitly disabled.
  Evidence: /tmp/slipstream-0230-windows-startup-wlgxwt5i. Fixture processes and
  the disposable Wine prefix were removed. Physical camera hardware remains
  subject to the existing native-device follow-up.
- Android is arm64, non-debuggable, versionName 0.23.0 / versionCode 119,
  with camera/microphone foreground-service permissions. Its certificate is
  unchanged: d6ae3771378e9c00a82902e728012992834b9bf7e234fefed80dc85a4b879c6c.

Publication first replaced api/index.php and the managed relay binary,
preserving their ownership/modes, then restarted the managed relay. The
mount's SSH login was unavailable for a separate shell, so a random, one-use
PHP endpoint performed only that fixed restart. It required a random header
token (only its hash stored), expired in five minutes, invalidated the API
opcode cache and deleted itself before restarting. It was also removed in
client cleanup; zero deployment endpoints remain. Station configuration,
accounts, keys and user identities were preserved.

A live meeting smoke passed against the upgraded production station with
SLIPSTREAM_RELAY_ONLY=1 and public relays disabled: independent guest relay
lease, admission, group chat, real Opus/H.264 encode/decode and teardown.
It reads station settings without starting or changing the real client identity.

Clients were then staged, verified and published with the manifest last,
expecting **0.22.1 / 113 / 61367b6**. Authenticated live update offers and full
Linux/Windows/Android HTTPS downloads verified as **0.23.0 / 119 / 9c7ee24**,
including desktop provisioning trailers and unchanged signed APK bytes.

Private rollback backups, outside the public web root:

- Runtime: /home/doug/WebServer/home/relay/tmp/slipstream-runtime-rollback-0.23.0-jeom1qb9
- Clients: /home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.22.1-to-0.23.0-7172q_j5

Runtime rollback.json records original hashes, UID/GID and modes. Restore the
API and binary atomically with that metadata, then restart the managed relay.
For a client rollback, restore verified client files with current preserved
ownership/modes and restore the old manifest last. Keep the backups.

Desktop users update through About → Check for updates → Install & restart.
Android users install the station's new APK. Installed Windows services also
need the new build for incoming meeting calls. Publication does not replace
the user's already-running desktop app.

Final cache sizes: 9.31 GiB development/test and 30.05 GiB release, within
12/32 GiB budgets; incremental compilation stays disabled. The final release
build log is /tmp/slipstream-0230-release-build-final.log. Earlier documentation
reconciliation edits remain separate from this implementation/publication.

## Meeting usability follow-up — 0.23.1

User testing found blank device fields, automatic primary-screen sharing,
a crowded meeting page, an unclear lock control and a Close call button that
only dismissed invitation status after Answer. This patch changes the client UI:

- Named microphone/camera pickers, remembered selections, current device labels
  and asynchronous refresh. Linux enumerates real PulseAudio inputs (including
  PipeWire's Pulse server), excludes loopback monitors and resolves the named
  system default. Other platforms use CPAL stable microphone identities.
- Explicit screen selection with resolution, main display and arrangement map;
  cancellation preserves the current source. Capture requires the chosen display
  index and fails if it has disappeared, without falling back to another screen.
- Live meeting, Chat, Participants and Devices views with persistent media/end
  controls, unread chat and Enter/Shift+Enter composition.
- Cancel call exists only while preparing/dialing/ringing. Answer clears that
  status; Leave meeting exits a guest and End meeting confirms ending everyone.
- Participants explains Allow new join requests. Closing admission leaves current
  participants connected; requests already waiting can still be admitted.
  Organizer end/removal actions ask for confirmation.

The isolated native two-client audit passes with synthetic media and two screens:
named camera choice/persistence, cancel and second call/Answer, removal of stale
call controls, encoded/decoded camera video, separate chat/unread/Enter sending,
screen chooser cancellation, explicit second-screen sharing, 480×800 layout,
cancelled End confirmation and final group teardown. Evidence:
/tmp/slipstream-meetings-ui-nhuel3i4. No real user profile, calls or capture were
used. A separate metadata-only microphone example correctly listed this Linux
workstation's three inputs and identified the Yeti system default.

The session regression checks sparse display IDs, no capture during discovery,
no primary fallback for an invalid choice and no remote input/session creation.
This patch uses the existing meeting/remote protocols and needs only client
publication, with no station API or relay restart.

Pre-release checks: workspace tests pass (157 passed, five existing manual/
benchmark tests ignored). Strict Clippy passes for Linux workspace/all targets,
Windows x86_64/all client targets and Android arm64. The native audit described
above passes. All heavy checks used the serial resource-limited wrapper.

### 0.23.1 publication

Published **0.23.1 / build 121 / d3d5e34** to the existing mounted station.
Linux, Windows and the arm64 Android APK were rebuilt and signed with the
existing keys. Local desktop signatures/hashes, Linux version/glibc (maximum
referenced 2.30), Windows version resources (0.23.1.121), bundled Mesa and
Media Foundation delay imports all verify. Android remains non-debuggable,
versionName 0.23.1 / versionCode 121, with the existing capture permissions
and unchanged d6ae3771378e9c00a82902e728012992834b9bf7e234fefed80dc85a4b879c6c
release certificate.

The publisher required the previous **0.23.0 / 119 / 9c7ee24**, backed up its
clients outside the web root, preserved deployed ownership/modes, verified the
staged files and replaced the manifest last. Private rollback backup:
/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.23.0-to-0.23.1-lm1fp46a.
Restore verified backup client files with deployed ownership/modes and the old
manifest last if rollback is needed. No PHP/relay binary or station settings
were changed and the relay was not restarted.

Authenticated live Linux/Windows update offers and full HTTPS downloads for
Linux, Windows and Android all verify against the new release, including desktop
provisioning trailers and unchanged signed APK bytes. Desktop users install
through About → Check for updates → Install & restart; Android users install
the station's new APK. Running clients are not automatically replaced by
publication.

Release log: /tmp/slipstream-0231-release-build.log. Test log:
/tmp/slipstream-0231-tests.log. Final caches: 11.61 GiB development/test,
30.41 GiB release, within the existing 12/32 GiB budgets. This documentation
commit follows the packaged source commit and does not change build 121.
Earlier documentation reconciliation edits remain separate and uncommitted.

## Screen thumbnails, Linux camera correction and session tab groups

The Linux camera report reproduced exactly on this workstation: Nokhwa returned
[(1, "HD Pro Webcam C920"), (0, "HD Pro Webcam C920")]. Endpoint 1 produced
"Could not get device property CameraFormat: Failed to Fufill"; endpoint 0
successfully delivered two 640×480 RGBA frames. This was not a hardware failure:
the new default picked the webcam's metadata node. Linux discovery now queries
V4L2 capabilities, excludes metadata-only nodes and sorts real camera IDs. An
invalid saved choice is repaired when only one actual camera remains. Startup
also negotiates advertised decodable modes and tries bounded alternatives rather
than requiring a particular pixel format/resolution. A local camera probe reads
and discards frames; it does not save or send them.

The screen chooser now refreshes small local thumbnails in its screen cards and
arrangement map. Only one preview capture runs at a time, throttled separately
from meeting sharing. RGBA/BGRA stride conversion is bounded to 320×180 and keeps
aspect ratio. Cancellation propagates to the preview worker, which polls at
100 ms intervals with a three-second frame deadline. Leaving Meetings, ending
the room, refreshing or closing the chooser drops pending work/textures. There
is no publisher/network path from the preview; confirmation still starts sharing
only the explicit selected screen ID.

The existing tab implementation had a root-only drop target, one remote session
per native viewport and a + action with no destination. Native session windows
now render tab groups. Targets are registered per visible viewport in physical
screen coordinates; tabs reorder/move across root and floating groups or tear
out from either. Moving keeps the original session and releases input/fullscreen
state. Only the selected tab receives remote keyboard input. Empty source
windows close; closing a selected member leaves siblings connected. New-link
choices and saved-device password prompts carry the clicked window through the
connection attempt, overriding the separate-window preference for that action.

### Tray report and completed checks

The user saw phantom tray icons during the native audits. Xvfb isolated windows,
but the fixtures inherited the user's session D-Bus address, so each test client
registered an identically named Slipstream tray icon on KDE. The audit now
re-executes itself under dbus-run-session before starting any client or fixture;
this also isolates desktop-portal traffic. While three fixture clients were
running, the real KDE StatusNotifierWatcher still listed only the existing main
Slipstream registration (alongside unrelated applications). Fixture logs confirm
that their private bus has no StatusNotifierWatcher. No desktop service or real
client was restarted. Linux Tray::drop also explicitly asks ksni to shut down;
dropping a ksni handle alone does not close its service.

Workspace tests pass: 161 passed, five existing manual/benchmark tests ignored.
The local C920 probe lists only video endpoint 0 and reads/discards two valid
640×480 RGBA frames. No camera image is saved or transmitted by the probe.

Isolated native checks pass:

- Three-client tab audit: + adds to its source floating group despite the global
  separate-windows preference, floating tear-out, floating-to-floating merge,
  root/group moves and closing one selected member without ending its sibling.
  Evidence: /tmp/slipstream-meetings-ui-z35ekatz.
- Two-client meeting audit: remembered camera, Cancel/Answer, call-status cleanup,
  real encoded/decoded synthetic video, chat/unread/Enter, local chooser thumbnails
  with no peer screen published before confirmation, cancelled chooser, explicit
  second-screen sharing, narrow layout and confirmed teardown. Evidence:
  /tmp/slipstream-meetings-ui-khqsdazx. Native OCR required slower fixture typing
  and fixed chooser-footer hit areas; screenshots verify the actual controls.

The cache wrapper enforced its development budget during this work and removed
only generated Cargo caches when they reached 12 GiB. Subsequent builds remained
serialized with two Cargo jobs. Pre-existing documentation reconciliation edits
remain separate from these implementation changes.

Strict Linux workspace/all-target Clippy also passes with warnings denied.

### 0.23.2 publication

Published **0.23.2 / build 123 / 83e507b** to the mounted production station.
The existing release keys signed the desktop update manifest and arm64 Android
APK. Local signature/hash checks pass; Linux starts with the expected release
identity, references at most glibc 2.30 and has no GLIBC_ABI_DT_RELR dependency.
Windows version resources are 0.23.2.123, bundled Mesa remains included and the
optional Media Foundation DLLs remain delay imports. Android is non-debuggable,
versionName 0.23.2 / versionCode 123, with the existing capture permissions and
unchanged d6ae3771378e9c00a82902e728012992834b9bf7e234fefed80dc85a4b879c6c
certificate. All three release builds completed through the bounded wrapper.

The publisher required **0.23.1 / 121 / d3d5e34**, backed up the previous clients,
preserved deployment ownership/modes, verified staged/replaced hashes and wrote
the new manifest last. Private rollback backup:
/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.23.1-to-0.23.2-8l5v6bfk.
Restore verified backup client files with deployed ownership/modes and the old
manifest last if needed. This was a clients-only publication; the station API,
managed relay and station configuration were unchanged.

Authenticated live Linux/Windows update offers and full HTTPS downloads for
Linux, Windows and Android verify against 0.23.2 / 123 / 83e507b. Provisioning
trailers match the station and the signed APK bytes remain intact. Install via
About → Check for updates → Install & restart on desktop, or the station's new
APK on Android. Publication does not replace a currently running app.

Build log: /tmp/slipstream-0232-release-build.log. Workspace test log:
/tmp/slipstream-0232-tests.log. Final Cargo caches: 5.23 GiB development/test and
30.76 GiB release, within the 12/32 GiB budgets. This later documentation commit
does not change the packaged source/build identity.
