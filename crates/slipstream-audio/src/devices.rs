//! Named microphone inputs, using the same identifiers as capture.
use anyhow::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicrophoneDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn microphones() -> Result<Vec<MicrophoneDevice>> {
    #[cfg(target_os = "linux")]
    return pulse::microphones();
    #[cfg(any(windows, target_os = "macos", target_os = "android"))]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let default = host.default_input_device().and_then(|d| d.id().ok());
        host.input_devices()?
            .map(|device| {
                let id = device.id()?;
                Ok(MicrophoneDevice {
                    is_default: default.as_ref() == Some(&id),
                    id: id.to_string(),
                    name: device.description()?.name().to_owned(),
                })
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
mod pulse {
    use super::*;
    use anyhow::{bail, ensure};
    use std::{
        ffi::{CStr, c_char, c_int, c_void},
        sync::OnceLock,
        time::{Duration, Instant},
    };
    // Prefixes of the stable libpulse introspection structures, through the
    // last field read here. See pulse/{sample,channelmap,volume,introspect}.h.
    #[repr(C)]
    struct Spec {
        format: c_int,
        rate: u32,
        channels: u8,
    }
    #[repr(C)]
    struct Map {
        channels: u8,
        positions: [c_int; 32],
    }
    #[repr(C)]
    struct Volume {
        channels: u8,
        values: [u32; 32],
    }
    #[repr(C)]
    struct Source {
        name: *const c_char,
        index: u32,
        description: *const c_char,
        spec: Spec,
        map: Map,
        owner: u32,
        volume: Volume,
        mute: c_int,
        monitor_of_sink: u32,
    }
    #[repr(C)]
    struct Server {
        user: *const c_char,
        host: *const c_char,
        version: *const c_char,
        name: *const c_char,
        spec: Spec,
        default_sink: *const c_char,
        default_source: *const c_char,
    }
    type Ptr = *mut c_void;
    type SourceCallback = unsafe extern "C" fn(Ptr, *const Source, c_int, Ptr);
    type ServerCallback = unsafe extern "C" fn(Ptr, *const Server, Ptr);
    struct Api {
        main_new: unsafe extern "C" fn() -> Ptr,
        main_free: unsafe extern "C" fn(Ptr),
        main_api: unsafe extern "C" fn(Ptr) -> Ptr,
        iterate: unsafe extern "C" fn(Ptr, c_int, *mut c_int) -> c_int,
        context_new: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
        connect: unsafe extern "C" fn(Ptr, *const c_char, c_int, Ptr) -> c_int,
        state: unsafe extern "C" fn(Ptr) -> c_int,
        disconnect: unsafe extern "C" fn(Ptr),
        unref: unsafe extern "C" fn(Ptr),
        sources: unsafe extern "C" fn(Ptr, SourceCallback, Ptr) -> Ptr,
        server: unsafe extern "C" fn(Ptr, ServerCallback, Ptr) -> Ptr,
        cancel_op: unsafe extern "C" fn(Ptr),
        unref_op: unsafe extern "C" fn(Ptr),
    }
    fn api() -> Result<&'static Api> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        API.get_or_init(|| {
            // SAFETY: libpulse's documented C signatures. Keep the library
            // loaded for the process lifetime, like the playback backend.
            unsafe {
                let library =
                    libc::dlopen(c"libpulse.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                if library.is_null() {
                    return Err("PulseAudio device discovery is unavailable".into());
                }
                macro_rules! symbol {
                    ($name:literal,$ty:ty) => {{
                        let pointer = libc::dlsym(library, concat!($name, "\0").as_ptr().cast());
                        if pointer.is_null() {
                            return Err(concat!("Missing PulseAudio function: ", $name).into());
                        }
                        std::mem::transmute::<Ptr, $ty>(pointer)
                    }};
                }
                Ok(Api {
                    main_new: symbol!("pa_mainloop_new", unsafe extern "C" fn() -> Ptr),
                    main_free: symbol!("pa_mainloop_free", unsafe extern "C" fn(Ptr)),
                    main_api: symbol!("pa_mainloop_get_api", unsafe extern "C" fn(Ptr) -> Ptr),
                    iterate: symbol!(
                        "pa_mainloop_iterate",
                        unsafe extern "C" fn(Ptr, c_int, *mut c_int) -> c_int
                    ),
                    context_new: symbol!(
                        "pa_context_new",
                        unsafe extern "C" fn(Ptr, *const c_char) -> Ptr
                    ),
                    connect: symbol!(
                        "pa_context_connect",
                        unsafe extern "C" fn(Ptr, *const c_char, c_int, Ptr) -> c_int
                    ),
                    state: symbol!("pa_context_get_state", unsafe extern "C" fn(Ptr) -> c_int),
                    disconnect: symbol!("pa_context_disconnect", unsafe extern "C" fn(Ptr)),
                    unref: symbol!("pa_context_unref", unsafe extern "C" fn(Ptr)),
                    sources: symbol!(
                        "pa_context_get_source_info_list",
                        unsafe extern "C" fn(Ptr, SourceCallback, Ptr) -> Ptr
                    ),
                    server: symbol!(
                        "pa_context_get_server_info",
                        unsafe extern "C" fn(Ptr, ServerCallback, Ptr) -> Ptr
                    ),
                    cancel_op: symbol!("pa_operation_cancel", unsafe extern "C" fn(Ptr)),
                    unref_op: symbol!("pa_operation_unref", unsafe extern "C" fn(Ptr)),
                })
            }
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
    }
    struct Connection {
        api: &'static Api,
        main: Ptr,
        context: Ptr,
        operations: Vec<Ptr>,
    }
    impl Drop for Connection {
        fn drop(&mut self) {
            // SAFETY: only this thread iterates the mainloop; cancel callbacks
            // before their user data can be dropped, including on timeout.
            unsafe {
                for operation in &self.operations {
                    (self.api.cancel_op)(*operation);
                    (self.api.unref_op)(*operation);
                }
                if !self.context.is_null() {
                    (self.api.disconnect)(self.context);
                    (self.api.unref)(self.context);
                }
                if !self.main.is_null() {
                    (self.api.main_free)(self.main);
                }
            }
        }
    }
    #[derive(Default)]
    struct Discovery {
        devices: Vec<MicrophoneDevice>,
        default: String,
        server_done: bool,
        sources_done: bool,
        failed: bool,
    }
    unsafe fn string(pointer: *const c_char) -> String {
        if pointer.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned()
        }
    }
    unsafe extern "C" fn source(_context: Ptr, info: *const Source, end: c_int, data: Ptr) {
        // SAFETY: libpulse owns info during this call; Discovery remains alive
        // until Connection's drop cancels every operation and disconnects.
        unsafe {
            let state = &mut *data.cast::<Discovery>();
            if end != 0 {
                state.sources_done = true;
                state.failed |= end < 0;
                return;
            }
            if info.is_null() || state.devices.len() >= 128 {
                return;
            }
            let info = &*info;
            if info.monitor_of_sink != u32::MAX {
                return;
            }
            let id = string(info.name);
            if id.is_empty() {
                return;
            }
            let name = string(info.description);
            state.devices.push(MicrophoneDevice {
                name: if name.is_empty() { id.clone() } else { name },
                id,
                is_default: false,
            });
        }
    }
    unsafe extern "C" fn server(_context: Ptr, info: *const Server, data: Ptr) {
        unsafe {
            let state = &mut *data.cast::<Discovery>();
            state.server_done = true;
            if !info.is_null() {
                state.default = string((*info).default_source);
            }
        }
    }
    pub fn microphones() -> Result<Vec<MicrophoneDevice>> {
        let api = api()?;
        let mut result = Box::<Discovery>::default();
        // Declared after result so disconnect/cancellation happens first.
        let mut connection = Connection {
            api,
            main: unsafe { (api.main_new)() },
            context: std::ptr::null_mut(),
            operations: Vec::new(),
        };
        ensure!(
            !connection.main.is_null(),
            "Could not open audio device discovery"
        );
        connection.context = unsafe {
            (api.context_new)(
                (api.main_api)(connection.main),
                c"Slipstream microphone choices".as_ptr(),
            )
        };
        ensure!(
            !connection.context.is_null(),
            "Could not connect to audio device discovery"
        );
        ensure!(
            unsafe {
                (api.connect)(
                    connection.context,
                    std::ptr::null(),
                    1,
                    std::ptr::null_mut(),
                )
            } >= 0,
            "Could not connect to PulseAudio"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut requested = false;
        while !(result.server_done && result.sources_done) {
            ensure!(
                Instant::now() < deadline,
                "Microphone discovery timed out; refresh devices to retry"
            );
            ensure!(
                unsafe { (api.iterate)(connection.main, 0, std::ptr::null_mut()) } >= 0,
                "Audio discovery stopped"
            );
            match unsafe { (api.state)(connection.context) } {
                4 if !requested => {
                    requested = true;
                    let data = (&mut *result as *mut Discovery).cast();
                    for operation in [
                        unsafe { (api.server)(connection.context, server, data) },
                        unsafe { (api.sources)(connection.context, source, data) },
                    ] {
                        if !operation.is_null() {
                            connection.operations.push(operation);
                        }
                    }
                    ensure!(
                        connection.operations.len() == 2,
                        "Could not list microphone devices"
                    );
                }
                5 | 6 => bail!("Audio service is unavailable"),
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        ensure!(!result.failed, "Could not read microphone devices");
        drop(connection);
        for device in &mut result.devices {
            device.is_default = device.id == result.default;
        }
        Ok(std::mem::take(&mut result.devices))
    }
}
