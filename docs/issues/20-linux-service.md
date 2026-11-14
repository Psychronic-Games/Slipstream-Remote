---
title: "Linux service: login screen and locked sessions"
labels: [platform:linux, feature, testing]
---

## Background

A Linux host runs inside one desktop session. On Wayland the lock screen
and the display manager's login screen are kept out of the screen-sharing
portal, so a locked or logged-out machine shows a black picture. The Linux
service (`slipstream service install`) is one root process that captures
what the graphics card shows (KMS) and types through kernel input devices
(uinput). See [ARCHITECTURE.md](../ARCHITECTURE.md#the-linux-service).

## Tasks

- [x] KMS capture of linear framebuffers, uinput keyboard and pointer, the
      `system` backend.
- [x] `slipstream service install | uninstall | start | status | check`,
      the systemd unit, the machine profile and identity handover.
- [x] App: Config → Linux service, Bridge shows the service's ID and state,
      the app stops hosting.
- [ ] Run on a real virtual machine (VMware `vmwgfx` first): `sudo
      slipstream service check --picture screen.png`, then a session at the
      login screen, a locked session, across log in and log out, and after
      a reboot. Check the blanked-screen wake and a resolution change.
- [ ] vmwgfx with 3D acceleration on (surface-backed framebuffers): is the
      mapped memory current?
- [ ] Bridge to the signed-in user's app, as on Windows: one-time code,
      link approval, host chat, clipboard, sound, meeting calls, and the
      user's folders as the Files window's defaults.
- [ ] Tiled framebuffers (Intel, AMD, NVIDIA): import the buffer through
      EGL/GBM and read it back, or VA-API for Intel/AMD.
- [ ] Display arrangement on Wayland with several displays (positions are
      the compositor's; the service assumes left to right).
- [ ] Draw the pointer from the cursor plane when the viewer asks for it.
- [ ] Systems without systemd.

## Pointers

- `crates/slipstream-platform/src/kms.rs`, `uinput.rs`, `system.rs`
- `apps/slipstream/src/service_linux.rs`
- `apps/slipstream/src/gui/settings.rs` (the Config row),
  `gui/mod.rs` (`service_action`, `selected_dir_status`)
