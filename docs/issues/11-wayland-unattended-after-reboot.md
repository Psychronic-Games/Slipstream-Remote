---
title: "Wayland: keep unattended access working after reboot"
labels: [platform:linux, testing]
---

## Background

On Wayland, screen sharing goes through the desktop portal. Since 0.18.0,
the GUI/headless host prepares permission at startup, and Config offers
Approve now and Reset / approve again without waiting for a viewer.
Slipstream stores the portal's restore token when supplied
(`host.portal_restore_token`) so later sessions don't ask again. It isn't
known whether that holds after logout or reboot on KDE, GNOME and wlroots,
or what happens when the token has expired. If it fails, an unattended
machine becomes unreachable in practice until someone clicks *Share*.

0.22.0 added bounded portal startup/approval waits, request cancellation,
reopening an empty cached backend, and recovery of idle KDE portal services
using stale Qt libraries. Automated/isolated checks cover these paths; a
future real KDE upgrade and fresh consent/reboot remain device checks.

## Tasks

- [ ] Test on KDE Plasma 6, GNOME 46+ and Sway/Hyprland (xdg-desktop-portal-wlr
      or -hyprland): reboot, autostart, then connect from another computer
      with nobody at the machine.
- [x] Report screen preparation to the viewer and bound stalled portal
      requests; offer local Approve now, Cancel request and reset controls.
- [ ] After reboot/expired consent, verify the viewer sees an actionable
      preparation/failure notice and the local approval controls recover.
      Document tested desktop-specific permission settings.
- [ ] Check that a token rejected by the portal is dropped and replaced, so
      it doesn't fail again and again.
- [ ] After a KDE/Qt upgrade, verify stale portal services refresh only while
      idle. If another app is sharing, preserve that session and recover after
      it closes; verify cancellation and retry without signing out where possible.

## Pointers

- `crates/slipstream-platform/src/wayland.rs`
- `crates/slipstream-session/src/host.rs` (`set_restore_token`, the
  "Preparing screen sharing on the remote computer…" notice)
- `crates/slipstream-platform/src/portal_health.rs`
- `apps/slipstream/src/gui/screen_access.rs` (prepare/cancel lifecycle)
- *Config → Screen permission* reset in `apps/slipstream/src/gui/settings.rs`
