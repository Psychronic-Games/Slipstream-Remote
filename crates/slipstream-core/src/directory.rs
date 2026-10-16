//! Directory protocol: hosts register to get a short numeric ID and stay
//! findable; viewers look IDs up.
//!
//! It's plain JSON over HTTPS so a server can be a PHP folder on any web
//! host or the `slipstream-server` binary — both speak the same API:
//!
//! ```text
//! POST {server}/api/   {"op":"hello","version":2}
//!                   →  {"ok":true,"version":2,"time":…,"relays":[…],"motd":…}
//! POST {server}/api/   {"op":"register","payload":"<json>","sig":"<hex>"}
//!                   →  {"ok":true,"id":482913077}
//! POST {server}/api/   {"op":"unregister","payload":"<json>","sig":"<hex>"}
//! POST {server}/api/   {"op":"lookup","id":482913077}
//!                   →  {"ok":true,"status":"online"|"offline"|"not_found","addr":…,"info":…}
//! ```
//!
//! `register`/`unregister` payloads are signed with the device's Ed25519
//! key (its EndpointId), so only the key's owner can claim or update its ID.
//! The server is otherwise untrusted: password auth happens end to end.

use std::{fmt, str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const DIRECTORY_VERSION: u32 = 2;

/// Prefix for signed directory payloads (domain separation).
pub const SIGNING_CONTEXT: &str = "slipstream-directory-v2\n";

/// Hosts re-register this often; servers consider a host offline after
/// [`ONLINE_WINDOW_SECS`] without hearing from it.
pub const HEARTBEAT_SECS: u64 = 30;
pub const ONLINE_WINDOW_SECS: u64 = 75;

/// Short device ID shown to users, e.g. `482 913 077`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
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

/// What `register` signs.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterPayload {
    pub op: String,
    /// Hex EndpointId (Ed25519 public key) — also the verifying key.
    pub endpoint: String,
    /// Unix seconds (server clock, see [`Hello::time`]).
    pub time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<EndpointAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<DeviceInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hello {
    pub version: u32,
    /// Server's clock, used to timestamp signed requests.
    pub time: i64,
    #[serde(default)]
    pub relays: Vec<RelayUrl>,
    #[serde(default)]
    pub motd: Option<String>,
    /// Organisation running this station.
    #[serde(default)]
    pub org: Option<String>,
    /// Whether clients may fall back to public relays.
    #[serde(default = "yes")]
    pub public_relays: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone)]
pub enum Lookup {
    Online { addr: EndpointAddr, info: DeviceInfo },
    Offline { info: DeviceInfo },
    NotFound,
}

/// Turns what a user typed ("relay.example.com", "http://10.0.0.5:8080/sub")
/// into the server's base URL with a trailing slash.
pub fn normalize_server_url(input: &str) -> Result<reqwest::Url> {
    let s = input.trim();
    if s.is_empty() {
        bail!("enter a server address, like relay.example.com");
    }
    let with_scheme = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
    let mut url = reqwest::Url::parse(&with_scheme).with_context(|| format!("“{s}” is not a valid address"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("“{s}” is not a valid web address");
    }
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct LookupResp {
    status: String,
    #[serde(default)]
    addr: Option<EndpointAddr>,
    #[serde(default)]
    info: Option<DeviceInfo>,
}

/// HTTP client for a directory server.
#[derive(Debug, Clone)]
pub struct DirectoryClient {
    http: reqwest::Client,
    base: reqwest::Url,
    api: reqwest::Url,
    /// server_time - local_time, learned from `hello`.
    clock_offset: i64,
    /// Access key for private stations; sent with every request.
    key: Option<String>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl DirectoryClient {
    pub fn new(server: &str, key: Option<String>) -> Result<Self> {
        crate::init_crypto();
        let base = normalize_server_url(server)?;
        let api = base.join("api/")?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("slipstream/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self { http, base, api, clock_offset: 0, key: key.filter(|k| !k.is_empty()) })
    }

    pub fn base_url(&self) -> &reqwest::Url {
        &self.base
    }

    async fn call<T: DeserializeOwned>(&self, mut body: serde_json::Value) -> Result<T> {
        if let (Some(key), Some(obj)) = (&self.key, body.as_object_mut()) {
            obj.insert("key".into(), key.clone().into());
        }
        let resp = self
            .http
            .post(self.api.clone())
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("cannot reach {}: {}", self.base.host_str().unwrap_or("server"), root_cause(&e)))?;
        let status = resp.status();
        let text = resp.text().await?;
        let env: Envelope = serde_json::from_str(&text).map_err(|_| {
            anyhow!(
                "{} did not answer like a Slipstream server (HTTP {status}) — check the address",
                self.base.host_str().unwrap_or("server")
            )
        })?;
        if !env.ok {
            bail!("server: {}", env.error.unwrap_or_else(|| format!("HTTP {status}")));
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Checks the server and learns its relays and clock.
    pub async fn hello(&mut self) -> Result<Hello> {
        let h: Hello = self.call(serde_json::json!({ "op": "hello", "version": DIRECTORY_VERSION })).await?;
        if h.version != DIRECTORY_VERSION {
            bail!("server speaks directory v{}, this app v{DIRECTORY_VERSION} — please update", h.version);
        }
        self.clock_offset = h.time - now();
        Ok(h)
    }

    fn signed(&self, secret: &SecretKey, op: &str, addr: Option<EndpointAddr>, info: Option<DeviceInfo>) -> Result<serde_json::Value> {
        let payload = serde_json::to_string(&RegisterPayload {
            op: op.into(),
            endpoint: secret.public().to_string(),
            time: now() + self.clock_offset,
            addr,
            info,
        })?;
        let mut msg = SIGNING_CONTEXT.as_bytes().to_vec();
        msg.extend_from_slice(payload.as_bytes());
        let sig = secret.sign(&msg);
        Ok(serde_json::json!({ "op": op, "payload": payload, "sig": hex(&sig.to_bytes()) }))
    }

    /// Claims (or refreshes) this device's ID and marks it online.
    pub async fn register(&self, secret: &SecretKey, addr: EndpointAddr, info: DeviceInfo) -> Result<DeviceId> {
        #[derive(Deserialize)]
        struct R {
            id: DeviceId,
        }
        let r: R = self.call(self.signed(secret, "register", Some(addr), Some(info))?).await?;
        Ok(r.id)
    }

    /// Marks this device offline right away (on shutdown).
    pub async fn unregister(&self, secret: &SecretKey) -> Result<()> {
        let _: serde_json::Value = self.call(self.signed(secret, "unregister", None, None)?).await?;
        Ok(())
    }

    pub async fn lookup(&self, id: DeviceId) -> Result<Lookup> {
        let r: LookupResp = self.call(serde_json::json!({ "op": "lookup", "id": id.0 })).await?;
        Ok(match (r.status.as_str(), r.addr, r.info) {
            ("online", Some(addr), info) => Lookup::Online { addr, info: info.unwrap_or_default() },
            ("offline", _, info) | ("online", None, info) => Lookup::Offline { info: info.unwrap_or_default() },
            _ => Lookup::NotFound,
        })
    }
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

/// Verifies a signed payload (used by the Rust server; the PHP server does
/// the same with libsodium).
pub fn verify_signed(payload: &str, sig_hex: &str) -> Result<RegisterPayload> {
    let p: RegisterPayload = serde_json::from_str(payload).context("bad payload")?;
    let key: EndpointId = p.endpoint.parse().context("bad endpoint key")?;
    let sig_bytes: Vec<u8> = (0..sig_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(sig_hex.get(i..i + 2).unwrap_or("zz"), 16))
        .collect::<Result<_, _>>()
        .context("bad signature encoding")?;
    let sig_bytes: [u8; 64] = sig_bytes.try_into().map_err(|_| anyhow!("bad signature length"))?;
    let sig = iroh::Signature::from_bytes(&sig_bytes);
    let mut msg = SIGNING_CONTEXT.as_bytes().to_vec();
    msg.extend_from_slice(payload.as_bytes());
    key.verify(&msg, &sig).map_err(|_| anyhow!("signature does not match"))?;
    if let Some(addr) = &p.addr
        && addr.id != key
    {
        bail!("address belongs to a different key");
    }
    Ok(p)
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
    fn server_urls_normalize() {
        assert_eq!(normalize_server_url("relay.example.com").unwrap().as_str(), "https://relay.example.com/");
        assert_eq!(normalize_server_url(" http://10.0.0.5:8080/sub ").unwrap().as_str(), "http://10.0.0.5:8080/sub/");
        assert_eq!(normalize_server_url("https://x.org/a/").unwrap().join("api/").unwrap().as_str(), "https://x.org/a/api/");
        assert!(normalize_server_url("").is_err());
        assert!(normalize_server_url("ftp://x").is_err());
    }

    #[test]
    fn signatures_verify_and_bind_key() {
        let secret = SecretKey::generate();
        let dc = DirectoryClient::new("example.com", None).unwrap();
        let addr = EndpointAddr::from_parts(secret.public(), []);
        let body = dc.signed(&secret, "register", Some(addr), Some(DeviceInfo::default())).unwrap();
        let payload = body["payload"].as_str().unwrap();
        let sig = body["sig"].as_str().unwrap();
        assert!(verify_signed(payload, sig).is_ok());
        // Tampering breaks it.
        let tampered = payload.replace("register", "registeR");
        assert!(verify_signed(&tampered, sig).is_err());
        // Someone else's address can't be registered under our key.
        let other = SecretKey::generate();
        let body = dc
            .signed(&secret, "register", Some(EndpointAddr::from_parts(other.public(), [])), None)
            .unwrap();
        assert!(verify_signed(body["payload"].as_str().unwrap(), body["sig"].as_str().unwrap()).is_err());
    }
}
