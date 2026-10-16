//! iroh endpoint construction.

use std::sync::Arc;

use anyhow::Result;
use iroh::{Endpoint, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey, endpoint::presets};

use crate::{config::ServerSettings, proto::ALPN_REMOTE};

/// Builds the relay map: the server's relays, plus n0's public relays if enabled.
pub fn relay_map(server: &ServerSettings) -> RelayMap {
    let map = if server.public_relays_enabled() {
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
    // Relay TLS uses our clock-corrected verifier too (see `clock`).
    let ca = iroh::tls::CaTlsConfig::custom_server_cert_verifier(Arc::new(|provider| {
        crate::clock::verifier(provider).map_err(std::io::Error::other)
    }));
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(alpns)
        .relay_mode(mode)
        .ca_tls_config(ca);
    // Troubleshooting aid: force every connection through the relay.
    if std::env::var_os("SLIPSTREAM_RELAY_ONLY").is_some() {
        tracing::warn!("SLIPSTREAM_RELAY_ONLY set: direct connections disabled");
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder.bind().await?;
    Ok(endpoint)
}

/// Removes the public n0 relays (a private station forbade them).
pub async fn remove_public_relays(endpoint: &Endpoint) {
    for url in RelayMode::Default.relay_map().urls::<Vec<_>>() {
        endpoint.remove_relay(&url).await;
    }
}

/// Adds relays the directory server told us about at runtime.
pub async fn add_relays(endpoint: &Endpoint, relays: &[RelayUrl]) {
    for url in relays {
        endpoint.insert_relay(url.clone(), Arc::new(RelayConfig::from(url.clone()))).await;
    }
}
