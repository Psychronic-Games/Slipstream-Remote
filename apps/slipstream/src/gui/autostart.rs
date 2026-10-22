//! "Start with the system" for unattended access.

use anyhow::Result;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use anyhow::Context;

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn exe() -> Result<String> {
    Ok(std::env::current_exe().context("locating executable")?.display().to_string())
}

#[cfg(target_os = "linux")]
fn entry_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("autostart").join("slipstream-remote.desktop"))
}

#[cfg(target_os = "linux")]
pub fn is_enabled() -> bool {
    entry_path().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|entry| entry_enabled(&entry))
}

#[cfg(target_os = "linux")]
fn entry_enabled(entry: &str) -> bool {
    let mut desktop = false;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') {
            desktop = line == "[Desktop Entry]";
        } else if desktop && let Some((key, value)) = line.split_once('=') {
            match (key.trim(), value.trim()) {
                ("Hidden", "true") | ("X-GNOME-Autostart-enabled" | "X-KDE-autostart-enabled", "false") => return false,
                _ => {}
            }
        }
    }
    entry.lines().any(|line| line.trim() == "[Desktop Entry]")
}

/// A portable executable can move after autostart was enabled. Follow the
/// normally launched copy without enabling missing or desktop-disabled entries.
#[cfg(target_os = "linux")]
pub fn refresh_enabled() -> Result<()> {
    if std::env::var_os("SLIPSTREAM_HOME").is_some() { return Ok(()); }
    let path = entry_path().context("no config directory")?;
    refresh_entry(&path, &std::env::current_exe()?)
}

#[cfg(target_os = "linux")]
fn refresh_entry(path: &std::path::Path, executable: &std::path::Path) -> Result<()> {
    let entry = match std::fs::read_to_string(path) {
        Ok(entry) => entry,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !entry_enabled(&entry) { return Ok(()); }
    let command = format!("Exec={} --minimized", super::desktop::exec_argument(executable)?);
    let mut desktop = false;
    let mut updated = String::new();
    for line in entry.lines() {
        if line.trim().starts_with('[') { desktop = line.trim() == "[Desktop Entry]"; }
        if desktop && line.split_once('=').is_some_and(|(key, _)| key.trim() == "Exec") {
            updated.push_str(&command);
        } else {
            updated.push_str(line);
        }
        updated.push('\n');
    }
    if updated != entry {
        std::fs::write(path, updated)?;
        tracing::info!("refreshed autostart executable to {}", executable.display());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn set_enabled(on: bool) -> Result<()> {
    let path = entry_path().context("no config directory")?;
    if on {
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        std::fs::write(
            &path,
            format!(
                "[Desktop Entry]\nType=Application\nName=Slipstream Remote\nExec={} --minimized\nIcon=slipstream-remote\nTerminal=false\nX-GNOME-Autostart-enabled=true\nNoDisplay=false\n",
                super::desktop::exec_argument(&std::env::current_exe()?)?
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn moved_executable_refreshes_enabled_entry_without_changing_desktop_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slipstream.desktop");
        let old = "[Desktop Entry]\nType=Application\nExec=\"/removed/download\" --minimized\nOnlyShowIn=KDE;\nX-KDE-autostart-after=panel\n[Desktop Action Other]\nExec=keep-this\n";
        std::fs::write(&path, old).unwrap();
        refresh_entry(&path, std::path::Path::new("/home/test/Slipstream Remote/app")).unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert_eq!(updated, old.replace("\"/removed/download\"", "\"/home/test/Slipstream Remote/app\""));
        refresh_entry(&path, std::path::Path::new("/home/test/Slipstream Remote/app")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), updated);
    }

    #[test]
    fn refresh_does_not_enable_missing_or_desktop_disabled_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slipstream.desktop");
        let executable = std::path::Path::new("/new/app");
        refresh_entry(&path, executable).unwrap();
        assert!(!path.exists());
        for disabled in ["Hidden=true", "X-GNOME-Autostart-enabled=false", "X-KDE-autostart-enabled=false"] {
            let original = format!("[Desktop Entry]\nType=Application\nExec=/old/app\n{disabled}\n");
            std::fs::write(&path, &original).unwrap();
            assert!(!entry_enabled(&original));
            refresh_entry(&path, executable).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }
}
