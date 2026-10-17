//! Linux desktop integration: Wayland compositors show an app's icon (and
//! list it in menus) only via an installed `.desktop` file whose name matches
//! the window's app ID. A downloaded binary has none, so we install one for
//! the current user — like AppImages do — pointing at wherever this copy
//! lives. Nothing is written outside ~/.local/share.

use std::{fs, path::Path};

use anyhow::{Context, Result};

pub const APP_ID: &str = "slipstream-remote";

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if fs::read(path).is_ok_and(|cur| cur == bytes) {
        return Ok(false);
    }
    fs::create_dir_all(path.parent().context("no parent")?)?;
    fs::write(path, bytes)?;
    Ok(true)
}

pub fn integrate() -> Result<()> {
    // A system-wide install (package, install script) already provides it.
    if Path::new(&format!("/usr/share/applications/{APP_ID}.desktop")).exists() {
        return Ok(());
    }
    // Side profiles (testing, portable use) leave the user's launcher alone.
    if std::env::var_os("SLIPSTREAM_HOME").is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let data = directories::BaseDirs::new().context("no home directory")?.data_dir().to_path_buf();
    let mut changed = false;
    for size in [64u32, 128, 256] {
        let icon = super::theme::icon(size);
        let mut png = Vec::new();
        let mut enc = png::Encoder::new(&mut png, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&icon.rgba)?;
        let path = data.join(format!("icons/hicolor/{size}x{size}/apps/{APP_ID}.png"));
        changed |= write_if_changed(&path, &png)?;
    }
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Slipstream Remote\nGenericName=Remote Desktop\n\
         Comment=Fast, free, open-source remote desktop\nExec=\"{}\" %U\nIcon={APP_ID}\nTerminal=false\n\
         Categories=Network;RemoteAccess;\nKeywords=remote;desktop;support;teamviewer;\nStartupWMClass={APP_ID}\n\
         MimeType=x-scheme-handler/slipstream;\n",
        exe.display()
    );
    changed |= write_if_changed(&data.join(format!("applications/{APP_ID}.desktop")), entry.as_bytes())?;
    if changed {
        tracing::info!("installed desktop entry and icon for {}", exe.display());
        // Nudge menu caches; harmless if these tools are missing.
        let _ = std::process::Command::new("update-desktop-database").arg(data.join("applications")).status();
        // Open slipstream:// links (the station page's Connect button) with us.
        let _ = std::process::Command::new("xdg-mime")
            .args(["default", &format!("{APP_ID}.desktop"), "x-scheme-handler/slipstream"])
            .status();
        let _ = std::process::Command::new("gtk-update-icon-cache").args(["-q", "-t"]).arg(data.join("icons/hicolor")).status();
    }
    Ok(())
}
