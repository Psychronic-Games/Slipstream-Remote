//! Linux desktop integration: Wayland compositors show an app's icon (and
//! list it in menus) only via an installed `.desktop` file whose name matches
//! the window's app ID. A downloaded binary has none, so we install one for
//! the current user — like AppImages do — pointing at wherever this copy
//! lives. Nothing is written outside ~/.local/share.

use std::{fs, path::Path};

use anyhow::{Context, Result};

pub const APP_ID: &str = "slipstream-remote";

// File managers identify an ELF file through MIME magic, not its window icon.
// A real ELF note keeps this stable marker near the start across builds, names,
// station provisioning trailers and self-updates.
#[repr(C, align(4))]
struct ExecutableNote {
    name_len: u32,
    description_len: u32,
    kind: u32,
    name: [u8; 12],
    description: [u8; 32],
}

#[used]
#[unsafe(link_section = ".note.slipstream")]
static EXECUTABLE_NOTE: ExecutableNote = ExecutableNote {
    name_len: 11, description_len: 32, kind: 0x534c4950,
    name: *b"SLIPSTREAM\0\0",
    description: *b"slipstream-remote-executable-v1\0",
};

const FILE_MIME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-slipstream-remote">
    <comment>Slipstream Remote application</comment>
    <sub-class-of type="application/x-executable"/>
    <icon name="application-x-slipstream-remote"/>
    <magic priority="80">
      <match type="string" value="\x7fELF" offset="0">
        <match type="string" value="slipstream-remote-executable-v1" offset="0:65536"/>
      </match>
    </magic>
  </mime-type>
</mime-info>
"#;

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if fs::read(path).is_ok_and(|cur| cur == bytes) {
        return Ok(false);
    }
    fs::create_dir_all(path.parent().context("no parent")?)?;
    fs::write(path, bytes)?;
    Ok(true)
}

/// Exec is parsed as a desktop string and then as a quoted command argument.
pub(super) fn exec_argument(path: &Path) -> Result<String> {
    let path = path.to_str().context("executable path is not UTF-8")?;
    anyhow::ensure!(!path.chars().any(|c| c.is_control() || c == '='), "executable path cannot be represented in a desktop entry");
    let mut quoted = String::from("\"");
    for ch in path.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => { quoted.push_str("\\\\"); quoted.push(ch); }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

pub fn integrate() -> Result<()> {
    std::hint::black_box(&EXECUTABLE_NOTE);
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
        changed |= write_if_changed(&data.join(format!("icons/hicolor/{size}x{size}/mimetypes/application-x-slipstream-remote.png")), &png)?;
    }
    let mime_changed = write_if_changed(&data.join("mime/packages/slipstream-remote.xml"), FILE_MIME.as_bytes())?;
    if mime_changed || !data.join("mime/mime.cache").exists() {
        let result = std::process::Command::new("update-mime-database").arg(data.join("mime")).status();
        if !result.is_ok_and(|s| s.success()) { tracing::warn!("could not refresh the downloaded application's file icon"); }
    }
    changed |= mime_changed;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Slipstream Remote\nGenericName=Remote Desktop\n\
         Comment=Fast, free, open-source remote desktop\nExec={} %U\nIcon={APP_ID}\nTerminal=false\n\
         Categories=Network;RemoteAccess;\nKeywords=remote;desktop;support;teamviewer;\nStartupWMClass={APP_ID}\n\
         MimeType=x-scheme-handler/slipstream;\n",
        exec_argument(&exe)?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn executable_paths_escape_both_desktop_and_command_syntax() {
        assert_eq!(exec_argument(Path::new("/home/test/Slipstream Remote (2)")).unwrap(), "\"/home/test/Slipstream Remote (2)\"");
        assert_eq!(exec_argument(Path::new(r#"/tmp/a\b"$`%"#)).unwrap(), r#""/tmp/a\\\\b\\"\\$\\`%%""#);
        assert!(exec_argument(Path::new("/tmp/app\nHidden=true")).is_err());
    }

    #[test]
    fn executable_marker_survives_linking_in_the_mime_probe_region() {
        let mut prefix = Vec::new();
        fs::File::open(std::env::current_exe().unwrap()).unwrap().take(65536).read_to_end(&mut prefix).unwrap();
        let marker = b"slipstream-remote-executable-v1";
        assert!(prefix.windows(marker.len()).any(|bytes| bytes == marker), "ELF branding note missing from the first 64 KiB");
    }
}
