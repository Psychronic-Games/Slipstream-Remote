---
title: Code signing, and downloads browsers trust (Windows, macOS)
labels: [platform:windows, platform:macos, infra, priority:high]
---

## Background

As of 0.24.0, desktop updates have Ed25519 release signatures and Android
APKs use the established Android release certificate. Windows publisher
signing and macOS Developer ID signing/notarisation remain pending. These
are separate from the signed desktop update manifest.

The Windows executable is not Authenticode-signed and may trigger download
or SmartScreen warnings. Unsigned Linux/Windows downloads carry a station
config trailer, so their complete file hash varies by station. Files marked
as code-signed, Mac bundles and Android APKs are served unchanged.

macOS packaging currently uses ad-hoc signing. The intended first-install
workaround is *System Settings → Privacy & Security → Open Anyway*;
permission retention after updates and native install behavior still need
the real-Mac checks in issue 03. Publisher signing does not by itself
guarantee that browsers or operating systems will trust a download.

## Prepared release design

1. **An unchanged publisher-signed executable or bundle per release**,
   shared by stations once publisher signing is configured.
2. Station config travels **outside** files served unchanged:
   - *Zero clicks:* the app reads where it was downloaded from (Windows:
     the `Zone.Identifier` stream's `HostUrl`; macOS: the
     `kMDItemWhereFroms` attribute). That URL is the station's
     `download.php?os=…&c=<setup code>`. The app redeems the code at
     the station for the address, access key and name.
   - *One click on supported platforms:* the download page's step 2 is a *Connect this app to
     Acme IT* button, a `slipstream://link?...` URL that the app registers
     on first start. macOS URL delivery remains unimplemented; setup codes
     are the fallback there.
   - *By hand:* type the station address and a short setup code.
3. The prepared GitHub Actions workflow builds Linux, Windows and macOS
   artifacts plus `release.json`. Its Windows SignPath step is conditional
   on configuration. CI has not run. `build-station.sh --from-release DIR`
   assembles downloaded artifacts and **creates/signs the update manifest
   locally**; the private update key stays out of CI and off the station.

## Decided

Windows: pursue **SignPath Foundation**; application/acceptance and workflow
credentials are pending. Apple Developer Program enrollment and notarisation
are also pending. See the [current signing policy](../CODE-SIGNING.md).

The October 22 review records Microsoft Artifact Signing as an alternative for
direct downloads, and the Foundation's scanner-related eligibility restriction.
Confirm eligibility before relying on free SignPath signing; no change of
provider or enrollment is selected. Publisher signing builds reputation over
time; EV certificates do not guarantee an immediate SmartScreen exemption.

## Tasks

- [x] App: read the download origin, redeem setup codes, register
      `slipstream://` (Windows, Linux).
- [x] Station: setup codes (stateless, 30 days) in download links and on
      the page, the `link` API op, *Connect this app* button; signed builds
      and the Mac zip served untouched; the trailer kept for unsigned builds.
- [x] Release workflow (`.github/workflows/release.yml`) with the SignPath
      step, and `build-station.sh --from-release`.
- [x] Draft the signing policy page (`docs/CODE-SIGNING.md`).
- [ ] Name the actual GitHub reviewers and signing approvers in the policy
      before applying/submitting a signing request.
- [ ] Confirm SignPath Foundation eligibility for the network scanner, or
      select another publisher-signing provider and complete its validation.
- [ ] Make the repository public; apply at signpath.org; set the
      repository variables and secret listed in `release.yml`.
- [ ] First CI-tagged release with signing configured: verify the Windows
      publisher signature, unchanged station download bytes, station linking
      and update signatures. Record browser/SmartScreen behavior on real Windows.
- [ ] macOS: Developer ID signing, notarisation and stapling in the
      workflow, once the Apple membership is there.

## Pointers

- `crates/slipstream-core/src/provision.rs`
- `server/station/download.php`, `server/station/index.php`
- `crates/slipstream-session/src/update.rs` (`base_len`)
- `tools/slipstream-release` (manifest signing)
