//! The directory JSON API (`POST /api/`). Same protocol as the PHP server
//! in `server/web/` — see `slipstream_core::directory`.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use slipstream_core::directory::{DIRECTORY_VERSION, DeviceId, DeviceInfo, verify_signed};
use tracing::{info, warn};

use crate::{
    App,
    db::{Found, now},
};

/// Max requests per client IP per minute, per kind.
const LOOKUPS_PER_MIN: u32 = 60;
const REGISTERS_PER_MIN: u32 = 30;
/// Signed requests must be this fresh (seconds).
const MAX_SKEW: i64 = 600;

/// (client, kind) → (window start, requests in window)
type Buckets = HashMap<(IpAddr, &'static str), (Instant, u32)>;

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<Buckets>,
}

impl RateLimiter {
    fn allow(&self, ip: IpAddr, kind: &'static str, per_min: u32) -> bool {
        let mut b = self.buckets.lock().unwrap();
        if b.len() > 50_000 {
            b.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(60));
        }
        let e = b.entry((ip, kind)).or_insert((Instant::now(), 0));
        if e.0.elapsed() > Duration::from_secs(60) {
            *e = (Instant::now(), 0);
        }
        e.1 += 1;
        e.1 <= per_min
    }
}

/// Constant-time key comparison.
pub fn same_key(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Appends `?token=KEY` so iroh presents the key to our relay.
fn with_token(url: String, key: Option<&str>) -> String {
    match key {
        Some(k) => format!("{url}?token={k}"),
        None => url,
    }
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": msg }))).into_response()
}

/// The real client IP: the socket peer, or X-Forwarded-For when a local
/// reverse proxy (Apache, Nginx, Caddy) sits in front of us.
pub fn client_ip(peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if peer.ip().is_loopback()
        && let Some(fwd) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(ip) = fwd.split(',').next().and_then(|s| s.trim().parse().ok())
    {
        return ip;
    }
    peer.ip()
}

/// Relay URL as the client sees this server (scheme://host/).
fn relay_url_from_request(peer: SocketAddr, headers: &HeaderMap) -> Option<String> {
    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let behind_proxy = peer.ip().is_loopback();
    let host = if behind_proxy { h("x-forwarded-host").or_else(|| h("host")) } else { h("host") }?;
    let scheme = if behind_proxy { h("x-forwarded-proto").unwrap_or_else(|| "http".into()) } else { "http".into() };
    Some(format!("{scheme}://{host}/"))
}

pub async fn handle(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let Some(Json(req)) = body else { return err(StatusCode::BAD_REQUEST, "expected a JSON body") };
    let ip = client_ip(peer, &headers);
    if let Some(key) = &app.access_key
        && !req.get("key").and_then(Value::as_str).is_some_and(|k| same_key(k, key))
    {
        if !app.limits.allow(ip, "badkey", 20) {
            return err(StatusCode::TOO_MANY_REQUESTS, "too many requests, slow down");
        }
        return err(StatusCode::FORBIDDEN, "this is a private Slipstream server — use the app from its download page");
    }
    match req.get("op").and_then(Value::as_str).unwrap_or("") {
        "hello" => {
            let mut relays: Vec<String> = Vec::new();
            if app.relay_enabled
                && let Some(url) = app.public_url.clone().or_else(|| relay_url_from_request(peer, &headers))
            {
                relays.push(with_token(url, app.access_key.as_deref()));
            }
            relays.extend(app.extra_relays.iter().cloned());
            Json(json!({
                "ok": true,
                "version": DIRECTORY_VERSION,
                "time": now(),
                "relays": relays,
                "motd": app.motd,
                "org": app.org_name,
                "public_relays": app.public_relays,
            }))
            .into_response()
        }
        "lookup" => {
            if !app.limits.allow(ip, "lookup", LOOKUPS_PER_MIN) {
                return err(StatusCode::TOO_MANY_REQUESTS, "too many lookups, slow down");
            }
            let Some(id) = req.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()) else {
                return err(StatusCode::BAD_REQUEST, "missing id");
            };
            match app.db.lookup(DeviceId(id)) {
                Ok(Found::Online { addr_json, info }) => {
                    let addr: Value = serde_json::from_str(&addr_json).unwrap_or(Value::Null);
                    Json(json!({ "ok": true, "status": "online", "addr": addr, "info": info })).into_response()
                }
                Ok(Found::Offline { info }) => Json(json!({ "ok": true, "status": "offline", "info": info })).into_response(),
                Ok(Found::Unknown) => Json(json!({ "ok": true, "status": "not_found" })).into_response(),
                Err(e) => {
                    warn!("lookup failed: {e:#}");
                    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
                }
            }
        }
        op @ ("register" | "unregister") => {
            if !app.limits.allow(ip, "register", REGISTERS_PER_MIN) {
                return err(StatusCode::TOO_MANY_REQUESTS, "too many requests, slow down");
            }
            let (Some(payload), Some(sig)) =
                (req.get("payload").and_then(Value::as_str), req.get("sig").and_then(Value::as_str))
            else {
                return err(StatusCode::BAD_REQUEST, "missing payload or signature");
            };
            let p = match verify_signed(payload, sig) {
                Ok(p) => p,
                Err(e) => return err(StatusCode::FORBIDDEN, &format!("{e}")),
            };
            if p.op != op {
                return err(StatusCode::BAD_REQUEST, "op mismatch");
            }
            if (p.time - now()).abs() > MAX_SKEW {
                return err(StatusCode::FORBIDDEN, "request expired — check this computer's clock");
            }
            if op == "unregister" {
                let _ = app.db.unregister(&p.endpoint);
                return Json(json!({ "ok": true })).into_response();
            }
            let Some(addr) = p.addr else { return err(StatusCode::BAD_REQUEST, "missing addr") };
            let mut info = p.info.unwrap_or_default();
            info = DeviceInfo {
                name: info.name.chars().take(64).collect(),
                os: std::mem::take(&mut info.os).chars().take(32).collect(),
                app_version: std::mem::take(&mut info.app_version).chars().take(32).collect(),
            };
            let addr_json = serde_json::to_string(&addr).unwrap_or_default();
            match app.db.register(&p.endpoint, &info, &addr_json) {
                Ok(id) => {
                    tracing::debug!(%id, name = %info.name, "heartbeat");
                    if app.seen_once(&p.endpoint) {
                        info!(%id, name = %info.name, os = %info.os, "host online");
                    }
                    Json(json!({ "ok": true, "id": id })).into_response()
                }
                Err(e) => {
                    warn!("register failed: {e:#}");
                    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
                }
            }
        }
        _ => err(StatusCode::BAD_REQUEST, "unknown op"),
    }
}
