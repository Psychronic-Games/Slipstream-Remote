---
title: Publish to GitHub and get CI green on Linux, Windows and macOS
labels: [infra, priority:high]
---

## Background

The repository is local only (git history, tag `v0.5.0`).
`.github/workflows/ci.yml` builds and tests on Ubuntu, Windows and macOS,
but it has never run. The macOS code has therefore never been compiled.

## Tasks

- [ ] Create the GitHub repository and push `main` and the `v0.5.0` tag.
- [ ] Get all three CI jobs green. Expect fixes:
  - Linux job: the apt package list may need adjusting for the x11rb and
    PipeWire-dlopen changes.
  - macOS: first-ever compile of `xcap`, `enigo` and the tray on macOS.
  - Windows: native MSVC build (so far only cross-compiled with
    cargo-xwin).
- [ ] Run `cargo clippy --workspace -- -D warnings` in CI.
- [ ] Release workflow on tags: build the station package (portable Linux
      via zigbuild, Windows, and macOS once it's packaged) and attach
      `slipstream-station.zip` to the GitHub release.
- [ ] Install `gh` locally and run `scripts/file-issues.sh` to turn
      `docs/issues/` into GitHub issues.

## Done when

A push to `main` shows three green checks, and pushing a tag produces a
downloadable station package.

## Pointers

- `.github/workflows/ci.yml`
- `scripts/build-station.sh` (the release build steps the workflow should
  reuse)
- `docs/BUILDING.md` (tools: zig, cargo-zigbuild, cargo-xwin)
