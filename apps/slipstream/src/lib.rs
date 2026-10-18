//! Slipstream Remote: the app's GUI and command line, shared by the desktop
//! program (`main.rs`) and the Android app (`apps/slipstream-android`).

#[cfg(not(target_os = "android"))]
pub mod cli;
pub mod gui;
pub mod logging;
#[cfg(windows)]
pub mod service;
#[cfg(any(windows, test))]
pub mod service_chat;

/// If this copy of the app came from a station's download page, adopt that
/// station's address and access key (see `slipstream_core::provision`).
pub fn apply_station_config() {
    let Some(cfg) = slipstream_core::provision::embedded() else { return };
    let Ok(store) = slipstream_core::config::Store::open_default() else { return };
    let mut settings = store.load_settings();
    if slipstream_core::provision::apply(&mut settings, &cfg) {
        tracing::info!(server = %cfg.server, "configured from station download");
        let _ = store.save_settings(&settings);
    }
}
