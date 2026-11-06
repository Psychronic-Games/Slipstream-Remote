//! Technician accounts on a station: signing in from the app (to any
//! station, not only the one this app came from) and the lists of devices
//! kept there. The station side is the accounts section of
//! `server/station/lib/slipstream.php`.
//!
//! ```text
//! POST {station}/api/  {"op":"login","username":…,"password":…,"device":…}
//!                   →  {"ok":true,"token":…,"user":…,"station":{server,key,org},"vault_salt":…}
//! POST {station}/api/  {"op":"account","token":…}          (also device_save,
//!                   →  {"ok":true,"user":…,"devices":[…],"shared":[…],…}   device_delete,
//!                                                           shared_save, shared_delete, logout)
//! ```
//!
//! A sign-in hands the app the station's access key, so it can look up
//! that station's devices and use its relay.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    directory::{DeviceId, DirectoryClient},
    provision::StationConfig,
    vault::SealedSecret,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUser {
    pub username: String,
    pub name: String,
    /// May change the station's shared devices.
    #[serde(default)]
    pub admin: bool,
}

/// One entry in a technician's list (or the station's shared one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDevice {
    pub id: DeviceId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub notes: String,
    /// A station-synced folder name; empty means Ungrouped.
    #[serde(default)]
    pub group: String,
    /// The device's password, sealed with the technician's vault key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SealedSecret>,
    /// Shared list only: who added it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
    /// Reaching it when its own system can't help (personal list only).
    /// Always sent, so `null` clears it; stations before 0.33 drop it.
    #[serde(default)]
    pub management: Option<Management>,
}

/// Reaching a device beyond its operating system: another Slipstream
/// computer on its network (the gateway) can wake it, and can open the way
/// to its management controller (a server's BMC, Intel vPro) for power and
/// a console that works in the BIOS and while the system is off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Management {
    /// A saved device on the same station and network as this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<DeviceId>,
    /// Its network adapter's address for Wake-on-LAN, `00:11:22:33:44:55`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wake_mac: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<Controller>,
}

impl Management {
    pub fn is_empty(&self) -> bool {
        self.gateway.is_none() && self.wake_mac.is_empty() && self.controller.is_none()
    }

    /// The wake MAC in canonical form, if it is one.
    pub fn mac(&self) -> Option<String> {
        normalize_mac(&self.wake_mac)
    }
}

/// `00-11-22-aa-bb-cc`, `0011.22aa.bbcc`, … as `00:11:22:AA:BB:CC`.
pub fn normalize_mac(text: &str) -> Option<String> {
    let hex: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let separators = text.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '-' | '.' | ' '));
    (hex.len() == 12 && separators).then(|| {
        hex.as_bytes().chunks(2).map(|b| String::from_utf8_lossy(b).to_ascii_uppercase()).collect::<Vec<_>>().join(":")
    })
}

/// A device's out-of-band management controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controller {
    pub kind: ControllerKind,
    /// As seen from the gateway: `10.0.0.20`, `bmc.lan:8443` or a URL.
    pub address: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// Its password, sealed like saved device passwords.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SealedSecret>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerKind {
    /// A server BMC with Redfish (Supermicro, Dell iDRAC, HPE iLO, Lenovo
    /// XClarity…): web console plus power control.
    Redfish,
    /// Intel AMT (vPro): its web page on port 16992/16993.
    IntelAmt,
    /// Any other management web page (console only).
    Web,
}

impl ControllerKind {
    pub const ALL: [Self; 3] = [Self::Redfish, Self::IntelAmt, Self::Web];

    pub fn label(self) -> &'static str {
        match self {
            Self::Redfish => "Server BMC (Redfish: Supermicro, iDRAC, iLO…)",
            Self::IntelAmt => "Intel vPro (AMT)",
            Self::Web => "Other management web page",
        }
    }

    /// Port assumed when the address names none.
    pub fn default_port(self) -> u16 {
        match self {
            Self::IntelAmt => 16993,
            Self::Redfish | Self::Web => 443,
        }
    }
}

impl Controller {
    /// Where the gateway connects (host, port) and whether it speaks HTTPS.
    pub fn target(&self) -> Option<(String, u16, bool)> {
        let text = self.address.trim();
        let (scheme, rest) = match text.split_once("://") {
            Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
            None => (None, text),
        };
        let authority = rest.split('/').next().unwrap_or("").trim();
        if authority.is_empty() || authority.contains(['@', ' ']) {
            return None;
        }
        let (host, port) = match authority.rsplit_once(':') {
            // [v6]:port or host:port; a bare IPv6 address has several colons.
            Some((host, port)) if !host.contains(':') || host.ends_with(']') => (host, Some(port.parse::<u16>().ok()?)),
            _ => (authority, None),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return None;
        }
        let tls = match scheme.as_deref() {
            Some("http") => false,
            Some("https") => true,
            None => port != Some(80) && port != Some(16992),
            Some(_) => return None,
        };
        let port = port.unwrap_or(match (tls, self.kind) {
            (false, ControllerKind::IntelAmt) => 16992,
            (false, _) => 80,
            (true, kind) => kind.default_port(),
        });
        Some((host.to_string(), port, tls))
    }
}

/// Everything a station keeps for a signed-in technician.
#[derive(Debug, Clone, Deserialize)]
pub struct AccountState {
    pub user: AccountUser,
    #[serde(default)]
    pub devices: Vec<SavedDevice>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub shared: Vec<SavedDevice>,
    #[serde(default)]
    pub shared_groups: Vec<String>,
    pub vault_salt: String,
    /// The station's current address and access key (they can change).
    pub station: StationConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Login {
    pub token: String,
    pub user: AccountUser,
    pub station: StationConfig,
    pub vault_salt: String,
}

impl DirectoryClient {
    /// Signs in; needs no access key (it returns the station's).
    pub async fn login(&self, username: &str, password: &str, device: &str) -> Result<Login> {
        self.call(serde_json::json!({ "op": "login", "username": username, "password": password, "device": device })).await.map_err(no_accounts)
    }

    pub async fn account(&self, token: &str) -> Result<AccountState> {
        self.call(serde_json::json!({ "op": "account", "token": token })).await.map_err(no_accounts)
    }

    /// Adds or replaces (by ID) a device in the technician's list.
    pub async fn save_device(&self, token: &str, device: &SavedDevice) -> Result<AccountState> {
        self.call(serde_json::json!({ "op": "device_save", "token": token, "device": device })).await
    }

    pub async fn delete_device(&self, token: &str, id: DeviceId) -> Result<AccountState> {
        self.call(serde_json::json!({ "op": "device_delete", "token": token, "id": id.0 })).await
    }

    pub async fn save_group(&self, token: &str, group: &str) -> Result<AccountState> {
        self.call(serde_json::json!({"op":"group_save","token":token,"group":group})).await
    }

    pub async fn delete_group(&self, token: &str, group: &str) -> Result<AccountState> {
        self.call(serde_json::json!({"op":"group_delete","token":token,"group":group})).await
    }

    /// Station admins: adds or replaces a station device. Group assignments control visibility.
    pub async fn save_shared(&self, token: &str, device: &SavedDevice) -> Result<AccountState> {
        let mut device = device.clone();
        device.secret = None;
        self.call(serde_json::json!({ "op": "shared_save", "token": token, "device": device })).await
    }

    pub async fn delete_shared(&self, token: &str, id: DeviceId) -> Result<AccountState> {
        self.call(serde_json::json!({ "op": "shared_delete", "token": token, "id": id.0 })).await
    }

    pub async fn logout(&self, token: &str) -> Result<()> {
        let _: serde_json::Value = self.call(serde_json::json!({ "op": "logout", "token": token })).await?;
        Ok(())
    }
}

/// Is this the station telling us the sign-in ended (token revoked,
/// password reset, account removed)?
pub fn is_signed_out(e: &anyhow::Error) -> bool {
    format!("{e}").contains("signed out")
}

/// Stations from before accounts (or the standalone `slipstream-server`)
/// don't know these requests.
fn no_accounts(e: anyhow::Error) -> anyhow::Error {
    if format!("{e}").contains("unknown op") {
        anyhow::anyhow!("that station doesn't have technician sign-in (it needs updating)")
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_saved_devices_default_to_ungrouped() {
        let device: SavedDevice = serde_json::from_str(r#"{"id":482913077,"name":"Old client"}"#).unwrap();
        assert!(device.group.is_empty());
        // Empty is sent explicitly so a new client can clear a group.
        assert_eq!(serde_json::to_value(device).unwrap()["group"], "");
    }

    #[test]
    fn station_groups_default_for_older_servers_and_survive_the_settings_cache() {
        let state: AccountState = serde_json::from_value(serde_json::json!({
            "user":{"username":"tech","name":"Tech"}, "vault_salt":"fixture",
            "station":{"server":"station.example"}
        })).unwrap();
        assert!(state.shared_groups.is_empty());
        let mut settings = crate::config::Settings::default();
        settings.accounts.push(crate::config::StationAccount { shared_groups: vec!["Empty station group".into()], ..Default::default() });
        let loaded: crate::config::Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(loaded.accounts[0].shared_groups, ["Empty station group"]);
    }

    #[test]
    fn group_and_multiline_notes_survive_json_and_settings_cache() {
        let device: SavedDevice = serde_json::from_value(serde_json::json!({
            "id":482913077, "name":"Server", "group":"Lab / Servers",
            "notes":"First line\nSecond line", "added_by":"admin"
        })).unwrap();
        assert_eq!(serde_json::from_value::<SavedDevice>(serde_json::to_value(&device).unwrap()).unwrap(), device);
        let mut settings = crate::config::Settings::default();
        settings.accounts.push(crate::config::StationAccount { devices:vec![device.clone()], ..Default::default() });
        let text = toml::to_string(&settings).unwrap();
        let loaded: crate::config::Settings = toml::from_str(&text).unwrap();
        assert_eq!(loaded.accounts[0].devices[0], device);
    }

    #[test]
    fn controller_addresses_and_macs() {
        let c = |address: &str, kind| Controller { kind, address: address.into(), username: String::new(), secret: None };
        assert_eq!(c("10.0.0.20", ControllerKind::Redfish).target(), Some(("10.0.0.20".into(), 443, true)));
        assert_eq!(c("https://bmc.lan:8443/", ControllerKind::Redfish).target(), Some(("bmc.lan".into(), 8443, true)));
        assert_eq!(c("http://10.0.0.21", ControllerKind::Web).target(), Some(("10.0.0.21".into(), 80, false)));
        assert_eq!(c("10.0.0.22", ControllerKind::IntelAmt).target(), Some(("10.0.0.22".into(), 16993, true)));
        assert_eq!(c("http://10.0.0.22", ControllerKind::IntelAmt).target(), Some(("10.0.0.22".into(), 16992, false)));
        assert_eq!(c("[fd00::5]:443", ControllerKind::Redfish).target(), Some(("fd00::5".into(), 443, true)));
        assert_eq!(c("user@host", ControllerKind::Web).target(), None);
        assert_eq!(c("ftp://host", ControllerKind::Web).target(), None);
        assert_eq!(normalize_mac("00-11-22-aa-bb-cc").as_deref(), Some("00:11:22:AA:BB:CC"));
        assert_eq!(normalize_mac("0011.22aa.bbcc").as_deref(), Some("00:11:22:AA:BB:CC"));
        assert_eq!(normalize_mac("00:11:22:aa:bb"), None);
        assert_eq!(normalize_mac("00:11:22:aa:bb:cg"), None);
    }
}
