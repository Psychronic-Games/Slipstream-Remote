---
title: Address book with saved devices and passwords
labels: [feature]
---

## Status

Done another way in 0.6.0: **technician accounts** on the station keep
each technician's saved devices, with passwords sealed in the app
(`core/src/vault.rs`) so the station can't read them, live presence, a
shared list kept by the admin, and sign-in to several stations
(`session/src/accounts.rs`, `gui/saved.rs`). The plan below (OS keychain
per computer) is kept for reference. Still to do: try it against the live
station.

## Background

The Bridge page only lists *Recent links*. Technicians who support the
same machines every day want saved entries: name, ID, notes and, for
unattended machines, the password, connected with one click.

## Tasks

- [ ] Saved devices: add from a recent link or by hand; rename; group or
      tag; search.
- [ ] Store passwords in the OS keychain (Windows Credential Manager,
      Secret Service on Linux, macOS Keychain) via the `keyring` crate.
      Never store them in `settings.toml`.
- [ ] Online status for saved devices. This needs a batched `lookup` in the
      directory API (PHP and `slipstream-server`).
- [ ] Optional: the station's device list (admin page) as a shared address
      book for the organisation's technicians.

## Pointers

- `apps/slipstream/src/gui/home.rs` (`recent_panel`)
- `crates/slipstream-core/src/config.rs`
- `server/station/api/index.php`, `apps/slipstream-server/src/api.rs`
