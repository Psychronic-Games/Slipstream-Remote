---
title: "Windows: service mode for UAC prompts, elevated apps and the login screen"
labels: [platform:windows, feature]
---

## Background

The app runs as the logged-in user. Windows therefore blocks it from:

- seeing or controlling UAC prompts (secure desktop),
- sending input to elevated (administrator) windows (UIPI),
- reaching the computer before anyone has logged in, or after logout.

These matter for real IT support and for unattended servers.

## Approach

- A small Windows service (running as `LocalSystem`) that keeps the device
  registered with the station and accepts sessions.
- For each session, it launches a helper process into the active console
  session (`WTSGetActiveConsoleSessionId`, `CreateProcessAsUser` with a
  SYSTEM token) on the current input desktop. The helper follows desktop
  switches (`OpenInputDesktop` / `SetThreadDesktop`) between Default,
  Winlogon and the UAC secure desktop.
- Capture on the secure desktop: DXGI duplication has to be re-created when
  the desktop switches. Fall back to GDI if needed.
- Install and uninstall from the app (*Config → Install as service*, which
  needs elevation) and from the command line (`slipstream service
  install`).
- Self-update has to stop the service, swap the binary and start it again.

## Status

Implemented (`apps/slipstream/src/service.rs`,
`crates/slipstream-platform/src/windesk.rs`): the supervisor service, the
host in the console session as SYSTEM, desktop following, SendSAS for
Ctrl+Alt+Del, a locked-down machine profile, install / update / remove from
Config or the CLI, automatic updates (the service restarts itself when its
program changes). Install, start, a session and uninstall work under Wine;
session 0, the secure desktop and SendSAS can only be checked on real
Windows.

## Still to check on real Windows

- [ ] Install from Config on Windows 11 and on Server 2016.
- [ ] Approve a UAC prompt remotely; type into an elevated window.
- [ ] Ctrl+Alt+Del from *Keys*; log in at the login screen; lock and unlock.
- [ ] Log off and back on, switch users: the host follows the session.
- [ ] An update arrives: the service restarts on the new program.
- [ ] Remove from Config: the app hosts again.

## Done when

From another computer you can approve a UAC prompt, type into an elevated
window, and log in at the Windows login screen of an unattended machine.

## Pointers

- `crates/slipstream-platform/src/generic.rs` (capture and input on
  Windows)
- `crates/slipstream-session/src/lib.rs` (`Client`, `accept_task`)
- `crates/slipstream-session/src/update.rs`
