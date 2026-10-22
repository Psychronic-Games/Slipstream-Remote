# Meetings and Show me

Open **Meetings**, enter your name and a room title, and press **Create meeting**.
The room defaults to 20 participants; its capacity can be set from 2 to 32.
Before joining, open **Devices** to choose named microphone and camera inputs.
The system-default microphone shows its device name; choices are remembered.
Use **Refresh devices** after connecting a headset or camera.
Prefer a desktop organizer and a good upload connection for larger rooms.

**Copy invitation** shares a link that another installed Slipstream client can
paste into Meetings or open as a Slipstream link. Guests wait for the organizer
to admit them. The organizer can decline requests, remove participants and end the room.
In **Participants**, **Allow new join requests** controls admission: turn it off to block new arrivals while current members
stay connected. Existing waiting requests can still be admitted. Invitations expire after four
hours. A removed participant's old admission is invalid; a new endpoint still requires approval.

Use **Meeting** beside a saved system, or **Call system** in the room, to ring
that client. The recipient sees **Answer** and **Decline**. Answer joins the
inviting room muted. **Cancel call** stops a dialing/ringing invitation; it
appears only before Answer. After joining, **Leave meeting** exits the room;
the organizer's **End meeting** ends it for everyone after confirmation.
Calling requires authorization for the recipient's station;
a meeting invitation itself grants no remote-control or station directory rights.
Installed clients must be updated for meetings; update the Windows service too.

During a meeting, use **Live meeting**, **Chat**, **Participants** and
**Devices** to switch views. Mute, camera, screen sharing and Leave/End controls
stay above the selected view. Chat shows an unread badge when you are elsewhere;
Enter sends a message and Shift+Enter inserts a new line.

Press **Unmute** to enable your microphone. **Camera** and **Share screen** are
separate explicit actions. **Share screen** opens a chooser with screen names,
resolution, main-display labels and a layout map. Choose a screen and confirm
**Share selected screen**. **Change screen** opens the chooser again; Cancel
keeps your current sharing unchanged. Up to four selected remote sources and a
local preview are displayed; screen shares have priority, and a participant can be
pinned. The organizer forwards encoded streams rather than transcoding them.
The organizer's upload and each participant's connection determine video capacity.

Windows Server/N camera sharing requires the optional Media Foundation feature
(Media Feature Pack on N editions). The app reports a camera error when that
feature is unavailable; remote support and meeting audio continue to work.

On Android, press **Show me** and allow camera access to share the rear camera.
Switch to the front camera or enable the torch where the phone supports it.
Stop Show me locally, leave the room, or use the ongoing notification's
**Stop camera and microphone** action to stop capture. Screen sharing and
accessibility approval are not needed for Show me. Microphone permission is
requested separately when unmuting.

Use the room's **Subspace chat** for messages to all admitted participants.
Support sessions and their chats continue independently. Closing a remote tab
keeps the meeting running; leaving a meeting keeps support links running.
Support system-sound capture pauses during meeting playback to avoid feedback.

The organizer is a meeting participant and can access its content. Direct peer
connections are preferred; permitted station relay fallback carries encrypted
transport. Guest relay leases expire and never contain the station access key.
A shared invitation does not bypass the lobby. Participant names are display
labels, not verified human identities.

Rooms end when their organizer leaves/disconnects or after four hours. There
is no automatic host migration. Android incoming calls require a running,
reachable app; notifications do not wake a force-stopped app. The 20-person
capacity is a practical target, not a guarantee of 20 full-resolution cameras.
