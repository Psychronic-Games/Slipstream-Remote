//! A software OpenGL driver (Mesa's llvmpipe) for Windows machines whose
//! graphics can't run the full window: wgpu needs Direct3D 12 with Shader
//! Model 6 or Vulkan, which Windows Server 2016 and VMs without a display
//! driver don't have. The station serves the DLLs (gzipped); only the exact
//! files pinned in `packaging/windows/softgl.txt` are accepted.

use std::{io::Read, path::PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const PINS: &str = include_str!("../../../packaging/windows/softgl.txt");

fn pin(kind: &str) -> impl Iterator<Item = Vec<&'static str>> + '_ {
    PINS.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).filter(move |w| w.first() == Some(&kind))
}

/// The pinned Mesa version.
pub fn version() -> &'static str {
    pin("version").next().and_then(|w| w.get(1).copied()).unwrap_or("unknown")
}

/// (file name, SHA-256) of each DLL.
pub fn files() -> Vec<(&'static str, &'static str)> {
    pin("file").filter_map(|w| Some((*w.get(1)?, *w.get(2)?))).collect()
}

/// Where the DLLs go: per user, per pinned version.
pub fn dir() -> PathBuf {
    let base = directories::ProjectDirs::from("dev", "Slipstream", "Slipstream")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    base.join("softgl").join(version())
}

/// Downloaded already?
pub fn installed() -> bool {
    let dir = dir();
    files().iter().all(|(name, _)| dir.join(name).is_file())
}

/// Downloads the DLLs from the station and checks them against the pins.
/// `progress` gets (bytes so far, bytes in all, as compressed).
pub async fn install(station: &reqwest::Url, key: Option<&str>, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let client = slipstream_core::directory::http_client()?;
    let dir = dir();
    tokio::fs::create_dir_all(&dir).await.context("creating the driver folder")?;
    // Sizes first, for one progress bar over both files.
    let mut bodies = Vec::new();
    let mut total = 0;
    for (name, sha) in files() {
        let mut req = client.get(station.join(&format!("download.php?extra={name}"))?);
        if let Some(k) = key {
            req = req.header("X-Slipstream-Key", k);
        }
        let resp = req.send().await.context("contacting the station")?;
        match resp.status().as_u16() {
            200 => {}
            404 => bail!("your station doesn't offer the full window yet: it needs updating (build 20 or later)"),
            s => bail!("the station refused the download (HTTP {s})"),
        }
        total += resp.content_length().unwrap_or(0);
        bodies.push((name, sha, resp));
    }
    let mut done = 0;
    for (name, sha, mut resp) in bodies {
        let mut gz = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            gz.extend_from_slice(&chunk);
            done += chunk.len() as u64;
            progress(done, total);
        }
        let dll = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut out).context("the download is damaged")?;
            anyhow::Ok(out)
        })
        .await??;
        let hex: String = Sha256::digest(&dll).iter().map(|b| format!("{b:02x}")).collect();
        if hex != sha {
            bail!("{name} from the station isn't the expected file (checksum mismatch)");
        }
        let path = dir.join(name);
        let part = path.with_extension("part");
        tokio::fs::write(&part, &dll).await?;
        tokio::fs::rename(&part, &path).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pins_parse() {
        let files = super::files();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|(n, h)| n.ends_with(".dll") && h.len() == 64));
        assert_ne!(super::version(), "unknown");
    }
}
