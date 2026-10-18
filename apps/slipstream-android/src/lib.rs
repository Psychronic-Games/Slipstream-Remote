//! Slipstream Remote on Android: the desktop app's GUI, started by the
//! GameActivity in `app/` (the Gradle project that builds the APK; see
//! scripts/build-android.sh).
#![cfg(target_os = "android")]

use android_activity::AndroidApp;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // Settings, the device identity and the log live in the app's private
    // storage (there's no home folder to find them in).
    if let Some(dir) = app.internal_data_path() {
        // SAFETY: nothing else runs yet.
        unsafe { std::env::set_var("SLIPSTREAM_HOME", dir) };
    }
    slipstream::logging::init();
    let result = slipstream::gui::run(slipstream::gui::GuiOptions { android_app: Some(app), ..Default::default() });
    if let Err(e) = result {
        tracing::error!("Slipstream Remote couldn't start: {e:#}");
    }
}

/// `MainActivity.onLink`: the activity was opened with a `slipstream://`
/// link (the station page's button), at start or while running.
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_MainActivity_onLink(mut env: jni::JNIEnv, _class: jni::objects::JClass, url: jni::objects::JString) {
    match env.get_string(&url) {
        Ok(url) => slipstream::gui::open_link(url.into()),
        Err(e) => tracing::warn!("unreadable link: {e}"),
    }
}

/// `MainActivity.onInsets`: the edges the status and navigation bars, the
/// camera cutout and the soft keyboard cover, in pixels.
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_MainActivity_onInsets(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
    left: jni::sys::jint,
    top: jni::sys::jint,
    right: jni::sys::jint,
    bottom: jni::sys::jint,
) {
    slipstream::gui::set_insets([left, top, right, bottom].map(|v| v as f32));
}

/// `MainActivity.onImeAction`: Enter or Done on the soft keyboard.
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_MainActivity_onImeAction(_env: jni::JNIEnv, _class: jni::objects::JClass) {
    slipstream::gui::editor_action();
}
