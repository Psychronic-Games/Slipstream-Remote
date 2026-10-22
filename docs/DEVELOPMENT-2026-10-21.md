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
- All substantial checks use scripts/check.sh serially with the project
  memory, job and cache limits. Fixture profiles do not touch real identities.

A 20-member synthetic transport check is not a physical 20-camera benchmark.
Automatic active-speaker selection, adaptive bitrate, invitation rotation,
room renewal and host migration are follow-up work.

## Release and publication

Pending final package and mounted-server publication; append exact release
metadata, checks and rollback locations here before closeout.
