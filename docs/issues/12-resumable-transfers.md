---
title: Resume interrupted file transfers
labels: [feature]
---

## Background

Uploads and downloads use private staging files, publish only completed
transfers and clean staging on handled interruption/cancellation. Resume is
not implemented: a dropped connection loses progress, which is painful for
multi-gigabyte files over slow links. Persistent resumable staging below is
future work and must retain the existing no-overwrite and integrity guarantees.

## Tasks

- [ ] Keep the partial file on interruption, recording its target and
      expected size.
- [ ] `Upload` and `Download` take an offset. Before resuming, both sides
      compare a hash of the existing prefix (blake3).
- [ ] The file manager offers *Resume* on failed transfers, also after
      reconnecting.
- [ ] Delete partial files left behind for more than a day.
- [ ] Protocol change: bumps `PROTOCOL_VERSION`.

## Pointers

- `crates/slipstream-session/src/files.rs`, `fsops.rs`
- `crates/slipstream-core/src/proto.rs` (`StreamOpen`)
- `apps/slipstream/src/gui/files.rs` (transfer list)
