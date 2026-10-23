---
title: Get CI green on Linux, Windows and macOS
labels: [infra, priority:high]
---

## Background

The repository is public at
https://github.com/Psychronic-Games/Slipstream-Remote (October 2026), and
releases are published there by `scripts/publish-github-release.sh`.
`.github/workflows/ci.yml` builds and tests on Ubuntu, Windows and macOS
on every push; its first runs need fixes. macOS has been cross-type-checked for Apple Silicon
from Linux; native Mac builds and runtime checks remain pending.

`.github/workflows/release.yml` builds Linux/Windows/macOS by hand (from the
Actions tab) into a draft pre-release. Windows publisher signing is
conditional on SignPath configuration. The workflow produces platform
artifacts and `release.json`; local `build-station.sh --from-release DIR`
assembles the station and signs its update manifest with the private update
key kept on the build machine. Android is currently built locally.

## Tasks

- [x] Create the GitHub repository, configure its remote, and push `main`.
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
- [x] Publish the locally assembled station package with the release
      (`scripts/publish-github-release.sh`).
- [ ] Authenticate `gh` and run `scripts/file-issues.sh` to turn
      `docs/issues/` into GitHub issues.

## Done when

A push to `main` shows green native build/test/Clippy checks on all three
OSes, and a manual CI release build produces verified platform artifacts
that `build-station.sh --from-release` accepts.

## Pointers

- `.github/workflows/ci.yml`
- `scripts/build-station.sh` (the release build steps the workflow should
  reuse)
- `docs/BUILDING.md` (tools: zig, cargo-zigbuild, cargo-xwin)
