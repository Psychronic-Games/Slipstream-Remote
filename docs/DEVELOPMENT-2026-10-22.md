# Development handoff — 2026-10-22

## Latest publication — 0.26.0

Published **0.26.0 / build 137 / source 2949b2d** to both
https://public.slipstreamremote.com and https://relay.slipstreamremote.com.
This release includes automatic Windows app/service updates and fresh signed
update offers, described below. Later documentation commits do not change
packaged build identity.

All three platforms were rebuilt serially under the resource wrapper. Package
verification passed for signatures and hashes, Linux portability (maximum glibc
2.30, no RELR requirement), Windows 0.26.0.137 resources, embedded Mesa and
optional camera DLL delay imports, and Android 0.26.0 / 137, arm64,
non-debuggable, with the existing release certificate and capture permissions.
The packaged Linux app passed private default, one own ID, separate per-station
partner drafts and narrow layout. Evidence:
`/tmp/slipstream-station-tabs-xm82iwfk` (fixture connection errors are intentional).

Both mounted stations were verified as 0.25.1 / 134 / ee0f156 before publishing.
Clients were backed up outside the web roots, copied with preserved ownership
and modes, verified, and each manifest published last. Both station settings
fingerprints stayed unchanged. Relay runtime, web configuration and admin
passwords were not changed. Ordinary trusted HTTPS checks passed for both
stations' signed update offers and full Linux, Windows and Android downloads,
including desktop provisioning trailers and unchanged signed APK bytes.

Rollback copies:

- Public: `/home/doug/WebServer/home/public/tmp/slipstream-rollback-0.25.1-to-0.26.0-_vnkuv_3`
- Private: `/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.25.1-to-0.26.0-oc7of36q`

The previous assembled station is preserved at
`dist/station-archives/0.25.1-134`. Restore verified old clients with current
ownership/modes and the old manifest last for a client rollback. Preserve backups.

| Artifact | SHA-256 |
|---|---|
| Linux | `2d3000c0e45086d687fbfbb0ce8e7b5128f5d45fa9a53a18f3ddfaeeb87e0d9b` |
| Windows | `2e870fac3fd1d58ca62c75e7c89c9fd815bb727e603be593caef952ada7094cd` |
| Android | `dff31e28698e4daa55eff816205908d21c192262170bc56558a41a9e786b4f20` |

Release, package and publication/live-check logs:
`/tmp/slipstream-0260-release-build.log`,
`/tmp/slipstream-0260-package-checks.log`,
`/tmp/slipstream-0260-publication-live.log`.
Final caches: **9.73 GiB development/test, 21.50 GiB release**, within 12/32 GiB
budgets. Obsolete generated workspace release artifacts were cleaned before
building; assembled station packages, keys, profiles and rollback copies remain.
No paid Windows publisher signing was added.

Use About → Check for updates → Install & restart. Windows may request UAC;
updating the app now includes an installed service. If an old .24 updater still
reports a bad checksum after reopening/rechecking, download the latest app from
the station once and retain its existing profile. The new app checks for an
older service on startup. Real Windows UAC and running-service restart remain
physical-device checks; the Wine and automated evidence below has narrower scope.

## Windows service and checksum implementation — 0.26.0

Windows full/compact app and CLI updates now include an already-installed
service. The UAC helper verifies a protected snapshot again, resolves the
actual SCM executable, refuses downgrades and replaces both programs with
rollback on replacement/restart failure. Failed recovery retains the original
executables. Machine identity, station and unattended settings are preserved;
**Apply settings** is the explicit configuration-copy action. No service is
installed implicitly. A newly upgraded app catches an older service at startup,
covering the first update from an old updater. Service run/host and the helper
ignore unsigned provisioning trailers; machine settings remain authoritative.

The updater refreshes its signed offer before downloading and puts the hash
in the URL. A publication race retries once only with a separately verified
newer signed offer. The user's reported 0.24 checksum failure has not been
reproduced on their machine; OS/station/offered-version details were requested.
Live 0.25.1 Windows offers matched the manifest on both stations. Older apps
should reopen/recheck; if still blocked, download the latest app directly once
and retain the existing profile. The fix takes effect after upgrading.

Final workspace verification: **175 passed, five ignored**. Strict all-target
Linux workspace, Windows desktop and arm64 Android Clippy pass. Logs:
`/tmp/slipstream-service-update-final-tests.log` and the matching
`final-{linux,windows,android}-clippy.log` files. Isolated Wine checks pass for
actual SCM stopped-service updates, unchanged profile bytes, retaining a newer
service, damaged staging rejection, app-only updates, Windows argument parsing
and fixed-directory error reports. Four transaction tests pass on Linux and
Windows, including partial replacement, restart failure and blocked recovery.
Wine evidence: `/tmp/slipstream-service-update-toobf8lj`; summary log:
`/tmp/slipstream-service-update-wine.log`. Temporary Wine/Xvfb/D-Bus processes
were stopped and the prefix removed. Real Windows UAC and a running service's
capture/helper restart remain device checks; Wine is not evidence for those.

## Previous publication — 0.25.1

Published **0.25.1 / build 134 / source ee0f156** to both
https://public.slipstreamremote.com and https://relay.slipstreamremote.com.
This client update ships the blue/violet station tabs reviewed below. The
previously verified shared-code authentication policy is unchanged.

Linux, Windows and arm64 Android were rebuilt serially with the resource-limited
build script. Desktop update signatures use the existing trusted key; the APK
uses the existing Android release certificate. Package checks verified version
metadata, Linux portability (maximum glibc 2.30, no RELR requirement), Windows
0.25.1.134 resources, embedded Mesa and delayed optional camera DLL imports,
and the non-debuggable arm64 APK version 0.25.1 / 134 and capture permissions.
The final packaged Linux UI passed private default, per-station partner drafts,
one station's own ID at a time, and narrow layout. Its blue/violet colors were
visually checked. Evidence: `/tmp/slipstream-station-tabs-ise2waqa`.

Both mounted stations were verified as 0.25.0 / 131 / 4732fea before publishing.
Their old clients were backed up, new files copied with preserved ownership/modes,
checksums verified and each manifest published last. Both station settings file
fingerprints were unchanged. This was a client-only publication, without a relay
runtime or web configuration upgrade. Ordinary trusted HTTPS checks passed for
both sites' signed desktop update offers and full Linux, Windows and Android
downloads, including station-specific desktop provisioning trailers and unmodified
signed APK bytes.

Rollback copies outside the web roots:

- Public: `/home/doug/WebServer/home/public/tmp/slipstream-rollback-0.25.0-to-0.25.1-0ewhrq76`
- Private: `/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.25.0-to-0.25.1-lr8wrtvo`

The previous assembled local station is retained at
`dist/station-archives/0.25.0-131`. Restore old verified clients with current
ownership/modes and the old manifest last for a client rollback. Preserve backups.

| Artifact | SHA-256 |
|---|---|
| Linux | `8bde48f00a7db1894ddb14606850911d76394fa7aa3a57ae763f8732cd791347` |
| Windows | `7af9fd54a1992de7b3fb88dafe58a4d6d48190f2a2d883990cc6b0391636639b` |
| Android | `05d429f4fbff79c056091afc530cd90b375cf3edc866949ae1fad533df40ed94` |

Build, package and live-check logs are `/tmp/slipstream-0251-release-build.log`,
`/tmp/slipstream-0251-package-checks.log` and `/tmp/slipstream-0251-live-checks.log`.
Final caches are 4.31 GiB development/test and 31.84 GiB release, inside the
12/32 GiB limits. No extra workspace suite was needed for this version-only
release step: the preceding style/authentication follow-up checks are recorded
below. Use About → Check for updates → Install & restart on desktop; install
the station's new APK on Android. Later documentation commits do not change
packaged build identity.

## Public station and separate identities — 0.25.0

Published **0.25.0 / build 131 / source 4732fea** to both
https://public.slipstreamremote.com and https://relay.slipstreamremote.com.
Later documentation and test-style commits do not change packaged build identity.
The public web root is `/home/doug/WebServer/home/public/public_html/`.
The private station's configuration, accounts, access key and runtime were preserved;
only its client downloads were updated.

Raw apps default to Slipstream Public. A configured self-hosted station stays
the default. Bridge has one shared station tab bar above both Your ID and
Establish link: selecting a station changes the displayed ID and partner lookup
together. Each tab retains its own partner ID/password draft; recent links,
saved devices, reconnects and calls retain their station. Only one own ID is
shown, including in the status bar and tray. Public downloads do not overwrite
an existing custom station, including a provisioned private configuration.

The public endpoint uses a stable key derived from the machine's local secret,
separate from its private endpoint. Its cached public ID survives station changes;
imaged-device detection clears both caches. Independent directory registrations
and listeners share local permissions, passwords, sessions, call coordination
and authentication throttling. Private station keys and relay credentials are
removed from public connection/call settings before directory requests. Tests
assign the same numeric ID on two stations to verify isolation.

Windows service snapshots now provide both directory states and the configured
station URL over the authenticated local pipe. The GUI displays the service's
public ID, rather than a temporary interactive-process identity. A known station
mismatch hides the wrong ID and directs the user to update the service; an old
service does not invent a public ID. Updating the service preserves its public
identity and preserves the private ID only when the station is unchanged.
Physical Windows-service runtime verification remains open.

No n0-operated relay is inserted into new endpoint relay maps. iroh remains
the networking library. The optional private-endpoint Public backup relay uses
Slipstream Public and remains separate from the public ID. Public meeting
invitations retain their explicit Slipstream relay even when a guest's private
backup preference is disabled. Calls sanitize public routing and proof settings.
Normal disconnects now classify the connection close reason if the video writer
sees it before the control reader; the existing regression was strengthened.

## Public deployment and capacity

The public station is keyless, named Slipstream Public, and advertises only
its own HTTPS relay. Its PHP settings and managed relay are independent of
the private station, with data outside the document root at
`/home/public/slipstream-data`. Admin authentication was initialized before
exposing the site; there was no open initial-setup claim window.

Initial public capacity controls in its `config.php`:

- 5 MiB/s of received relay traffic per connection.
- 256 simultaneous relay connections, including idle endpoints. Both endpoints
  of a relayed session count; this is not a 256-session promise.
- 2,400 directory registrations and lookups per minute per IP, separately.

These are initial operating settings, not measured server capacity or monthly
transfer limits. Monitor server traffic, link utilization and hosting billing.
Direct connections avoid relayed media traffic. Self-hosted defaults remain
unlimited unless their owner configures limits. The relay CLI exposes matching
bandwidth/connection options, and the PHP launcher forwards them. A live probe
verified the actual running process received both public limits and that the
installed server binary matches the assembled package.

The public domain initially had a self-signed certificate. Using the server's
installed Virtualmin CLI, issued a trusted Let's Encrypt certificate for
`public.slipstreamremote.com`, enabled automatic renewal with failure-only
notifications, and applied it through Virtualmin's normal configuration reload.
It is valid until January 20, 2027. Ordinary HTTPS verification now succeeds;
no app TLS bypass or custom trust configuration was added.

The authenticated mount provides file access but a separate noninteractive SSH
shell was unavailable. A root-owned, fixed-command one-shot cron job invoked
Virtualmin, deleted its cron entry before running and had a ten-minute expiry
and five-minute process timeout. It exited 0. Certificate rollback copies and
its log remain under:
`/home/doug/WebServer/root/slipstream-public-tls-17ef6a65bc6c`.
A separate random, short-lived, token-protected one-use PHP probe checked only
the relay's two limit variables and binary hash; it deleted itself, and client
cleanup also removed it. No deployment endpoints or one-shot cron entry remain.
The private station configuration fingerprint is unchanged.

The generated public admin credential is stored with mode 0600 outside Dropbox:
`/home/doug/.local/state/slipstream/public-station-admin.json`.
Its password was verified by an authenticated admin login and was not printed
in logs. The mount briefly disconnected during builds; deployment began after
it was remounted. The hosting placeholder's dangling icon symlinks required
copying links without dereferencing them; the first incomplete backup is retained.

Rollback paths, outside both web roots:

- Public initial files: `/home/doug/WebServer/home/public/tmp/slipstream-public-initial-o7v13cvu`
- Private clients: `/home/doug/WebServer/home/relay/tmp/slipstream-rollback-0.24.0-to-0.25.0-olr7wixt`

Public rollback metadata lists original files and modes. Stop its managed relay
before removing its data or restoring placeholder files. Client rollback restores
verified old files with preserved owner/modes and the old manifest last. Preserve
all rollback directories. Private runtime did not need upgrading for this release.

## Verification

All substantial checks/builds used the serialized resource-limited wrapper.

- Workspace: **169 passed, five ignored**. The final session suite passed
  59 tests (one ignored), and the final core suite passed 36 (one ignored).
  The strengthened normal-disconnect regression passed three isolated reruns.
- Strict Linux workspace/all-target Clippy passed again on the final source
  with warnings denied. Windows desktop/all-target and arm64 Android Clippy
  passed during preparation. Final release source compiled successfully for
  all three platforms after the last routing/provisioning refinements.
- Signed desktop manifest, checksums and release identity verified with the
  unchanged trusted key. Linux starts as 0.25.0 / 131 / 4732fea. Linux app and
  server reference at most glibc 2.30 and require no GLIBC_ABI_DT_RELR; the build
  target remains glibc 2.31. Windows resources are 0.25.0.131, embedded Mesa is
  present and optional Media Foundation DLLs remain delay imports.
- Android is arm64 and non-debuggable, version 0.25.0 / 131, with the existing
  camera/microphone capture permissions and unchanged signing certificate:
  `d6ae3771378e9c00a82902e728012992834b9bf7e234fefed80dc85a4b879c6c`.
- PHP syntax and station regressions pass. Final native UI audit verifies private
  station default, exactly one own ID, separate partner drafts restored across
  tab switches, and a narrow window. Evidence:
  `/tmp/slipstream-station-tabs-t5pfgka0`. D-Bus and Xvfb were isolated and stopped.
- Both live sites passed signed update-offer checks and full Linux, Windows and
  Android HTTPS download verification, including desktop provisioning trailers
  and unchanged APK bytes. Public checks used ordinary system certificate trust.
- Public API hello is keyless and reports only its own relay, admin login is
  protected, internal package/data paths return 403/404, and relay ping/204 probes
  pass. The generated admin login and running process limits were verified.

A fresh packaged Linux app, with no configured station and capture disabled,
registered on Public over ordinary trusted HTTPS. Live lookup found its real
public ID, and its screenshot showed Public selected with that ID. Evidence:
`/tmp/slipstream-public-raw-p8zdvhd8`. Its isolated D-Bus/Xvfb processes stopped.

A live meeting smoke passed against the new public station with
`SLIPSTREAM_RELAY_ONLY=1` and private backup relays disabled: independent guest
admission, chat, real Opus audio, H.264 video and teardown. The same example's
success label mentions a guest relay lease generically; this public station is
keyless and does not require a private-station guest credential. The check used
independent fixture endpoints and did not change a real user's identity.

Latest recorded cache sizes are 2.69 GiB development/test and 31.48 GiB release,
inside the 12/32 GiB budgets. Incremental compilation remains disabled.

Artifact SHA-256 values:

| Artifact | SHA-256 |
|---|---|
| Linux | `ecaf54c23a7e8b12d96b30a347c4901126a4b19c27107626f88c210e78d840c9` |
| Windows | `b5ad32daa34a49622776a766df1b20cda089eeea303393d1fb3073a81f4bcd64` |
| Android | `806e3c2ca9fcee9943f78a94f06374156c9aa463b80b18dea4ada5d739667163` |

The manifest signer remains
`5e3e61d87100ea74f0d1c51778ce3bb19208bae122b8087d047c22c0fec55eb2`.
Desktop updater signatures are separate from operating-system publisher signing;
no paid Windows signing was introduced.

## Remaining device checks

Real next-login CachyOS autostart/tray behavior, physical Windows service calls,
real Android camera/audio, macOS native execution, mixed-DPI window behavior and
physical meeting load remain follow-ups. Publishing does not update an already
running desktop app: use About → Check for updates → Install & restart. Android
users install the new APK. Update installed Windows services for the new public-ID
snapshot support. Previous 0.24.0 autostart/search work and its verification are
preserved in the [October 21 handoff](DEVELOPMENT-2026-10-21.md).

## Station colors and authentication review — included in 0.25.1

Station tabs now use blue for the configured private station and violet for
Slipstream Public, with solid selected fills and matching hover/focus accents.
The override is scoped to these controls. The local debug app passed the existing
native audit for private default, separate partner drafts, exactly one station's
ID and narrow layout. Visual evidence:
`/tmp/slipstream-station-tabs-87q7mabl`. Fixture server errors in these screenshots
are intentional: the test uses cached IDs and an unreachable local station.
These initial checks used a local debug build; the 0.25.1 publication above
records final package verification and publication to both stations.

Reviewed the shared-code design: both listeners share one temporary code and
unattended password because they control the same host. The SPAKE2 exchange
binds the actual listening host and viewer endpoint identities. Core authentication
checks pass for correct/wrong passwords, both password types, endpoint identity
mismatch and lockout. Extended the existing dual-station integration regression
to alternate wrong passwords between the listeners, then verify both reject even
a correct password during the resulting shared lockout. It passes. This verifies
the reviewed path, not a comprehensive security audit. No authentication policy
change was made. Separate temporary codes were proposed to the user; either route
still grants access to the same computer, and the unattended password remains shared.

Public administration is at https://public.slipstreamremote.com/admin/ and asks
only for the admin password. The local mode-0600 JSON credential file documented
above contains the initial `admin_password`; the app's one-time code is unrelated
to it. Changing the password on the website does not update that local file.

The follow-up Linux build, five core authentication checks, extended dual-station
regression, native UI audit and strict desktop/session all-target Clippy all pass.
Follow-up caches: 4.31 GiB development/test and 31.48 GiB release.
