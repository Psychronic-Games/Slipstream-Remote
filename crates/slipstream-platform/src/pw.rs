//! Minimal PipeWire client, loaded at runtime with `dlopen`.
//!
//! Linking libpipewire directly would stop the whole app from starting on
//! machines without PipeWire (servers, older or minimal distros), even
//! though it's only needed to share a Wayland screen. So we load
//! `libpipewire-0.3.so.0` on first use and bind just the handful of
//! functions we need; data structures come from the header-only libspa.

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    os::fd::{IntoRawFd, OwnedFd},
    ptr,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use libloading::Library;
use libspa::{
    param::{
        ParamType,
        format::{MediaSubtype, MediaType},
        format_utils,
        video::{VideoFormat, VideoInfoRaw},
    },
    pod::Pod,
    sys as spa,
};
use slipstream_codec::{PixelFormat, RawFrame};

use crate::FrameSlot;

type Opaque = c_void;

/// `struct pw_stream_events` (version 2).
#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const spa::spa_pod)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// Leading fields of `struct pw_buffer` (all we read).
#[repr(C)]
struct PwBuffer {
    buffer: *mut spa::spa_buffer,
}

const PW_DIRECTION_INPUT: u32 = 0;
const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;
const PW_STREAM_STATE_ERROR: c_int = -1;

macro_rules! api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        struct Api {
            _lib: Library,
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            fn load() -> Result<Self, String> {
                // SAFETY: loading a well-known system library; its init
                // routines are safe to run.
                let lib = unsafe { Library::new("libpipewire-0.3.so.0") }
                    .or_else(|_| unsafe { Library::new("libpipewire-0.3.so") })
                    .map_err(|_| "PipeWire is not installed (libpipewire-0.3.so.0 not found). It is needed to share the screen on Wayland.".to_string())?;
                // SAFETY: the signatures match the PipeWire 0.3/1.x C API.
                unsafe {
                    Ok(Self {
                        $($name: *lib
                            .get(concat!("pw_", stringify!($name), "\0").as_bytes())
                            .map_err(|e| format!("PipeWire is too old: {e}"))?,)*
                        _lib: lib,
                    })
                }
            }
        }
    };
}

api! {
    init: fn(*mut c_int, *mut *mut *mut c_char);
    thread_loop_new: fn(*const c_char, *const spa::spa_dict) -> *mut Opaque;
    thread_loop_get_loop: fn(*mut Opaque) -> *mut Opaque;
    thread_loop_start: fn(*mut Opaque) -> c_int;
    thread_loop_stop: fn(*mut Opaque);
    thread_loop_destroy: fn(*mut Opaque);
    thread_loop_lock: fn(*mut Opaque);
    thread_loop_unlock: fn(*mut Opaque);
    context_new: fn(*mut Opaque, *mut Opaque, usize) -> *mut Opaque;
    context_destroy: fn(*mut Opaque);
    context_connect_fd: fn(*mut Opaque, c_int, *mut Opaque, usize) -> *mut Opaque;
    core_disconnect: fn(*mut Opaque) -> c_int;
    properties_new_string: fn(*const c_char) -> *mut Opaque;
    stream_new: fn(*mut Opaque, *const c_char, *mut Opaque) -> *mut Opaque;
    stream_add_listener: fn(*mut Opaque, *mut spa::spa_hook, *const StreamEvents, *mut c_void);
    stream_connect: fn(*mut Opaque, u32, u32, u32, *mut *const spa::spa_pod, u32) -> c_int;
    stream_dequeue_buffer: fn(*mut Opaque) -> *mut PwBuffer;
    stream_queue_buffer: fn(*mut Opaque, *mut PwBuffer) -> c_int;
    stream_disconnect: fn(*mut Opaque) -> c_int;
    stream_destroy: fn(*mut Opaque);
}

fn api() -> Result<&'static Api> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let api = Api::load()?;
        // SAFETY: pw_init accepts null argc/argv.
        unsafe { (api.init)(ptr::null_mut(), ptr::null_mut()) };
        Ok(api)
    })
    .as_ref()
    .map_err(|e| anyhow!("{e}"))
}

/// Is PipeWire usable on this machine?
pub fn available() -> Result<()> {
    api().map(|_| ())
}

/// Per-stream state, touched only from PipeWire's loop thread.
struct State {
    api: &'static Api,
    stream: *mut Opaque,
    format: VideoInfoRaw,
    slot: Arc<FrameSlot>,
    last: Instant,
    min_interval: Duration,
}

/// A running capture of one PipeWire node. Dropping it stops the capture.
pub struct Capture {
    api: &'static Api,
    tloop: *mut Opaque,
    context: *mut Opaque,
    core: *mut Opaque,
    stream: *mut Opaque,
    _hook: Box<spa::spa_hook>,
    _events: Box<StreamEvents>,
    _state: Box<State>,
}

// SAFETY: every PipeWire object is only touched while holding the thread
// loop's lock (or after the loop has stopped), as PipeWire requires.
unsafe impl Send for Capture {}

impl Capture {
    /// Starts streaming frames from `node` (on the portal's PipeWire `fd`)
    /// into `slot`.
    pub fn start(fd: OwnedFd, node: u32, slot: Arc<FrameSlot>, min_interval: Duration) -> Result<Self> {
        let api = api()?;
        // SAFETY: straightforward use of the PipeWire C API; all pointers
        // are checked, and objects are torn down in Drop.
        unsafe {
            let tloop = (api.thread_loop_new)(c"slipstream-capture".as_ptr(), ptr::null());
            if tloop.is_null() {
                bail!("PipeWire: cannot create loop");
            }
            let mut cap = Capture {
                api,
                tloop,
                context: ptr::null_mut(),
                core: ptr::null_mut(),
                stream: ptr::null_mut(),
                _hook: Box::new(std::mem::zeroed()),
                _events: Box::new(StreamEvents {
                    version: 2,
                    destroy: None,
                    state_changed: Some(on_state_changed),
                    control_info: None,
                    io_changed: None,
                    param_changed: Some(on_param_changed),
                    add_buffer: None,
                    remove_buffer: None,
                    process: Some(on_process),
                    drained: None,
                    command: None,
                    trigger_done: None,
                }),
                _state: Box::new(State {
                    api,
                    stream: ptr::null_mut(),
                    format: VideoInfoRaw::default(),
                    slot,
                    last: Instant::now() - min_interval,
                    min_interval,
                }),
            };
            let lp = (api.thread_loop_get_loop)(tloop);
            cap.context = (api.context_new)(lp, ptr::null_mut(), 0);
            if cap.context.is_null() {
                bail!("PipeWire: cannot create context");
            }
            if (api.thread_loop_start)(tloop) < 0 {
                bail!("PipeWire: cannot start loop");
            }
            (api.thread_loop_lock)(tloop);
            let result = cap.connect(fd, node);
            (api.thread_loop_unlock)(tloop);
            result?;
            Ok(cap)
        }
    }

    /// Must be called with the loop locked.
    unsafe fn connect(&mut self, fd: OwnedFd, node: u32) -> Result<()> {
        let api = self.api;
        // SAFETY: called with the loop lock held; see `start`.
        unsafe {
            self.core = (api.context_connect_fd)(self.context, fd.into_raw_fd(), ptr::null_mut(), 0);
            if self.core.is_null() {
                bail!("PipeWire: cannot connect to the screen-sharing session");
            }
            let props = (api.properties_new_string)(c"media.type=Video media.category=Capture media.role=Screen".as_ptr());
            self.stream = (api.stream_new)(self.core, c"slipstream-capture".as_ptr(), props);
            if self.stream.is_null() {
                bail!("PipeWire: cannot create stream");
            }
            self._state.stream = self.stream;
            (api.stream_add_listener)(
                self.stream,
                &mut *self._hook,
                &*self._events,
                (&mut *self._state as *mut State).cast(),
            );
            let pod_bytes = format_pod()?;
            let mut params = [pod_bytes.as_ptr().cast::<spa::spa_pod>()];
            let rc = (api.stream_connect)(
                self.stream,
                PW_DIRECTION_INPUT,
                node,
                PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS,
                params.as_mut_ptr(),
                1,
            );
            if rc < 0 {
                bail!("PipeWire: cannot connect stream (error {rc})");
            }
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let api = self.api;
        // SAFETY: tear down in reverse order; stream/core under the lock,
        // the loop is stopped before the context is destroyed.
        unsafe {
            (api.thread_loop_lock)(self.tloop);
            if !self.stream.is_null() {
                (api.stream_disconnect)(self.stream);
                (api.stream_destroy)(self.stream);
            }
            if !self.core.is_null() {
                (api.core_disconnect)(self.core);
            }
            (api.thread_loop_unlock)(self.tloop);
            (api.thread_loop_stop)(self.tloop);
            if !self.context.is_null() {
                (api.context_destroy)(self.context);
            }
            (api.thread_loop_destroy)(self.tloop);
        }
    }
}

/// The formats we accept: 32-bit RGB variants, any size, up to 1000 fps.
fn format_pod() -> Result<Vec<u8>> {
    use libspa::{
        param::format::FormatProperties,
        pod::{Value, serialize::PodSerializer},
        utils::{Fraction, Rectangle, SpaTypes},
    };
    let obj = libspa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        libspa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        libspa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        libspa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        libspa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        libspa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 60, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    Ok(PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .map_err(|e| anyhow!("pod serialize: {e:?}"))?
        .0
        .into_inner())
}

unsafe extern "C" fn on_state_changed(data: *mut c_void, _old: c_int, new: c_int, error: *const c_char) {
    if new == PW_STREAM_STATE_ERROR {
        // SAFETY: `data` is our State; `error` is a C string or null.
        let st = unsafe { &*(data as *const State) };
        let msg = if error.is_null() { "stream error".into() } else { unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned() };
        st.slot.fail(format!("PipeWire: {msg}"));
    }
}

unsafe extern "C" fn on_param_changed(data: *mut c_void, id: u32, param: *const spa::spa_pod) {
    if param.is_null() || id != ParamType::Format.as_raw() {
        return;
    }
    // SAFETY: `data` is our State; `param` is a valid pod for this call.
    let st = unsafe { &mut *(data as *mut State) };
    let pod = unsafe { Pod::from_raw(param) };
    let Ok((media_type, media_subtype)) = format_utils::parse_format(pod) else { return };
    if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
        return;
    }
    if st.format.parse(pod).is_ok() {
        let size = st.format.size();
        tracing::info!("capture format {:?} {}x{}", st.format.format(), size.width, size.height);
    }
}

unsafe extern "C" fn on_process(data: *mut c_void) {
    // SAFETY: `data` is our State, used only on the loop thread.
    let st = unsafe { &mut *(data as *mut State) };
    let buf = unsafe { (st.api.stream_dequeue_buffer)(st.stream) };
    if buf.is_null() {
        return;
    }
    // SAFETY: buffers are mapped (MAP_BUFFERS); we bounds-check the chunk.
    unsafe {
        take_frame(st, buf);
        (st.api.stream_queue_buffer)(st.stream, buf);
    }
}

unsafe fn take_frame(st: &mut State, buf: *mut PwBuffer) {
    // Allow 20% early: frames arrive with jitter, and a strict check would
    // drop every other frame at the monitor's refresh rate.
    if st.last.elapsed() < st.min_interval.mul_f32(0.8) {
        return;
    }
    // SAFETY: see on_process.
    let sbuf = unsafe { (*buf).buffer };
    if sbuf.is_null() || unsafe { (*sbuf).n_datas } == 0 {
        return;
    }
    let d = unsafe { &*(*sbuf).datas };
    if d.data.is_null() || d.chunk.is_null() {
        return;
    }
    let chunk = unsafe { &*d.chunk };
    let format = match st.format.format() {
        VideoFormat::BGRx | VideoFormat::BGRA => PixelFormat::Bgra,
        VideoFormat::RGBx | VideoFormat::RGBA => PixelFormat::Rgba,
        _ => return,
    };
    let dims = st.format.size();
    let (w, h) = (dims.width, dims.height);
    let stride = if chunk.stride > 0 { chunk.stride as u32 } else { w * 4 };
    let offset = chunk.offset as usize;
    let needed = (stride * h.saturating_sub(1) + w * 4) as usize;
    if chunk.size == 0 || w == 0 || h == 0 || offset + needed > d.maxsize as usize {
        return; // cursor-only update or bad buffer
    }
    let bytes = unsafe { std::slice::from_raw_parts((d.data as *const u8).add(offset), needed) };
    st.last = Instant::now();
    st.slot.put(RawFrame { width: w, height: h, stride, format, data: bytes.to_vec() });
}
