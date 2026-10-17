---
title: Code signing, and downloads browsers trust (Windows, macOS)
labels: [platform:windows, platform:macos, infra, priority:high]
---

## Background

Unsigned downloads hit three separate barriers:

- **The browser.** Chrome and Edge rate downloads by how common the file
  and its signer are. A station appends its config trailer
  (`[json][u32 len]SLIPCFG1`) to every download, so **every download is a
  file nobody has seen before** and always counts as "uncommon". Signing
  alone doesn't fix that while each copy differs.
- **SmartScreen** on Windows: "Windows protected your PC". Its reputation
  attaches to the signing identity and to the file hash. Microsoft says no
  certificate (not even EV, not Azure Artifact Signing) gives instant
  trust any more; reputation builds over consecutive signed releases.
- **Gatekeeper** on macOS: an app that isn't Developer ID signed *and*
  notarised is blocked. Users must go to *System Settings → Privacy &
  Security → Open Anyway* (the right-click → Open shortcut is reportedly
  gone in recent macOS). Ad-hoc signed apps are also expected to lose their
  Screen Recording and Accessibility permissions whenever the binary
  changes, so every self-update would need someone at the Mac. That makes
  unattended Mac access impractical without proper signing.

Self-updates are not affected: the app downloads them itself, so they
carry no Mark-of-the-Web or quarantine flag. Only the first install meets
these warnings.

## Proposed design

1. **One identical, signed file per platform and release**, for every
   station. Hash reputation then builds globally instead of starting from
   zero on every download.
2. The station config travels **outside** the binary:
   - *Zero clicks:* the app reads where it was downloaded from (Windows:
     the `Zone.Identifier` stream's `HostUrl`; macOS: the
     `kMDItemWhereFroms` attribute). That URL is the station's
     `download.php?os=…&c=<setup code>`. The app redeems the code at
     the station for the address, access key and name.
   - *One click:* the download page's step 2 is a *Connect this app to
     Acme IT* button, a `slipstream://link?...` URL that the app registers
     on first start.
   - *By hand:* type the station address and a short setup code.
3. Releases are built and signed in CI (GitHub Actions; macOS needs a Mac
   runner anyway). `build-station.sh` assembles a station from a release
   instead of compiling, and copies its pre-signed `manifest.json`: the
   update signature covers platform, version, build, size and hash, none
   of which depend on the station.

## Signing options

| | Cost | Notes |
|---|---|---|
| SignPath Foundation | free | Open-source projects with a public repo, builds from CI, and a published signing policy. The publisher shown is "SignPath Foundation". |
| Azure Artifact Signing | about $10/month | Individuals in the USA and Canada; organisations in the USA, Canada, EU and UK. Needs a paid Azure subscription and identity validation. |
| OV certificate | about $150–300/year | Needs a hardware token or cloud HSM. |
| Apple Developer Program | $99/year | Required for Developer ID signing and notarisation. No free route. |

## Decided

Windows: **SignPath** (free). Apple Developer Program: **not yet**.

## Tasks

- [x] App: read the download origin, redeem setup codes, register
      `slipstream://` (Windows, Linux).
- [x] Station: setup codes (stateless, 30 days) in download links and on
      the page, the `link` API op, *Connect this app* button; signed builds
      and the Mac zip served untouched; the trailer kept for unsigned builds.
- [x] Release workflow (`.github/workflows/release.yml`) with the SignPath
      step, and `build-station.sh --from-release`.
- [x] Signing policy page (`docs/CODE-SIGNING.md`): fill in the GitHub
      username.
- [ ] Make the repository public; apply at signpath.org; set the
      repository variables and secret listed in `release.yml`.
- [ ] First tagged release; confirm the signed .exe and that SmartScreen
      reputation builds over a few releases.
- [ ] macOS: Developer ID signing, notarisation and stapling in the
      workflow, once the Apple membership is there.

## Pointers

- `crates/slipstream-core/src/provision.rs`
- `server/station/download.php`, `server/station/index.php`
- `crates/slipstream-session/src/update.rs` (`base_len`)
- `tools/slipstream-release` (manifest signing)
