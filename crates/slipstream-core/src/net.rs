//! iroh endpoint construction.

use std::sync::Arc;

use anyhow::Result;
use iroh::{Endpoint, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey, endpoint::presets};

use crate::{config::ServerSettings, proto::ALPN_REMOTE};

/// Builds the relay map: the server's relays, plus n0's public relays if enabled.
pub fn relay_map(server: &ServerSettings) -> RelayMap {
    let map = if server.use_public_relays {
        RelayMode::Default.relay_map()
    } else {
        RelayMap::empty()
    };
    for url in &server.relays {
        map.insert(url.clone(), Arc::new(RelayConfig::from(url.clone())));
    }
    map
}

/// Binds the endpoint every Slipstream client uses, both to host and to view.
pub async fn bind_client(secret: SecretKey, server: &ServerSettings, accept_sessions: bool) -> Result<Endpoint> {
    let map = relay_map(server);
    let mode = if map.is_empty() { RelayMode::Disabled } else { RelayMode::Custom(map) };
    let alpns = if accept_sessions { vec![ALPN_REMOTE.to_vec()] } else { vec![] };
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(alpns)
        .relay_mode(mode)
        .bind()
        .await?;
    Ok(endpoint)
}

/// Adds relays the directory server told us about at runtime.
pub async fn add_relays(endpoint: &Endpoint, relays: &[RelayUrl]) {
    for url in relays {
        endpoint.insert_relay(url.clone(), Arc::new(RelayConfig::from(url.clone()))).await;
    }
}
