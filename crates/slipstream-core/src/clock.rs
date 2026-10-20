//! A clock that keeps working when the computer's clock is wrong.
//!
//! TLS certificates are only valid between two dates, so a PC whose clock
//! is off by weeks can't verify any HTTPS server — not the station, not the
//! relays. Slipstream keeps its own notion of "now": the system clock plus a
//! correction learned from servers, and never earlier than the date the app
//! was built. All certificate checks (directory and relays) use it.
//!
//! When a certificate fails purely on dates, we read the server's clock from
//! its HTTP `Date` header and retry the full verification. The worst a forged
//! time could do is let an *expired* certificate for that same domain — still
//! CA-signed — pass, which is an acceptable trade for remote support working
//! on machines with a dead CMOS battery.

use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
};

/// Seconds to add to the system clock (server time − local time).
static OFFSET: AtomicI64 = AtomicI64::new(0);

/// When this build was made; a clock earlier than this is certainly wrong.
pub const BUILD_UNIX: i64 = {
    match i64::from_str_radix(env!("SLIPSTREAM_BUILD_UNIX"), 10) {
        Ok(t) => t,
        Err(_) => 0,
    }
};

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Corrected current time, Unix seconds.
pub fn now() -> i64 {
    (system_now() + OFFSET.load(Ordering::Relaxed)).max(BUILD_UNIX)
}

/// How far off the system clock is, in seconds (as far as we know).
pub fn offset() -> i64 {
    OFFSET.load(Ordering::Relaxed)
}

/// Records a trusted server's idea of the current time.
pub fn learn_server_time(server_unix: i64) {
    let off = server_unix - system_now();
    let old = OFFSET.swap(off, Ordering::Relaxed);
    if off.abs() > 120 && (off - old).abs() > 60 {
        tracing::warn!("this computer's clock is off by {} — compensating", human(off));
    }
}

fn human(secs: i64) -> String {
    let a = secs.unsigned_abs();
    let s = if a >= 86_400 {
        format!("{:.1} days", a as f64 / 86_400.0)
    } else if a >= 3600 {
        format!("{:.1} hours", a as f64 / 3600.0)
    } else {
        format!("{a} seconds")
    };
    format!("{s} ({})", if secs > 0 { "behind" } else { "ahead" })
}

/// Does this error text describe a certificate date problem?
pub fn is_time_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("certificate") && (m.contains("expired") || m.contains("not valid") || m.contains("notvalidyet"))
}

/// Verifies certificates against Mozilla's roots using our corrected clock.
#[derive(Debug)]
pub struct ClockedVerifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for ClockedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let now = UnixTime::since_unix_epoch(Duration::from_secs(now().max(0) as u64));
        self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Builds the verifier (used for the directory and, via iroh, the relays).
pub fn verifier(provider: Arc<CryptoProvider>) -> Result<Arc<dyn ServerCertVerifier>> {
    let roots = Arc::new(rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() });
    let inner = WebPkiServerVerifier::builder_with_provider(roots, provider).build()?;
    Ok(Arc::new(ClockedVerifier { inner }))
}

/// Accepts any certificate. Used *only* to read a server's `Date` header.
#[derive(Debug)]
struct DateOnly(Arc<CryptoProvider>);

impl ServerCertVerifier for DateOnly {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Learns the real time from `url`'s HTTP `Date` header (when our clock is
/// too wrong for certificates to verify).
pub async fn sync_from(url: &reqwest::Url) -> Result<()> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(DateOnly(provider)))
        .with_no_client_auth();
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = client.head(url.clone()).send().await.context("asking the server for the time")?;
    let date = resp
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|d| d.to_str().ok())
        .context("server sent no Date header")?;
    let when = httpdate::parse_http_date(date).context("unreadable Date header")?;
    let unix = when.duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    learn_server_time(unix);
    Ok(())
}

/// A readable UTC timestamp for exported reports and their preview.
pub fn format_utc(seconds: u64) -> String {
    if seconds == 0 {return "Not collected".into();}
    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds))
        .map(httpdate::fmt_http_date).unwrap_or_else(|| "Unavailable".into())
}

#[cfg(test)]
pub(crate) fn set_offset_for_test(off: i64) {
    OFFSET.store(off, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_errors_are_recognised() {
        assert!(is_time_error("invalid peer certificate: Expired"));
        assert!(is_time_error("invalid peer certificate: certificate not valid yet: verification time 1"));
        assert!(!is_time_error("invalid peer certificate: UnknownIssuer"));
    }

    #[test]
    fn never_before_build() {
        assert!(now() >= BUILD_UNIX);
    }
}
