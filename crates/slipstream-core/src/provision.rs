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
//!
//! Signed apps can't be changed per download (it breaks the signature, and
//! a file that differs on every download is "uncommon" to every browser), so
//! stations serve them untouched and hand out **links** instead: a station
//! address plus a setup code that the station exchanges for its config.
//! A link arrives as a `slipstream://link?s=…&c=…` URL (the download page's
//! *Connect* button), as the download URL the browser recorded on the file
//! ([`download_origins`]), or typed in by hand.

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

/// Length of a setup code (letters and digits, without look-alikes).
pub const CODE_LEN: usize = 8;

/// Uppercases a typed setup code and drops spaces and dashes.
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase()
}

/// A station's address plus a setup code, to be exchanged for its config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationLink {
    pub station: String,
    pub code: String,
}

impl StationLink {
    /// Reads a `slipstream://link?s=STATION&c=CODE` URL, or a station
    /// download URL (`…/download.php?os=…&c=CODE`).
    pub fn parse(url: &str) -> Option<Self> {
        let u = reqwest::Url::parse(url.trim()).ok()?;
        let param = |name: &str| u.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        let code = normalize_code(&param("c")?);
        if code.len() != CODE_LEN {
            return None;
        }
        let station = match u.scheme() {
            "slipstream" => param("s")?,
            "http" | "https" => {
                let base = u.path().strip_suffix("download.php")?;
                format!("{}{base}", u.origin().ascii_serialization())
            }
            _ => return None,
        };
        Some(Self { station, code })
    }
}

/// URLs the browser recorded as where this copy of the app came from:
/// Windows' `Zone.Identifier` stream, the `user.xdg.origin.url` attribute
/// on Linux (Chrome, Firefox) and `kMDItemWhereFroms` on macOS. Best effort:
/// copying the file elsewhere usually loses them.
pub fn download_origins() -> Vec<String> {
    let Ok(exe) = std::env::current_exe() else { return Vec::new() };
    #[cfg(windows)]
    {
        let ads = format!("{}:Zone.Identifier", exe.display());
        let text = std::fs::read_to_string(ads).unwrap_or_default();
        return text
            .lines()
            .filter_map(|l| l.strip_prefix("HostUrl=").or_else(|| l.strip_prefix("ReferrerUrl=")))
            .map(|u| u.trim().to_string())
            .collect();
    }
    #[cfg(unix)]
    {
        // On macOS the attribute is on the downloaded bundle, not the binary.
        let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf);
        let attrs = ["user.xdg.origin.url", "com.apple.metadata:kMDItemWhereFroms"];
        return [Some(exe), bundle]
            .into_iter()
            .flatten()
            .flat_map(|p| attrs.iter().filter_map(move |a| xattr::get(&p, a).ok().flatten()))
            .flat_map(|bytes| attr_urls(&bytes))
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// The URLs in an origin attribute: a plain string on Linux, a binary
/// plist array of strings on macOS.
#[cfg(unix)]
fn attr_urls(bytes: &[u8]) -> Vec<String> {
    if bytes.starts_with(b"bplist") {
        return match plist::Value::from_reader(std::io::Cursor::new(bytes)) {
            Ok(plist::Value::Array(items)) => items.into_iter().filter_map(|v| v.into_string()).collect(),
            Ok(plist::Value::String(s)) => vec![s],
            _ => Vec::new(),
        };
    }
    std::str::from_utf8(bytes).map(|s| vec![s.trim_end_matches('\0').trim().to_string()]).unwrap_or_default()
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

    #[test]
    fn links_parse() {
        let want = StationLink { station: "https://support.acme.com/".into(), code: "K7MQ2XPA".into() };
        assert_eq!(StationLink::parse("https://support.acme.com/download.php?os=windows&c=K7MQ-2XPA"), Some(want.clone()));
        assert_eq!(
            StationLink::parse("slipstream://link?s=https%3A%2F%2Fsupport.acme.com%2F&c=k7mq2xpa"),
            Some(want)
        );
        let sub = StationLink::parse("http://10.0.0.5:8080/remote/download.php?c=AAAABBBB&os=linux").unwrap();
        assert_eq!(sub.station, "http://10.0.0.5:8080/remote/");
        assert!(StationLink::parse("https://support.acme.com/download.php?os=windows").is_none(), "no code");
        assert!(StationLink::parse("https://example.com/other.php?c=AAAABBBB").is_none(), "not a station");
        assert!(StationLink::parse("https://support.acme.com/download.php?c=SHORT").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn origin_attributes_parse() {
        let url = "https://support.acme.com/download.php?os=macos&c=AAAABBBB";
        // macOS: a binary plist array, as Safari and Chrome write it.
        let mut bin = Vec::new();
        plist::Value::Array(vec![url.into(), "https://support.acme.com/".into()]).to_writer_binary(&mut bin).unwrap();
        assert_eq!(attr_urls(&bin), vec![url.to_string(), "https://support.acme.com/".to_string()]);
        // Linux: the plain URL.
        assert_eq!(attr_urls(url.as_bytes()), vec![url.to_string()]);
        assert!(StationLink::parse(&attr_urls(&bin)[0]).is_some());
    }
}
