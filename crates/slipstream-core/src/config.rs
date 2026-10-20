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
    /// Explicit empty groups, in addition to the groups used by devices.
    pub groups: Vec<String>,
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
    /// Permit authenticated controllers to send LAN wake packets.
    pub allow_wake_on_lan: bool,
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
    /// Open new desktop links in independent windows rather than console tabs.
    pub separate_windows: bool,
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
    /// Station used for this link. Older profiles used the app's own station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
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
            allow_wake_on_lan: false,
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
            separate_windows: false,
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
    pub fn remember_connection(&mut self, id: DeviceId, name: &str, os: &str, station: Option<String>) {
        let normalize = |s: Option<&str>| s.map(|s| crate::directory::normalize_server_url(s).map(|u| u.to_string()).unwrap_or_else(|_| s.to_lowercase()));
        let key = normalize(station.as_deref());
        self.recent.retain(|r| r.id != id || normalize(r.station.as_deref().or(self.server.url.as_deref())) != key);
        self.recent.insert(
            0,
            RecentConnection {
                id,
                name: name.to_string(),
                os: os.to_string(),
                station,
                last_used: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default(),
            },
        );
        self.recent.truncate(20);
    }
}

/// Whether this process can read reliable per-device firmware evidence.
pub fn clone_detection_available() -> bool {!crate::hardware_id::current().is_empty()}

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

    /// Loads the persistent key, creating it on first run or when reliable
    /// firmware evidence proves this profile was moved to a different device.
    /// Unreadable/damaged keys are never silently replaced.
    pub fn identity(&self) -> Result<SecretKey> {
        self.identity_for_hardware(crate::hardware_id::current())
    }

    fn identity_for_hardware(&self, evidence: &crate::hardware_id::Evidence) -> Result<SecretKey> {
        if evidence.is_empty() {return self.identity_unbound();}
        let mut options=fs::OpenOptions::new();options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
        let lock=options.open(self.dir.join("identity.lock"))?;
        lock.lock().context("locking the device identity")?;
        let binding_path=self.dir.join("device-binding.json");
        let previous=match fs::read(&binding_path) {
            Ok(bytes)=>match serde_json::from_slice::<DeviceBinding>(&bytes) {
                Ok(binding)=>Some(binding),
                Err(_)=>{tracing::warn!("device hardware binding is damaged; retaining the existing identity");return self.identity_unbound();}
            },
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
            Err(e)=>return Err(e).context("reading device hardware binding"),
        };
        let mut key=self.identity_unbound()?;
        let mut changed_device=false;
        if let Some(binding)=&previous {
            if binding.version!=1 {tracing::warn!("unknown device-binding version; retaining the existing identity");return Ok(key);}
            // Prefer the firmware UUID; use a serial only when no comparable
            // UUID is available. A generic/repeated serial must not override a
            // changed UUID on two laptops of the same model.
            let different=["system-uuid","system-serial"].into_iter().find_map(|kind| {
                binding.evidence.get(kind).zip(evidence.get(kind)).map(|(old,new)| old!=new)
            }).unwrap_or(false);
            if different {
                changed_device=true;
                key=SecretKey::generate();
                write_private(&self.dir.join("identity.key"),&key.to_bytes())?;
                let settings_path=self.dir.join("settings.toml");
                let mut settings=match fs::read_to_string(&settings_path) {
                    Ok(text)=>toml::from_str::<Settings>(&text).context("settings are damaged; cannot finish cloned-device identity initialization")?,
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Settings::default(),
                    Err(e)=>return Err(e).context("reading cloned-device settings"),
                };
                settings.host.last_device_id=None;
                if settings.device_name.eq_ignore_ascii_case(&binding.system_name) {settings.device_name=default_device_name();}
                self.save_settings(&settings)?;
                for name in ["device-id.txt","chat-identity.txt"] {
                    match fs::remove_file(self.dir.join("public").join(name)) {
                        Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e).context("clearing the previous service's public identity"),
                    }
                }
                tracing::info!("different physical device detected; generated a fresh identity and cleared the cached station ID while preserving configuration");
            }
        }
        let mut evidence=evidence.clone();
        // Keep earlier evidence if today's permissions temporarily hide it.
        if !changed_device && let Some(previous)=&previous {for (kind,value) in &previous.evidence {evidence.entry(kind.clone()).or_insert_with(||value.clone());}}
        let next=DeviceBinding {version:1,evidence,system_name:default_device_name()};
        if previous.as_ref()!=Some(&next) {write_private(&binding_path,&serde_json::to_vec(&next)?)?;}
        Ok(key)
    }

    fn identity_unbound(&self) -> Result<SecretKey> {
        let path = self.dir.join("identity.key");
        let mut attempt = 0;
        let bytes = loop {
            match fs::read(&path) {
                Ok(bytes) => break bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let key = SecretKey::generate();
                    match private_temp(&path, &key.to_bytes())?.persist_noclobber(&path) {
                        Ok(_) => {
                            tracing::info!("created this device's identity in {}", path.display());
                            return Ok(key);
                        }
                        // Another process published a complete identity first.
                        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(error.error).context("saving this device's identity"),
                    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct DeviceBinding {
    version: u32,
    evidence: crate::hardware_id::Evidence,
    system_name: String,
}

/// Writes a file readable only by the current user.
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    private_temp(path, data)?.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn private_temp(path: &Path, data: &[u8]) -> Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::with_prefix_in(".slipstream-", path.parent().context("settings path has no parent")?)?;
    // NamedTempFile creates the file with mode 0600 on Unix, before any
    // passwords or keys are written, and uses a unique staging name.
    temp.write_all(data)?;
    Ok(temp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_identity_and_settings_writes_stay_complete_and_private() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(dir.path()).unwrap();
        let barrier=std::sync::Barrier::new(8);
        let keys=std::thread::scope(|scope| {
            let jobs:Vec<_>=(0..8).map(|n| {
                let (store,barrier)=(&store,&barrier);
                scope.spawn(move || {
                    barrier.wait();let key=store.identity().unwrap();
                    for _ in 0..10 {store.save_settings(&Settings {device_name:format!("writer {n}"),..Default::default()}).unwrap();}
                    key.to_bytes()
                })
            }).collect();
            jobs.into_iter().map(|job|job.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(keys.iter().all(|key|key==&keys[0]));
        assert_eq!(store.identity().unwrap().to_bytes(),keys[0]);
        let settings:Settings=toml::from_str(&fs::read_to_string(dir.path().join("settings.toml")).unwrap()).unwrap();
        assert!(settings.device_name.starts_with("writer "));
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            for file in ["settings.toml","identity.key"] {assert_eq!(fs::metadata(dir.path().join(file)).unwrap().permissions().mode() & 0o777,0o600);}
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(),if crate::hardware_id::current().is_empty(){2}else{4});
    }

    #[test]
    fn cloned_profile_rekeys_once_preserving_settings_and_removing_cached_identity() {
        use crate::hardware_id::Evidence;
        let master=tempfile::tempdir().unwrap();let source=Store::open(master.path()).unwrap();
        let mut settings=Settings {device_name:"Custom workstation label".into(),..Default::default()};
        settings.server.url=Some("station.example".into());settings.server.key=Some("fixture-key".into());
        settings.host.set_permanent_password(Some("fixture unattended password"));settings.host.last_device_id=Some(DeviceId(123456789));
        source.save_settings(&settings).unwrap();
        let evidence=|id:&str| Evidence::from([("system-uuid".into(),id.into())]);
        let original=source.identity_for_hardware(&evidence("device-a")).unwrap().to_bytes();
        assert_eq!(source.identity_for_hardware(&evidence("device-a")).unwrap().to_bytes(),original);
        for n in 0..2 {
            let clone=tempfile::tempdir().unwrap();let store=Store::open(clone.path()).unwrap();
            for name in ["settings.toml","identity.key","device-binding.json"] {fs::copy(master.path().join(name),clone.path().join(name)).unwrap();}
            fs::create_dir(clone.path().join("public")).unwrap();
            for name in ["device-id.txt","chat-identity.txt"] {fs::write(clone.path().join("public").join(name),"old identity").unwrap();}
            let cloned=store.identity_for_hardware(&evidence(&format!("device-{n}"))).unwrap().to_bytes();
            assert_ne!(cloned,original);assert_eq!(cloned,store.identity_for_hardware(&evidence(&format!("device-{n}"))).unwrap().to_bytes());
            let actual=store.load_settings();assert_eq!(actual.device_name,settings.device_name);assert_eq!(actual.server.key,settings.server.key);assert_eq!(actual.server.url,settings.server.url);
            assert_eq!(actual.host.permanent_secret(),settings.host.permanent_secret());assert!(actual.host.last_device_id.is_none());
            assert!(!clone.path().join("public/device-id.txt").exists());assert!(!clone.path().join("public/chat-identity.txt").exists());
            // No usable evidence is not evidence of another device.
            assert_eq!(cloned,store.identity_for_hardware(&Evidence::new()).unwrap().to_bytes());
            fs::write(clone.path().join("device-binding.json"),b"invalid metadata").unwrap();
            assert_eq!(cloned,store.identity_for_hardware(&evidence("device-new")).unwrap().to_bytes());
        }
        assert_eq!(original,source.identity_for_hardware(&evidence("device-a")).unwrap().to_bytes());
    }

    #[test]
    fn partial_hardware_reads_preserve_identity_and_concurrent_clone_starts_agree() {
        use crate::hardware_id::Evidence;
        let dir=tempfile::tempdir().unwrap();let store=Store::open(dir.path()).unwrap();
        let original=Evidence::from([("system-uuid".into(),"uuid-a".into()),("system-serial".into(),"serial-a".into())]);
        let key=store.identity_for_hardware(&original).unwrap().to_bytes();
        assert_eq!(key,store.identity_for_hardware(&Evidence::from([("system-serial".into(),"serial-a".into())])).unwrap().to_bytes());
        let new=Evidence::from([("system-uuid".into(),"uuid-b".into()),("system-serial".into(),"serial-b".into())]);
        let results=std::thread::scope(|scope| {
            let jobs:Vec<_>=(0..8).map(|_| {let (store,new)=(&store,&new);scope.spawn(move ||store.identity_for_hardware(new).unwrap().to_bytes())}).collect();
            jobs.into_iter().map(|job|job.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(results.iter().all(|k|k==&results[0]));assert_ne!(results[0],key);
    }

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
        assert!(!s.viewer.separate_windows);
        s.viewer.separate_windows = true;
        s.host.set_permanent_password(Some("hunter2"));
        store.save_settings(&s).unwrap();
        let loaded = store.load_settings();
        assert!(loaded.viewer.separate_windows);
        assert_eq!(loaded.host.permanent_secret(), Some(crate::auth::password_secret("hunter2")));
        let k1 = store.identity().unwrap();
        let k2 = store.identity().unwrap();
        assert_eq!(k1.public(), k2.public());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn recent_links_preserve_station_routes_and_read_older_profiles() {
        let old: RecentConnection = toml::from_str("id=123456789\nname='Old'\nos='Linux'\nlast_used=0").unwrap();
        assert!(old.station.is_none());
        let mut settings = Settings::default();
        settings.server.url = Some("first.example".into());
        settings.recent.push(old);
        settings.remember_connection(DeviceId(123456789),"First","Linux",Some("https://first.example/".into()));
        settings.remember_connection(DeviceId(123456789),"Second","Windows",Some("second.example".into()));
        assert_eq!(settings.recent.len(),2);
        let loaded: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(loaded.recent[0].station.as_deref(),Some("second.example"));
        assert_eq!(loaded.recent[1].name,"First");
    }
}
