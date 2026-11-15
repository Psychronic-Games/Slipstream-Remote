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
- [x] Say on the Bridge when the service can't read the screen (a Debian
      12 VM became unreachable after installing, 2026-11-15: `GETFB`
      answered `ENODEV`, the kernel's answer for a framebuffer whose
      driver has no `create_handle`).
- [x] The signed-in desktop hosts in the service's place while its
      session is in front and unlocked (`service_desktop.rs`): screen on
      any graphics card, clipboard, sound, the user's folders.
- [x] The handover on real machines: lock, unlock and log out with a
      link open (user, 2026-11-15).
- [ ] Finer handover checks: switching users, an app update and a service
      update with a link open; a second user's app (refused: other
      identity); XFCE and others that don't set `LockedHint`.
- [x] vmwgfx: surface framebuffers have no `create_handle` before Linux
      6.11, and with screen targets every framebuffer is a surface (3D on,
      or off: `vmw_create_bo_proxy` wraps the dumb buffer). So Debian 12
      (6.1) can't be read, with 3D acceleration off too; Debian 13 (6.12)
      works on the user's VMs (2026-11-15).
- [ ] The rest of the bridge to the signed-in user's app, as on Windows:
      one-time code, link approval, host chat, meeting calls, and a sign
      that someone is connected.
- [ ] Tiled framebuffers (Intel, AMD, NVIDIA): import the buffer through
      EGL/GBM and read it back, or VA-API for Intel/AMD.
- [ ] Display arrangement on Wayland with several displays (positions are
      the compositor's; the service assumes left to right).
- [ ] Draw the pointer from the cursor plane when the viewer asks for it.
- [ ] Systems without systemd.

## Pointers

- `crates/slipstream-platform/src/kms.rs`, `uinput.rs`, `system.rs`
- `apps/slipstream/src/service_linux.rs`, `service_desktop.rs`
- `apps/slipstream/src/gui/settings.rs` (the Config row),
  `gui/mod.rs` (`service_action`, `selected_dir_status`)
