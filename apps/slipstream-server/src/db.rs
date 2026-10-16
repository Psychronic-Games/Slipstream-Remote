//! Persistent ID assignments (SQLite).

use std::{path::Path, sync::Mutex};

use anyhow::Result;
use iroh::EndpointId;
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use slipstream_core::directory::{DeviceId, DeviceInfo};

pub struct Db {
    conn: Mutex<Connection>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS devices (
                 endpoint_id BLOB PRIMARY KEY,
                 device_id   INTEGER NOT NULL UNIQUE,
                 name        TEXT NOT NULL DEFAULT '',
                 os          TEXT NOT NULL DEFAULT '',
                 app_version TEXT NOT NULL DEFAULT '',
                 created     INTEGER NOT NULL,
                 last_seen   INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Returns this endpoint's ID, assigning a fresh random one on first sight.
    pub fn register(&self, endpoint: &EndpointId, info: &DeviceInfo) -> Result<DeviceId> {
        let conn = self.conn.lock().unwrap();
        let key = endpoint.as_bytes().as_slice();
        let existing: Option<u32> = conn
            .query_row("SELECT device_id FROM devices WHERE endpoint_id = ?1", [key], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            conn.execute(
                "UPDATE devices SET name = ?2, os = ?3, app_version = ?4, last_seen = ?5 WHERE endpoint_id = ?1",
                params![key, info.name, info.os, info.app_version, now()],
            )?;
            return Ok(DeviceId(id));
        }
        let mut rng = rand::rng();
        loop {
            let id = rng.random_range(DeviceId::MIN..=DeviceId::MAX);
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO devices (endpoint_id, device_id, name, os, app_version, created, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![key, id, info.name, info.os, info.app_version, now()],
            )?;
            if inserted == 1 {
                return Ok(DeviceId(id));
            }
        }
    }

    pub fn lookup(&self, id: DeviceId) -> Result<Option<(EndpointId, DeviceInfo)>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(Vec<u8>, String, String, String)> = conn
            .query_row(
                "SELECT endpoint_id, name, os, app_version FROM devices WHERE device_id = ?1",
                [id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((key, name, os, app_version)) = row else { return Ok(None) };
        let Ok(key) = <[u8; 32]>::try_from(key.as_slice()) else { return Ok(None) };
        let endpoint = EndpointId::from_bytes(&key)?;
        Ok(Some((endpoint, DeviceInfo { name, os, app_version })))
    }

    pub fn device_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_unique() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        let info = DeviceInfo { name: "a".into(), ..Default::default() };
        let id_a = db.register(&a, &info).unwrap();
        assert_eq!(db.register(&a, &info).unwrap(), id_a);
        let id_b = db.register(&b, &info).unwrap();
        assert_ne!(id_a, id_b);
        assert_eq!(db.lookup(id_a).unwrap().unwrap().0, a);
        assert!(db.lookup(DeviceId(99)).unwrap().is_none());
    }
}
