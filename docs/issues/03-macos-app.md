---
title: "macOS: package Slipstream.app and test it on a real Mac"
labels: [platform:macos, priority:high]
---

## Background

The macOS code paths exist (xcap capture, enigo input, tray, LaunchAgent
autostart, and the station's support for `station.json` in the bundle), but
nothing has been built or run on a Mac. `build-station.sh` only copies
`dist/slipstream-macos.zip` when someone provides one.

## Tasks

- [ ] Bundle layout: `Slipstream.app/Contents/{MacOS/slipstream, Info.plist,
      Resources/slipstream.icns}`. The `.icns` can be made from
      `slipstream export-icon`.
- [ ] `Info.plist` needs:
  - the bundle id (`dev.slipstream.remote`),
  - `NSScreenCaptureUsageDescription` and an Apple Events / Accessibility
    explanation,
  - `LSMinimumSystemVersion` 12.0.
- [ ] Universal binary (`aarch64` + `x86_64` via `lipo`), or two downloads.
- [ ] Check that `download.php` puts `station.json` inside the zipped bundle
      correctly, and that `provision::apply` reads it on first start.
- [ ] On a real Mac, test:
  - Screen Recording and Accessibility prompts appear, and the app works
    after granting them;
  - capture, input, keyboard layout, multi-monitor, Retina scaling;
  - whether the video shows the pointer. If it does, set
    `cursor_in_video` true for macOS in `generic.rs`.
- [ ] Self-update on macOS: replacing a binary inside a bundle and
      restarting.
- [ ] Build it in CI and add it to `build-station.sh` and the update
      manifest (`macos-aarch64`).

## Done when

A Mac user can download from the station page, grant the two permissions,
be controlled from Linux and Windows, and control them in turn.

## Pointers

- `crates/slipstream-platform/src/generic.rs` (macOS stubs for pointer
  drawing, `cursor_in_video`)
- `server/station/download.php` (macOS branch)
- `crates/slipstream-core/src/provision.rs`
- `crates/slipstream-session/src/update.rs`
- `apps/slipstream/src/gui/autostart.rs` (LaunchAgent)
