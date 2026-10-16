//! Self-update from the station the app came from.
//!
//! A station publishes `clients/manifest.json` (version, build number and
//! SHA-256 of each platform's app). The app asks its station for the build
//! that fits it, downloads it — preconfigured for the station, like the
//! download page does — verifies it against the manifest, swaps it in and
//! restarts. Settings, identity and passwords live in the settings folder,
//! so they survive updates untouched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// What the station offers for this platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub build: u64,
    #[serde(default)]
    pub commit: String,
    /// Size of the app itself (without the station's config trailer).
    pub size: u64,
    /// SHA-256 (hex) of the app itself.
    pub sha256: String,
    /// `os` value for the station's download page.
    pub os: String,
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let mut it = v.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

impl UpdateInfo {
    /// Is this newer than the running app?
    pub fn is_newer(&self) -> bool {
        let mine = (parse_version(slipstream_core::APP_VERSION), slipstream_core::BUILD_NUMBER);
        (parse_version(&self.version), self.build) > mine
    }

    pub fn label(&self) -> String {
        format!("{} (build {})", self.version, self.build)
    }
}

/// The running executable, if it can be replaced in place.
fn exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate the app")?;
    let dir = exe.parent().context("app has no folder")?;
    let probe = dir.join(".slipstream-write-test");
    if std::fs::write(&probe, b"").is_err() {
        bail!("this copy of Slipstream is installed in a protected folder ({}) — download the new version from your station instead", dir.display());
    }
    let _ = std::fs::remove_file(probe);
    Ok(exe)
}

/// Length of the app without a station config trailer (see `provision`).
fn base_len(bytes: &[u8]) -> usize {
    let n = bytes.len();
    if n >= 12 && &bytes[n - 8..] == slipstream_core::provision::TRAILER_MAGIC {
        let json = u32::from_le_bytes(bytes[n - 12..n - 8].try_into().expect("4 bytes")) as usize;
        if json + 12 <= n {
            return n - 12 - json;
        }
    }
    n
}

/// Downloads `info` from the station and verifies it. Returns the staged
/// file, ready for [`install`].
pub async fn download(
    station: &reqwest::Url,
    key: Option<&str>,
    info: &UpdateInfo,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let exe = exe_path()?;
    let staged = exe.with_file_name(format!(
        "{}.update",
        exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "slipstream".into())
    ));
    let url = station.join(&format!("download.php?os={}", info.os))?;
    let client = slipstream_core::directory::http_client()?;
    let mut req = client.get(url);
    if let Some(k) = key {
        req = req.header("X-Slipstream-Key", k);
    }
    let mut resp = req.send().await.context("downloading the update")?;
    if !resp.status().is_success() {
        bail!("the station refused the download (HTTP {})", resp.status());
    }
    let mut file = tokio::fs::File::create(&staged).await.context("writing the update")?;
    let mut done = 0u64;
    let total = resp.content_length().unwrap_or(info.size);
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().await?;
    drop(file);

    let bytes = tokio::fs::read(&staged).await?;
    let base = &bytes[..base_len(&bytes)];
    let digest = Sha256::digest(base);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if base.len() as u64 != info.size || !hex.eq_ignore_ascii_case(&info.sha256) {
        let _ = tokio::fs::remove_file(&staged).await;
        bail!("the downloaded update is damaged (checksum mismatch) — try again");
    }
    Ok(staged)
}

/// Swaps the staged update in for the running app. Returns the app path to
/// relaunch. (Windows can't overwrite a running .exe but can rename it, so
/// the old one is moved aside and deleted on the next start.)
pub fn install(staged: &Path) -> Result<PathBuf> {
    let exe = exe_path()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(staged, &exe).context("replacing the app")?;
    }
    #[cfg(windows)]
    {
        let old = exe.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).context("moving the old app aside")?;
        if let Err(e) = std::fs::rename(staged, &exe) {
            let _ = std::fs::rename(&old, &exe); // roll back
            return Err(e).context("installing the update");
        }
    }
    Ok(exe)
}

/// Removes leftovers of a previous update (call at startup).
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
        if let Some(name) = exe.file_name() {
            let _ = std::fs::remove_file(exe.with_file_name(format!("{}.update", name.to_string_lossy())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        let mk = |v: &str, b: u64| UpdateInfo { version: v.into(), build: b, commit: String::new(), size: 0, sha256: String::new(), os: String::new() };
        assert!(mk("99.0.0", 0).is_newer());
        assert!(!mk("0.0.1", 0).is_newer());
        assert!(mk(slipstream_core::APP_VERSION, slipstream_core::BUILD_NUMBER + 1).is_newer());
        assert!(!mk(slipstream_core::APP_VERSION, slipstream_core::BUILD_NUMBER).is_newer());
    }

    #[test]
    fn trailer_is_excluded_from_checksum() {
        let mut bytes = b"APP BYTES".to_vec();
        let cfg = slipstream_core::provision::StationConfig { server: "s".into(), key: None, org: None };
        bytes.extend(slipstream_core::provision::trailer(&cfg));
        assert_eq!(base_len(&bytes), 9);
        assert_eq!(base_len(b"no trailer"), 10);
    }
}
