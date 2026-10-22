//! Linux audio through PulseAudio's simple API, loaded at runtime (dlopen)
//! like PipeWire is for video, so the portable binary starts without any
//! audio libraries. PipeWire desktops provide it through pipewire-pulse.
//! Capture records the default output's monitor, which is what the
//! computer is playing.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    sync::OnceLock,
};

use anyhow::{Result, anyhow, bail};

use crate::opus::SAMPLE_RATE;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const SAMPLE_S16LE: c_int = 3;
const PLAYBACK: c_int = 1;
const RECORD: c_int = 2;
const DEFAULT: u32 = u32::MAX;

type New = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const SampleSpec,
    *const c_void,
    *const BufferAttr,
    *mut c_int,
) -> *mut c_void;
type Io = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_int) -> c_int;
type Free = unsafe extern "C" fn(*mut c_void);
type StrError = unsafe extern "C" fn(c_int) -> *const c_char;

struct Api {
    new: New,
    read: Io,
    write: Io,
    free: Free,
    strerror: StrError,
}

fn api() -> Result<&'static Api> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: dlopen/dlsym with valid C strings; each symbol is cast to
        // the signature documented in <pulse/simple.h> and <pulse/error.h>.
        unsafe {
            let lib = libc::dlopen(c"libpulse-simple.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if lib.is_null() {
                return Err("PulseAudio isn't available (libpulse-simple.so.0 not found)".into());
            }
            let sym = |name: &CStr| {
                let p = libc::dlsym(lib, name.as_ptr());
                if p.is_null() { Err(format!("PulseAudio: {} missing", name.to_string_lossy())) } else { Ok(p) }
            };
            Ok(Api {
                new: std::mem::transmute::<*mut c_void, New>(sym(c"pa_simple_new")?),
                read: std::mem::transmute::<*mut c_void, Io>(sym(c"pa_simple_read")?),
                write: std::mem::transmute::<*mut c_void, Io>(sym(c"pa_simple_write")?),
                free: std::mem::transmute::<*mut c_void, Free>(sym(c"pa_simple_free")?),
                strerror: std::mem::transmute::<*mut c_void, StrError>(sym(c"pa_strerror")?),
            })
        }
    })
    .as_ref()
    .map_err(|e| anyhow!("{e}"))
}

pub fn available() -> bool {
    api().is_ok()
}

/// One PulseAudio stream: 48 kHz stereo, 16-bit.
pub struct Stream {
    raw: *mut c_void,
    api: &'static Api,
}

// SAFETY: a pa_simple handle may be used from any one thread at a time,
// which `&mut self` guarantees.
unsafe impl Send for Stream {}

impl Stream {
    fn open(direction: c_int, device: Option<&CStr>, latency_ms: u32) -> Result<Self> {
        let api = api()?;
        let spec = SampleSpec { format: SAMPLE_S16LE, rate: SAMPLE_RATE, channels: 2 };
        let bytes = SAMPLE_RATE / 1000 * 4 * latency_ms;
        let attr = BufferAttr {
            maxlength: DEFAULT,
            // Playback: keep only ~latency queued in the server.
            tlength: if direction == PLAYBACK { bytes } else { DEFAULT },
            prebuf: DEFAULT,
            minreq: DEFAULT,
            // Capture: hand us audio in pieces of ~latency.
            fragsize: if direction == RECORD { bytes } else { DEFAULT },
        };
        let name = CString::new("Slipstream Remote").expect("no NUL");
        let what = if direction == RECORD { c"Remote session (sharing sound)" } else { c"Remote computer's sound" };
        let mut err = 0;
        // SAFETY: all pointers are valid for the call; NULL server/map = defaults.
        let raw = unsafe {
            (api.new)(
                std::ptr::null(),
                name.as_ptr(),
                direction,
                device.map_or(std::ptr::null(), CStr::as_ptr),
                what.as_ptr(),
                &spec,
                std::ptr::null(),
                &attr,
                &mut err,
            )
        };
        if raw.is_null() {
            bail!("PulseAudio: {}", error(api, err));
        }
        Ok(Self { raw, api })
    }

    /// Records what the computer plays (the default output's monitor).
    pub fn record(latency_ms: u32) -> Result<Self> {
        Self::open(RECORD, Some(c"@DEFAULT_MONITOR@"), latency_ms)
    }

    pub fn microphone(source: Option<&str>) -> Result<Self> {
        let source = source.map(CString::new).transpose()?;
        Self::open(RECORD, source.as_deref(), 20)
    }

    pub fn playback(latency_ms: u32) -> Result<Self> {
        Self::open(PLAYBACK, None, latency_ms)
    }

    /// Fills `pcm` (blocks until it has that much).
    pub fn read(&mut self, pcm: &mut [i16]) -> Result<()> {
        let mut err = 0;
        // SAFETY: `pcm` is valid for writes of its byte length.
        let rc = unsafe { (self.api.read)(self.raw, pcm.as_mut_ptr().cast(), std::mem::size_of_val(pcm), &mut err) };
        if rc < 0 {
            bail!("PulseAudio: {}", error(self.api, err));
        }
        Ok(())
    }

    /// Plays `pcm` (blocks while the server's buffer is full).
    pub fn write(&mut self, pcm: &[i16]) -> Result<()> {
        let mut err = 0;
        // SAFETY: `pcm` is valid for reads of its byte length; pa_simple_write
        // takes a const pointer despite the shared signature here.
        let rc = unsafe { (self.api.write)(self.raw, pcm.as_ptr().cast_mut().cast(), std::mem::size_of_val(pcm), &mut err) };
        if rc < 0 {
            bail!("PulseAudio: {}", error(self.api, err));
        }
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: created by pa_simple_new, freed once.
        unsafe { (self.api.free)(self.raw) }
    }
}

fn error(api: &Api, code: c_int) -> String {
    // SAFETY: pa_strerror returns a static string (or NULL).
    let p = unsafe { (api.strerror)(code) };
    if p.is_null() { format!("error {code}") } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}
