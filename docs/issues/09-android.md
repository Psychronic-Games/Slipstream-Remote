---
title: "Android app: control PCs from a phone or tablet, and Android devices from a PC"
labels: [platform:android, feature]
---

## Background

Two goals: from an Android phone or tablet, control a computer with touch;
and from a computer, control an Android device whose user installed the
APK and opened it. egui, wgpu and iroh all run on Android, so the app is
the desktop app's GUI and session code, plus a small Java activity (see
ARCHITECTURE.md, "The Android app", and BUILDING.md, "Android").

## Phase 0: the app runs on Android (shipped in 0.8.0)

- [x] `apps/slipstream` split into a library and the desktop program;
      `apps/slipstream-android` is the library a GameActivity loads.
      GameActivity rather than NativeActivity for its soft-keyboard
      support.
- [x] Gradle project (Java, no Kotlin), `scripts/build-android.sh`, APK in
      `dist/android/` (Android 8+; arm64-v8a station release, x86_64
      available for emulator development).
- [x] Settings, identity and log in the app's private storage; log to
      logcat.
- [x] Clear of the status and navigation bars, camera cutout and
      keyboard (edge to edge since Android 15).
- [x] Soft-keyboard typing, Enter/Done, the keyboard opening on the first
      tap, the focused field kept in view above it, no flicker while it
      slides up (it flickered on a real phone).
- [x] `slipstream://link` opens the app and links it to a station (tried
      against the live station with a bad code: refused, as it should be).
- [x] Sound plays through AAudio (cpal).
- [x] On the emulator (Android 16): online with a local server, a session
      to a test-pattern host at 60 fps.

## Phase 1: control a PC from Android

- [x] Phone layout: a handheld console below 600 points (sections along
      the bottom, stacked forms), slimmer elbows on short windows.
- [ ] Toasts that fit a phone; the Files and System information windows
      on a phone.
- [x] Touch in a session (`gui/touch.rs`): tap, long-press for right
      click, one-finger drag, pinch to zoom, two fingers to pan (or scroll
      when not zoomed). Tried with a scripted pinch on the emulator.
- [ ] A trackpad mode (a pointer moved by dragging), for precise work on
      small screens.
- [ ] Keyboard in a session: the soft keyboard's text typed on the remote
      computer (text to key presses), and a bar of special keys (Ctrl,
      Alt, Win, Esc, Tab, arrows, F-keys, Ctrl+Alt+Del).
- [x] Session toolbar sized for a phone (Console menu).
- [ ] Landscape full screen without the system bars.
- [x] Device name from manufacturer/model for default identities (0.17.0).
- [x] Station: Android on the download page (picked for phones), the
      arm64 APK in `clients/`, served untouched (0.8.0).
- [ ] A QR code of the `slipstream://link` URL on the page for desktop
      visitors (the phone's camera app opens it, so the app needs no
      scanner).
- [x] Release signing key for the APK, beside the update key (0.8.0).
- [ ] `android` in the update manifest, once the app can install updates
      (phase 3).
- [ ] Try on real phones and tablets (which models and Android versions?).

## Phase 2: control an Android device from a PC

Initial host shipped in 0.17.0: a persistent ID/password, explicit capture
consent, MediaProjection foreground service, accessibility input and desktop
Android navigation. Android 16 emulator runtime checks cover view-only access,
PC input/typing/dragging, background capture, rotation and notification stop.
Real phones still need confirmation. Capture currently uses bounded RGBA
ImageReader output and the existing software H.264 encoder.

0.17.1 fixes retention of the last pending still frame and disables Android
UDP segmentation offload after reproducing a cold direct-link stall. Three
fresh direct connections then passed; a cold relayed connection also passed.
Idle capture revocation/regrant, PC control and rotation were rechecked.
See the [audit](../AUDIT-2026-10-18.md). Physical-device and older-Android checks remain
open; emulator checks do not complete the real-phone task above.


- [ ] Screen capture with MediaProjection into the hardware H.264 encoder
      (MediaCodec with an input surface: no copies through the CPU),
      OpenH264 as a fallback.
- [x] Input through an AccessibilityService: taps and swipes
      (`dispatchGesture`), Back/Home/Recents (`performGlobalAction`), text
      into the focused field.
- [x] A foreground service (type `mediaProjection`) keeps the host
      reachable with the app in the background.
- [x] Guided setup screen: Android 13+ greys out the accessibility switch
      for apps installed from a browser until *App info → ⋮ → Allow
      restricted settings*. Without it a session can only watch.
- [x] Viewer side: Back/Home/Recents actions for an Android host, screen
      rotation. Protocol v6.

Limits to explain to users: the person at the device taps *Allow* when
screen sharing starts, and again after the app or phone restarts (Android
14+ asks for every new capture). Fully unattended Android needs
manufacturer agreements (Samsung Knox and others).

## Phase 3

- [ ] Sound from Android hosts (AudioPlaybackCapture, Android 10+).
- [ ] Clipboard (JNI `ClipboardManager`; arboard has no Android support).
- [ ] Files: the app's own folder and Downloads (MediaStore).
- [ ] Hardware H.264 decoding (MediaCodec) for the viewer.
- [ ] Self-update through `PackageInstaller` (the user confirms).

## Pointers

- `apps/slipstream-android/` (`src/lib.rs`: `android_main` and the JNI
  callbacks; `app/`: the activity, manifest, icons)
- `apps/slipstream/src/gui/android.rs` (insets, typing, focus in view)
- `apps/slipstream/src/gui/single.rs` (`inbox`: links)
- `crates/slipstream-platform/` (capture and input backends; `android.rs` bridges JNI and
  `android_frames.rs` owns the tested bounded capture handoff)
- `server/station/index.php` (download page and OS detection)
