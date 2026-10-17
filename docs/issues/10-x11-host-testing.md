---
title: "Linux X11: test the host on a real X11 session"
labels: [platform:linux, testing]
---

## Background

X11 capture (MIT-SHM with a GetImage fallback, XFixes pointer, RandR
monitors) and XTest input are implemented, and unit tests cover the
pointer blending. But no one has been controlled from a real X11 desktop
yet. The dev machine runs Wayland.

## Checklist

On an X11 session (e.g. Xfce, MATE, Cinnamon, or KDE/GNOME "on Xorg"):

- [ ] The host starts without prompts; the stats show about 60 fps on a
      busy screen.
- [ ] The pointer is visible and shapes change.
- [ ] Multi-monitor: switching screens; the offsets of a monitor not at
      0,0 are right for both picture and input.
- [ ] Keyboard: non-US layout, AltGr characters, the special-keys menu.
- [ ] Clipboard both ways; file manager; drag and drop to the Desktop.
- [ ] Without MIT-SHM (e.g. a remote X display over `ssh -X`, where shared
      memory isn't available) it still works through GetImage.
- [ ] HiDPI (`Xft.dpi` 144).

## Pointers

- `crates/slipstream-platform/src/x11.rs`
- `crates/slipstream-platform/src/generic.rs` (enigo input on Linux)
