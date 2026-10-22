---
title: Group meetings with incoming calls and mobile Show me
labels: [feature, priority:high]
---

## Requested behavior

Add meetings for people running the Slipstream client, including group calls
with a target of about **20 participants**. Authorized guests can join with
their installed client. Subspace chat remains available during the meeting,
and technicians can continue remote support alongside it.

Calling a remote system with its Meeting button rings that client and presents
an incoming-call prompt with Answer and Decline. On mobile, Show me lets the
user share their camera to show equipment, cabling or a network closet while
talking to the technician.

The 20-person target was selected on October 21, 2026. Favor simple operation
and room to grow; it is a design target rather than a measured capacity or
a requirement for 20 simultaneous full-resolution camera tiles.

## Intended meeting experience

- Create a meeting from the console and invite people with a meeting link or
  code. A device's Meeting action can start a call or invite that device to
  the current meeting. Joining a meeting does not require a remote-control
  session or an unattended-access password.
- Guests enter a lobby. The organizer admits or declines them, can remove
  participants, lock admission and revoke invitations. A shared invitation
  locates a meeting; it does not bypass admission. Keep meeting authorization
  separate from station administrator permissions and remote-control rights.
- Incoming calls identify the calling device and show Answer and Decline,
  with ringing, cancelled, declined, busy, unavailable and unanswered states
  on both ends. Treat a supplied display name as a label, not verified human
  identity. Bound and rate-limit ringing; provide a local calls preference.
- The meeting offers microphone mute, optional camera, participant list,
  shared screen, pinned video, Subspace chat and Leave. The organizer also has
  End meeting. Starting a call never automatically enables screen capture,
  remote input or the camera. Joining audio follows the user's explicit
  choice and platform microphone permission.
- Meeting chat reaches all admitted participants. Existing support chat
  continues independently, with its workstation clearly identified, so a
  technician cannot accidentally send private support messages to the room.
- Show me selects the phone's rear camera with a local preview. Provide
  front/rear switching, torch where supported and an obvious Stop sharing
  control. The technician may request Show me; the phone user starts it.
  Camera video is its own source and does not depend on Android screen
  capture or accessibility approval.
- Remote-control tabs and meeting controls remain independently usable.
  Ending a support link leaves the meeting running; leaving the meeting
  leaves support links running.

## Transport and capacity

Use a **participant-hosted forwarding topology**. One participant's client
coordinates membership and forwards encoded media to other participants.
Each participant uploads one copy of each published source to that host.
The host forwards selected streams without decoding and re-encoding them.
The host's own streams enter the same forwarding rules locally.

Reuse iroh's authenticated, encrypted connections and NAT traversal, with
station relay fallback where allowed. The meeting host is an authorized
participant that can access the meeting's content. A station transport relay
only carries encrypted traffic. This topology requires no separate cloud
media server, although network conditions can still require the station's
transport relay. See [iroh's transport documentation](https://docs.iroh.computer/protocols/using-quic).

The host's upload remains a capacity constraint: forwarding one 1 Mbit/s
source to 19 other participants requires about 19 Mbit/s of host upload,
before overhead. More video sources add to that. Subscribe to a bounded
selection of active-speaker, pinned and shared-screen streams; adapt frame
rate, resolution and bitrate. Preserve room audio and chat when video has
to reduce quality. Avoid making every client decode every published camera.
Prefer a desktop host for larger rooms.

Keep capacity limits configurable and report a full room explicitly. Increase
the supported participant count when measurements justify it. Initially, loss
of the hosting client ends the meeting with a clear reason; automatic host
migration can follow later. A normal participant leaving only removes that
participant. Detect a stalled host and bound any reconnect attempt.

## Integration boundaries

**Meeting protocol:** add a separate protocol namespace and lifecycle from
remote control. Reuse transport, codec and rendering components without
requiring a desktop host backend to start a meeting. Version membership,
signaling and media envelopes explicitly. Keep existing protocol-6 layouts
intact, and show an update hint when a peer lacks meetings.

**Admission:** bind participant membership and media publication to the
authenticated endpoint identity, meeting instance and current admission.
Pending guests receive no room media, chat or participant roster. Removing a
participant closes their subscriptions and prevents rejoining through an old
admission ticket. Limit pending invitations, lobby entries, message sizes,
streams and buffered media. Decide how authorized callers may ring a device
before exposing calls to arbitrary station peers.

**Station routes:** invitations must work for authorized guests whose clients
come from other stations. Keep station-wide access keys out of invitations,
meeting codes, rosters and diagnostic output. The existing private relay
accepts the station access key, so guest relay access requires a scoped
credential/routing design before cross-station invitations can be complete.
Respect each station's public-relay policy.

**Windows service:** the service currently owns the persistent device endpoint
while the interactive app uses an ephemeral endpoint. Route incoming call
offers and Answer/Decline through the authenticated local service bridge;
keep microphone, speaker and camera ownership in the signed-in user's app.
Return unavailable when no interactive receiver can answer. Clear pending
offers on service epochs, sign-out, cancellation and expiry.

**Audio:** current capture shares system output. Add microphone capture,
device selection, voice encoding and per-source playback/mixing. Echo
cancellation must use the actual rendered call audio as its reference, with
noise suppression and level handling suitable for conversation. Suppress
recapture of call playback when sharing system sound. Existing loopback
capture is insufficient for this. WebRTC's [audio processing module](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/g3doc/audio_processing_module.md)
is one implementation option, subject to build and portability evaluation.

**Video:** encode each local camera/share once and forward its encoded
packets. Track source identity, dimensions, orientation and timestamps;
handle source changes and keyframe requests. Bound queues per subscriber so
one slow receiver cannot stall the room. Reuse existing video components
after separating desktop-specific pacing and lifecycle assumptions.

**Android:** add runtime camera/microphone permissions and the corresponding
foreground-service types, consent, notification and capture cleanup. Start
capture from an eligible user interaction; Android restricts creating camera
and microphone services from the background. An installed but stopped app
does not automatically have reliable incoming-call delivery: define reachable
states and background notification behavior explicitly. See [Android service
types](https://developer.android.com/develop/background-work/services/fgs/service-types)
and [background-start restrictions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start).

## Implementation status — October 21, 2026

Implemented for 0.23.0:

- [x] Separate meeting/call protocols, endpoint-bound admission, four-hour
      invitations, one-use call tickets and relay-only guest credentials.
- [x] Create/join/lobby/leave/end, roster, organizer lock/admit/decline/remove,
      group Subspace chat and remote-support independence.
- [x] Saved-device Meeting actions, authenticated ringing, Answer/Decline,
      cancellation, calls preference and epoch-bound Windows service bridge.
- [x] Microphone capture, echo cancellation using the playback reference,
      noise suppression, level control, per-source audio mixing and mute.
      Desktop microphone names can select a non-default input.
- [x] Selected H.264 forwarding, two-frame acknowledgement windows,
      up to four remote video sources, local preview and pin selection.
      Screen sources take priority; camera defaults are bounded to 15 fps,
      800 kbit/s and a 1280-pixel long edge. Media queues are bounded.
- [x] Android Show me, rear/front switching, preview, torch where available,
      permission handling and notification Stop camera and microphone.
- [x] Exercise 20 real transport participants with synthetic audio/video,
      lobby overflow, capacity, admission, chat, removal and organizer exit.

Usability fixes through 0.23.2 add Live meeting/Chat/Participants/Devices views,
named remembered inputs, explicit screen selection with local preview thumbnails,
clear Cancel call versus Leave/End actions, admission wording and Linux camera
endpoint/mode negotiation. The [handoff](../DEVELOPMENT-2026-10-21.md) records
native audit, hardware probe and publication evidence.

Remaining capacity/product follow-up:

- Measure representative 20-person calls on physical machines and constrained
  uplinks. The transport check establishes membership/forwarding behavior,
  not 20 simultaneous camera encoders or a guaranteed network capacity.
- Add automatic active-speaker switching and adaptive video quality. The first
  version selects screens, pinned video and available cameras with fixed
  media defaults; it does not yet negotiate bitrate per subscriber.
- Add invitation rotation/revocation, organizer migration and room renewal.
  Lock admission or end the meeting to stop use of a general invitation now.
- Consider a dedicated Show me request action; technicians currently request
  it through voice or chat, and the phone user presses Show me locally.
- Incoming Android calls require the app's endpoint to remain running and
  reachable. A notification opens the app to Answer; Decline works from the
  notification. There is no push wake-up for a force-stopped app.
- Windows full-console calls require the updated service and interactive app.
  Compact host-only interfaces return unavailable instead of silently ringing.

## Done when

Authorized installed clients can join one meeting, speak, use group Subspace
chat and view selected shared video. A user can answer or decline a call made
to their system. An Android participant can show the technician their camera
and stop sharing locally. A technician can continue remote control alongside
the meeting, with independent chat, input and disconnect behavior.

Unadmitted or removed clients receive no further room content. Expired,
cancelled and stale service offers cannot start capture. A slow or disconnected
participant cannot retain unbounded media or stall other participants.

Targeted automated checks should cover admission, cancellation, member removal,
forwarding bounds, chat routing and session independence. Use synthetic media
for larger room checks; keep native camera/render checks small and run heavy
checks through `scripts/check.sh` under the project's resource limits.

## Pointers

- `crates/slipstream-core/src/net.rs`, `proto.rs`, `auth.rs` and `directory.rs`
- `crates/slipstream-session/src/lib.rs`, `host.rs`, `viewer.rs` and `video.rs`
- `crates/slipstream-audio/src/io.rs`, `pulse.rs`, `opus.rs` and `buffer.rs`
- `apps/slipstream/src/gui/mod.rs`, `home.rs`, `saved.rs`, `remote.rs` and
  `quick_connect.rs`
- `apps/slipstream/src/service_chat.rs`
- `apps/slipstream-server/src/relay.rs` and `server/station/api/index.php`
- `apps/slipstream-android/app/src/main/AndroidManifest.xml` and
  `java/com/slipstreamremote/app/{MainActivity,ScreenShareService}.java`
