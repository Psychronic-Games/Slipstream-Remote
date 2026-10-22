//! Embedded iroh relay, served on our own port by proxying `/relay`
//! (WebSocket), `/ping` and `/generate_204` to it.

use std::{net::SocketAddr, sync::Arc};

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
struct KeyAccess(Option<String>);

impl relay::AccessControl for KeyAccess {
    async fn on_connect(&self, req: &relay::ClientRequest) -> relay::Access {
        match &self.0 {
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
        }
    }
}

/// Starts the relay on a private loopback port.
pub async fn spawn(access_key: Option<String>) -> Result<(relay::Server, SocketAddr)> {
    let mut cfg = relay::ServerConfig::default();
    let mut relay_cfg = relay::RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    relay_cfg.access = Arc::new(KeyAccess(access_key));
    cfg.relay = Some(relay_cfg);
    let server = relay::Server::spawn(cfg).await.context("starting relay")?;
    let addr = server.http_addr().context("relay has no HTTP address")?;
    Ok((server, addr))
}

/// Forwards a request (including WebSocket upgrades) to the internal relay.
pub async fn proxy(State(app): State<Arc<App>>, mut req: Request) -> Response {
    let Some(upstream) = app.relay_addr else { return StatusCode::NOT_FOUND.into_response() };
    let client_upgrade = hyper::upgrade::on(&mut req);
    let stream = match tokio::net::TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
        Ok(x) => x,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let path = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
    *req.uri_mut() = path.parse().unwrap_or_default();
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let upstream_upgrade = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            let (Ok(client), Ok(server)) = (client_upgrade.await, upstream_upgrade.await) else { return };
            let mut client = TokioIo::new(client);
            let mut server = TokioIo::new(server);
            let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
        });
    }
    resp.map(Body::new)
}
