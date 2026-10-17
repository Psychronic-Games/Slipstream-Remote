//! Windows: registers the `slipstream://` URL scheme for this user, so the
//! station page's "Connect this app" button opens Slipstream. (Linux does
//! the same through the desktop entry, see `desktop.rs`.)

use std::{os::windows::process::CommandExt, process::Command};

use anyhow::{Result, ensure};

const KEY: &str = r"HKCU\Software\Classes\slipstream";
/// Run `reg` without flashing a console window.
const NO_WINDOW: u32 = 0x0800_0000;

pub(super) fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("reg").args(args).creation_flags(NO_WINDOW).output()
}

pub fn register() -> Result<()> {
    // Side profiles (testing, portable use) leave the user's handler alone.
    if std::env::var_os("SLIPSTREAM_HOME").is_some() {
        return Ok(());
    }
    let command = format!("\"{}\" \"%1\"", std::env::current_exe()?.display());
    let command_key = format!(r"{KEY}\shell\open\command");
    if reg(&["query", &command_key, "/ve"]).is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&command)) {
        return Ok(());
    }
    for args in [
        vec!["add", KEY, "/ve", "/d", "URL:Slipstream Remote", "/f"],
        vec!["add", KEY, "/v", "URL Protocol", "/d", "", "/f"],
        vec!["add", &command_key, "/ve", "/d", &command, "/f"],
    ] {
        ensure!(reg(&args)?.status.success(), "registry update failed");
    }
    tracing::info!("registered the slipstream:// link handler");
    Ok(())
}
