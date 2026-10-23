//! NVIDIA hardware H.264 encoding (NVENC).
//!
//! Uses the driver's own libraries, loaded when first needed: the CUDA driver
//! (`nvcuda.dll` / `libcuda.so.1`) for a context and GPU memory, and the
//! encoder (`nvEncodeAPI64.dll` / `libnvidia-encode.so.1`). Nothing NVIDIA is
//! linked at build time, so machines without the driver just never get here.
//!
//! Each frame is copied into GPU memory as BGRA (or RGBA), and NVENC does the
//! colour conversion and encoding there: Baseline profile H.264 (what the
//! viewer's OpenH264 decoder takes), no B-frames, one-frame CBR buffer and
//! IDR only on request, for the lowest latency.
mod sys;

use std::{
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::OnceLock,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use sys::*;

use crate::{EncodedFrame, PixelFormat, RawFrame};

type CuResult = c_int;
type CuContext = *mut c_void;
type CuDevicePtr = u64;

/// `CUDA_MEMCPY2D` from the CUDA driver API.
#[repr(C)]
struct CudaMemcpy2d {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: c_int,
    src_host: *const c_void,
    src_device: CuDevicePtr,
    src_array: *mut c_void,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: c_int,
    dst_host: *mut c_void,
    dst_device: CuDevicePtr,
    dst_array: *mut c_void,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}
const CU_MEMORYTYPE_HOST: c_int = 1;
const CU_MEMORYTYPE_DEVICE: c_int = 2;

/// The oldest NVENC API this module speaks: 12.0 (drivers from late 2022,
/// 520 on Linux and 522 on Windows).
const MIN_API: u32 = (NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION;
const INFINITE_GOP: u32 = 0xffff_ffff;

const fn struct_version(ver: u32) -> u32 {
    NVENCAPI_VERSION | (ver << 16) | (0x7 << 28)
}

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> GUID {
    GUID { Data1: d1, Data2: d2, Data3: d3, Data4: d4 }
}
const H264: GUID = guid(0x6bc82762, 0x4e63, 0x4ca4, [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf]);
const BASELINE: GUID = guid(0x0727bcaa, 0x78c4, 0x4c83, [0x8c, 0x2f, 0xef, 0x3d, 0xff, 0x26, 0x7c, 0x6a]);
/// P1: the fastest preset. Screen content gains little from slower ones.
const PRESET_P1: GUID = guid(0xfc0a8d3e, 0x45f8, 0x4cf8, [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf]);

/// The driver's libraries and the functions taken from them.
struct Api {
    _cuda: libloading::Library,
    _nvenc: libloading::Library,
    nv: NV_ENCODE_API_FUNCTION_LIST,
    /// The first NVIDIA GPU's primary context, shared by every encoder for
    /// the life of the process (creating one costs ~100 ms).
    ctx: CuContext,
    /// Its name, e.g. "NVIDIA GeForce RTX 3060 Laptop GPU".
    device: String,
    cu_ctx_push: unsafe extern "system" fn(CuContext) -> CuResult,
    cu_ctx_pop: unsafe extern "system" fn(*mut CuContext) -> CuResult,
    cu_mem_alloc_pitch: unsafe extern "system" fn(*mut CuDevicePtr, *mut usize, usize, usize, u32) -> CuResult,
    cu_mem_free: unsafe extern "system" fn(CuDevicePtr) -> CuResult,
    cu_memcpy_2d: unsafe extern "system" fn(*const CudaMemcpy2d) -> CuResult,
}

// SAFETY: plain function pointers into libraries that stay loaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// Loads a driver library: only from System32 on Windows, so a DLL planted
/// beside the app (e.g. in Downloads) can't stand in for the driver's.
fn open(name: &str) -> Result<libloading::Library> {
    #[cfg(windows)]
    // SAFETY: loading a system library; its initialisers are the driver's.
    let lib = unsafe {
        libloading::os::windows::Library::load_with_flags(name, libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32)
            .map(Into::into)
    };
    #[cfg(not(windows))]
    // SAFETY: as above.
    let lib = unsafe { libloading::Library::new(name) };
    lib.with_context(|| format!("{name} isn't available (no NVIDIA driver?)"))
}

impl Api {
    fn load() -> Result<Self> {
        let (cuda_name, nvenc_name) =
            if cfg!(windows) { ("nvcuda.dll", "nvEncodeAPI64.dll") } else { ("libcuda.so.1", "libnvidia-encode.so.1") };
        let cuda = open(cuda_name)?;
        let nvenc = open(nvenc_name)?;
        // SAFETY: the symbols have these signatures in the CUDA driver API
        // and the NVENC API (nvEncodeAPI.h).
        unsafe {
            macro_rules! sym {
                ($lib:expr, $name:literal) => {
                    *$lib.get(concat!($name, "\0").as_bytes()).context(concat!("missing ", $name))?
                };
            }
            let max_version: unsafe extern "system" fn(*mut u32) -> NVENCSTATUS =
                sym!(nvenc, "NvEncodeAPIGetMaxSupportedVersion");
            let create: unsafe extern "system" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS =
                sym!(nvenc, "NvEncodeAPICreateInstance");
            let mut version = 0;
            check(max_version(&mut version), "NvEncodeAPIGetMaxSupportedVersion")?;
            ensure!(
                version >= MIN_API,
                "the NVIDIA driver supports NVENC {}.{}, 12.0 is needed: update the driver",
                version >> 4,
                version & 15
            );
            let mut nv: NV_ENCODE_API_FUNCTION_LIST = std::mem::zeroed();
            nv.version = struct_version(2);
            check(create(&mut nv), "NvEncodeAPICreateInstance")?;

            let cu_init: unsafe extern "system" fn(u32) -> CuResult = sym!(cuda, "cuInit");
            let cu_device_get: unsafe extern "system" fn(*mut c_int, c_int) -> CuResult = sym!(cuda, "cuDeviceGet");
            let cu_device_get_name: unsafe extern "system" fn(*mut c_char, c_int, c_int) -> CuResult =
                sym!(cuda, "cuDeviceGetName");
            let cu_primary_ctx_retain: unsafe extern "system" fn(*mut CuContext, c_int) -> CuResult =
                sym!(cuda, "cuDevicePrimaryCtxRetain");
            cu(cu_init(0), "cuInit")?;
            let mut dev = 0;
            cu(cu_device_get(&mut dev, 0), "cuDeviceGet")?;
            let mut name = [0 as c_char; 128];
            let device = if cu_device_get_name(name.as_mut_ptr(), name.len() as c_int, dev) == 0 {
                std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned()
            } else {
                "NVIDIA GPU".into()
            };
            let mut ctx = ptr::null_mut();
            // Retained for good: released when the process ends.
            cu(cu_primary_ctx_retain(&mut ctx, dev), "cuDevicePrimaryCtxRetain")?;
            Ok(Self {
                ctx,
                device,
                cu_ctx_push: sym!(cuda, "cuCtxPushCurrent_v2"),
                cu_ctx_pop: sym!(cuda, "cuCtxPopCurrent_v2"),
                cu_mem_alloc_pitch: sym!(cuda, "cuMemAllocPitch_v2"),
                cu_mem_free: sym!(cuda, "cuMemFree_v2"),
                cu_memcpy_2d: sym!(cuda, "cuMemcpy2D_v2"),
                nv,
                _cuda: cuda,
                _nvenc: nvenc,
            })
        }
    }

    /// Loaded once per process; an error is remembered too.
    fn get() -> Result<&'static Api> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        API.get_or_init(|| Api::load().map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| anyhow!("{e}"))
    }
}

fn check(status: NVENCSTATUS, what: &str) -> Result<()> {
    if status == _NVENCSTATUS_NV_ENC_SUCCESS { Ok(()) } else { bail!("{what} failed (NVENC status {status})") }
}

fn cu(result: CuResult, what: &str) -> Result<()> {
    if result == 0 { Ok(()) } else { bail!("{what} failed (CUDA error {result})") }
}

/// Calls an NVENC function from the function list.
macro_rules! nv {
    ($api:expr, $f:ident($($arg:expr),*)) => {
        check(($api.nv.$f.ok_or_else(|| anyhow!(concat!(stringify!($f), " missing")))?)($($arg),*), stringify!($f))
    };
}

/// One NVENC session for one picture size.
pub struct NvEncoder {
    api: &'static Api,
    encoder: *mut c_void,
    config: Box<NV_ENC_CONFIG>,
    init: Box<NV_ENC_INITIALIZE_PARAMS>,
    input: CuDevicePtr,
    pitch: usize,
    registered: NV_ENC_REGISTERED_PTR,
    bitstream: NV_ENC_OUTPUT_PTR,
    format: PixelFormat,
    width: u32,
    height: u32,
    frames: u64,
}

// SAFETY: the CUDA context is made current around every call
// (`Current`), so the encoder may move between threads; it's used by one
// thread at a time (&mut self), and NVENC sessions are independent.
unsafe impl Send for NvEncoder {}

/// Makes the encoder's CUDA context current on this thread for a scope.
struct Current<'a>(&'a Api);
impl<'a> Current<'a> {
    fn new(api: &'a Api, ctx: CuContext) -> Result<Self> {
        // SAFETY: a live context.
        cu(unsafe { (api.cu_ctx_push)(ctx) }, "cuCtxPushCurrent")?;
        Ok(Self(api))
    }
}
impl Drop for Current<'_> {
    fn drop(&mut self) {
        let mut ctx = ptr::null_mut();
        // SAFETY: pops what `new` pushed.
        unsafe { (self.0.cu_ctx_pop)(&mut ctx) };
    }
}

impl NvEncoder {
    /// Opens an encoder for `width`×`height` (even) frames of `format`.
    pub fn new(width: u32, height: u32, fps: f32, kbps: u32, format: PixelFormat) -> Result<Self> {
        ensure!(width >= 2 && height >= 2 && width.is_multiple_of(2) && height.is_multiple_of(2), "odd picture size");
        let api = Api::get()?;
        let mut enc = Self {
            api,
            encoder: ptr::null_mut(),
            // SAFETY: plain C structs, all-zero is their documented start.
            config: Box::new(unsafe { std::mem::zeroed() }),
            init: Box::new(unsafe { std::mem::zeroed() }),
            input: 0,
            pitch: 0,
            registered: ptr::null_mut(),
            bitstream: ptr::null_mut(),
            format,
            width,
            height,
            frames: 0,
        };
        enc.open(fps, kbps)?;
        Ok(enc)
    }

    /// The GPU doing the work, e.g. "NVIDIA GeForce RTX 3060 Laptop GPU".
    pub fn device(&self) -> &str {
        &self.api.device
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    fn open(&mut self, fps: f32, kbps: u32) -> Result<()> {
        let api = self.api;
        // SAFETY: CUDA driver and NVENC calls with structs set up as
        // nvEncodeAPI.h describes; every handle created here is released
        // in Drop, also when a later step fails.
        unsafe {
            let _current = Current::new(api, api.ctx)?;

            let mut session: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
            session.version = struct_version(1);
            session.deviceType = _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA;
            session.device = api.ctx;
            session.apiVersion = NVENCAPI_VERSION;
            nv!(api, nvEncOpenEncodeSessionEx(&mut session, &mut self.encoder))?;

            let mut preset: NV_ENC_PRESET_CONFIG = std::mem::zeroed();
            preset.version = struct_version(4) | (1 << 31);
            preset.presetCfg.version = struct_version(8) | (1 << 31);
            nv!(api, nvEncGetEncodePresetConfigEx(
                self.encoder,
                H264,
                PRESET_P1,
                NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                &mut preset
            ))?;
            *self.config = preset.presetCfg;
            let c = &mut *self.config;
            c.version = struct_version(8) | (1 << 31);
            c.profileGUID = BASELINE;
            c.gopLength = INFINITE_GOP;
            c.frameIntervalP = 1;
            let h264 = &mut c.encodeCodecConfig.h264Config;
            h264.idrPeriod = INFINITE_GOP;
            h264.entropyCodingMode = _NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC;
            h264.set_repeatSPSPPS(1);
            h264.maxNumRefFrames = 1;
            self.set_rate(fps, kbps);

            let i = &mut *self.init;
            i.version = struct_version(5) | (1 << 31);
            i.encodeGUID = H264;
            i.presetGUID = PRESET_P1;
            i.encodeWidth = self.width;
            i.encodeHeight = self.height;
            i.darWidth = self.width;
            i.darHeight = self.height;
            i.maxEncodeWidth = self.width;
            i.maxEncodeHeight = self.height;
            i.frameRateNum = fps.round().clamp(1.0, 240.0) as u32;
            i.frameRateDen = 1;
            i.enablePTD = 1;
            i.tuningInfo = NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
            i.encodeConfig = &mut *self.config;
            nv!(api, nvEncInitializeEncoder(self.encoder, &mut *self.init))?;

            let row = self.width as usize * 4;
            cu((api.cu_mem_alloc_pitch)(&mut self.input, &mut self.pitch, row, self.height as usize, 16), "cuMemAllocPitch")?;
            let mut reg: NV_ENC_REGISTER_RESOURCE = std::mem::zeroed();
            reg.version = struct_version(4);
            reg.resourceType = _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR;
            reg.width = self.width;
            reg.height = self.height;
            reg.pitch = self.pitch as u32;
            reg.resourceToRegister = self.input as *mut c_void;
            reg.bufferFormat = self.buffer_format();
            reg.bufferUsage = _NV_ENC_BUFFER_USAGE_NV_ENC_INPUT_IMAGE;
            nv!(api, nvEncRegisterResource(self.encoder, &mut reg))?;
            self.registered = reg.registeredResource;

            let mut out: NV_ENC_CREATE_BITSTREAM_BUFFER = std::mem::zeroed();
            out.version = struct_version(1);
            nv!(api, nvEncCreateBitstreamBuffer(self.encoder, &mut out))?;
            self.bitstream = out.bitstreamBuffer;
        }
        tracing::info!(device = %api.device, width = self.width, height = self.height, kbps, "NVENC hardware encoding");
        Ok(())
    }

    /// NVENC's "ARGB" is a 32-bit word with B in the low byte: BGRA in memory.
    fn buffer_format(&self) -> NV_ENC_BUFFER_FORMAT {
        match self.format {
            PixelFormat::Bgra => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_ARGB,
            PixelFormat::Rgba => _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_ABGR,
        }
    }

    /// Constant bitrate with a one-frame buffer: each frame stays about the
    /// size the link can carry in one frame interval.
    fn set_rate(&mut self, fps: f32, kbps: u32) {
        let bps = kbps.saturating_mul(1000).max(100_000);
        let per_frame = (bps as f32 / fps.clamp(1.0, 240.0)) as u32;
        let rc = &mut self.config.rcParams;
        rc.rateControlMode = _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR;
        rc.multiPass = _NV_ENC_MULTI_PASS_NV_ENC_MULTI_PASS_DISABLED;
        rc.averageBitRate = bps;
        rc.maxBitRate = bps;
        rc.vbvBufferSize = per_frame;
        rc.vbvInitialDelay = per_frame;
    }

    /// Changes the bitrate in place, without a keyframe.
    pub fn set_bitrate(&mut self, fps: f32, kbps: u32) -> Result<()> {
        self.set_rate(fps, kbps);
        let api = self.api;
        let _current = Current::new(api, api.ctx)?;
        // SAFETY: a live encoder; the init params point at our config.
        unsafe {
            let mut re: NV_ENC_RECONFIGURE_PARAMS = std::mem::zeroed();
            re.version = struct_version(1) | (1 << 31);
            self.init.encodeConfig = &mut *self.config;
            re.reInitEncodeParams = *self.init;
            nv!(api, nvEncReconfigureEncoder(self.encoder, &mut re))
        }
    }

    /// Encodes the top-left `width`×`height` of `frame`.
    pub fn encode(&mut self, frame: &RawFrame, force_keyframe: bool) -> Result<EncodedFrame> {
        ensure!(frame.format == self.format, "pixel format changed");
        ensure!(frame.width >= self.width && frame.height >= self.height, "frame smaller than the encoder");
        frame.validate()?;
        let api = self.api;
        let _current = Current::new(api, api.ctx)?;
        // SAFETY: the copy reads `height` rows of `width * 4` bytes at
        // `stride` from `frame.data`, which validate() checked, into our
        // pitched buffer; the NVENC structs follow nvEncodeAPI.h, and each
        // map/lock is undone before returning.
        unsafe {
            let copy = CudaMemcpy2d {
                src_x_in_bytes: 0,
                src_y: 0,
                src_memory_type: CU_MEMORYTYPE_HOST,
                src_host: frame.data.as_ptr().cast(),
                src_device: 0,
                src_array: ptr::null_mut(),
                src_pitch: frame.stride as usize,
                dst_x_in_bytes: 0,
                dst_y: 0,
                dst_memory_type: CU_MEMORYTYPE_DEVICE,
                dst_host: ptr::null_mut(),
                dst_device: self.input,
                dst_array: ptr::null_mut(),
                dst_pitch: self.pitch,
                width_in_bytes: self.width as usize * 4,
                height: self.height as usize,
            };
            cu((api.cu_memcpy_2d)(&copy), "cuMemcpy2D")?;

            let mut map: NV_ENC_MAP_INPUT_RESOURCE = std::mem::zeroed();
            map.version = struct_version(4);
            map.registeredResource = self.registered;
            nv!(api, nvEncMapInputResource(self.encoder, &mut map))?;
            let result = self.encode_mapped(map.mappedResource, force_keyframe);
            let unmapped = nv!(api, nvEncUnmapInputResource(self.encoder, map.mappedResource));
            let encoded = result?;
            unmapped?;
            Ok(encoded)
        }
    }

    /// SAFETY: `input` is a mapped input resource of this encoder.
    unsafe fn encode_mapped(&mut self, input: NV_ENC_INPUT_PTR, force_keyframe: bool) -> Result<EncodedFrame> {
        let api = self.api;
        unsafe {
            let mut pic: NV_ENC_PIC_PARAMS = std::mem::zeroed();
            pic.version = struct_version(6) | (1 << 31);
            pic.inputWidth = self.width;
            pic.inputHeight = self.height;
            pic.inputPitch = self.pitch as u32;
            pic.inputBuffer = input;
            pic.outputBitstream = self.bitstream;
            pic.bufferFmt = self.buffer_format();
            pic.pictureStruct = _NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME;
            pic.inputTimeStamp = self.frames;
            pic.frameIdx = self.frames as u32;
            if force_keyframe || self.frames == 0 {
                pic.encodePicFlags = _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR | _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
            }
            self.frames += 1;
            nv!(api, nvEncEncodePicture(self.encoder, &mut pic))?;

            let mut lock: NV_ENC_LOCK_BITSTREAM = std::mem::zeroed();
            lock.version = struct_version(2);
            lock.outputBitstream = self.bitstream;
            nv!(api, nvEncLockBitstream(self.encoder, &mut lock))?;
            let data = std::slice::from_raw_parts(lock.bitstreamBufferPtr as *const u8, lock.bitstreamSizeInBytes as usize).to_vec();
            let keyframe = matches!(lock.pictureType, _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR | _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_I);
            nv!(api, nvEncUnlockBitstream(self.encoder, self.bitstream))?;
            Ok(EncodedFrame { data, keyframe, width: self.width, height: self.height })
        }
    }
}

impl Drop for NvEncoder {
    fn drop(&mut self) {
        let api = self.api;
        // SAFETY: releases what `open` created, in reverse order, each only
        // if it was created. The shared context stays.
        unsafe {
            if let Ok(_current) = Current::new(api, api.ctx) {
                if !self.encoder.is_null() {
                    if !self.bitstream.is_null()
                        && let Some(f) = api.nv.nvEncDestroyBitstreamBuffer
                    {
                        f(self.encoder, self.bitstream);
                    }
                    if !self.registered.is_null()
                        && let Some(f) = api.nv.nvEncUnregisterResource
                    {
                        f(self.encoder, self.registered);
                    }
                    if let Some(f) = api.nv.nvEncDestroyEncoder {
                        f(self.encoder);
                    }
                }
                if self.input != 0 {
                    (api.cu_mem_free)(self.input);
                }
            }
        }
    }
}

/// Whether NVENC can be used here (the driver loads and is new enough).
#[cfg(test)]
pub fn available() -> Result<()> {
    Api::get().map(|_| ())
}
