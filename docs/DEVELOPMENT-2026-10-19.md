# Development handoff — 2026-10-19

Published **0.18.0, build 90, commit ba3161e** at
https://relay.slipstreamremote.com. Implementation commit: **4b9a5f0**.
Subsequent documentation commits do not change the deployed build.

## Changes

- Detached File Transfer, transfer receipts and Network Scan hide immediately
  on close. Their parent receives a repaint request that can run while
  occluded, allowing native-window cleanup without another main-window click.
  System Information discards its closing pass and redraws without the panel.
  This targets the user's Windows black-panel report; physical VM confirmation
  remains open.
- Wayland GUI and headless hosts prepare screen/control access on startup.
  Config offers Approve now and Reset / approve again. Local preparation and
  incoming sessions share one cached backend and save desktop restore tokens.
  The grant stays usable while the process runs in the tray. Reset refuses
  active sessions; session registration holds the backend lock until the
  viewer is registered, closing the reset/authentication race.
- Windows needs no portal consent. Config explains automatic screen access
  and the background service's login/UAC access rather than showing Reset.
  Android retains explicit Start sharing consent. macOS retains OS permission
  requirements, explained in Settings.
- Station download buttons include the same filename as the HTTP attachment,
  including the APK. Ordinary ASCII attachment headers omit the unnecessary
  extended filename parameter; non-ASCII names retain UTF-8 support. Signed
  APK downloads remain byte-for-byte copies, with the APK MIME type.

## Verification

- `cargo test --workspace --locked`: **116 passed, zero failed, five ignored**.
  New coverage prepares access concurrently, connects through the prepared
  backend, refuses reset with an active viewer, then forgets/reopens access
  after disconnect. Viewer-only/disabled hosts cannot prepare incoming access.
- Linux and Windows all-target workspace Clippy passed with `-D warnings`;
  Windows used `cargo xwin clippy`. Arm64 Android Clippy passed through
  `cargo ndk`. These checks include compilation, not physical-device runtime.
- Isolated PHP station checks passed: Android button filename, HTTP filename,
  MIME type and exact bytes, plus the existing groups/accounts checks.
  Changed PHP files passed lint. Production accounts were not test fixtures.
- All desktop package hashes and Ed25519 signatures verified, with the same
  embedded release trust key as the previous deployment. The portable Linux
  CLI starts under a private temporary profile and requires glibc no newer
  than 2.31. Windows includes Mesa 26.2.4 for the software-renderer fallback.
- The arm64-only release APK has versionName 0.18.0, versionCode 90, no
  debuggable flag and the same valid signing certificate as the prior APK.
- Live authenticated HTTPS update offers report 0.18.0 / 90 / ba3161e with
  valid signatures. Full Linux/Windows downloads match signed prefixes and
  carry valid provisioning trailers. The APK response has the app filename,
  correct MIME type and exact signed bytes; the live page includes its
  download filename hint. Direct access to `clients/` remains denied as
  intended; verification uses the API and `download.php`.

## Deployment and rollback

Station root: `/home/doug/WebServer/home/relay/public_html/`.

Updated `lib/slipstream.php`, `download.php`, `index.php` and the Linux,
Windows and Android files in `clients/`. Files were backed up, staged in
their destination directories, checked, and atomically renamed with existing
ownership/modes preserved. The shared PHP helper was published before its
callers; `clients/manifest.json` was published last. Copied files were checked
again after publication.

Rollback directory:
`/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.17.1-build87-to-0.18.0-build90-20261019T222231Z`

It contains the previous PHP files and 0.17.1 clients/manifest, plus a file
list. Restore the listed PHP/binary files atomically, then restore the old
manifest last. No relay-program replacement or restart was needed. Signing
keys remain outside the repository. No GitHub push or release was performed.

## Remaining checks

On the Windows VM, repeatedly open/close Files and System Information,
including with the main app covered/minimized, and confirm no black panel
remains and tools reopen normally. Confirm background service access using
the unattended password with the GUI closed.

On Wayland, approve locally at startup, connect later while in the tray,
change the screen with Reset / approve again, and check restart/reboot token
behavior on each supported desktop. Automated coverage uses a synthetic
backend; it does not establish real portal persistence. On a real Android
browser, confirm the saved APK name as well as the verified HTTP/page hints.
