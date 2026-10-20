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

    /// Station admins: adds or replaces a device everyone sees.
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
}
