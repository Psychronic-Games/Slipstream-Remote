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
    // Retain the activity class (not the activity) for calls from session threads.
    let vm=unsafe {jni::JavaVM::from_raw(app.vm_as_ptr().cast())}.expect("Android VM");
    let class={
        let env=vm.attach_current_thread().expect("attach Android thread");
        let activity=unsafe {jni::objects::JObject::from_raw(app.activity_as_ptr().cast())};
        let class=env.get_object_class(&activity).expect("activity class");
        env.new_global_ref(class).expect("retain activity class")
    };
    slipstream_platform::android::install(vm,class);
    if let Ok(name)=slipstream_platform::android::device_name() {
        unsafe {std::env::set_var("SLIPSTREAM_DEVICE_NAME",&name)};
        if let Ok(store)=slipstream_core::config::Store::open_default() {
            let mut settings=store.load_settings();
            if settings.device_name.trim().is_empty() || settings.device_name=="My Computer" {
                settings.device_name=name;let _=store.save_settings(&settings);
            }
        }
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

/// Screen data stays valid only for this JNI call: Rust copies it into bounded slots.
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_ScreenShareService_onFrame(
    env:jni::JNIEnv,_class:jni::objects::JClass,buffer:jni::objects::JByteBuffer,
    width:i32,height:i32,row:i32,pixel:i32,
) {
    if width<2 || height<2 || row<0 || pixel<4 {return;}
    if let (Ok(ptr),Ok(len))=(env.get_direct_buffer_address(&buffer),env.get_direct_buffer_capacity(&buffer)) {
        // ImageReader keeps its direct plane buffer alive until this callback returns.
        let data=unsafe {std::slice::from_raw_parts(ptr,len)};
        slipstream_platform::android::frame(data,width as u32,height as u32,row as usize,pixel as usize);
    }
}
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_ScreenShareService_onCaptureStarted(
    _env:jni::JNIEnv,_class:jni::objects::JClass,width:i32,height:i32,full:u8,
) {if width>0 && height>0 {slipstream_platform::android::started(width as u32,height as u32,full!=0);}}
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_ScreenShareService_onFullDisplay(
    _env:jni::JNIEnv,_class:jni::objects::JClass,full:u8,
) {slipstream_platform::android::set_full_display(full!=0);}
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_ScreenShareService_onCaptureStopped(
    mut env:jni::JNIEnv,_class:jni::objects::JClass,message:jni::objects::JString,
) {let message=env.get_string(&message).map(String::from).unwrap_or_else(|_|"Screen sharing stopped".into());slipstream_platform::android::stopped(message);}
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_RemoteInputService_onAvailability(
    _env:jni::JNIEnv,_class:jni::objects::JClass,enabled:u8,
) {slipstream_platform::android::set_accessibility(enabled!=0);}
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_RemoteInputService_inputAllowed(
    _env:jni::JNIEnv,_class:jni::objects::JClass,
)->u8 {u8::from(slipstream_platform::android::input_allowed())}
