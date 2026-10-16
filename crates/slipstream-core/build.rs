//! Records the build time: the app uses it as a lower bound for the clock
//! when checking certificates (a PC whose clock is behind the date the app
//! was built is certainly wrong).
fn main() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=SLIPSTREAM_BUILD_UNIX={now}");
    println!("cargo:rerun-if-changed=build.rs");
}
