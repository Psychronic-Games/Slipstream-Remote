# Development handoff — 2026-10-22

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
