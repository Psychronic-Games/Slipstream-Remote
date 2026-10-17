---
title: "Android app: viewer first, then host"
labels: [platform:android, feature]
---

## Background

Android support was part of the original plan. egui, wgpu and iroh all
run on Android, so the session and UI code can be shared.

## Tasks

**Viewer (phase 1)**
- [ ] `cargo-ndk` / `android-activity` build of `apps/slipstream`, and an
      APK in the station's `clients/`.
- [ ] Touch input: tap to click, two-finger scroll, long-press for right
      click, pinch to zoom the remote screen, an on-screen keyboard and a
      special-keys bar.
- [ ] Hardware H.264 decode (MediaCodec), or keep OpenH264.
- [ ] Provisioning: the station config travels with the APK. A trailer
      breaks the APK signature, so use a QR code or deep link from the
      download page instead.

**Host (phase 2)**
- [ ] Screen capture with MediaProjection.
- [ ] Input through an AccessibilityService (gestures).
- [ ] A foreground service to stay reachable.

## Pointers

- `apps/slipstream/` (egui app), `crates/slipstream-session/` (portable
  session logic)
- `server/station/index.php` (download page and OS detection, which
  currently treats Android as unknown)
