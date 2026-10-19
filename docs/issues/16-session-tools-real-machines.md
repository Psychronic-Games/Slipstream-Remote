---
title: Try privacy mode, copied files and port forwarding on real machines
labels: [testing]
---

## Background

Three session tools arrived together with protocol v5. Each has an
end-to-end test (`crates/slipstream-session/src/lib.rs`), but the parts
that touch the operating system can only be checked on real desktops.

Updated 2026-10-18 for 0.17.1 (current protocol 6). The
[development handoff](../DEVELOPMENT-2026-10-18.md) records isolated GUI and
Android emulator evidence. The physical-machine checks below remain open.

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

- [ ] *Tools ▾ → Blank remote screen*: the remote monitors go black with the
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

Network scan and inventory:

- [ ] Scan from a Windows service host, verify names and a single gateway
      node, and try a manually entered routed subnet/VLAN.
- [ ] Click HTTP/HTTPS results: the signed-in user's browser opens the exact
      remote address, without toggling the viewer's control prompt.
- [ ] Check peripheral/printer/scanner models, saved network-drive mappings,
      shared folders and public IP against the remote machine's configuration.
- [ ] Move/maximize the detached scanner beyond a small session window; check
      its native corners and contrasting controls on Windows.

Files and service-update recovery:

- [ ] Drop files/folders from Linux onto Windows Explorer and onto the desktop;
      verify the resolved destination, nested/empty folders and exact contents,
      including when the Windows service hosts the session.
- [ ] In both file panes, check thumbnails/hover previews and Name/Modified
      sorting; selected Send/Receive operations must retain the intended file.
- [ ] Use detached Files and transfer receipts with the main window minimized.
      Closing a tool must leave active transfers running; cancellation and
      errors must stay visible, and Complete must follow the saved-file receipt.
- [ ] Update the Windows service during a link. Confirm visible recovery and
      reconnection in the same tab, released input and restored tools. Interrupted
      transfers should fail visibly and port forwards should close.
- [ ] Confirm incoming service chat reaches the signed-in user's LCARS window
      and replies return, including after a host restart.

## Pointers

- `crates/slipstream-platform/src/privacy.rs`, `generic.rs` (`BlockLocal`)
- `crates/slipstream-session/src/tunnel.rs`, `files.rs` (`StreamOpen::Tunnel`,
  `ClipboardFiles`)
- `crates/slipstream-session/src/viewer.rs` (`paste_files`,
  `fetch_clipboard_files`)
- `apps/slipstream/src/gui/mod.rs` (`raw_input_hook`: Ctrl+V)
