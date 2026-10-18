//! Slipstream core: protocol, authentication, networking and session logic.

pub mod account;
pub mod auth;
pub mod clock;
pub mod config;
pub mod directory;
pub mod framing;
pub mod net;
pub mod proto;
pub mod provision;
pub mod release;
pub mod vault;

pub use iroh;

/// Human-readable OS name for display to peers.
pub fn os_name() -> String {
    match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        "android" => "Android",
        "ios" => "iOS",
        other => other,
    }
    .to_string()
}

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Monotonic build number (git commit count) — newer builds are bigger.
pub const BUILD_NUMBER: u64 = {
    match u64::from_str_radix(env!("SLIPSTREAM_BUILD_NUMBER"), 10) {
        Ok(n) => n,
        Err(_) => 0,
    }
};

/// Short git commit of this build.
pub const COMMIT: &str = env!("SLIPSTREAM_COMMIT");

/// "0.6.0 (build 13 · 1a2b3c4)"
pub const VERSION_STRING: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("SLIPSTREAM_BUILD_NUMBER"),
    " · ",
    env!("SLIPSTREAM_COMMIT"),
    ")"
);

pub fn version_string() -> String {
    VERSION_STRING.to_string()
}

/// Identifies which client build fits this machine, e.g. "linux-x86_64".
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Selects the TLS crypto backend (ring) for HTTPS. Call once at startup;
/// later calls are no-ops.
pub fn init_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
