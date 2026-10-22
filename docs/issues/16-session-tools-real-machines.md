---
title: Verify session windows and support tools on real machines
labels: [testing]
---

## Background

Current release: **0.24.0, protocol 6**. Local encrypted-session tests and
isolated native Linux/Wine checks cover the implemented tools and windows.
The [latest handoff](../DEVELOPMENT-2026-10-21.md) records that evidence and
links earlier releases. The OS/device checks below remain open; run the
current published build on both peers. Windows service elevation/identity
checks are in issue 04, and phone hosting is in issue 09.

## Checklist

Session windows and current UI:

- [ ] On Windows, move tabs between root/floating groups, reorder and tear out
      without reconnecting; close one member while another remains usable.
      Try real mixed-DPI monitors and + in each group, including password prompts.
- [ ] Verify Tabs / Separate windows persists for new links; reconnect returns
      to the same workstation window with the selected screen/quality.
- [ ] New Link finds private/shared devices, empty groups and recent links;
      saved passwords and duplicate numeric IDs use the correct station.
- [ ] Compact settings, group chooser and sign-in controls remain inside their
      panels; long workstation/station names leave actions accessible.
- [ ] Windows Files/receipts/scanner close without black remnants or unwanted
      native captions; late video/tool repaints do not create placeholder windows.

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

Support reports, host status and network assistance:

- [ ] Collect/review/copy/export a report; inventory, scan results and completed
      diagnostics match the selected workstation. An optional screenshot appears
      only when selected, in its own report folder with the report text.
- [ ] Closing collection or losing the link cancels pending requests. Reopening
      starts fresh work; an already-started local export finishes in its folder.
- [ ] Show/clear host status on full and compact Windows hosts. Dismiss hides
      the message while retaining the link; Stop ends only its session without
      automatic recovery. Controls remain visible above the compact footer.
- [ ] Connect through a discovered TCP service with Port forwarding enabled;
      closing the scanner retains the tunnel, and ending the link closes it.
- [ ] With explicit host Wake-on-LAN permission, send a wake packet through the
      selected LAN adapter and confirm the intended sleeping device wakes.
- [ ] Gateway/DNS/station diagnostics report bounded results on real networks,
      including failed lookups and an explicitly entered comparison DNS server.
- [ ] Disabled host permissions refuse the corresponding tools and preserve
      the active session.

## Pointers

- `crates/slipstream-platform/src/privacy.rs`, `generic.rs` (`BlockLocal`)
- `crates/slipstream-session/src/tunnel.rs`, `files.rs` (`StreamOpen::Tunnel`,
  `ClipboardFiles`)
- `crates/slipstream-session/src/viewer.rs` (`paste_files`,
  `fetch_clipboard_files`)
- `apps/slipstream/src/gui/mod.rs` (`raw_input_hook`: Ctrl+V)
- `apps/slipstream/src/gui/session_tabs.rs`, `remote.rs` (window ownership)
- `apps/slipstream/src/gui/support_report.rs`, `support_status.rs`
- `crates/slipstream-session/src/support.rs`, `network.rs`
