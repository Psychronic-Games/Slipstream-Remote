//! Embedded iroh relay, served on our own port by proxying `/relay`
//! (WebSocket), `/ping` and `/generate_204` to it.

use std::{
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    sync::Arc,
};

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use hyper_util::rt::TokioIo;
use iroh_relay::server as relay;

use crate::{
    App,
    relay_log::{Identity, RelayLog, TUNNEL_HEADER},
};

/// Admits only clients presenting the station's access key (sent by iroh
/// as `?token=` on the relay URL or an `Authorization: Bearer` header).
struct KeyAccess {
    key: Option<String>,
    maximum: usize,
    log: Arc<RelayLog>,
}

impl std::fmt::Debug for KeyAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyAccess")
            .field("private", &self.key.is_some())
            .field("maximum", &self.maximum)
            .finish_non_exhaustive()
    }
}
impl relay::AccessControl for KeyAccess {
    async fn on_connect(&self, req: &relay::ClientRequest) -> relay::Access {
        let endpoint = req.endpoint_id().to_string();
        let connection = req.connection_id().to_string();
        let tunnel = req
            .headers()
            .get(TUNNEL_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if let Some(key) = &self.key
            && !req.auth_token().is_some_and(|t| {
                crate::api::same_key(&t, key)
                    || slipstream_core::meeting::valid_relay_token(key, &t)
            })
        {
            self.log.deny(
                &endpoint,
                &connection,
                tunnel,
                "access_key_missing_or_invalid",
                self.maximum,
            );
            return relay::Access::Deny {
                reason: Some("this relay requires an access key".into()),
            };
        }
        // Resolve before reserving capacity: cancellation during file/DB I/O
        // must not leak a slot before upstream creates its disconnect guard.
        let host = self.log.identity(endpoint.clone()).await;
        let identity = Identity {
            endpoint: endpoint.clone(),
            connection: connection.clone(),
            protocol: format!("{:?}", req.protocol_version()),
            device_id: host.as_ref().map(|h| h.0),
            name: host.map(|h| h.1),
        };
        if self.log.admit(identity, tunnel, self.maximum).is_err() {
            self.log.deny(
                &endpoint,
                &connection,
                tunnel,
                "connection_capacity_reached",
                self.maximum,
            );
            return relay::Access::Deny {
                reason: Some("relay is at capacity; try again shortly".into()),
            };
        }
        relay::Access::Allow
    }
    fn on_disconnect(&self, _: iroh::EndpointId, connection: relay::ConnectionId) {
        self.log.disconnected(&connection.to_string());
    }
}

/// Starts the relay on a private loopback port.
pub async fn spawn(
    access_key: Option<String>,
    bytes_per_second: u32,
    max_connections: usize,
    log: Arc<RelayLog>,
) -> Result<(relay::Server, SocketAddr)> {
    let mut cfg = relay::ServerConfig::default();
    let mut relay_cfg = relay::RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    relay_cfg.access = Arc::new(KeyAccess {
        key: access_key,
        maximum: max_connections,
        log,
    });
    relay_cfg.limits.client_rx = NonZeroU32::new(bytes_per_second).map(relay::ClientRateLimit::new);
    cfg.relay = Some(relay_cfg);
    let server = relay::Server::spawn(cfg).await.context("starting relay")?;
    let addr = server.http_addr().context("relay has no HTTP address")?;
    Ok((server, addr))
}

/// Forwards a request (including WebSocket upgrades) to the internal relay.
/// Trust the last proxy-added address only when the direct socket is loopback.
/// A directly connected remote client cannot choose its logged IP via a header.
fn peer_address(req: &Request) -> Option<IpAddr> {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>()?.0.ip();
    if peer.is_loopback() {
        req.headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
            .or(Some(peer))
    } else {
        Some(peer)
    }
}

pub async fn proxy(State(app): State<Arc<App>>, mut req: Request) -> Response {
    let path = req.uri().path().to_owned(); // Never log the query: it may carry an access key.
    let tunnel = (path == "/relay").then(|| app.relay_log.begin(peer_address(&req)));
    // Replace external correlation headers, including when no ConnectInfo exists.
    req.headers_mut().remove(TUNNEL_HEADER);
    if let Some(tunnel) = tunnel {
        req.headers_mut().insert(
            TUNNEL_HEADER,
            HeaderValue::from_str(&tunnel.to_string()).unwrap(),
        );
    }
    let Some(upstream) = app.relay_addr else {
        app.relay_log
            .request_failed(tunnel, &path, "relay_disabled", None, Some(404));
        return StatusCode::NOT_FOUND.into_response();
    };
    let client_upgrade = hyper::upgrade::on(&mut req);
    let stream = match tokio::net::TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(e) => {
            app.relay_log.request_failed(
                tunnel,
                &path,
                "upstream_connect_failed",
                Some(e.to_string()),
                Some(502),
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
    {
        Ok(x) => x,
        Err(e) => {
            app.relay_log.request_failed(
                tunnel,
                &path,
                "upstream_http_handshake_failed",
                Some(e.to_string()),
                Some(502),
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::warn!(target: "slipstream_relay", event="relay_upstream_http_failed", tunnel, error=%e, "Relay upstream HTTP connection failed");
        }
    });
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    *req.uri_mut() = path_and_query.parse().unwrap_or_default();
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => {
            app.relay_log.request_failed(
                tunnel,
                &path,
                "upstream_request_failed",
                Some(e.to_string()),
                Some(502),
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let upstream_upgrade = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            let (client, server) = tokio::join!(client_upgrade, upstream_upgrade);
            let (client, server) = match (client, server) {
                (Ok(c), Ok(s)) => (c, s),
                (Err(e), _) => {
                    app.relay_log.request_failed(
                        tunnel,
                        &path,
                        "client_upgrade_failed",
                        Some(e.to_string()),
                        None,
                    );
                    return;
                }
                (_, Err(e)) => {
                    app.relay_log.request_failed(
                        tunnel,
                        &path,
                        "upstream_upgrade_failed",
                        Some(e.to_string()),
                        None,
                    );
                    return;
                }
            };
            let mut client = TokioIo::new(client);
            let mut server = TokioIo::new(server);
            let result = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            drop(client);
            drop(server);
            if let Some(tunnel) = tunnel {
                app.relay_log.finish(tunnel, result).await;
            }
        });
    } else if tunnel.is_some() {
        app.relay_log.request_failed(
            tunnel,
            &path,
            "websocket_upgrade_rejected",
            None,
            Some(resp.status().as_u16()),
        );
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
    #[test]
    fn peer_addresses_use_the_socket_or_last_trusted_proxy_hop() {
        let request = |socket: &str, forwarded: &str| {
            Request::builder()
                .uri("/relay?token=never-log-this")
                .extension(ConnectInfo(socket.parse::<SocketAddr>().unwrap()))
                .header("x-forwarded-for", forwarded)
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            peer_address(&request("192.0.2.5:1234", "198.51.100.99")),
            Some("192.0.2.5".parse().unwrap())
        );
        assert_eq!(
            peer_address(&request("127.0.0.1:1234", "198.51.100.99, 192.0.2.5")),
            Some("192.0.2.5".parse().unwrap())
        );
        assert_eq!(
            peer_address(&request("127.0.0.1:1234", "not an address")),
            Some("127.0.0.1".parse().unwrap())
        );
    }
    #[tokio::test]
    async fn capacity_preserves_key_checks_and_releases_slots() {
        let access = KeyAccess {
            key: Some("correct".into()),
            maximum: 1,
            log: Arc::new(RelayLog::default()),
        };
        let capture = crate::relay_log::tests::Capture::default();
        let _guard = tracing::subscriber::set_default(crate::relay_log::tests::subscriber(
            access.log.clone(),
            capture.clone(),
        ));
        assert!(!format!("{access:?}").contains("correct"));
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
        assert_eq!(access.log.active_count(), 1);
        access.on_disconnect(second.endpoint_id(), second.connection_id());
        assert_eq!(access.log.active_count(), 0);
        let text = capture.text();
        assert!(
            text.contains("access_key_missing_or_invalid")
                && text.contains("connection_capacity_reached")
        );
        assert!(!text.contains("correct") && !text.contains("wrong"));
    }
}
