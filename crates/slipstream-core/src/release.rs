//! Update signing.
//!
//! The SHA-256 in a station's manifest only proves the download arrived
//! intact; whoever controls the web host could replace the app and its
//! checksum together. So each manifest entry is also signed with an Ed25519
//! release key that stays on the build machine, and apps only install
//! updates signed by a key built into them (`SLIPSTREAM_UPDATE_KEYS` at
//! compile time: comma-separated hex public keys, set by
//! `scripts/build-station.sh`). A build without keys can't verify updates
//! and doesn't install any.

use anyhow::{Result, anyhow, bail};
use iroh::{PublicKey, SecretKey, Signature};

use crate::directory::{hex, unhex};

/// Compile-time list of trusted release keys (hex, comma-separated).
const UPDATE_KEYS: Option<&str> = option_env!("SLIPSTREAM_UPDATE_KEYS");

/// Release keys this build trusts for updates.
pub fn trusted_keys() -> Vec<PublicKey> {
    parse_keys(UPDATE_KEYS.unwrap_or(""))
}

/// Parses a comma-separated list of hex public keys, skipping blanks.
/// (`build.rs` rejects malformed lists, so nothing is silently dropped.)
pub fn parse_keys(list: &str) -> Vec<PublicKey> {
    list.split(',').map(str::trim).filter(|k| !k.is_empty()).filter_map(|k| k.parse().ok()).collect()
}

/// What a release signature covers. The platform stops a build for one OS
/// being passed off as another's; version and build stop a signature being
/// reused for a different build.
pub struct SignedUpdate<'a> {
    pub platform: &'a str,
    pub version: &'a str,
    pub build: u64,
    pub size: u64,
    pub sha256: &'a str,
}

impl SignedUpdate<'_> {
    fn message(&self) -> Vec<u8> {
        format!(
            "slipstream update v1\n{}\n{}\n{}\n{}\n{}",
            self.platform,
            self.version,
            self.build,
            self.size,
            self.sha256.to_ascii_lowercase()
        )
        .into_bytes()
    }

    /// Signs this update; returns the hex signature for the manifest.
    pub fn sign(&self, key: &SecretKey) -> String {
        hex(&key.sign(&self.message()).to_bytes())
    }

    /// Checks `sig_hex` against any of `keys`.
    pub fn verify(&self, keys: &[PublicKey], sig_hex: &str) -> Result<()> {
        if keys.is_empty() {
            bail!("this build of Slipstream has no update key, so it can't verify updates — download new versions from your station");
        }
        if sig_hex.is_empty() {
            bail!("the station's update isn't signed — the station needs a newer scripts/build-station.sh");
        }
        let bytes: [u8; 64] = unhex(sig_hex)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| anyhow!("the update's signature is malformed"))?;
        let sig = Signature::from_bytes(&bytes);
        let msg = self.message();
        if keys.iter().any(|k| k.verify(&msg, &sig).is_ok()) {
            Ok(())
        } else {
            bail!("the update's signature doesn't match this app's release key — not installing it")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(build: u64) -> SignedUpdate<'static> {
        SignedUpdate { platform: "linux-x86_64", version: "0.5.1", build, size: 1234, sha256: "ABCDEF" }
    }

    #[test]
    fn signed_update_verifies() {
        let key = SecretKey::generate();
        let sig = update(15).sign(&key);
        assert!(update(15).verify(&[key.public()], &sig).is_ok());
        // Either of several trusted keys will do.
        assert!(update(15).verify(&[SecretKey::generate().public(), key.public()], &sig).is_ok());
    }

    #[test]
    fn tampering_is_rejected() {
        let key = SecretKey::generate();
        let keys = [key.public()];
        let sig = update(15).sign(&key);
        assert!(update(16).verify(&keys, &sig).is_err());
        let other_platform = SignedUpdate { platform: "windows-x86_64", ..update(15) };
        assert!(other_platform.verify(&keys, &sig).is_err());
        let other_hash = SignedUpdate { sha256: "abcdee", ..update(15) };
        assert!(other_hash.verify(&keys, &sig).is_err());
        // Wrong key, no signature, no keys.
        assert!(update(15).verify(&[SecretKey::generate().public()], &sig).is_err());
        assert!(update(15).verify(&keys, "").is_err());
        assert!(update(15).verify(&[], &sig).is_err());
        assert!(update(15).verify(&keys, "zz").is_err());
    }

    #[test]
    fn key_list_parsing() {
        let a = SecretKey::generate().public();
        let b = SecretKey::generate().public();
        assert_eq!(parse_keys(&format!(" {a}, {b},")), vec![a, b]);
        assert!(parse_keys("").is_empty());
    }
}
