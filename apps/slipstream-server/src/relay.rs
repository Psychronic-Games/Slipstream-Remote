//! Embedded iroh relay, served on our own port by proxying `/relay`
//! (WebSocket), `/ping` and `/generate_204` to it.

use std::{
    collections::HashSet,
    net::SocketAddr,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hyper_util::rt::TokioIo;
use iroh_relay::server as relay;

use crate::App;

/// Admits only clients presenting the station's access key (sent by iroh
/// as `?token=` on the relay URL or an `Authorization: Bearer` header).
#[derive(Debug)]
struct KeyAccess {
    key: Option<String>,
    maximum: usize,
    active: Mutex<HashSet<relay::ConnectionId>>,
}

impl relay::AccessControl for KeyAccess {
    async fn on_connect(&self, req: &relay::ClientRequest) -> relay::Access {
        let allowed = match &self.key {
            None => relay::Access::Allow,
            Some(key)
                if req.auth_token().is_some_and(|t| {
                    crate::api::same_key(&t, key)
                        || slipstream_core::meeting::valid_relay_token(key, &t)
                }) =>
            {
                relay::Access::Allow
            }
            Some(_) => relay::Access::Deny {
                reason: Some("this relay requires an access key".into()),
            },
        };
        if !matches!(allowed, relay::Access::Allow) {
            return allowed;
        }
        let mut active = self.active.lock().unwrap();
        if self.maximum > 0 && active.len() >= self.maximum {
            return relay::Access::Deny {
                reason: Some("relay is at capacity; try again shortly".into()),
            };
        }
        active.insert(req.connection_id());
        relay::Access::Allow
    }
    fn on_disconnect(&self, _: iroh::EndpointId, connection: relay::ConnectionId) {
        self.active.lock().unwrap().remove(&connection);
    }
}

/// Starts the relay on a private loopback port.
pub async fn spawn(
    access_key: Option<String>,
    bytes_per_second: u32,
    max_connections: usize,
) -> Result<(relay::Server, SocketAddr)> {
    let mut cfg = relay::ServerConfig::default();
    let mut relay_cfg = relay::RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    relay_cfg.access = Arc::new(KeyAccess {
        key: access_key,
        maximum: max_connections,
        active: Mutex::new(HashSet::new()),
    });
    relay_cfg.limits.client_rx = NonZeroU32::new(bytes_per_second).map(relay::ClientRateLimit::new);
    cfg.relay = Some(relay_cfg);
    let server = relay::Server::spawn(cfg).await.context("starting relay")?;
    let addr = server.http_addr().context("relay has no HTTP address")?;
    Ok((server, addr))
}

/// Forwards a request (including WebSocket upgrades) to the internal relay.
pub async fn proxy(State(app): State<Arc<App>>, mut req: Request) -> Response {
    let Some(upstream) = app.relay_addr else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let client_upgrade = hyper::upgrade::on(&mut req);
    let stream = match tokio::net::TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
    {
        Ok(x) => x,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    *req.uri_mut() = path.parse().unwrap_or_default();
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let upstream_upgrade = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            let (Ok(client), Ok(server)) = (client_upgrade.await, upstream_upgrade.await) else {
                return;
            };
            let mut client = TokioIo::new(client);
            let mut server = TokioIo::new(server);
            let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
        });
    }
    resp.map(Body::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay::AccessControl;

    fn request(token: &str) -> relay::ClientRequest {
        let (parts, _) = axum::http::Request::builder()
            .uri(format!("/relay?token={token}"))
            .body(())
            .unwrap()
            .into_parts();
        relay::ClientRequest::new(
            iroh::SecretKey::generate().public(),
            iroh_relay::http::ProtocolVersion::V2,
            parts,
        )
    }
    #[tokio::test]
    async fn capacity_preserves_key_checks_and_releases_slots() {
        let access = KeyAccess {
            key: Some("correct".into()),
            maximum: 1,
            active: Mutex::new(HashSet::new()),
        };
        let denied = request("wrong");
        assert!(matches!(
            access.on_connect(&denied).await,
            relay::Access::Deny { .. }
        ));
        let first = request("correct");
        let second = request("correct");
        assert!(matches!(
            access.on_connect(&first).await,
            relay::Access::Allow
        ));
        assert!(matches!(
            access.on_connect(&second).await,
            relay::Access::Deny { .. }
        ));
        access.on_disconnect(first.endpoint_id(), first.connection_id());
        assert!(matches!(
            access.on_connect(&second).await,
            relay::Access::Allow
        ));
        access.on_disconnect(first.endpoint_id(), first.connection_id());
        assert_eq!(access.active.lock().unwrap().len(), 1);
        access.on_disconnect(second.endpoint_id(), second.connection_id());
        assert!(access.active.lock().unwrap().is_empty());
    }
}
