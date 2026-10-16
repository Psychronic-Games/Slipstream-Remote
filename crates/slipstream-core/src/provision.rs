//! Station provisioning: an app downloaded from a Slipstream station's
//! website carries that station's address and access key, so it works out
//! of the box with no typing.
//!
//! The station appends a small trailer to the executable it serves:
//!
//! ```text
//! [executable][JSON config][u32 LE length of JSON][b"SLIPCFG1"]
//! ```
//!
//! Executables ignore trailing bytes, so the app still runs. Alternatively
//! a `station.json` file next to the executable (or in a macOS bundle's
//! `Contents/Resources`) works the same way.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::config::Settings;

pub const TRAILER_MAGIC: &[u8; 8] = b"SLIPCFG1";
const MAX_CONFIG: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationConfig {
    /// Station address, e.g. `relay.example.com`.
    pub server: String,
    /// Access key for a private station.
    #[serde(default)]
    pub key: Option<String>,
    /// Organisation name for display.
    #[serde(default)]
    pub org: Option<String>,
}

/// Reads a config trailer from the end of `path`, if present.
pub fn read_trailer(path: &Path) -> Option<StationConfig> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len < 12 {
        return None;
    }
    let mut tail = [0u8; 12];
    f.seek(SeekFrom::End(-12)).ok()?;
    f.read_exact(&mut tail).ok()?;
    if &tail[4..] != TRAILER_MAGIC {
        return None;
    }
    let n = u32::from_le_bytes(tail[..4].try_into().ok()?);
    if n == 0 || n > MAX_CONFIG || n as u64 + 12 > len {
        return None;
    }
    let mut json = vec![0u8; n as usize];
    f.seek(SeekFrom::End(-12 - n as i64)).ok()?;
    f.read_exact(&mut json).ok()?;
    serde_json::from_slice(&json).ok()
}

/// Builds the trailer bytes for `cfg` (used by tests and tools; the PHP
/// station produces the same layout).
pub fn trailer(cfg: &StationConfig) -> Vec<u8> {
    let json = serde_json::to_vec(cfg).expect("serializable");
    let mut out = json.clone();
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(TRAILER_MAGIC);
    out
}

fn sidecar_paths(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else { return Vec::new() };
    vec![dir.join("station.json"), dir.join("../Resources/station.json")]
}

/// Finds the station config this copy of the app was distributed with.
pub fn embedded() -> Option<StationConfig> {
    let exe = std::env::current_exe().ok()?;
    if let Some(cfg) = read_trailer(&exe) {
        return Some(cfg);
    }
    sidecar_paths(&exe)
        .into_iter()
        .find_map(|p| std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()))
}

/// Applies a station config to settings. A station config wins when no
/// server is set, when the settings came from a station before (so a newer
/// download can update the key), or when it's the same server. A server the
/// user typed in by hand is left alone. Returns true if anything changed.
pub fn apply(settings: &mut Settings, cfg: &StationConfig) -> bool {
    let s = &mut settings.server;
    let same_server = s.url.as_deref().map(str::trim) == Some(cfg.server.trim());
    if s.url.is_some() && !s.provisioned && !same_server {
        return false;
    }
    let before = (s.url.clone(), s.key.clone(), s.org_name.clone(), s.provisioned);
    s.url = Some(cfg.server.clone());
    s.key = cfg.key.clone();
    if cfg.org.is_some() {
        s.org_name = cfg.org.clone();
    }
    s.provisioned = true;
    before != (s.url.clone(), s.key.clone(), s.org_name.clone(), s.provisioned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_roundtrip_and_apply() {
        let cfg = StationConfig { server: "relay.example.com".into(), key: Some("k123".into()), org: Some("Acme IT".into()) };
        let path = std::env::temp_dir().join(format!("slipstream-trailer-{}", std::process::id()));
        let mut bytes = b"\x7fELF fake executable body".to_vec();
        bytes.extend(trailer(&cfg));
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(read_trailer(&path), Some(cfg.clone()));
        std::fs::write(&path, b"no trailer here at all").unwrap();
        assert_eq!(read_trailer(&path), None);
        std::fs::remove_file(&path).ok();

        let mut s = Settings::default();
        assert!(apply(&mut s, &cfg));
        assert_eq!(s.server.key.as_deref(), Some("k123"));
        assert!(!apply(&mut s, &cfg), "idempotent");
        // A hand-entered server is not overridden by a different station.
        let mut manual = Settings::default();
        manual.server.url = Some("other.example.org".into());
        assert!(!apply(&mut manual, &cfg));
        assert_eq!(manual.server.url.as_deref(), Some("other.example.org"));
    }
}
