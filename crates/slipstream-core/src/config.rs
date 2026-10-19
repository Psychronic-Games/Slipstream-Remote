//! Persistent settings and device identity.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use iroh::{RelayUrl, SecretKey};
use serde::{Deserialize, Serialize};

use crate::directory::DeviceId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Name shown to people connecting to this device.
    pub device_name: String,
    pub server: ServerSettings,
    pub host: HostSettings,
    pub viewer: ViewerSettings,
    pub app: AppSettings,
    /// Recently used connections, most recent first.
    pub recent: Vec<RecentConnection>,
    /// Stations a technician signed in to (see `account`), each with its
    /// lists of devices.
    pub accounts: Vec<StationAccount>,
}

/// A sign-in to a station: any station, not only this app's own.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StationAccount {
    /// Station address as it gives it, e.g. `relay.example.com`.
    pub server: String,
    /// Its access key, handed over at sign-in.
    pub key: Option<String>,
    pub org: Option<String>,
    pub user: crate::account::AccountUser,
    pub token: String,
    /// Opens the passwords saved in this account (hex; made from the
    /// sign-in password, which isn't kept).
    pub vault_key: Option<String>,
    /// The lists as last fetched, shown before the station answers.
    pub devices: Vec<crate::account::SavedDevice>,
    pub shared: Vec<crate::account::SavedDevice>,
    /// The station ended this sign-in (password reset, signed out on the
    /// admin page): sign in again.
    pub signed_out: bool,
}

impl std::fmt::Debug for StationAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No token or key in logs.
        f.debug_struct("StationAccount")
            .field("server", &self.server)
            .field("user", &self.user.username)
            .field("devices", &self.devices.len())
            .field("shared", &self.shared.len())
            .field("signed_out", &self.signed_out)
            .finish_non_exhaustive()
    }
}

impl StationAccount {
    /// "Acme IT" or the address.
    pub fn label(&self) -> String {
        self.org.clone().filter(|o| !o.trim().is_empty()).unwrap_or_else(|| self.server.clone())
    }

    /// Same station? (addresses compared as the apps normalise them)
    pub fn is_station(&self, server: &str) -> bool {
        let norm = |s: &str| crate::directory::normalize_server_url(s).map(|u| u.to_string()).unwrap_or_else(|_| s.to_lowercase());
        norm(&self.server) == norm(server)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// Station (directory server) address: a domain or URL, e.g. `relay.example.com`.
    pub url: Option<String>,
    /// Access key for a private station.
    pub key: Option<String>,
    /// Organisation running the station (from its download or `hello`).
    pub org_name: Option<String>,
    /// Settings came from a station download (see `provision`).
    pub provisioned: bool,
    /// The station allows falling back to public relays.
    pub station_allows_public_relays: bool,
    /// Relays learned from the directory server (cached for faster startup).
    pub relays: Vec<RelayUrl>,
    /// Also use the public n0 relays as a fallback.
    pub use_public_relays: bool,
}

impl ServerSettings {
    /// Public relays are used only if both the user and the station allow it.
    pub fn public_relays_enabled(&self) -> bool {
        self.use_public_relays && self.station_allows_public_relays
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    /// Accept incoming connections at all.
    pub enabled: bool,
    /// Length of the one-time password.
    pub one_time_password_len: usize,
    /// Digest of the permanent (unattended) password, hex encoded.
    pub permanent_password_secret: Option<String>,
    pub allow_input: bool,
    pub allow_clipboard: bool,
    pub allow_file_transfer: bool,
    /// Let viewers hear this computer's sound.
    pub allow_audio: bool,
    /// Let viewers forward ports through this computer (reach its own
    /// services and its network). Off by default.
    pub allow_tunnels: bool,
    /// Let authenticated viewers discover devices on connected IPv4 subnets.
    pub allow_network_scan: bool,
    /// Last numeric ID the server gave us (display only; the server is authoritative).
    pub last_device_id: Option<DeviceId>,
    /// Wayland portal token so screen-sharing permission survives restarts.
    pub portal_restore_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerSettings {
    pub max_fps: u32,
    /// 0 = automatic.
    pub bitrate_kbps: u32,
    pub sync_clipboard: bool,
    /// Play the remote computer's sound when a link starts (it can be
    /// switched per session).
    pub play_audio: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Closing the window keeps Slipstream running in the system tray
    /// (for unattended access). Off by default: closing ends access.
    pub run_in_tray: bool,
    /// The "still running in the tray" explanation was shown.
    pub tray_hint_shown: bool,
    /// Windows: no tray icon while the Slipstream service hosts this
    /// computer (by default the app sits in the tray then, from sign-in).
    pub hide_tray_with_service: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecentConnection {
    pub id: DeviceId,
    pub name: String,
    pub os: String,
    /// Unix seconds.
    pub last_used: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            server: ServerSettings::default(),
            host: HostSettings::default(),
            viewer: ViewerSettings::default(),
            app: AppSettings::default(),
            recent: Vec::new(),
            accounts: Vec::new(),
        }
    }
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            url: None,
            key: None,
            org_name: None,
            provisioned: false,
            station_allows_public_relays: true,
            relays: Vec::new(),
            use_public_relays: true,
        }
    }
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            one_time_password_len: 6,
            permanent_password_secret: None,
            allow_input: true,
            allow_clipboard: true,
            allow_file_transfer: true,
            allow_audio: true,
            allow_tunnels: false,
            allow_network_scan: true,
            last_device_id: None,
            portal_restore_token: None,
        }
    }
}

impl Default for ViewerSettings {
    fn default() -> Self {
        Self {
            max_fps: 60,
            bitrate_kbps: 0,
            sync_clipboard: true,
            play_audio: false,
        }
    }
}

impl HostSettings {
    pub fn set_permanent_password(&mut self, password: Option<&str>) {
        self.permanent_password_secret = password
            .filter(|p| !p.trim().is_empty())
            .map(|p| hex(&crate::auth::password_secret(p)));
    }

    pub fn permanent_secret(&self) -> Option<[u8; 32]> {
        self.permanent_password_secret.as_deref().and_then(unhex)
    }
}

impl Settings {
    pub fn remember_connection(&mut self, id: DeviceId, name: &str, os: &str) {
        self.recent.retain(|r| r.id != id);
        self.recent.insert(
            0,
            RecentConnection {
                id,
                name: name.to_string(),
                os: os.to_string(),
                last_used: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default(),
            },
        );
        self.recent.truncate(20);
    }
}

fn default_device_name() -> String {
    std::env::var("SLIPSTREAM_DEVICE_NAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "My Computer".to_string())
}

use crate::directory::hex;

fn unhex(s: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if s.len() != 64 {
        return None;
    }
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// On-disk locations for settings and identity.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The platform's standard config directory, or `$SLIPSTREAM_HOME` if set
    /// (handy for running two instances on one machine).
    pub fn open_default() -> Result<Self> {
        let dir = match std::env::var_os("SLIPSTREAM_HOME") {
            Some(d) => PathBuf::from(d),
            None => directories::ProjectDirs::from("dev", "Slipstream", "Slipstream")
                .context("no home directory")?
                .config_dir()
                .to_path_buf(),
        };
        Self::open(dir)
    }

    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_settings(&self) -> Settings {
        let path = self.dir.join("settings.toml");
        match fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("ignoring unreadable {}: {e}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        write_private(&self.dir.join("settings.toml"), toml::to_string_pretty(settings)?.as_bytes())
    }

    /// Loads this device's keypair, creating one on first run. The key is
    /// the device's ID on its station, so it's only ever made when there is
    /// none: a key that can't be read right now (Windows antivirus scanning
    /// a freshly copied one, say) is retried, then an error. Replacing it
    /// would quietly give the computer a new ID.
    pub fn identity(&self) -> Result<SecretKey> {
        let path = self.dir.join("identity.key");
        let mut attempt = 0;
        let bytes = loop {
            match fs::read(&path) {
                Ok(bytes) => break bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let key = SecretKey::generate();
                    write_private(&path, &key.to_bytes())?;
                    tracing::info!("created this device's identity in {}", path.display());
                    return Ok(key);
                }
                Err(_) if attempt < 20 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(e) => return Err(e).with_context(|| format!("reading this device's identity ({})", path.display())),
            }
        };
        match <[u8; 32]>::try_from(bytes.as_slice()) {
            Ok(bytes) => Ok(SecretKey::from_bytes(&bytes)),
            Err(_) => anyhow::bail!("{} is damaged ({} bytes instead of 32): restore it, or delete it to give this device a new ID", path.display(), bytes.len()),
        }
    }
}

/// Writes a file readable only by the current user.
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_identity_is_never_replaced() {
        let dir = std::env::temp_dir().join(format!("slipstream-test-badkey-{}", std::process::id()));
        let store = Store::open(&dir).unwrap();
        std::fs::write(dir.join("identity.key"), b"short").unwrap();
        assert!(store.identity().is_err());
        assert_eq!(std::fs::read(dir.join("identity.key")).unwrap(), b"short");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_roundtrip_and_identity() {
        let dir = std::env::temp_dir().join(format!("slipstream-test-{}", std::process::id()));
        let store = Store::open(&dir).unwrap();
        let mut s = store.load_settings();
        s.host.set_permanent_password(Some("hunter2"));
        store.save_settings(&s).unwrap();
        let loaded = store.load_settings();
        assert_eq!(loaded.host.permanent_secret(), Some(crate::auth::password_secret("hunter2")));
        let k1 = store.identity().unwrap();
        let k2 = store.identity().unwrap();
        assert_eq!(k1.public(), k2.public());
        fs::remove_dir_all(dir).ok();
    }
}
