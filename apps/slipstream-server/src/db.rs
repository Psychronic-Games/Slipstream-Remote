//! Device registry (SQLite).

use std::{path::Path, sync::Mutex};

use anyhow::Result;
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use slipstream_core::directory::{DeviceId, DeviceInfo, ONLINE_WINDOW_SECS};

pub struct Db {
    conn: Mutex<Connection>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub enum Found {
    Online { addr_json: String, info: DeviceInfo },
    Offline { info: DeviceInfo },
    Unknown,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS devices (
                 endpoint    TEXT PRIMARY KEY,
                 device_id   INTEGER NOT NULL UNIQUE,
                 name        TEXT NOT NULL DEFAULT '',
                 os          TEXT NOT NULL DEFAULT '',
                 app_version TEXT NOT NULL DEFAULT '',
                 addr        TEXT,
                 created     INTEGER NOT NULL,
                 last_seen   INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Records a heartbeat; returns the device's ID (assigning one if new).
    pub fn register(&self, endpoint: &str, info: &DeviceInfo, addr_json: &str) -> Result<DeviceId> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<u32> = conn
            .query_row("SELECT device_id FROM devices WHERE endpoint = ?1", [endpoint], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            conn.execute(
                "UPDATE devices SET name = ?2, os = ?3, app_version = ?4, addr = ?5, last_seen = ?6 WHERE endpoint = ?1",
                params![endpoint, info.name, info.os, info.app_version, addr_json, now()],
            )?;
            return Ok(DeviceId(id));
        }
        let mut rng = rand::rng();
        loop {
            let id = rng.random_range(DeviceId::MIN..=DeviceId::MAX);
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO devices (endpoint, device_id, name, os, app_version, addr, created, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![endpoint, id, info.name, info.os, info.app_version, addr_json, now()],
            )?;
            if inserted == 1 {
                return Ok(DeviceId(id));
            }
        }
    }

    pub fn unregister(&self, endpoint: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE devices SET last_seen = 0 WHERE endpoint = ?1", [endpoint])?;
        Ok(())
    }

    pub fn lookup(&self, id: DeviceId) -> Result<Found> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(String, String, String, Option<String>, i64)> = conn
            .query_row(
                "SELECT name, os, app_version, addr, last_seen FROM devices WHERE device_id = ?1",
                [id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((name, os, app_version, addr, last_seen)) = row else { return Ok(Found::Unknown) };
        let info = DeviceInfo { name, os, app_version };
        Ok(match addr {
            Some(addr_json) if now() - last_seen <= ONLINE_WINDOW_SECS as i64 => Found::Online { addr_json, info },
            _ => Found::Offline { info },
        })
    }

    /// (known devices, online now)
    /// Private relay diagnostics: identify an authenticated endpoint without reading vaults.
    pub fn describe_endpoint(&self, endpoint: &str) -> Result<Option<(u32, String)>> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT device_id, name FROM devices WHERE endpoint = ?1", [endpoint],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?)
    }

    pub fn counts(&self) -> Result<(i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let total = conn.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0))?;
        let online = conn.query_row(
            "SELECT COUNT(*) FROM devices WHERE last_seen >= ?1",
            [now() - ONLINE_WINDOW_SECS as i64],
            |r| r.get(0),
        )?;
        Ok((total, online))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_presence_works() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let info = DeviceInfo { name: "a".into(), ..Default::default() };
        let id_a = db.register("aa", &info, "{}").unwrap();
        assert_eq!(db.register("aa", &info, "{}").unwrap(), id_a);
        let id_b = db.register("bb", &info, "{}").unwrap();
        assert_ne!(id_a, id_b);
        assert!(matches!(db.lookup(id_a).unwrap(), Found::Online { .. }));
        db.unregister("aa").unwrap();
        assert!(matches!(db.lookup(id_a).unwrap(), Found::Offline { .. }));
        assert!(matches!(db.lookup(DeviceId(99)).unwrap(), Found::Unknown));
        assert_eq!(db.counts().unwrap(), (2, 1));
    }
}
