//! Meetings have their own protocol and admission, independent of desktop access.
use anyhow::{Context, Result, bail, ensure};
use hmac::{Hmac, Mac};
use iroh::{EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const ALPN: &[u8] = b"slipstream/meeting/1";
pub const CALL_ALPN: &[u8] = b"slipstream/call/1";
pub const VERSION: u32 = 1;
pub const CAPACITY: usize = 20;
pub const MAX_CAPACITY: usize = 32;
pub const MAX_SUBSCRIPTIONS: usize = 4;
pub const MAX_VIDEO: usize = 1024 * 1024;
pub const MAX_CONTROL: usize = 16 * 1024;
pub const AUDIO_TAG: u8 = 0xb1;

pub fn random_secret() -> String {
    crate::directory::hex(&rand::random::<[u8; 32]>())
}
pub fn now() -> u64 {
    crate::clock::now().max(0) as u64
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    pub version: u32,
    pub address: EndpointAddr,
    pub token: String,
    pub title: String,
    pub expires: u64,
    pub public_relays: bool,
}
impl std::fmt::Debug for Invitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invitation")
            .field("host", &self.address.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}
impl Invitation {
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        Ok(format!(
            "slipstream://meeting?i={}",
            crate::directory::hex(&serde_json::to_vec(self)?)
        ))
    }
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= 16384, "Meeting invitation is too long");
        let url = reqwest::Url::parse(text.trim())
            .context("Paste a complete Slipstream meeting invitation")?;
        ensure!(
            url.scheme() == "slipstream" && url.host_str() == Some("meeting"),
            "This is not a meeting invitation"
        );
        let encoded = url
            .query_pairs()
            .find(|(k, _)| k == "i")
            .context("Meeting invitation is incomplete")?
            .1
            .into_owned();
        let data = crate::directory::unhex(&encoded).context("Meeting invitation is damaged")?;
        let invite: Self =
            serde_json::from_slice(&data).context("Meeting invitation is damaged")?;
        invite.validate()?;
        Ok(invite)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "Update Slipstream to join this meeting"
        );
        ensure!(
            self.token.len() == 64 && self.token.bytes().all(|b| b.is_ascii_hexdigit()),
            "Meeting invitation is damaged"
        );
        ensure!(
            self.expires > now() && self.expires <= now() + 24 * 3600,
            "Meeting invitation has expired"
        );
        ensure!(
            self.title.chars().count() <= 80 && !self.title.chars().any(char::is_control),
            "Meeting title is too long"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[default]
    None,
    Camera,
    Screen,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Participant {
    pub id: u64,
    pub name: String,
    pub microphone: bool,
    pub source: Source,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub title: String,
    pub local: u64,
    pub host: bool,
    pub waiting: bool,
    pub locked: bool,
    pub capacity: usize,
    pub participants: Vec<Participant>,
    pub lobby: Vec<Participant>,
    pub ended: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chat {
    pub sender: u64,
    pub name: String,
    pub text: String,
}
#[derive(Serialize, Deserialize)]
pub enum ClientMessage {
    Join {
        version: u32,
        token: String,
        name: String,
    },
    Chat(String),
    Media {
        microphone: bool,
        source: Source,
    },
    Subscribe(Vec<u64>),
    Keyframe(u64),
    FrameAck {
        participant: u64,
        generation: u64,
        seq: u64,
    },
    Leave,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    State(State),
    Chat(Chat),
    Keyframe,
    VideoAck { generation: u64, seq: u64 },
    Ended(String),
}
#[derive(Serialize, Deserialize)]
pub struct Video {
    pub participant: u64,
    pub generation: u64,
    pub seq: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}
impl Video {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.width >= 2
                && self.height >= 2
                && self.width <= 1920
                && self.height <= 1920
                && u64::from(self.width) * u64::from(self.height) <= 1920 * 1080
                && !self.data.is_empty()
                && self.seq > 0
                && self.data.len() <= MAX_VIDEO - 128,
            "Invalid meeting video frame"
        );
        Ok(())
    }
}

/// A call proves station access without transmitting the access key.
pub fn call_proof(
    key: Option<&str>,
    nonce: &[u8; 32],
    caller: EndpointId,
    target: EndpointId,
    invite: &Invitation,
) -> Result<[u8; 32]> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.unwrap_or("").as_bytes()).expect("HMAC accepts any key");
    mac.update(b"slipstream-call-v1\n");
    mac.update(nonce);
    mac.update(caller.as_bytes());
    mac.update(target.as_bytes());
    mac.update(&serde_json::to_vec(invite)?);
    Ok(mac.finalize().into_bytes().into())
}
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
#[derive(Serialize, Deserialize)]
pub enum CallMessage {
    Hello,
    Challenge([u8; 32]),
    Offer {
        name: String,
        invitation: Invitation,
        proof: [u8; 32],
    },
    Ringing,
    Answered,
    Declined(String),
    Cancel,
}

/// Short-lived relay-only credential. It never authorizes the directory API.
pub fn relay_token(key: &str, expires: u64, nonce: &str) -> String {
    let body = format!("meeting1.{expires}.{nonce}");
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key");
    mac.update(body.as_bytes());
    format!(
        "{body}.{}",
        crate::directory::hex(&mac.finalize().into_bytes())
    )
}
pub fn valid_relay_token(key: &str, token: &str) -> bool {
    if token.len() > 220 {
        return false;
    }
    let fields: Vec<_> = token.split('.').collect();
    if fields.len() != 4
        || fields[0] != "meeting1"
        || fields[2].len() != 32
        || !fields[2].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return false;
    }
    let Ok(expires) = fields[1].parse::<u64>() else {
        return false;
    };
    if expires <= now() || expires > now() + 5 * 3600 {
        return false;
    }
    equal(
        relay_token(key, expires, fields[2]).as_bytes(),
        token.as_bytes(),
    )
}
pub fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        bail!("Enter a name of 1–64 characters");
    }
    Ok(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_relay_tokens_do_not_accept_wrong_keys_or_expired_leases() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let token = relay_token("private", now() + 300, nonce);
        assert!(valid_relay_token("private", &token));
        assert!(!valid_relay_token("other", &token));
        assert!(!valid_relay_token(
            "private",
            &relay_token("private", now() - 1, nonce)
        ));
        assert!(!valid_relay_token(
            "private",
            &relay_token("private", now() + 86400, nonce)
        ));
        assert!(!valid_relay_token("private", "private"));
    }
    #[test]
    fn invitation_roundtrip_and_expiry() {
        let invite = Invitation {
            version: VERSION,
            address: EndpointAddr::new(iroh::SecretKey::generate().public()),
            token: random_secret(),
            title: "Engineering".into(),
            expires: now() + 3600,
            public_relays: false,
        };
        assert_eq!(
            Invitation::parse(&invite.encode().unwrap()).unwrap().token,
            invite.token
        );
        assert!(!format!("{invite:?}").contains(&invite.token));
        let mut expired = invite;
        expired.expires = now() - 1;
        assert!(expired.encode().is_err());
    }
}
