# Development handoff — 2026-10-20

## Published release: 0.22.1

Published **0.22.1, build 113, commit 61367b6** at
https://relay.slipstreamremote.com. Implementation is bfd072e. Later handoff
commits do not change deployed client metadata.

The + now follows the last session tab, or Bridge when empty. New Link uses
complete LCARS framing and shows private/shared and empty groups before recent
links, with a collapsible manual link form. The device editor integrates its
group chooser into the field with a full-width popup. Settings fields and
station sign-in controls fit compact panels; long titles/device names no
longer overlap actions. Connection/tray dialogs share the same frame, and file
browser refresh is drawn with vectors.

Settings now says **Unique device detection — Automatic** without imaging
instructions. This desktop's DMI UUID/serial files are root-only; its enp9s0
permanent PCI adapter is readable by the normal user. A locally hashed adapter
fallback preserves an existing ID on migration and adapter additions/removals
while a known adapter remains. Firmware still takes priority. Virtual,
randomized, user-assigned and removable USB addresses are excluded. No system
permissions or real user identity/settings were changed.

Workspace tests: **148 passed, zero failed, five ignored**. Linux/Windows
all-target workspace Clippy and Android arm64 Clippy pass with warnings denied.
Native isolated PHP/Xvfb checks verified saved-password connection, the inline
+ after a live tab, private/shared/empty groups, compact New Link/manual forms,
a group selection actually saved to the account, and unchanged profile identity
when learning this desktop's adapter fallback. Compact configuration fields,
group popup and Automatic status were visually checked again after final edits.
Native evidence: /tmp/slipstream-0221-quality-3gm2qqpv and
/tmp/slipstream-0221-final-o252jbel. All fixture processes were closed.

Screenshots: [New Link](screenshots/new-link-0.22.1.png),
[group chooser](screenshots/group-menu-0.22.1.png),
[automatic device recognition](screenshots/unique-device-0.22.1.png),
[+ following an active tab](screenshots/tab-following-plus-0.22.1.png).

### Package and live verification

- Release signatures, SHA-256 hashes and sizes pass; update signing key is
  unchanged. Portable Linux starts with the correct build, requires no glibc
  newer than 2.31, and retains the executable MIME note. Windows version
  resources and bundled software renderer pass. Android is arm64,
  non-debuggable, versionCode 113, signed by the existing certificate.
- Packaged Windows under Wine connects to an isolated Linux host, transfers
  byte-exact files, and shows the progress receipt and compact 640×360 file
  browser without default placeholder windows. The new vector Refresh icon
  is visible. Evidence: /tmp/slipstream-0221-windows-4n6aa79u. All fixture
  processes stopped; the disposable Wine prefix was removed, keeping evidence.
  Physical Windows DWM caption behavior still needs its real-device check.
- Authenticated live update offers and full Linux/Windows/Android HTTPS
  downloads pass for **0.22.1/113/61367b6**, including provisioning trailers
  for desktop downloads and APK bytes/content headers.

Only client files/manifest changed on the mount; no PHP/API or relay service
change was needed. Publication preserved ownership/modes, staged and verified
files, and replaced the manifest last, expecting 0.22.0/110/9dbae20.
Private rollback backup:
/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.22.0-to-0.22.1-91kpummm.
Restore client files atomically with saved metadata, then the old manifest last.
The user's running app/session and real identity/settings were not replaced.

All substantial checks/builds used scripts/check.sh serially. Post-build
housekeeping removed the oldest generated release APK. Current Cargo caches:
**5.60 GiB development/test** and **27.87 GiB release**, within the 12/32 GiB
budgets; incremental compilation remains disabled. Logs are under
/tmp/slipstream-0221-{tests-final,clippy-linux-final,clippy-windows-final,
clippy-android,release-build,verify,wine,publish,live}.log.

## Published release: 0.22.0

Published **0.22.0, build 110, commit 9dbae20** at
https://relay.slipstreamremote.com. Implementation is daa28c2. Later build-cache
and handoff changes do not change the deployed client build. All requested
client changes are included:

- Fixed + tab button: searchable recent/private/shared systems, manual IDs,
  saved passwords and remembered station routes, including duplicate IDs
  across different stations.
- Direct + Group and + Device in Saved devices; empty groups sync to the
  technician account, with per-group Add device and removal of empty groups.
  Shared-device editing retains its existing administrator requirement.
- More compact desktop menus/dialogs, a 720×480 console frame and 640×360
  file browser, vector Delta transfer arrows and a vector recent-link star.
- Clip the animated menu-bar pulse to each actual rounded segment and gap.
- Suppress native Windows captions over custom frames, register native
  tools once, and guard late browser/progress/preview viewport repaints.
- Bound Linux portal startup/approval waits, close abandoned requests,
  Cancel/Retry, monitor revoked sessions and invalidate stale access.
- Recognize downloaded Linux executables through an ELF note and per-user
  MIME icon after first normal launch, including renamed/provisioned copies.

The real Linux approval failure was KDE's running portal retaining deleted
Qt 6.11.2 libraries after Qt 6.12.0 was installed. Its RemoteDesktopDialog
could not load the new QtQuick Layouts plugin. Restarting the KDE/frontend
portal services restored the dialog; the user confirmed Windows→Linux worked.
The new recovery detects that exact version mismatch and refreshes services
only when the portal reports no active requests or sessions. Saved approvals
are preserved; busy/unknown state produces a useful error rather than
interrupting another application's share. The user's running app/session was
not upgraded or restarted during publication.

### Verification

- Workspace: **143 passed, zero failed, five ignored**. Linux/Windows
  all-target workspace Clippy and Android arm64 Clippy pass with warnings denied.
- PHP account integration covers empty groups, duplicates, isolation,
  populated-group deletion refusal, legacy edits and encrypted secret retention.
- Native Xvfb: actual byte-exact file transfer, progress receipt, compact
  file/Bridge layouts, recent/private/shared picker and a + Group form saving
  to an isolated PHP station. Qt MIME recognition passes for original and
  renamed/provisioned executables, while an unrelated executable is unaffected.
- Packaged Windows under Wine: actual byte-exact transfer, browser/progress
  windows, + picker, 640×360 layout and no default egui placeholder window.
  X11 automation must raise/focus Wine windows before activating them; otherwise
  keystrokes hit the root window. Final evidence:
  /tmp/slipstream-022-native-dewzalo4; Linux/group evidence:
  /tmp/slipstream-022-native-dd9k97fe and /tmp/slipstream-022-native-qoo9qm7t.
- Release sizes, SHA-256 hashes, Ed25519 signatures and update trust continuity
  pass. Portable Linux starts, reports the correct build, retains the ELF note
  and requires glibc no newer than 2.31. Windows version/icon resources and
  software-renderer inclusion pass. Android is arm64/non-debuggable,
  versionCode 110, signed by the existing certificate
  d6ae3771378e9c00a82902e728012992834b9bf7e234fefed80dc85a4b879c6c.
- Authenticated live HTTPS offers identify **0.22.0 / 110 / 9dbae20**.
  Full Linux/Windows downloads match signed prefixes and valid station
  provisioning trailers. Android's full download is unchanged and has the
  APK MIME type and filename. Read-only account API returns groups/legacy fields.
- Physical Windows DWM caption behavior and the new automatic KDE recovery
  after a future system upgrade remain separate real-device checks. Wine
  cannot reproduce Windows DWM exactly. No real Android/macOS runtime was used.

Screenshots: [picker](screenshots/new-link-0.22.0.png),
[640×360 file browser](screenshots/files-compact-0.22.0.png),
[720×480 Bridge](screenshots/bridge-compact-0.22.0.png),
[empty group](screenshots/groups-0.22.0.png).

### Deployment and rollback

Only backward-compatible api/index.php and the Linux/Windows/Android clients
plus manifest changed on the mount. Existing ownership/modes were preserved.
API deployed first; staged/verified clients followed, manifest last. No relay
service restart or production account mutation was performed.

Private backups outside the public web root:

- API: /home/doug/WebServer/home/relay/tmp/slipstream-api-0.22.0-7n1lc5ci
  (old index.php and rollback.json with hashes/owner/mode).
- Clients: /home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.21.1-to-0.22.0-wpkpzgex
  (old clients/manifest plus rollback.json).

Restore client files atomically with deployed ownership/modes, then the saved
manifest last. Restore the API index.php if rolling that change back as well;
the API is also compatible with 0.21.1 clients. Publication used
scripts/publish-station-clients.py with expected 0.21.1/106/2d1a128.

### Build disk usage

The user reported a 183 GiB workspace: target/ accounted for 182 GiB,
including 85 GiB incremental state and 57 GiB native debug dependencies/tests.
Cleanup removed all development/test caches, including cross targets, and
pruned generated Android APKs to the newest two release/debug versions each.
The workspace is now about **29 GiB**, with **27.55 GiB reusable release
caches**, zero development caches, source/keys/settings/assembled packages and
production rollback backups preserved. Signed package verification passed
again after cleanup.

Incremental development caching is now disabled, local debug info reduced while
retaining line-number backtraces, with automatic pre/post housekeeping in station/Android builds
and scripts/check.sh: 12 GiB combined development/test budget and 32 GiB
combined release budget. They are cleanup thresholds, not filesystem quotas;
a command can exceed them until the post-build pass. Nested Android assembly
defers cleanup until station signing finishes. Gradle uses no persistent daemon
and two workers. AGENTS.md preserves resource/cache instructions. See
[BUILDING](BUILDING.md#disk-space) for commands and overrides. Shell syntax,
Cargo metadata/profile validation, actual cache trimming, package preservation
and check-command failure-status propagation were verified. Future substantial
checks use scripts/check.sh, which runs codex-check serially on this machine.

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
