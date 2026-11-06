---
title: Verify remote management (restarts, Wake-on-LAN, management controllers) on real hardware
labels: [feature, testing]
---

## Status

Implemented (unreleased): **Manage** on saved devices, **Edit → Remote
management**, restarts in session **Tools**. End-to-end tests cover Redfish
power through a gateway link against a local test controller, the console port
forward, Control permission for restarts, and station storage. Real hardware
hasn't been tried.

## Checklist

- [ ] Supermicro BMC (Redfish, X10 or newer): power state, Power on from off,
      Shut down, Restart, Power cycle, Force power off; the HTML5 console
      through **Open management console** shows POST and the BIOS setup.
- [ ] Dell iDRAC and HPE iLO, if available: console page and Redfish power.
- [ ] Intel vPro (AMT) workstation: web page on 16992/16993 through the
      gateway.
- [ ] Wake-on-LAN through a gateway on the same subnet wakes a powered-off PC.
- [ ] Windows (app and service): **Restart** comes back and the session tab
      reconnects. **Restart in Safe Mode with Networking** (service) starts in
      Safe Mode, hosts there, and the following restart is normal. **Restart
      into BIOS setup** enters UEFI setup (seen through a controller).
- [ ] Linux (`systemctl reboot`, `--firmware-setup`) from the desktop app.
- [ ] The gateway's banner and permissions: Port forwarding, Wake-on-LAN,
      Control, Network scan.

## Pointers

- `crates/slipstream-session/src/management.rs`, `power.rs`
- `crates/slipstream-core/src/account.rs` (`Management`, `Controller`)
- `apps/slipstream/src/gui/manage.rs`, `saved.rs` (editor section)
- `server/station/lib/slipstream.php` (`slip_clean_management`)
