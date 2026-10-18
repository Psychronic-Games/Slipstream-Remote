//! Device passwords a technician saves on a station, sealed on their own
//! computer. The key comes from their sign-in password (Argon2id, with a
//! per-account salt from the station) and never leaves the app, so the
//! station keeps the sealed passwords without being able to read them.
//! Each one is bound to its device ID: the station can't swap them around.

use anyhow::{Result, anyhow, bail};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use serde::{Deserialize, Serialize};

use crate::directory::{DeviceId, hex, unhex};

/// A sealed password as the station stores it: XChaCha20-Poly1305 nonce
/// and ciphertext, hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedSecret {
    pub n: String,
    pub c: String,
}

/// The key that seals and opens one account's saved passwords.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultKey([u8; 32]);

impl std::fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultKey(…)")
    }
}

impl VaultKey {
    /// From the sign-in password and the account's salt (hex). Deliberately
    /// slow (about a quarter of a second): run it off the UI thread.
    pub fn derive(password: &str, salt_hex: &str) -> Result<Self> {
        let salt = unhex(salt_hex).filter(|s| s.len() >= 8).ok_or_else(|| anyhow!("the station sent an unusable vault salt"))?;
        // OWASP's Argon2id baseline: 19 MiB, 2 passes, 1 lane.
        let params = argon2::Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| anyhow!("{e}"))?;
        let mut key = [0u8; 32];
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
            .hash_password_into(password.as_bytes(), &salt, &mut key)
            .map_err(|e| anyhow!("{e}"))?;
        Ok(Self(key))
    }

    /// Hex, for keeping in the app's settings.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        unhex(s)?.try_into().ok().map(Self)
    }

    fn aad(device: DeviceId) -> Vec<u8> {
        format!("slipstream vault v1\n{}", device.0).into_bytes()
    }

    pub fn seal(&self, device: DeviceId, password: &str) -> SealedSecret {
        let nonce: [u8; 24] = rand::random();
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let aad = Self::aad(device);
        let c = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: password.as_bytes(), aad: &aad })
            .expect("encrypting in memory can't fail");
        SealedSecret { n: hex(&nonce), c: hex(&c) }
    }

    pub fn open(&self, device: DeviceId, sealed: &SealedSecret) -> Result<String> {
        let (Some(nonce), Some(c)) = (unhex(&sealed.n), unhex(&sealed.c)) else { bail!("damaged saved password") };
        if nonce.len() != 24 {
            bail!("damaged saved password");
        }
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let aad = Self::aad(device);
        let plain = cipher
            .decrypt(XNonce::from_slice(&nonce), Payload { msg: &c, aad: &aad })
            .map_err(|_| anyhow!("the saved password can't be opened (was your sign-in password changed?)"))?;
        String::from_utf8(plain).map_err(|_| anyhow!("damaged saved password"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_per_device() {
        let salt = hex(&[7u8; 16]);
        let key = VaultKey::derive("correct horse 1", &salt).unwrap();
        assert_eq!(VaultKey::derive("correct horse 1", &salt).unwrap(), key, "deterministic");
        let a = DeviceId(309_524_609);
        let sealed = key.seal(a, "unattended pass 42");
        assert_eq!(key.open(a, &sealed).unwrap(), "unattended pass 42");
        // Bound to the device: moved to another entry it won't open.
        assert!(key.open(DeviceId(771_222_054), &sealed).is_err());
        // Another password (or salt) gives another key.
        let other = VaultKey::derive("correct horse 2", &salt).unwrap();
        assert!(other.open(a, &sealed).is_err());
        let resalted = VaultKey::derive("correct horse 1", &hex(&[8u8; 16])).unwrap();
        assert!(resalted.open(a, &sealed).is_err());
        // Hex round trip, as kept in settings.
        assert_eq!(VaultKey::from_hex(&key.to_hex()).unwrap(), key);
        // Fresh nonce every time.
        assert_ne!(key.seal(a, "x").n, key.seal(a, "x").n);
    }
}
