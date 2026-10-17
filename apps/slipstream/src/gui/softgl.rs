//! The built-in software renderer, for Windows machines whose graphics can't
//! run the full window: wgpu needs Direct3D 12 with Shader Model 6, Vulkan
//! or OpenGL 3.3, and Windows Server 2016 or a VM without a display driver
//! has none of them. Release builds carry Mesa's CPU OpenGL driver
//! (llvmpipe), gzipped; it's unpacked next to the settings when needed and
//! loaded instead of System32's `opengl32.dll`. The files are checked
//! against the checksums pinned in `packaging/windows/softgl.txt`.

use std::{
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

// `EMBEDDED: Option<&[(&str, &[u8])]>`: (file name, gzipped DLL), written by
// build.rs; `None` in builds made without the Mesa files.
include!(concat!(env!("OUT_DIR"), "/softgl.rs"));

const PINS: &str = include_str!("../../../../packaging/windows/softgl.txt");

fn pinned(name: &str) -> Option<&'static str> {
    PINS.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|w| w.len() == 3 && w[0] == "file" && w[1] == name).map(|w| w[2])
}

fn version() -> &'static str {
    PINS.lines().find_map(|l| l.strip_prefix("version ")).map(str::trim).unwrap_or("unknown")
}

/// Where the DLLs are unpacked: per user, per Mesa version.
pub fn dir() -> PathBuf {
    let base = directories::ProjectDirs::from("dev", "Slipstream", "Slipstream")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    base.join("softgl").join(version())
}

/// Left while the app starts on the software renderer and removed once the
/// window has drawn (see [`started`]). Still there at the next start means
/// the renderer crashed or hung, so that start uses the compact window.
fn marker() -> PathBuf {
    dir().join("starting")
}

/// This build carries the renderer, but it failed to start last time: the
/// compact window offers to try again.
pub fn failed_before() -> bool {
    EMBEDDED.is_some() && marker().exists()
}

/// Forgets a failed start, so the next start tries the renderer again.
pub fn retry() {
    let _ = std::fs::remove_file(marker());
}

/// Unpacks the renderer (once) and arms the start marker. Errors if this
/// build doesn't carry it, or it failed to start last time.
pub fn prepare() -> Result<()> {
    let Some(files) = EMBEDDED else { bail!("this build has no software renderer") };
    if marker().exists() {
        bail!("the software renderer didn't start last time");
    }
    let dir = dir();
    std::fs::create_dir_all(&dir).context("creating the renderer folder")?;
    for (name, gz) in files {
        let path = dir.join(name);
        if path.is_file() {
            continue;
        }
        let mut dll = Vec::new();
        flate2::read::GzDecoder::new(*gz).read_to_end(&mut dll).context("unpacking the renderer")?;
        let hex: String = Sha256::digest(&dll).iter().map(|b| format!("{b:02x}")).collect();
        if Some(hex.as_str()) != pinned(name) {
            bail!("{name} doesn't match its pinned checksum");
        }
        let part = path.with_extension("part");
        std::fs::write(&part, &dll)?;
        std::fs::rename(&part, &path)?;
    }
    std::fs::write(marker(), b"")?;
    PENDING.store(true, Ordering::Relaxed);
    Ok(())
}

/// Starting on the software renderer, first frame not drawn yet.
static PENDING: AtomicBool = AtomicBool::new(false);

/// Called every frame: the first one on the software renderer clears the
/// start marker.
pub fn started() {
    if PENDING.swap(false, Ordering::Relaxed) {
        let _ = std::fs::remove_file(marker());
        tracing::info!("the software renderer is drawing");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pins_parse() {
        assert!(super::pinned("opengl32.dll").is_some_and(|h| h.len() == 64));
        assert!(super::pinned("libgallium_wgl.dll").is_some_and(|h| h.len() == 64));
        assert_ne!(super::version(), "unknown");
    }
}
