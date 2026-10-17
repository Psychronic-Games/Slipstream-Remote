---
title: Code signing (Windows Authenticode, macOS notarisation)
labels: [platform:windows, platform:macos, infra]
---

## Background

Unsigned downloads trigger SmartScreen on Windows ("More info → Run
anyway") and Gatekeeper on macOS. That is a real barrier for the
non-technical clients a station serves.

## Catch

Stations append a config trailer to each download
(`[json][u32 len]SLIPCFG1`). That changes the file after signing:

- **Authenticode:** data appended after the signature invalidates it,
  unless it goes into the certificate table's padding (the approach some
  installers use). Alternatives: leave the exe untouched and put the
  station config in a separate file next to it, or have the station sign
  on the fly (needs the key on the server, which isn't acceptable).
- **macOS:** `station.json` inside the bundle invalidates the code
  signature. Alternative: put it in an extended attribute, or next to the
  bundle in the zip.

Pick a design that keeps "download and it's already configured" working.

## Tasks

- [ ] Decide how the station config travels without breaking signatures.
- [ ] Windows: get a code-signing certificate (Azure Trusted Signing is
      cheap) and sign in the release workflow.
- [ ] macOS: Developer ID signing, notarisation and stapling in CI.
- [ ] The update checksum (`sha256` of the app without the trailer) must
      still match.

## Pointers

- `crates/slipstream-core/src/provision.rs`
- `server/station/download.php`
- `crates/slipstream-session/src/update.rs` (`base_len`)
