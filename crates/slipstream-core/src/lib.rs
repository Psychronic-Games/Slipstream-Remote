//! Slipstream core: protocol, authentication, networking and session logic.

pub mod auth;
pub mod config;
pub mod directory;
pub mod framing;
pub mod net;
pub mod proto;

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
