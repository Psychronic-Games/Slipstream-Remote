---
title: Publish to GitHub and get CI green on Linux, Windows and macOS
labels: [infra, priority:high]
---

## Background

The repository is local only, with releases through 0.23.2 and no GitHub
remote configured.
`.github/workflows/ci.yml` builds and tests on Ubuntu, Windows and macOS,
but it has never run. macOS has been cross-type-checked for Apple Silicon
from Linux; native Mac builds and runtime checks remain pending.

`.github/workflows/release.yml` already defines tag-triggered Linux/Windows/
macOS builds and a draft GitHub release. Windows publisher signing is
conditional on SignPath configuration. The workflow produces platform
artifacts and `release.json`; local `build-station.sh --from-release DIR`
assembles the station and signs its update manifest with the private update
key kept on the build machine. Android is currently built locally.

## Tasks

- [ ] Create the GitHub repository, configure its remote, and push reviewed
      `main` and the release tags intended for publication.
- [ ] Get all three CI jobs green. Expect fixes:
  - Linux job: the apt package list may need adjusting for the x11rb and
    PipeWire-dlopen changes.
  - macOS: first native build of capture, input and tray dependencies.
  - Windows: native MSVC build (so far only cross-compiled with
    cargo-xwin).
- [ ] Add `cargo clippy --workspace --all-targets --locked -- -D warnings`
      to CI and verify it on all three OSes.
- [x] Prepare the tag release workflow and local station assembly from
      downloaded CI artifacts.
- [ ] Configure the update public key and, after acceptance, SignPath
      variables/secrets; verify a first tagged CI release and local station
      assembly. Keep the private update key out of CI.
- [ ] Publish the locally assembled station package with the release.
- [ ] Authenticate `gh` and run `scripts/file-issues.sh` to turn
      `docs/issues/` into GitHub issues.

## Done when

A push to `main` shows green native build/test/Clippy checks on all three
OSes. A release tag produces verified downloadable platform artifacts, and
the locally assembled, update-signed station package is attached to the release.

## Pointers

- `.github/workflows/ci.yml`
- `scripts/build-station.sh` (the release build steps the workflow should
  reuse)
- `docs/BUILDING.md` (tools: zig, cargo-zigbuild, cargo-xwin)
