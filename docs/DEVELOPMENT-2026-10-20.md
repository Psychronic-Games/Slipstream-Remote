# Development handoff — 2026-10-20

## In progress after 0.21.1 publication

The real Linux approval failure was KDE's running portal retaining deleted Qt
6.11.2 libraries after Qt 6.12.0 was installed. Its RemoteDesktopDialog could
not load the new QtQuick Layouts plugin. Restarting the KDE/frontend portal
services fixed the dialog; the user confirmed a successful Windows→Linux
connection. Existing approval settings were preserved.

Implementation for 0.22.0:

- Direct + Group, empty-group persistence on each technician account,
  per-group Add device and removal of empty groups; update the station API
  along with clients, preserving older-client/device/password behavior.
- Detect that exact stale Qt condition, repair only an idle portal, bound
  approval waits, clean incomplete sessions and notice revoked grants.
- Recognize the downloaded Linux executable with a stable ELF note and a
  per-user MIME icon after its first normal launch.
- Add a fixed + tab button with a searchable recent/saved/shared-device
  picker, existing saved-password prompts and correct station routes.
- Clip the animated menu-bar pulse to the actual rounded segments.
- Tighten menus/dialogs for smaller screens and draw transfer Delta arrows
  and the recent-link save star without platform font glyphs.
- Correct both file-browser and transfer-progress caption flicker: native
  frame suppression, single registration, and guarded asynchronous repaints.

Native Xvfb checks passed: a live test-pattern session, recent/private/shared
picker, actual byte-exact file transfer, progress receipt, 640×360 file window,
720×480 compact Bridge, and + Group form saving an empty group to a real isolated
PHP station. Qt's MIME database recognizes the executable and a renamed copy
with a provisioning trailer; an unrelated executable keeps its own MIME type.
PHP account tests also cover group isolation, legacy edits and encrypted secrets.
Evidence is under /tmp/slipstream-022-native-dd9k97fe and
/tmp/slipstream-022-native-qoo9qm7t. Final workspace checks: 143 tests passed, zero failed, five ignored;
Linux and Windows all-target workspace Clippy and Android arm64 Clippy passed
with warnings denied. Packaging and publication are next.

Screenshots: [picker](screenshots/new-link-0.22.0.png),
[640×360 file browser](screenshots/files-compact-0.22.0.png),
[720×480 Bridge](screenshots/bridge-compact-0.22.0.png),
[empty group](screenshots/groups-0.22.0.png).
Heavy commands must use codex-check serially.

## Published release: 0.21.1

Published **0.21.1, build 106, commit 2d1a128** at
https://relay.slipstreamremote.com. The five interrupted support features were
already implemented and audited in the
[October 19 handoff](DEVELOPMENT-2026-10-19.md). Today's continuation verified
the existing artifacts and completed their blocked publication. No client
rebuild was needed; the packaged version/build/commit remain unchanged.

The completed feature set is:

- Connect to a discovered TCP service through the remote workstation, with
  retained local loopback tunnels and browser actions for HTTP/HTTPS.
- Send Wake-on-LAN packets through a connected workstation's selected LAN
  adapter, with a separate host permission.
- Run bounded gateway, DNS and station API diagnostics.
- Review and export support reports with inventory, network results, notes
  and an explicitly selected screenshot.
- Show a plain-language support status on the host, with Dismiss and Stop
  support actions, including compact Windows hosts.

The release also includes automatic cloned-device identity detection. Run
the updated master app/service before imaging to establish its firmware
baseline. The original handoff retains the implemented scope, broader ideas
and physical-device verification limits for every feature.

## Verification

- `cargo test --workspace --locked`: **138 passed, zero failed, five ignored**.
- Linux all-target workspace Clippy passed with `-D warnings`.
- Packaged Linux and Windows sizes, SHA-256 hashes and Ed25519 signatures
  match the existing release key; both binaries contain the trusted update
  public key. Portable Linux `info` starts with an isolated profile.
- Android's APK signature verifies with the existing certificate:
  `d6ae3771378e9c00a82902e728012992834b9bf7e234fefed80dc85a4b879c6c`.
- Authenticated live HTTPS update offers for Linux and Windows report
  **0.21.1 / 106 / 2d1a128** and match the verified signed manifest.
- Full Linux/Windows downloads match their signed executable prefixes and
  valid provisioning trailers for the station. Android's full download is
  byte-identical, with the APK MIME type and `.apk` attachment filename.
- Six isolated publication regressions pass: manifest-last ordering/exact
  backups, recovery after an interrupted replacement, recovery after a
  manifest readback failure, refusal of corrupt/stale releases, rejection
  of bad signatures/missing mounts, and live offers/downloads with rejection
  of incorrect provisioning credentials.

Run substantial checks through `/home/doug/.local/bin/codex-check`, one at a
time, with its default two Cargo jobs and memory limits. Publication and the
publication regression check used this wrapper.

## Deployment and rollback

The WebServer mount became available during this continuation. Before
publication, both the mounted manifest and authenticated HTTPS offers still
identified 0.20.0 / 98 / 98e84e0. Only clients and the manifest changed under
`/home/doug/WebServer/home/relay/public_html/clients/`. Files were backed up,
staged, checked and atomically replaced, with the manifest last. Ownership
1065:1028 and each existing file's mode were preserved. PHP and the relay
were not updated or restarted.

Rollback containing 0.20.0 clients/manifest:

`/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.20.0-to-0.21.1-g3vhlywr`

Restore the saved client files atomically, preserving deployed ownership and
modes, then restore the saved manifest last. `rollback.json` records backup
hashes and any newly introduced files; this release introduced no new client
filenames.

## Durable continuation tooling

The old `/tmp/slipstream-*-0211.py` scripts did not survive. Their replacement
is [scripts/publish-station-clients.py](../scripts/publish-station-clients.py),
with usage and recovery details in [BUILDING.md](BUILDING.md#publishing-clients-through-a-mounted-station).
It verifies desktop signatures against an explicitly trusted key, publishes
only over the expected prior release, and checks authenticated offers/full
downloads without printing the station key. Its fixture suite is
[scripts/test-publish-station-clients.py](../scripts/test-publish-station-clients.py).

Public update key used for this publication:
`5e3e61d87100ea74f0d1c51778ce3bb19208bae122b8087d047c22c0fec55eb2`.

Candidate SHA-256 values:

| File | SHA-256 |
|---|---|
| manifest.json | cf8666ea92846e377832c4b16e2da91e71c6a318b901795b5d4fc42be6a8cd96 |
| slipstream-linux-x86_64 | 729f271c9d7923f7f0a950322e6a52b6c6fa8af9ccd4c736ba838782a60af0e5 |
| slipstream-windows-x86_64.exe | 7b36e64d98e3dc094b81d8893e0dbb12e8f86b81ddcd2aba9406690b9622945c |
| slipstream-android.apk | 8d8641d6ff35d117479f8dc1b6595d8bb9dbc1515b89197825bffb17fe62c5a6 |

## Remaining runtime checks

Physical Windows service/compact behavior, real LAN wake, Windows imaging
on different hardware, real Android and macOS still need their original
device checks. Linux native/Wine runtime evidence from October 19 remains
applicable; today's continuation did not repeat those UI checks. See
[STATUS.md](STATUS.md#next-steps-suggested-order) for the remaining queue.
