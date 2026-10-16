//! Directory service: ID registration, presence and lookup.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use iroh::{EndpointAddr, EndpointId, RelayUrl, endpoint::Connection};
use slipstream_core::{
    directory::{DIRECTORY_VERSION, DeviceId, DeviceInfo, DirRequest, DirResponse},
    framing::{read_msg, write_msg},
};
use tracing::{debug, info, warn};

use crate::db::Db;

struct Online {
    device_id: DeviceId,
    addr: EndpointAddr,
    info: DeviceInfo,
    conn_id: usize,
}

pub struct Directory {
    db: Db,
    online: Mutex<HashMap<EndpointId, Online>>,
    relays: Vec<RelayUrl>,
    motd: Option<String>,
}

/// Lookups allowed per connection per minute (slows ID scanning).
const LOOKUPS_PER_MINUTE: u32 = 30;

impl Directory {
    pub fn new(db: Db, relays: Vec<RelayUrl>, motd: Option<String>) -> Self {
        Self { db, online: Mutex::new(HashMap::new()), relays, motd }
    }

    pub fn online_count(&self) -> usize {
        self.online.lock().unwrap().len()
    }

    pub async fn handle_connection(self: Arc<Self>, conn: Connection) {
        let peer = conn.remote_id();
        let conn_id = conn.stable_id();
        debug!(%peer, "directory connection");
        let mut window = (Instant::now(), 0u32);
        loop {
            let (mut send, mut recv) = match conn.accept_bi().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let req: DirRequest = match read_msg(&mut recv).await {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(e) => {
                    debug!(%peer, "bad request: {e:#}");
                    break;
                }
            };
            if matches!(req, DirRequest::Lookup { .. }) {
                if window.0.elapsed() > Duration::from_secs(60) {
                    window = (Instant::now(), 0);
                }
                window.1 += 1;
                if window.1 > LOOKUPS_PER_MINUTE {
                    let _ = write_msg(&mut send, &DirResponse::Error("too many lookups, slow down".into())).await;
                    let _ = send.finish();
                    continue;
                }
            }
            let resp = self.handle(peer, conn_id, req).unwrap_or_else(|e| {
                warn!(%peer, "request failed: {e:#}");
                DirResponse::Error("internal server error".into())
            });
            if write_msg(&mut send, &resp).await.is_ok() {
                let _ = send.finish();
            }
        }
        let mut online = self.online.lock().unwrap();
        if online.get(&peer).is_some_and(|o| o.conn_id == conn_id) {
            let o = online.remove(&peer).unwrap();
            info!(id = %o.device_id, name = %o.info.name, "host offline");
        }
    }

    fn handle(&self, peer: EndpointId, conn_id: usize, req: DirRequest) -> Result<DirResponse> {
        Ok(match req {
            DirRequest::Hello { version } => {
                if version != DIRECTORY_VERSION {
                    return Ok(DirResponse::Error(format!(
                        "server speaks directory protocol v{DIRECTORY_VERSION}, client v{version}; please update"
                    )));
                }
                DirResponse::Welcome {
                    version: DIRECTORY_VERSION,
                    relays: self.relays.clone(),
                    motd: self.motd.clone(),
                }
            }
            DirRequest::Register { addr, mut info } => {
                if addr.id != peer {
                    return Ok(DirResponse::Error("address does not belong to this connection".into()));
                }
                info.name.truncate(64);
                info.os.truncate(32);
                info.app_version.truncate(32);
                let device_id = self.db.register(&peer, &info)?;
                info!(id = %device_id, name = %info.name, os = %info.os, "host online");
                self.online
                    .lock()
                    .unwrap()
                    .insert(peer, Online { device_id, addr, info, conn_id });
                DirResponse::Registered { id: device_id }
            }
            DirRequest::UpdateAddr { addr } => {
                if addr.id != peer {
                    return Ok(DirResponse::Error("address does not belong to this connection".into()));
                }
                match self.online.lock().unwrap().get_mut(&peer) {
                    Some(o) if o.conn_id == conn_id => {
                        o.addr = addr;
                        DirResponse::Updated
                    }
                    _ => DirResponse::Error("not registered".into()),
                }
            }
            DirRequest::Lookup { id } => match self.db.lookup(id)? {
                None => DirResponse::NotFound,
                Some((endpoint, info)) => match self.online.lock().unwrap().get(&endpoint) {
                    Some(o) => DirResponse::Found { addr: o.addr.clone(), info: o.info.clone() },
                    None => DirResponse::Offline { info },
                },
            },
        })
    }
}
