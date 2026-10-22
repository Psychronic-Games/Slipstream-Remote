//! iroh endpoint construction.

use std::sync::Arc;

use anyhow::Result;
use iroh::{Endpoint, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey, endpoint::presets};

use crate::{config::ServerSettings, proto::ALPN_REMOTE};

/// Station relays, with Slipstream's public relay as an optional backup.
pub fn relay_map(server: &ServerSettings) -> RelayMap {
    let map = if server.public_relays_enabled() {
        RelayMap::from_iter([RelayConfig::from(crate::config::PUBLIC_STATION_URL.parse::<RelayUrl>().expect("public relay URL"))])
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
    let mode = if map.is_empty() {
        RelayMode::Disabled
    } else {
        RelayMode::Custom(map)
    };
    let alpns = if accept_sessions {
        vec![ALPN_REMOTE.to_vec(), crate::meeting::CALL_ALPN.to_vec()]
    } else {
        vec![]
    };
    // Relay TLS uses our clock-corrected verifier too (see `clock`).
    let ca = iroh::tls::CaTlsConfig::custom_server_cert_verifier(Arc::new(|provider| {
        crate::clock::verifier(provider).map_err(std::io::Error::other)
    }));
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(alpns)
        .relay_mode(mode)
        .ca_tls_config(ca);
    // Some Android UDP stacks advertise segmentation offload but stall
    // when a connection first sends a batch. Use individual datagrams on
    // phones to avoid that first-keyframe fallback failure (iroh issue 4555).
    if cfg!(target_os = "android") || std::env::var_os("SLIPSTREAM_DISABLE_GSO").is_some() {
        builder = builder.transport_config(iroh::endpoint::QuicTransportConfig::builder().enable_segmentation_offload(false).build());
        tracing::info!("UDP segmentation offload disabled for compatibility");
    }
    // Troubleshooting aid: force every connection through the relay.
    if std::env::var_os("SLIPSTREAM_RELAY_ONLY").is_some() {
        tracing::warn!("SLIPSTREAM_RELAY_ONLY set: direct connections disabled");
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder.bind().await?;
    Ok(endpoint)
}

/// Removes the Slipstream public backup when a private station forbids it.
pub async fn remove_public_relays(endpoint: &Endpoint) {
    endpoint.remove_relay(&crate::config::PUBLIC_STATION_URL.parse().expect("public relay URL")).await;
}

/// Adds relays the directory server told us about at runtime.
pub async fn add_relays(endpoint: &Endpoint, relays: &[RelayUrl]) {
    for url in relays {
        endpoint.insert_relay(url.clone(), Arc::new(RelayConfig::from(url.clone()))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backup_is_slipstream_and_public_identity_has_no_private_routes() {
        let private: RelayUrl = "https://private.example/?token=private-key".parse().unwrap();
        let settings = ServerSettings { relays: vec![private.clone()], ..Default::default() };
        let map = relay_map(&settings);
        assert_eq!(map.urls::<Vec<_>>().len(), 2);
        assert!(map.urls::<Vec<_>>().contains(&private));
        assert!(map.urls::<Vec<_>>().iter().any(|url| crate::config::is_public_station(url.as_str())));
        let public = relay_map(&ServerSettings::public_station());
        assert_eq!(public.urls::<Vec<_>>().len(), 1);
        assert!(crate::config::is_public_station(public.urls::<Vec<_>>()[0].as_str()));
        let secret = SecretKey::generate();
        assert_ne!(crate::config::public_identity(&secret).public(), secret.public());
        assert_eq!(crate::config::public_identity(&secret).public(), crate::config::public_identity(&secret).public());
    }
}
