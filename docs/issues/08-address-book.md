---
title: Verify station-saved devices and passwords on the live station
labels: [testing]
---

## Status

Implemented since 0.6.0: **technician accounts** on the station keep
each technician's saved devices, with passwords sealed in the app
(`core/src/vault.rs`) before station storage, live presence, a
shared list kept by the admin, and sign-in to several stations
(`session/src/accounts.rs`, `gui/saved.rs`). Group selection and synced empty
groups are implemented through 0.22.1, alongside New Link search and private/
shared grouping. Isolated PHP/native GUI checks pass; a live-station workflow
check remains open.

## Background

Bridge lists Recent links and station-saved devices. Passwords are sealed
client-side with the technician's vault key before storage on the station.
The app retains its sign-in token and vault key in its local settings for
remembered sign-in. The original proposal to store each password in the OS
keychain was superseded by station accounts; OS credential-store integration
would be separate future hardening, not an unfinished address-book feature.

## Tasks

- [x] Save from recent links or by ID, edit names/notes, group and search.
- [x] Seal saved device passwords client-side; display online presence.
- [x] Admin-managed shared devices and simultaneous sign-in to several stations.
- [x] Sync groups, including empty private groups; support saved passwords
      and station-specific routes in New Link.
- [ ] On a live station with a designated test account, sign in from two
      computers and verify saved-device/group sync and saved-password connection.
- [ ] Verify admin-only shared-list edits and same-numbered IDs on different
      stations use the correct station route.
- [ ] With designated test accounts, confirm sign-out/revocation and password
      reset clear the expected remembered credentials and saved secrets.

## Pointers

- `apps/slipstream/src/gui/saved.rs`, `quick_connect.rs`
- `crates/slipstream-session/src/accounts.rs`
- `crates/slipstream-core/src/account.rs`, `vault.rs`
- `crates/slipstream-core/src/config.rs`
- `server/station/api/index.php`, `server/station/lib/slipstream.php`
- `scripts/test-station-accounts.py` (isolated fixture checks)
