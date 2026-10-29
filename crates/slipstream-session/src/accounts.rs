//! Stations a technician signs in to, besides (or instead of) this app's
//! own: their lists of devices, saved passwords, and connecting through
//! them. See `slipstream_core::account` for the station side.

use std::collections::BTreeSet;

use anyhow::{Context, Result, anyhow, bail};
use slipstream_core::{
    account::{AccountState, SavedDevice, is_signed_out},
    config::StationAccount,
    directory::{DeviceId, DirectoryClient, Lookup, PresenceStatus},
    vault::VaultKey,
};
use tokio::sync::mpsc;

use crate::{Client, ViewerEvent, ViewerSession};

/// Which sign-in: a station and a username on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountRef {
    pub server: String,
    pub username: String,
}

impl AccountRef {
    pub fn of(a: &StationAccount) -> Self {
        Self { server: a.server.clone(), username: a.user.username.clone() }
    }

    fn matches(&self, a: &StationAccount) -> bool {
        a.is_station(&self.server) && a.user.username == self.username
    }
}

/// A saved device's state as last looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    Online,
    Offline,
    Unknown,
}

impl Client {
    fn account(&self, r: &AccountRef) -> Result<StationAccount> {
        self.inner.settings.lock().unwrap().accounts.iter().find(|a| r.matches(a)).cloned().ok_or_else(|| anyhow!("not signed in to {}", r.server))
    }

    fn station_client(a: &StationAccount) -> Result<DirectoryClient> {
        DirectoryClient::new(&a.server, a.key.clone())
    }

    fn update_account(&self, r: &AccountRef, f: impl FnOnce(&mut StationAccount)) {
        self.update_settings(|s| {
            if let Some(a) = s.accounts.iter_mut().find(|a| r.matches(a)) {
                f(a);
            }
        });
    }

    /// Takes in what the station says about the account.
    fn apply_state(&self, r: &AccountRef, state: AccountState) {
        self.update_account(r, |a| {
            a.server = state.station.server;
            a.key = state.station.key;
            a.org = state.station.org;
            a.user = state.user;
            a.devices = state.devices;
            a.groups = state.groups;
            a.shared = state.shared;
            a.shared_groups = state.shared_groups;
            a.signed_out = false;
        });
    }

    /// A station call failed: if it says the sign-in ended, remember that.
    fn note_error(&self, r: &AccountRef, e: anyhow::Error) -> anyhow::Error {
        if is_signed_out(&e) {
            self.update_account(r, |a| a.signed_out = true);
        }
        e
    }

    /// Signs in to a station (any station: it hands over its access key).
    /// Replaces an earlier sign-in by the same user there.
    pub async fn sign_in(&self, server: &str, username: &str, password: &str) -> Result<AccountRef> {
        let username = username.trim().to_lowercase();
        let dc = DirectoryClient::new(server, None)?;
        let device = self.settings().device_name;
        let login = dc.login(&username, password, &device).await?;
        let (pw, salt) = (password.to_string(), login.vault_salt.clone());
        let vault = tokio::task::spawn_blocking(move || VaultKey::derive(&pw, &salt)).await??;
        let account = StationAccount {
            server: login.station.server.clone(),
            key: login.station.key.clone(),
            org: login.station.org.clone(),
            user: login.user.clone(),
            token: login.token.clone(),
            vault_key: Some(vault.to_hex()),
            ..Default::default()
        };
        let r = AccountRef::of(&account);
        self.update_settings(|s| {
            s.accounts.retain(|a| !r.matches(a));
            s.accounts.push(account);
        });
        self.sync_account(&r).await?;
        Ok(r)
    }

    /// Signs out here and on the station (best effort: the sign-in is
    /// forgotten here either way).
    pub async fn sign_out(&self, r: &AccountRef) {
        if let Ok(a) = self.account(r)
            && let Ok(dc) = Self::station_client(&a)
        {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), dc.logout(&a.token)).await;
        }
        self.update_settings(|s| s.accounts.retain(|a| !r.matches(a)));
    }

    /// Fetches the account's lists afresh.
    pub async fn sync_account(&self, r: &AccountRef) -> Result<()> {
        let a = self.account(r)?;
        let state = Self::station_client(&a)?.account(&a.token).await.map_err(|e| self.note_error(r, e))?;
        self.apply_state(r, state);
        Ok(())
    }

    pub async fn sync_accounts(&self) {
        let refs: Vec<AccountRef> = self.settings().accounts.iter().filter(|a| !a.signed_out).map(AccountRef::of).collect();
        for r in refs {
            if let Err(e) = self.sync_account(&r).await {
                tracing::debug!(server = %r.server, "account sync: {e:#}");
            }
        }
    }

    /// Adds or updates a device in the technician's list (or, for station
    /// admins, the shared one). `password`: `Some("")` forgets a saved
    /// one, `None` keeps what's there.
    pub async fn save_device(&self, r: &AccountRef, mut device: SavedDevice, password: Option<String>, shared: bool) -> Result<()> {
        let a = self.account(r)?;
        device.group = device.group.trim().chars().take(64).collect();
        if shared && !a.user.admin {
            bail!("only station admins can change the shared devices");
        }
        if !shared {
            device.secret = match password {
                Some(p) if p.is_empty() => None,
                Some(p) => {
                    let key = a.vault_key.as_deref().and_then(VaultKey::from_hex).context("sign in again to save passwords")?;
                    Some(key.seal(device.id, &p))
                }
                None => a.devices.iter().find(|d| d.id == device.id).and_then(|d| d.secret.clone()),
            };
        }
        let dc = Self::station_client(&a)?;
        let state = if shared { dc.save_shared(&a.token, &device).await } else { dc.save_device(&a.token, &device).await };
        let state = state.map_err(|e| self.note_error(r, e))?;
        let list = if shared { &state.shared } else { &state.devices };
        let group_saved = list.iter().find(|d| d.id == device.id).is_some_and(|d| d.group == device.group);
        self.apply_state(r, state);
        if !group_saved { bail!("that station needs an update to save device groups"); }
        Ok(())
    }

    pub async fn delete_device(&self, r: &AccountRef, id: DeviceId, shared: bool) -> Result<()> {
        let a = self.account(r)?;
        let dc = Self::station_client(&a)?;
        let state = if shared { dc.delete_shared(&a.token, id).await } else { dc.delete_device(&a.token, id).await };
        self.apply_state(r, state.map_err(|e| self.note_error(r, e))?);
        Ok(())
    }

    pub async fn save_group(&self, r: &AccountRef, name: &str) -> Result<()> {
        let name: String = name.trim().chars().take(64).collect();
        if name.is_empty() { bail!("Enter a group name"); }
        let a = self.account(r)?;
        let state = Self::station_client(&a)?.save_group(&a.token, &name).await
            .map_err(|e| self.note_error(r, e))?;
        let saved = state.groups.contains(&name);
        self.apply_state(r,state);
        if !saved { bail!("Update the station to create empty groups"); }
        Ok(())
    }

    pub async fn delete_group(&self, r: &AccountRef, name: &str) -> Result<()> {
        let a = self.account(r)?;
        let state = Self::station_client(&a)?.delete_group(&a.token,name).await
            .map_err(|e| self.note_error(r,e))?;
        self.apply_state(r,state);
        Ok(())
    }

    /// The password saved for a device in this account, if any.
    pub fn saved_password(&self, r: &AccountRef, id: DeviceId) -> Result<Option<String>> {
        let a = self.account(r)?;
        let Some(sealed) = a.devices.iter().find(|d| d.id == id).and_then(|d| d.secret.clone()) else { return Ok(None) };
        let key = a.vault_key.as_deref().and_then(VaultKey::from_hex).context("sign in again to use saved passwords")?;
        key.open(id, &sealed).map(Some)
    }

    /// Background status is batched separately from connection lookups.
    /// Older stations retain the legacy lookup path until they are updated.
    pub async fn presence(&self, r: &AccountRef, ids: &[DeviceId]) -> Vec<(DeviceId, Presence)> {
        let Ok(dc) = self.account(r).and_then(|a| Self::station_client(&a)) else { return Vec::new() };
        let ids: Vec<_> = ids.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        match dc.presence(&ids).await {
            Ok(Some(devices)) => return devices.into_iter().map(|device| (device.id, match device.status {
                PresenceStatus::Online => Presence::Online,
                PresenceStatus::Offline | PresenceStatus::NotFound => Presence::Offline,
            })).collect(),
            Err(_) => return ids.into_iter().map(|id| (id, Presence::Unknown)).collect(),
            Ok(None) => {}
        }
        let mut out = Vec::new();
        for id in ids {
            let p = match dc.lookup(id).await {
                Ok(Lookup::Online { .. }) => Presence::Online,
                Ok(_) => Presence::Offline,
                Err(_) => Presence::Unknown,
            };
            out.push((id, p));
        }
        out
    }

    /// Connects to a device on the account's station: with `password`, or
    /// the one saved for it.
    pub async fn connect_via(
        &self,
        r: &AccountRef,
        id: DeviceId,
        password: Option<String>,
        on_progress: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
        let a = self.account(r)?;
        let password = match password.filter(|p| !p.is_empty()) {
            Some(p) => p,
            None => self.saved_password(r, id)?.ok_or_else(|| anyhow!("enter the password for {id}"))?,
        };
        on_progress(format!("Locating {id} on {}…", a.label()));
        let found = Self::station_client(&a)?.lookup(id).await?;
        self.connect_found(id, found, &password, on_progress, Some(a.server)).await
    }
}
