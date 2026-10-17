---
title: "Wayland: keep unattended access working after reboot"
labels: [platform:linux, testing]
---

## Background

On Wayland, screen sharing goes through the desktop portal. The first
session asks the user, and Slipstream stores the portal's restore token
(`host.portal_restore_token`) so later sessions don't ask again. It isn't
known whether that holds after logout or reboot on KDE, GNOME and wlroots,
or what happens when the token has expired. If it fails, an unattended
machine becomes unreachable in practice until someone clicks *Share*.

## Tasks

- [ ] Test on KDE Plasma 6, GNOME 46+ and Sway/Hyprland (xdg-desktop-portal-wlr
      or -hyprland): reboot, autostart, then connect from another computer
      with nobody at the machine.
- [ ] If the prompt comes back: tell the viewer clearly
      ("waiting for someone at the remote computer to allow screen
      sharing") instead of timing out. Document the desktop-specific
      settings that pre-authorise it (e.g. KDE's *Remote Desktop*
      permissions).
- [ ] Check that a token rejected by the portal is dropped and replaced, so
      it doesn't fail again and again.

## Pointers

- `crates/slipstream-platform/src/wayland.rs`
- `crates/slipstream-session/src/host.rs` (`set_restore_token`, the
  "Waiting for the remote side…" notice)
- *Config → Screen permission* reset in `apps/slipstream/src/gui/settings.rs`
