---
title: Try privacy mode, copied files and port forwarding on real machines
labels: [testing]
---

## Background

Three session tools arrived together with protocol v5. Each has an
end-to-end test (`crates/slipstream-session/src/lib.rs`), but the parts
that touch the operating system can only be checked on real desktops.

## Checklist

Copy and paste files:

- [ ] Windows → Windows: copy a file and a folder in Explorer, Ctrl+V in
      an Explorer window on the remote screen. Both arrive, nested folders
      intact.
- [ ] Linux (KDE/Dolphin) → Windows, and Windows → Linux.
- [ ] Copy files on the remote computer, then paste them locally (the
      notice says *ready to paste here*).
- [ ] Too big (over 64 MB): the notice points to *Files*.
- [ ] Text paste still works (Ctrl+V into Notepad), and Ctrl+C/X/V still
      work in the app's own text fields.
- [ ] Mac: Cmd+V pastes files (once the Mac build runs, issue 03).

Privacy mode (Windows 10 2004 or later on the remote side):

- [ ] *Tools ▾ → Privacy mode*: the remote monitors go black with the
      message; the viewer still sees the desktop and can work.
- [ ] Under the Slipstream service, the remote keyboard and mouse are
      blocked; Ctrl+Alt+Del at the remote computer still works (Windows
      always lets it through).
- [ ] Disconnecting (or the link dropping) brings the screen back.
- [ ] Several monitors: all of them are covered.

Port forwarding:

- [ ] Forward to the remote computer's Remote Desktop (`localhost:3389`)
      and connect with `mstsc /v:127.0.0.1:<port>`.
- [ ] Forward to another machine on the remote network (a router's web
      page).
- [ ] With *Port forwarding* off on the host, the menu item is greyed out.

## Pointers

- `crates/slipstream-platform/src/privacy.rs`, `generic.rs` (`BlockLocal`)
- `crates/slipstream-session/src/tunnel.rs`, `files.rs` (`StreamOpen::Tunnel`,
  `ClipboardFiles`)
- `crates/slipstream-session/src/viewer.rs` (`paste_files`,
  `fetch_clipboard_files`)
- `apps/slipstream/src/gui/mod.rs` (`raw_input_hook`: Ctrl+V)
