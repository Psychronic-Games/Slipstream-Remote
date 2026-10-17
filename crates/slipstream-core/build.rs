//! Records build facts:
//! * the build time — used as a lower bound for the clock when checking
//!   certificates (a PC whose clock is behind the build date is wrong);
//! * the build number (git commit count) and commit — so a station's
//!   update manifest can tell which build is newer;
//! * that `SLIPSTREAM_UPDATE_KEYS` is well formed.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=SLIPSTREAM_BUILD_UNIX={now}");
    let number = git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into());
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "dev".into());
    println!("cargo:rustc-env=SLIPSTREAM_BUILD_NUMBER={number}");
    println!("cargo:rustc-env=SLIPSTREAM_COMMIT={commit}");
    println!("cargo:rerun-if-changed=build.rs");
    // Release keys trusted for updates (see src/release.rs). Fail the build
    // on a typo rather than ship an app that can never update.
    println!("cargo:rerun-if-env-changed=SLIPSTREAM_UPDATE_KEYS");
    if let Ok(keys) = std::env::var("SLIPSTREAM_UPDATE_KEYS") {
        for k in keys.split(',').map(str::trim).filter(|k| !k.is_empty()) {
            assert!(
                k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit()),
                "SLIPSTREAM_UPDATE_KEYS: {k:?} is not a 64-digit hex public key"
            );
        }
    }
    for p in ["../../.git/HEAD", "../../.git/refs/heads", "../../.git/packed-refs"] {
        if std::path::Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }
}
