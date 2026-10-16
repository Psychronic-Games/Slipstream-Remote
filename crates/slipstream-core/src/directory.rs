//! Directory protocol: hosts register to get a short numeric ID and stay
//! reachable; viewers look IDs up. Spoken over QUIC (ALPN [`ALPN_DIRECTORY`])
//! with one bi-stream per request.

use std::{fmt, net::SocketAddr, str::FromStr};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr, endpoint::Connection};
use serde::{Deserialize, Serialize};

use crate::framing::{expect_msg, write_msg};

pub const ALPN_DIRECTORY: &[u8] = b"slipstream/dir/1";
pub const DIRECTORY_VERSION: u32 = 1;
pub const DEFAULT_DIRECTORY_PORT: u16 = 21116;

/// Short device ID shown to users, e.g. `482 913 077`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(pub u32);

impl DeviceId {
    pub const MIN: u32 = 100_000_000;
    pub const MAX: u32 = 999_999_999;
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0.to_string();
        if s.len() == 9 {
            write!(f, "{} {} {}", &s[0..3], &s[3..6], &s[6..9])
        } else {
            f.write_str(&s)
        }
    }
}

impl FromStr for DeviceId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let digits: String = s.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        let n: u32 = digits.parse().map_err(|_| anyhow!("an ID is 9 digits, like 482 913 077"))?;
        if !(Self::MIN..=Self::MAX).contains(&n) {
            bail!("an ID is 9 digits, like 482 913 077");
        }
        Ok(Self(n))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub os: String,
    pub app_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DirRequest {
    /// Learn the server's relays. Sent first, before binding home relays.
    Hello { version: u32 },
    /// Claim (or re-claim) this endpoint's ID and mark it online for the
    /// lifetime of this QUIC connection.
    Register { addr: EndpointAddr, info: DeviceInfo },
    /// The host's addresses changed (network switch, new relay).
    UpdateAddr { addr: EndpointAddr },
    Lookup { id: DeviceId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DirResponse {
    Welcome { version: u32, relays: Vec<RelayUrl>, motd: Option<String> },
    Registered { id: DeviceId },
    Updated,
    Found { addr: EndpointAddr, info: DeviceInfo },
    Offline { info: DeviceInfo },
    NotFound,
    Error(String),
}

/// Where to find a directory server: `<endpoint-id>@<host>:<port>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerAddress {
    pub endpoint_id: EndpointId,
    pub host: String,
    pub port: u16,
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}:{}", self.endpoint_id, self.host, self.port)
    }
}

impl FromStr for ServerAddress {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let (id, hostport) = s.split_once('@').context("server address must look like <key>@<host>:<port>")?;
        let endpoint_id: EndpointId = id.parse().context("invalid server key")?;
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
                (h.trim_matches(['[', ']']).to_string(), p.parse().context("invalid port")?)
            }
            _ => (hostport.to_string(), DEFAULT_DIRECTORY_PORT),
        };
        if host.is_empty() {
            bail!("missing server host");
        }
        Ok(Self { endpoint_id, host, port })
    }
}

impl ServerAddress {
    pub async fn resolve(&self) -> Result<EndpointAddr> {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("cannot resolve {}", self.host))?
            .collect();
        if addrs.is_empty() {
            bail!("{} has no addresses", self.host);
        }
        Ok(EndpointAddr::from_parts(self.endpoint_id, addrs.into_iter().map(TransportAddr::Ip)))
    }
}

/// A connection to a directory server.
#[derive(Debug, Clone)]
pub struct DirectoryClient {
    conn: Connection,
}

impl DirectoryClient {
    pub async fn connect(endpoint: &Endpoint, server: &ServerAddress) -> Result<Self> {
        let addr = server.resolve().await?;
        let conn = endpoint
            .connect(addr, ALPN_DIRECTORY)
            .await
            .with_context(|| format!("cannot reach server {}", server.host))?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    async fn request(&self, req: &DirRequest) -> Result<DirResponse> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        write_msg(&mut send, req).await?;
        send.finish()?;
        let resp: DirResponse = expect_msg(&mut recv).await?;
        if let DirResponse::Error(e) = resp {
            bail!("server: {e}");
        }
        Ok(resp)
    }

    pub async fn hello(&self) -> Result<(Vec<RelayUrl>, Option<String>)> {
        match self.request(&DirRequest::Hello { version: DIRECTORY_VERSION }).await? {
            DirResponse::Welcome { relays, motd, .. } => Ok((relays, motd)),
            other => bail!("unexpected response {other:?}"),
        }
    }

    pub async fn register(&self, addr: EndpointAddr, info: DeviceInfo) -> Result<DeviceId> {
        match self.request(&DirRequest::Register { addr, info }).await? {
            DirResponse::Registered { id } => Ok(id),
            other => bail!("unexpected response {other:?}"),
        }
    }

    pub async fn update_addr(&self, addr: EndpointAddr) -> Result<()> {
        self.request(&DirRequest::UpdateAddr { addr }).await?;
        Ok(())
    }

    pub async fn lookup(&self, id: DeviceId) -> Result<Lookup> {
        Ok(match self.request(&DirRequest::Lookup { id }).await? {
            DirResponse::Found { addr, info } => Lookup::Online { addr, info },
            DirResponse::Offline { info } => Lookup::Offline { info },
            DirResponse::NotFound => Lookup::NotFound,
            other => bail!("unexpected response {other:?}"),
        })
    }
}

#[derive(Debug, Clone)]
pub enum Lookup {
    Online { addr: EndpointAddr, info: DeviceInfo },
    Offline { info: DeviceInfo },
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_roundtrip() {
        let id: DeviceId = "482 913 077".parse().unwrap();
        assert_eq!(id.0, 482913077);
        assert_eq!(id.to_string(), "482 913 077");
        assert!("12345".parse::<DeviceId>().is_err());
        assert!("482-913-077".parse::<DeviceId>().is_ok());
    }

    #[test]
    fn server_address_parse() {
        let key = iroh::SecretKey::generate().public();
        let a: ServerAddress = format!("{key}@example.com:4000").parse().unwrap();
        assert_eq!(a.host, "example.com");
        assert_eq!(a.port, 4000);
        let b: ServerAddress = format!("{key}@example.com").parse().unwrap();
        assert_eq!(b.port, DEFAULT_DIRECTORY_PORT);
        let c: ServerAddress = format!("{key}@[::1]:9").parse().unwrap();
        assert_eq!(c.host, "::1");
        assert_eq!(a.to_string().parse::<ServerAddress>().unwrap(), a);
    }
}
