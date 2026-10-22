---
title: "Windows: verify service control of UAC, elevated apps and the login screen"
labels: [platform:windows, testing]
---

## Background

The app runs as the logged-in user. Windows therefore blocks it from:

- seeing or controlling UAC prompts (secure desktop),
- sending input to elevated (administrator) windows (UIPI),
- reaching the computer before anyone has logged in, or after logout.

These matter for real IT support and for unattended servers.

## Implemented approach

- A small Windows service running as `LocalSystem` supervises a host helper.
  The helper registers the device and accepts remote sessions.
- The supervisor keeps that helper in the active Windows console
  session (`WTSGetActiveConsoleSessionId`, `CreateProcessAsUser` with a
  SYSTEM token) on the current input desktop. The helper follows desktop
  switches (`OpenInputDesktop` / `SetThreadDesktop`) between Default,
  Winlogon and the UAC secure desktop.
- Capture on the secure desktop: DXGI duplication has to be re-created when
  the desktop switches. Fall back to GDI if needed.
- Install and uninstall from the app (*Config → Incoming access → Windows service*, which
  needs elevation) and from the command line (`slipstream service
  install`).
- Self-update replaces the program; the supervisor restarts on the changed
  executable and restores the helper. Recovery needs the real-Windows checks below.

## Status

Implemented (`apps/slipstream/src/service.rs`,
`crates/slipstream-platform/src/windesk.rs`): the supervisor service, the
host in the console session as SYSTEM, desktop following, SendSAS for
Ctrl+Alt+Del, a locked-down machine profile, install / update / remove from
Config or the CLI, automatic updates (the service restarts itself when its
program changes). Install, start, a session and uninstall work under Wine;
session 0, the secure desktop and SendSAS can only be checked on real
Windows.

Since 0.6.0 (see ARCHITECTURE.md, "The Windows service"): one host per
computer, under the app's ID. The app keeps the service running without an
administrator prompt. The service log and its ID are readable from a
public folder, failures show Windows' own explanation, and the profile's
permissions repair themselves. Capture uses DXGI duplication on the normal
desktop and GDI on the login screen and UAC prompts.

On real Windows Server 2016 (a VMware VM), as of 0.6.0–0.7.0:

- Install, reinstall over an older service, and Start all work.
  Reinstalling failed with "Access is denied" on the identity until build
  36's permission fix.
- The service hosts under the app's ID, and DXGI capture runs there.
- Once, after *Update*, the service was left stopped. The cause is still
  unknown: the app restarts it now, and the public log should show why.

## Still to check on real Windows

- [ ] Install from Config on Windows 11 (Server 2016: done).
- [ ] *Update* leaves the service running (it was found stopped once on
      Server 2016).
- [ ] Approve a UAC prompt remotely; type into an elevated window.
- [ ] Ctrl+Alt+Del from *Keys*; log in at the login screen; lock and unlock.
- [ ] Log off and back on, switch users: the host follows the session.
- [ ] An update arrives: the service restarts on the new program.
- [ ] Remove from Config: the app hosts again.
- [ ] The current one-time code and unattended password both authenticate
      the service's ID; New code updates only the code and preserves a live link.
- [ ] On different Windows hardware, an imaged profile with an established
      binding gets a new ID and retains station/unattended settings. Ordinary
      service Update/reinstall preserves the existing machine identity.

## Done when

From another computer you can approve a UAC prompt, type into an elevated
window, and log in at the Windows login screen of an unattended machine.

## Pointers

- `crates/slipstream-platform/src/generic.rs` (capture and input on
  Windows)
- `crates/slipstream-session/src/lib.rs` (`Client`, `accept_task`)
- `crates/slipstream-session/src/update.rs`
