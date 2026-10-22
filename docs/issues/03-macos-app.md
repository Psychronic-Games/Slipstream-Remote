---
title: "macOS: build Slipstream.app on a Mac and test it"
labels: [platform:macos, priority:high]
---

## Background

`scripts/build-macos.sh` builds a universal (Apple Silicon + Intel), ad-hoc
signed `Slipstream.app` and zips it to `dist/slipstream-macos.zip`. The code
type-checks for `aarch64-apple-darwin` (cross-checked from Linux, which
already caught one macOS-only compile error), but it has never been built
or run on a Mac.

The station serves the zip untouched; the app links to its station from the
download address or a setup code. Self-updates replace the bundle's
`Contents` folder.

## Build

```sh
xcode-select --install          # once
brew install nasm               # once; rustup from rustup.rs
SLIPSTREAM_UPDATE_KEYS=PUBLIC_KEY scripts/check.sh scripts/build-macos.sh
```

## Checklist

**Install**
- [ ] The zip from the station page opens; *Slipstream* moves to
      Applications and starts after *Open Anyway* (Privacy & Security).
- [ ] The Dock and app switcher show the icon and "Slipstream Remote".
- [ ] It connects to the station by itself (download address), or with the
      setup code on the Bridge.

**Being controlled (host)**
- [ ] Screen Recording and Accessibility prompts appear; after granting and
      restarting, a viewer sees the screen and can move, click and type.
- [ ] Retina scaling: the pointer lands where it should; the keyboard works
      across layouts (try a non-US layout).
- [ ] Multi-monitor switching.
- [ ] Whether the video shows the pointer. If it does, set
      `cursor_in_video` true for macOS in `generic.rs`.

**Controlling others (viewer)**
- [ ] Control the Linux and Windows machines; full screen; special keys.
- [ ] File manager, drag and drop, clipboard and chat both ways.

**Updates**
- [ ] With a newer build on the station, *Install & restart* works and the
      app still opens afterwards (no "damaged" message).
- [ ] Note whether Screen Recording / Accessibility have to be granted
      again after the update (expected while it's ad-hoc signed).

## Known gaps

- `slipstream://` links don't reach the app: macOS delivers them as Apple
  Events and winit (through eframe) doesn't pass them on. Setup codes work.
  A fix needs an Apple Event handler (`NSAppleEventManager`) or winit
  support.
- Not notarised (Developer ID signing and Apple Developer Program membership
  remain pending; see issue 05).

## Pointers

- `scripts/build-macos.sh` (bundle, Info.plist, icon, signing)
- `crates/slipstream-platform/src/generic.rs` (capture thread)
- `crates/slipstream-session/src/update.rs` (`install_bundle`)
- `crates/slipstream-core/src/provision.rs` (`download_origins`)
