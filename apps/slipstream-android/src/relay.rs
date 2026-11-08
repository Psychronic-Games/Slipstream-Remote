//! Subspace Relay's own process (`:relay`, see SubspaceRelayService): the
//! relay server lives here, apart from the app's window, so closing or
//! reopening the app doesn't touch it. `RelayNative` in Java calls these.

use std::sync::{Mutex, OnceLock};

use jni::{
    JNIEnv,
    objects::{JClass, JObject, JString},
    sys::{jlongArray, jstring},
};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static SERVER: Mutex<Option<slipstream_relay::Server>> = Mutex::new(None);
static CONTEXT: OnceLock<jni::objects::GlobalRef> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("relay runtime"))
}

/// iroh reads network details through Android's APIs (ndk-context), which
/// the activity sets up in the app's process but nothing does in this one.
fn prepare(env: &mut JNIEnv, context: &JObject, home: String) -> jni::errors::Result<()> {
    if CONTEXT.get().is_some() {
        return Ok(());
    }
    let global = env.new_global_ref(context)?;
    let vm = env.get_java_vm()?;
    // SAFETY: once per process, with the VM and a context kept alive by
    // the global reference for the life of the process.
    unsafe { ndk_context::initialize_android_context(vm.get_java_vm_pointer().cast(), global.as_obj().as_raw().cast()) };
    let _ = CONTEXT.set(global);
    // SAFETY: before this process starts any other threads of ours.
    unsafe {
        std::env::set_var("SLIPSTREAM_HOME", &home);
        std::env::set_var("SLIPSTREAM_LOG_DIR", std::path::Path::new(&home).join("relay"));
    }
    slipstream::logging::init();
    Ok(())
}

fn text(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(String::from).unwrap_or_default()
}

/// Starts the relay on the group's address; empty on success, else why.
/// Called on the service's worker thread: it waits for the address.
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_RelayNative_startServer<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
    context: JObject,
    ip: JString,
    home: JString,
) -> jstring {
    let (ip, home) = (text(&mut env, &ip), text(&mut env, &home));
    let result = match prepare(&mut env, &context, home) {
        Err(e) => format!("Android setup failed: {e}"),
        Ok(()) => match ip.parse() {
            Err(_) => format!("Unexpected Wi-Fi Direct address {ip}"),
            Ok(ip) => runtime().block_on(async {
                let old = SERVER.lock().unwrap().take();
                if let Some(old) = old {
                    old.stop().await;
                }
                // A lasting identity, so computers recognise this phone.
                let key_path = std::path::Path::new(&std::env::var_os("SLIPSTREAM_HOME").unwrap_or_default()).join("relay.key");
                let secret_key = match slipstream_relay::load_or_create_key(&key_path) {
                    Ok(k) => Some(k),
                    Err(e) => {
                        tracing::warn!("relay key: {e:#}");
                        None
                    }
                };
                // The group's address can take a moment to appear.
                let mut last = String::new();
                for _ in 0..20 {
                    let opts = slipstream_relay::ServerOptions { ip, secret_key: secret_key.clone(), ..Default::default() };
                    match slipstream_relay::Server::start(opts).await {
                        Ok(server) => {
                            *SERVER.lock().unwrap() = Some(server);
                            return String::new();
                        }
                        Err(e) => last = format!("{e:#}"),
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                tracing::warn!("Subspace Relay couldn't listen: {last}");
                format!("The relay couldn't start: {last}")
            }),
        },
    };
    env.new_string(result).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_RelayNative_stopServer(_env: JNIEnv, _class: JClass) {
    if let Some(server) = SERVER.lock().unwrap().take() {
        runtime().block_on(server.stop());
    }
}

/// `[computers linked, connections, bytes up, bytes down]`, or empty
/// while no relay runs.
#[unsafe(no_mangle)]
extern "system" fn Java_com_slipstreamremote_app_RelayNative_stats(env: JNIEnv, _class: JClass) -> jlongArray {
    let values: Vec<i64> = match SERVER.lock().unwrap().as_ref() {
        Some(server) => {
            let s = server.stats().snapshot();
            vec![s.computers as i64, s.flows as i64, s.up as i64, s.down as i64]
        }
        None => Vec::new(),
    };
    let Ok(array) = env.new_long_array(values.len() as i32) else { return std::ptr::null_mut() };
    let _ = env.set_long_array_region(&array, 0, &values);
    array.into_raw()
}
