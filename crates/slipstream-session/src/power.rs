//! Restarting the host's computer for a technician: normally, into Safe
//! Mode with Networking (Windows, under the service), or into the firmware
//! (BIOS) setup. The request is answered first; the restart follows a few
//! seconds later so the answer reaches the viewer.

use anyhow::{Result, bail};
use slipstream_core::proto::PowerAction;

/// Seconds between answering and the system going down.
#[cfg_attr(not(any(windows, target_os = "linux", target_os = "macos")), allow(dead_code))]
const DELAY: u32 = 5;

/// Checks the action can be done here and starts it.
pub(crate) fn restart(action: PowerAction, service: bool) -> Result<()> {
    #[cfg(windows)]
    return windows::restart(action, service);
    #[cfg(target_os = "linux")]
    {
        let _ = service;
        return linux::restart(action);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = service;
        if action != PowerAction::Restart {
            bail!("{} isn't available on macOS", action.label());
        }
        return run_later("/usr/bin/osascript", &["-e", "tell application \"System Events\" to restart"]);
    }
    #[allow(unreachable_code)]
    {
        let _ = (action, service);
        bail!("Restarting isn't available on this system")
    }
}

/// Runs a command after the delay, detached from this request.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_later(program: &str, args: &[&str]) -> Result<()> {
    let (program, args): (String, Vec<String>) = (program.into(), args.iter().map(|a| a.to_string()).collect());
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(DELAY.into()));
        match std::process::Command::new(&program).args(&args).status() {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::warn!(%program, ?status, "restart command failed"),
            Err(e) => tracing::warn!(%program, "restart command failed: {e}"),
        }
    });
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub(super) fn restart(action: PowerAction) -> Result<()> {
        match action {
            PowerAction::Restart => run_later("systemctl", &["reboot"]),
            PowerAction::RestartToFirmware => {
                // Only UEFI systems whose firmware supports it.
                if !std::path::Path::new("/sys/firmware/efi").exists() {
                    bail!("This computer doesn't start through UEFI, so it can't restart into its setup");
                }
                run_later("systemctl", &["reboot", "--firmware-setup"])
            }
            PowerAction::RestartSafeMode => bail!("Safe Mode is a Windows feature"),
        }
    }
}

#[cfg(windows)]
pub mod windows {
    use super::*;
    use std::os::windows::process::CommandExt;

    const NO_WINDOW: u32 = 0x0800_0000;
    /// The service's name (apps/slipstream/src/service.rs NAME).
    const SERVICE: &str = "SlipstreamRemote";

    fn run(program: &str, args: &[&str]) -> Result<()> {
        let status = std::process::Command::new(program).args(args).creation_flags(NO_WINDOW).status()?;
        if !status.success() {
            bail!("{program} failed ({status})");
        }
        Ok(())
    }

    fn system32(program: &str) -> String {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{windir}\System32\{program}")
    }

    pub(super) fn restart(action: PowerAction, service: bool) -> Result<()> {
        let delay = DELAY.to_string();
        let comment = "Slipstream Remote: restart requested by a technician";
        match action {
            PowerAction::Restart => run(&system32("shutdown.exe"), &["/r", "/f", "/t", &delay, "/c", comment]),
            PowerAction::RestartToFirmware => {
                run(&system32("shutdown.exe"), &["/r", "/fw", "/f", "/t", &delay, "/c", comment]).map_err(|e| {
                    anyhow::anyhow!("This computer can't restart into its firmware setup (it needs UEFI and administrator rights): {e:#}")
                })
            }
            PowerAction::RestartSafeMode => {
                if !service {
                    bail!("Restarting in Safe Mode needs the Slipstream service on that computer (Config → Windows service)");
                }
                // Start the service in Safe Mode with Networking too, then
                // ask for Safe Mode once (cleared again when it starts there).
                run(&system32("reg.exe"), &[
                    "add", &format!(r"HKLM\SYSTEM\CurrentControlSet\Control\SafeBoot\Network\{SERVICE}"),
                    "/ve", "/t", "REG_SZ", "/d", "Service", "/f",
                ])?;
                run(&system32("bcdedit.exe"), &["/set", "{current}", "safeboot", "network"])?;
                if let Err(e) = run(&system32("shutdown.exe"), &["/r", "/f", "/t", &delay, "/c", comment]) {
                    let _ = run(&system32("bcdedit.exe"), &["/deletevalue", "{current}", "safeboot"]);
                    return Err(e);
                }
                Ok(())
            }
        }
    }

    /// Called when the service starts: in Safe Mode, ask for a normal start
    /// next time, so a Safe Mode restart never sticks.
    pub fn leave_safe_mode_next_time() {
        // SAFETY: GetSystemMetrics has no preconditions.
        let safe = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows_sys::Win32::UI::WindowsAndMessaging::SM_CLEANBOOT) } != 0;
        if safe {
            match run(&system32("bcdedit.exe"), &["/deletevalue", "{current}", "safeboot"]) {
                Ok(()) => tracing::info!("started in Safe Mode; the next restart is normal again"),
                Err(e) => tracing::warn!("couldn't clear Safe Mode for the next start: {e:#}"),
            }
        }
    }
}
