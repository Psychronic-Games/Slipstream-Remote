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
