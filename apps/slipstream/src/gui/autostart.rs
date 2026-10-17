//! "Start with the system" for unattended access.

use anyhow::{Context, Result};

fn exe() -> Result<String> {
    Ok(std::env::current_exe().context("locating executable")?.display().to_string())
}

#[cfg(target_os = "linux")]
fn entry_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("autostart").join("slipstream-remote.desktop"))
}

#[cfg(target_os = "linux")]
pub fn is_enabled() -> bool {
    entry_path().is_some_and(|p| p.exists())
}

#[cfg(target_os = "linux")]
pub fn set_enabled(on: bool) -> Result<()> {
    let path = entry_path().context("no config directory")?;
    if on {
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        std::fs::write(
            &path,
            format!(
                "[Desktop Entry]\nType=Application\nName=Slipstream Remote\nExec=\"{}\" --minimized\nIcon=slipstream\nX-GNOME-Autostart-enabled=true\nNoDisplay=false\n",
                exe()?
            ),
        )?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(target_os = "windows")]
pub fn is_enabled() -> bool {
    super::scheme::reg(&["query", RUN_KEY, "/v", "SlipstreamRemote"]).is_ok_and(|o| o.status.success())
}

#[cfg(target_os = "windows")]
pub fn set_enabled(on: bool) -> Result<()> {
    let status = if on {
        let command = format!("\"{}\" --minimized", exe()?);
        super::scheme::reg(&["add", RUN_KEY, "/v", "SlipstreamRemote", "/t", "REG_SZ", "/d", &command, "/f"])?.status
    } else {
        super::scheme::reg(&["delete", RUN_KEY, "/v", "SlipstreamRemote", "/f"])?.status
    };
    anyhow::ensure!(status.success() || !on, "registry update failed");
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().join("Library/LaunchAgents/dev.slipstream.remote.plist"))
}

#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    plist_path().is_some_and(|p| p.exists())
}

#[cfg(target_os = "macos")]
pub fn set_enabled(on: bool) -> Result<()> {
    let path = plist_path().context("no home directory")?;
    if on {
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        std::fs::write(
            &path,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>dev.slipstream.remote</string>
  <key>ProgramArguments</key><array><string>{}</string><string>--minimized</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
                exe()?
            ),
        )?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn set_enabled(_on: bool) -> Result<()> {
    anyhow::bail!("not supported on this platform")
}
