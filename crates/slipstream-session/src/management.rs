//! Reaching a device beyond its operating system, through another Slipstream
//! computer on its network: Wake-on-LAN from there, and the way to its
//! management controller (a server BMC's Redfish API and web console, Intel
//! vPro's web page). See `slipstream_core::account::Management`.
//!
//! Controllers stay on their local network: their traffic crosses the
//! internet only inside the gateway's encrypted link, via a port forward on
//! this computer's loopback address. Their certificates are self-signed
//! (they can't be verified), so this client accepts them; only that local
//! forward reaches it.

use std::{net::Ipv4Addr, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use slipstream_core::{
    account::{Controller, ControllerKind},
    directory::DeviceId,
    vault::{SealedSecret, VaultKey},
};
use tokio::sync::mpsc;

use crate::{AccountRef, Client, LinkMode, ViewerEvent, ViewerSession, tunnel::Tunnel};

/// What a management controller is asked to do with the computer's power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerPower {
    On,
    Shutdown,
    ForceOff,
    Restart,
    PowerCycle,
}

impl ControllerPower {
    pub const ALL: [Self; 5] = [Self::On, Self::Shutdown, Self::Restart, Self::PowerCycle, Self::ForceOff];

    pub fn label(self) -> &'static str {
        match self {
            Self::On => "Power on",
            Self::Shutdown => "Shut down",
            Self::ForceOff => "Force power off",
            Self::Restart => "Restart",
            Self::PowerCycle => "Power cycle",
        }
    }

    /// Redfish reset types, preferred first.
    fn reset_types(self) -> &'static [&'static str] {
        match self {
            Self::On => &["On", "ForceOn"],
            Self::Shutdown => &["GracefulShutdown"],
            Self::ForceOff => &["ForceOff"],
            Self::Restart => &["GracefulRestart", "ForceRestart"],
            Self::PowerCycle => &["PowerCycle", "ForceRestart"],
        }
    }
}

impl Client {
    /// A chat-only link to a saved device with its saved password, for tools
    /// that need no picture (a gateway, a restart).
    pub async fn tool_link(
        &self,
        r: &AccountRef,
        id: DeviceId,
        progress: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
        self.connect_via_with(r, id, None, LinkMode::ChatOnly, progress).await
    }

    /// Seals a secret for `id` with the account's vault key (as saved
    /// device passwords are).
    pub fn seal_secret(&self, r: &AccountRef, id: DeviceId, secret: &str) -> Result<SealedSecret> {
        Ok(self.vault(r)?.seal(id, secret))
    }

    pub fn open_secret(&self, r: &AccountRef, id: DeviceId, sealed: &SealedSecret) -> Result<String> {
        self.vault(r)?.open(id, sealed)
    }

    fn vault(&self, r: &AccountRef) -> Result<VaultKey> {
        let a = self.account(r)?;
        a.vault_key.as_deref().and_then(VaultKey::from_hex).context("sign in again to use saved passwords")
    }
}

/// Sends Wake-on-LAN packets for `mac` from each of the gateway's local
/// network adapters. Returns how many adapters sent them.
pub async fn wake_via(gateway: &ViewerSession, mac: &str) -> Result<usize> {
    let mac = slipstream_core::account::normalize_mac(mac).context("enter the Wake-on-LAN address like 00:11:22:33:44:55")?;
    let fs = gateway.fs();
    let interfaces = fs.network_interfaces().await.context("listing the gateway's network adapters")?;
    let (mut sent, mut last) = (0, None);
    for interface in interfaces.iter().filter(|i| local(i.address)) {
        match fs.wake_on_lan(interface.address, mac.clone()).await {
            Ok(()) => sent += 1,
            Err(e) => last = Some(e),
        }
    }
    match (sent, last) {
        (0, Some(e)) => Err(e.context("the gateway couldn't send the wake packet")),
        (0, None) => bail!("the gateway has no local network adapter to wake from"),
        (n, _) => Ok(n),
    }
}

fn local(address: Ipv4Addr) -> bool {
    !address.is_loopback() && !address.is_link_local() && !address.is_unspecified()
}

/// A management controller's web page, forwarded to this computer: open
/// `url` in a browser while this is kept.
pub struct Console {
    pub url: String,
    _tunnel: Tunnel,
}

/// Forwards a local port to the controller through the gateway.
pub async fn console(gateway: &ViewerSession, controller: &Controller) -> Result<Console> {
    let (tunnel, base) = forward(gateway, controller).await?;
    let path = match controller.kind {
        ControllerKind::IntelAmt | ControllerKind::Redfish | ControllerKind::Web => "/",
    };
    Ok(Console { url: format!("{base}{path}"), _tunnel: tunnel })
}

async fn forward(gateway: &ViewerSession, controller: &Controller) -> Result<(Tunnel, String)> {
    let (host, port, tls) = controller.target().context("the management controller's address isn't valid")?;
    let tunnel = gateway.tunnel(0, host, port).await?;
    let base = format!("{}://127.0.0.1:{}", if tls { "https" } else { "http" }, tunnel.local_port);
    Ok((tunnel, base))
}

/// Asks a Redfish controller to change the computer's power, through the
/// gateway. Returns a short report ("Power on sent (it was off)").
pub async fn controller_power(gateway: &ViewerSession, controller: &Controller, password: &str, action: ControllerPower) -> Result<String> {
    if controller.kind != ControllerKind::Redfish {
        bail!("Power control works with Redfish controllers; use the management console for this one");
    }
    let (_tunnel, base) = forward(gateway, controller).await?;
    let redfish = Redfish::new(base, &controller.username, password)?;
    let system = redfish.system().await?;
    let before = system["PowerState"].as_str().unwrap_or("unknown").to_string();
    let reset = &system["Actions"]["#ComputerSystem.Reset"];
    let target = reset["target"].as_str().map(str::to_string)
        .unwrap_or_else(|| format!("{}/Actions/ComputerSystem.Reset", system["@odata.id"].as_str().unwrap_or("").trim_end_matches('/')));
    let allowed: Vec<&str> = reset["ResetType@Redfish.AllowableValues"].as_array()
        .map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let kind = action.reset_types().iter().find(|t| allowed.is_empty() || allowed.contains(t))
        .with_context(|| format!("this controller doesn't offer {} (it offers {})", action.label().to_lowercase(), allowed.join(", ")))?;
    redfish.post(&target, json!({ "ResetType": kind })).await?;
    Ok(format!("{} sent ({kind}; the computer was {})", action.label(), before.to_lowercase()))
}

/// The computer's power state as its Redfish controller reports it.
pub async fn controller_state(gateway: &ViewerSession, controller: &Controller, password: &str) -> Result<String> {
    if controller.kind != ControllerKind::Redfish {
        bail!("Power state needs a Redfish controller");
    }
    let (_tunnel, base) = forward(gateway, controller).await?;
    let system = Redfish::new(base, &controller.username, password)?.system().await?;
    Ok(system["PowerState"].as_str().unwrap_or("unknown").to_string())
}

struct Redfish {
    http: reqwest::Client,
    base: String,
    user: String,
    password: String,
}

impl Redfish {
    fn new(base: String, user: &str, password: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            // Self-signed, and reached only through the local forward.
            .tls_backend_preconfigured(slipstream_core::clock::unverified_tls()?)
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http, base, user: user.into(), password: password.into() })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let response = self.http.get(format!("{}{path}", self.base)).basic_auth(&self.user, Some(&self.password)).send().await
            .map_err(|e| anyhow!("can't reach the controller through the gateway: {e}"))?;
        Self::check(response).await?.json().await.context("the controller's answer isn't Redfish")
    }

    async fn post(&self, path: &str, body: Value) -> Result<()> {
        let response = self.http.post(format!("{}{path}", self.base)).basic_auth(&self.user, Some(&self.password)).json(&body).send().await
            .map_err(|e| anyhow!("can't reach the controller through the gateway: {e}"))?;
        Self::check(response).await.map(drop)
    }

    async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
        match response.status().as_u16() {
            200..=299 => Ok(response),
            401 | 403 => bail!("the controller refused the username or password"),
            404 => bail!("the controller has no Redfish API at this address"),
            code => {
                let text = response.text().await.unwrap_or_default();
                let message = serde_json::from_str::<Value>(&text).ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("HTTP {code}"));
                bail!("the controller answered: {message}")
            }
        }
    }

    /// The (first) computer the controller manages.
    async fn system(&self) -> Result<Value> {
        let systems = self.get("/redfish/v1/Systems").await?;
        let path = systems["Members"].get(0).and_then(|m| m["@odata.id"].as_str())
            .context("the controller lists no computer")?.to_string();
        if !path.starts_with('/') {
            bail!("the controller gave an unexpected address");
        }
        self.get(&path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_actions_prefer_graceful_resets() {
        assert_eq!(ControllerPower::Restart.reset_types()[0], "GracefulRestart");
        assert_eq!(ControllerPower::PowerCycle.reset_types(), &["PowerCycle", "ForceRestart"]);
        assert!(local(Ipv4Addr::new(192, 168, 1, 10)));
        assert!(!local(Ipv4Addr::LOCALHOST));
        assert!(!local(Ipv4Addr::new(169, 254, 1, 1)));
    }
}
