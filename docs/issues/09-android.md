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

## Phase 0: the app runs on Android (done, unreleased)

- [x] `apps/slipstream` split into a library and the desktop program;
      `apps/slipstream-android` is the library a GameActivity loads.
      GameActivity rather than NativeActivity for its soft-keyboard
      support.
- [x] Gradle project (Java, no Kotlin), `scripts/build-android.sh`, APK in
      `dist/android/` (arm64-v8a and x86_64, Android 8+, about 33 MB).
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
- [ ] Device name from the phone's model (it's empty now: Android has no
      host name).
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

- [ ] Screen capture with MediaProjection into the hardware H.264 encoder
      (MediaCodec with an input surface: no copies through the CPU),
      OpenH264 as a fallback.
- [ ] Input through an AccessibilityService: taps and swipes
      (`dispatchGesture`), Back/Home/Recents (`performGlobalAction`), text
      into the focused field.
- [ ] A foreground service (type `mediaProjection`) keeps the host
      reachable with the app in the background.
- [ ] Guided setup screen: Android 13+ greys out the accessibility switch
      for apps installed from a browser until *App info → ⋮ → Allow
      restricted settings*. Without it a session can only watch.
- [ ] Viewer side: Back/Home/Recents buttons for an Android host, screen
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
- `crates/slipstream-platform/` (capture and input backends; Android has
  none yet)
- `server/station/index.php` (download page and OS detection)
