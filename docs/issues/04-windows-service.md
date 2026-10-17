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

## Done when

From another computer you can approve a UAC prompt, type into an elevated
window, and log in at the Windows login screen of an unattended machine.

## Pointers

- `crates/slipstream-platform/src/generic.rs` (capture and input on
  Windows)
- `crates/slipstream-session/src/lib.rs` (`Client`, `accept_task`)
- `crates/slipstream-session/src/update.rs`
