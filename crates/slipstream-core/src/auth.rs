//! Password authentication with SPAKE2.
//!
//! The host plays SPAKE2 side A once per password it accepts (one-time and/or
//! permanent); the viewer answers each with side B using the password the user
//! typed. Both transcripts are bound to the two EndpointIds as each side sees
//! them, so a man in the middle (e.g. a lying directory server) gets keys
//! that never agree. Passwords never cross the wire and cannot be attacked
//! offline.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use iroh::EndpointId;
use rand::Rng;
use spake2::{Ed25519Group, Identity, Password, Spake2};

const VIEWER_CONFIRM: &str = "slipstream v1 viewer confirm";
const HOST_CONFIRM: &str = "slipstream v1 host confirm";

fn confirm(key: &[u8], context: &str) -> [u8; 32] {
    let k = blake3::derive_key(context, key);
    *blake3::hash(&k).as_bytes()
}

fn ids(host: &EndpointId, viewer: &EndpointId) -> (Identity, Identity) {
    (Identity::new(host.as_bytes()), Identity::new(viewer.as_bytes()))
}

/// Normalizes user-typed passwords (case-insensitive, whitespace ignored) so
/// "ab12 CD" and "AB12CD" are the same secret.
pub fn normalize_password(pw: &str) -> String {
    pw.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

/// Derives the secret actually fed to SPAKE2. Hosts store only this digest,
/// never the plaintext permanent password.
pub fn password_secret(pw: &str) -> [u8; 32] {
    blake3::derive_key("slipstream v1 password", normalize_password(pw).as_bytes())
}

/// Generates a one-time password that's easy to read aloud: lowercase letters
/// and digits without look-alikes (0/o, 1/l/i).
pub fn generate_password(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Host side of the handshake.
pub struct HostAuth {
    states: Vec<Spake2<Ed25519Group>>,
}

impl HostAuth {
    /// Starts one SPAKE2 exchange per accepted password. Returns the state and
    /// the challenge messages to send.
    pub fn start(secrets: &[[u8; 32]], host: &EndpointId, viewer: &EndpointId) -> (Self, Vec<Vec<u8>>) {
        let (id_a, id_b) = ids(host, viewer);
        let (states, msgs) = secrets
            .iter()
            .map(|secret| Spake2::<Ed25519Group>::start_a(&Password::new(secret), &id_a, &id_b))
            .unzip();
        (Self { states }, msgs)
    }

    /// Verifies the viewer's response. On success returns the host
    /// confirmation to send back.
    pub fn finish(self, pakes: &[Vec<u8>], confirms: &[[u8; 32]]) -> Result<[u8; 32]> {
        if pakes.len() != self.states.len() || confirms.len() != self.states.len() {
            bail!("malformed auth response");
        }
        let mut ok = None;
        for ((state, msg), their_confirm) in self.states.into_iter().zip(pakes).zip(confirms) {
            let Ok(key) = state.finish(msg) else { continue };
            let expected = confirm(&key, VIEWER_CONFIRM);
            // Constant-time compare; keep looping so timing doesn't reveal which matched.
            if constant_time_eq(&expected, their_confirm) && ok.is_none() {
                ok = Some(confirm(&key, HOST_CONFIRM));
            }
        }
        ok.ok_or_else(|| anyhow::anyhow!("wrong password"))
    }
}

/// Viewer state plus the `(pakes, confirms)` response to send.
pub type ViewerResponse = (ViewerAuth, Vec<Vec<u8>>, Vec<[u8; 32]>);

/// Viewer side of the handshake.
pub struct ViewerAuth {
    expected_host_confirms: Vec<[u8; 32]>,
}

impl ViewerAuth {
    /// Answers the host's challenges. Returns the state (to verify the host
    /// later) and the response `(pakes, confirms)`.
    pub fn respond(
        password: &str,
        challenges: &[Vec<u8>],
        host: &EndpointId,
        viewer: &EndpointId,
    ) -> Result<ViewerResponse> {
        if challenges.is_empty() || challenges.len() > 4 {
            bail!("host sent {} auth challenges", challenges.len());
        }
        let (id_a, id_b) = ids(host, viewer);
        let password = Password::new(password_secret(password));
        let mut pakes = Vec::new();
        let mut confirms = Vec::new();
        let mut expected = Vec::new();
        for challenge in challenges {
            let (state, msg) = Spake2::<Ed25519Group>::start_b(&password, &id_a, &id_b);
            pakes.push(msg);
            match state.finish(challenge) {
                Ok(key) => {
                    confirms.push(confirm(&key, VIEWER_CONFIRM));
                    expected.push(confirm(&key, HOST_CONFIRM));
                }
                Err(_) => {
                    confirms.push([0; 32]);
                    expected.push([0xff; 32]);
                }
            }
        }
        Ok((Self { expected_host_confirms: expected }, pakes, confirms))
    }

    /// Checks that the host also knew the password (mutual authentication).
    pub fn verify_host(&self, host_confirm: &[u8; 32]) -> Result<()> {
        if self.expected_host_confirms.iter().any(|e| constant_time_eq(e, host_confirm)) {
            Ok(())
        } else {
            bail!("host failed to prove it knows the password — possible impersonation")
        }
    }
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Throttles password guessing on the host: after a few failures, every
/// attempt must wait an exponentially growing delay.
#[derive(Debug)]
pub struct AuthThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

impl Default for AuthThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthThrottle {
    const FREE_ATTEMPTS: u32 = 3;
    const MAX_LOCKOUT: Duration = Duration::from_secs(300);

    pub fn new() -> Self {
        Self { failures: 0, locked_until: None }
    }

    /// Remaining lockout, if attempts are currently blocked.
    pub fn locked(&self) -> Option<Duration> {
        self.locked_until
            .and_then(|t| t.checked_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    }

    pub fn record_failure(&mut self) {
        self.failures += 1;
        if self.failures >= Self::FREE_ATTEMPTS {
            let exp = (self.failures - Self::FREE_ATTEMPTS).min(8);
            let delay = (Duration::from_secs(2) * 2u32.pow(exp)).min(Self::MAX_LOCKOUT);
            self.locked_until = Some(Instant::now() + delay);
        }
    }

    pub fn record_success(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn keys() -> (EndpointId, EndpointId) {
        (SecretKey::generate().public(), SecretKey::generate().public())
    }

    fn run(host_pws: &[&str], typed: &str, host_seen_by_viewer: EndpointId, host: EndpointId, viewer: EndpointId) -> bool {
        let secrets: Vec<[u8; 32]> = host_pws.iter().map(|s| password_secret(s)).collect();
        let (h, challenges) = HostAuth::start(&secrets, &host, &viewer);
        let (v, pakes, confirms) = ViewerAuth::respond(typed, &challenges, &host_seen_by_viewer, &viewer).unwrap();
        match h.finish(&pakes, &confirms) {
            Ok(c) => v.verify_host(&c).is_ok(),
            Err(_) => false,
        }
    }

    #[test]
    fn correct_password_authenticates() {
        let (h, v) = keys();
        assert!(run(&["k7mq2x"], "K7MQ 2X", h, h, v));
    }

    #[test]
    fn either_password_works() {
        let (h, v) = keys();
        assert!(run(&["temp42", "permanent-secret"], "permanent-secret", h, h, v));
        assert!(run(&["temp42", "permanent-secret"], "temp42", h, h, v));
    }

    #[test]
    fn wrong_password_fails() {
        let (h, v) = keys();
        assert!(!run(&["k7mq2x"], "k7mq2y", h, h, v));
    }

    #[test]
    fn identity_mismatch_fails() {
        // Viewer thinks it's talking to `fake` (e.g. directory lied).
        let (h, v) = keys();
        let (fake, _) = keys();
        assert!(!run(&["k7mq2x"], "k7mq2x", fake, h, v));
    }

    #[test]
    fn throttle_backs_off() {
        let mut t = AuthThrottle::new();
        for _ in 0..3 {
            assert!(t.locked().is_none());
            t.record_failure();
        }
        assert!(t.locked().is_some());
        t.record_success();
        assert!(t.locked().is_none());
    }
}
