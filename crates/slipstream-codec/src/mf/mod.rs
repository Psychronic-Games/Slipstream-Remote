//! Hardware H.264 encoding through Media Foundation (Windows).
//!
//! GPU drivers register their H.264 encoders as Media Foundation transforms
//! (MFTs): Intel Quick Sync, AMD and NVIDIA all ship one. This finds them
//! with `MFTEnumEx` and uses the first that opens and passes a two-frame
//! trial. Media Foundation itself is optional (Windows N and Server
//! editions may lack it, and the app delay-loads it), so its DLLs are
//! checked before any call.
//!
//! Frames are converted to NV12 on the processor, BT.601 limited range like
//! everywhere else, into system-memory samples that the driver uploads
//! itself. An MFT that refuses those is tried again with a Direct3D 11
//! device on its own adapter and NV12 textures. Output suits the viewer's
//! OpenH264 decoder: Baseline profile (so CAVLC and no B-frames), constant
//! bitrate, low-latency mode and IDR only on request.
//!
//! Hardware MFTs are asynchronous: they say through events when they want
//! input and when they have output. Those events are polled, so a stuck
//! driver costs a timeout rather than the pipeline thread, and each frame
//! waits for its own access unit, which low-latency mode delivers one for
//! one.
//!
//! For testing, `SLIPSTREAM_MF_SOFTWARE=1` also accepts software MFTs
//! (Microsoft's own, or Wine's), to exercise this code where no hardware
//! encoder exists. Those may lack the bitrate and keyframe controls (Wine's
//! do), which are then only logged.

use std::{
    mem::ManuallyDrop,
    ptr,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, HMODULE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_BIND_VIDEO_ENCODER, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
                D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device,
                ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
            },
            Dxgi::{
                CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1,
                Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
            },
        },
        Media::MediaFoundation::*,
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree},
            LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
            Variant::VARIANT,
        },
    },
    core::{GUID, Interface, PWSTR, w},
};
use yuv::{BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix};

use crate::{EncodedFrame, PixelFormat, RawFrame};

/// How long a frame may take before the encoder counts as stuck.
const TIMEOUT: Duration = Duration::from_millis(250);
/// The trial frames in `new` may take longer: drivers set up on the first.
const FIRST_TIMEOUT: Duration = Duration::from_millis(1500);
/// How often the encoder's events are checked while waiting.
const POLL: Duration = Duration::from_micros(500);
/// IDR only on request, as far as the encoder's GOP size goes (65535 is
/// 18 minutes at 60 frames a second); smaller ones if it refuses.
const GOP_SIZES: [u32; 2] = [0xffff, 3000];
/// Media Foundation counts time in 100 ns units.
const SECOND: f64 = 10_000_000.0;

/// Loads Media Foundation from System32 and starts it, once per process.
/// The app delay-loads its DLLs, so calling into a missing one would end
/// the process: nothing here touches Media Foundation before this passes.
fn runtime() -> Result<()> {
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| {
            for name in [w!("mf.dll"), w!("mfplat.dll"), w!("mfreadwrite.dll")] {
                // SAFETY: a terminated literal and a System32-only search.
                // The reference is kept until the process ends, so the
                // delay-load stubs always find the module.
                if unsafe { LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.is_err() {
                    return Err("Windows Media Foundation isn't installed (Media Feature Pack on N editions)".into());
                }
            }
            // SAFETY: plain start-up call; never shut down, as encoders come
            // and go for the life of the process.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(|e| format!("MFStartup failed: {e}"))
        })
        .clone()
        .map_err(|e| anyhow!(e))
}

/// Joins this thread to COM's multithreaded apartment, once per thread. A
/// thread already in a single-threaded apartment stays there, which suits
/// Media Foundation's free-threaded objects too.
fn com() {
    thread_local!(static JOINED: () = {
        // SAFETY: no reserved pointer; the reference is never released, so
        // COM stays up while the thread lives.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    });
    JOINED.with(|_| ());
}

/// Whether software MFTs count too (`SLIPSTREAM_MF_SOFTWARE=1`).
fn software_allowed() -> bool {
    std::env::var_os("SLIPSTREAM_MF_SOFTWARE").is_some_and(|v| v == "1")
}

/// The H.264 encoders that take NV12, best first.
fn candidates() -> Result<Vec<IMFActivate>> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let mut flags = MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;
    if software_allowed() {
        flags = flags | MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_LOCALMFT;
    }
    let mut list: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the out-array is CoTaskMemAlloc'd with `count` owned
    // references, each moved out once before the array is freed.
    unsafe {
        MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count)
            .context("listing H.264 encoders (MFTEnumEx)")?;
        let mut found = Vec::with_capacity(count as usize);
        if !list.is_null() {
            for i in 0..count as usize {
                found.extend(ptr::read(list.add(i)));
            }
            CoTaskMemFree(Some(list as *const _));
        }
        Ok(found)
    }
}

/// A string attribute of an activation object, if it has one.
fn string(attrs: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut p = PWSTR::null();
    let mut len = 0;
    // SAFETY: on success `p` is a CoTaskMemAlloc'd terminated string,
    // freed after copying.
    unsafe {
        attrs.GetAllocatedString(key, &mut p, &mut len).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Packs two 32-bit halves as `MFSetAttributeSize`/`Ratio` do.
fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// A frame rate as a ratio, whole where it's close enough.
fn rate(fps: f32) -> (u32, u32) {
    let fps = fps.clamp(1.0, 240.0);
    if (fps - fps.round()).abs() < 0.01 { (fps.round() as u32, 1) } else { ((fps * 1000.0).round() as u32, 1000) }
}

/// Direct3D 11 input, for an MFT that won't take system memory.
struct D3d {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    manager: IMFDXGIDeviceManager,
}

impl D3d {
    /// A video-capable device on the MFT's own adapter (or the first one).
    fn new(activate: &IMFActivate) -> Result<Self> {
        let mut luid = [0u8; 8];
        // SAFETY: an 8-byte blob (LUID) into an 8-byte buffer.
        let wanted = unsafe { activate.GetBlob(&MFT_ENUM_ADAPTER_LUID, &mut luid, None) }.is_ok().then_some(luid);
        Self::on(wanted)
    }

    /// A video-capable device on the adapter with this LUID, if given and
    /// found, else on the first adapter.
    fn on(wanted: Option<[u8; 8]>) -> Result<Self> {
        // SAFETY: DXGI and D3D11 calls with their documented arguments.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().context("CreateDXGIFactory1")?;
            let mut adapter = None;
            for i in 0.. {
                let Ok(a) = factory.EnumAdapters1(i) else { break };
                let desc = a.GetDesc1().context("IDXGIAdapter1::GetDesc1")?;
                let id = [desc.AdapterLuid.LowPart.to_le_bytes(), desc.AdapterLuid.HighPart.to_le_bytes()].concat();
                if adapter.is_none() || wanted.is_some_and(|w| w[..] == id[..]) {
                    adapter = Some(a);
                }
                if wanted.is_some_and(|w| w[..] == id[..]) {
                    break;
                }
            }
            let adapter: IDXGIAdapter = adapter.context("no graphics adapter")?.cast()?;
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("creating a Direct3D 11 video device")?;
            let device: ID3D11Device = device.context("no Direct3D 11 device")?;
            let context = context.context("no Direct3D 11 context")?;
            // The MFT uses the device from its own threads.
            let _ = device.cast::<ID3D11Multithread>().context("ID3D11Multithread")?.SetMultithreadProtected(true);
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager).context("MFCreateDXGIDeviceManager")?;
            let manager: IMFDXGIDeviceManager = manager.context("no DXGI device manager")?;
            manager.ResetDevice(&device, token).context("IMFDXGIDeviceManager::ResetDevice")?;
            Ok(Self { device, context, manager })
        }
    }

    /// An NV12 texture holding `nv12` (`width` bytes a row), as a buffer.
    fn buffer(&self, nv12: &[u8], width: u32, height: u32) -> Result<IMFMediaBuffer> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_VIDEO_ENCODER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: nv12.as_ptr().cast(), SysMemPitch: width, SysMemSlicePitch: 0 };
        // SAFETY: `nv12` holds `height` rows of luma then `height / 2` of
        // chroma at `width` bytes, the layout D3D11 reads for NV12; it's
        // copied before CreateTexture2D returns.
        unsafe {
            let mut texture: Option<ID3D11Texture2D> = None;
            self.device.CreateTexture2D(&desc, Some(&init), Some(&mut texture)).context("creating an NV12 texture")?;
            // Some drivers only see the upload once it's flushed.
            self.context.Flush();
            let texture = texture.context("no texture")?;
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &texture, 0, false)
                .context("MFCreateDXGISurfaceBuffer")?;
            buffer.SetCurrentLength(buffer.GetMaxLength()?)?;
            Ok(buffer)
        }
    }
}

/// What the MFT gave back when asked for output.
enum Output {
    Sample(IMFSample),
    /// It changed its output type, now renegotiated.
    Changed,
    /// It wants more input first.
    NeedMore,
}

enum Event {
    NeedInput,
    HaveOutput,
}

/// One Media Foundation encoder for one picture size.
pub struct MfEncoder {
    activate: IMFActivate,
    transform: IMFTransform,
    codec: ICodecAPI,
    /// Present for asynchronous (hardware) MFTs.
    events: Option<IMFMediaEventGenerator>,
    d3d: Option<D3d>,
    input_id: u32,
    output_id: u32,
    input_size: u32,
    output_info: MFT_OUTPUT_STREAM_INFO,
    /// Input requests not yet answered (asynchronous MFTs).
    need_input: u32,
    device: String,
    width: u32,
    height: u32,
    format: PixelFormat,
    /// The next sample's time and each one's duration, in 100 ns.
    time: i64,
    duration: i64,
    /// No frame has been encoded yet (beyond the trial ones).
    first: bool,
    /// A software MFT under `SLIPSTREAM_MF_SOFTWARE`: missing controls
    /// are tolerated.
    lenient: bool,
    /// NV12 staging for texture input.
    nv12: Vec<u8>,
}

// SAFETY: Media Foundation transforms and the objects around them are
// free-threaded COM objects in the multithreaded apartment, which every
// method joins first (`com`). The encoder is created and used by one
// pipeline thread at a time (&mut self) and may be dropped on another.
unsafe impl Send for MfEncoder {}

impl MfEncoder {
    /// Opens a hardware encoder for `width`×`height` (even) frames of
    /// `format`, or fails if none works here.
    pub fn new(width: u32, height: u32, fps: f32, kbps: u32, format: PixelFormat) -> Result<Self> {
        ensure!(width >= 2 && height >= 2 && width.is_multiple_of(2) && height.is_multiple_of(2), "odd picture size");
        runtime()?;
        com();
        let candidates = candidates()?;
        ensure!(!candidates.is_empty(), "no hardware H.264 encoder (Media Foundation) on this machine");
        let mut errors = Vec::new();
        for activate in candidates {
            let name = string(&activate, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_else(|| "H.264 encoder MFT".into());
            for d3d11 in [false, true] {
                match Self::open(&activate, &name, d3d11, width, height, fps, kbps, format) {
                    Ok(enc) => {
                        tracing::info!(
                            device = %name,
                            width,
                            height,
                            kbps,
                            input = if d3d11 { "Direct3D 11" } else { "system memory" },
                            "Media Foundation hardware encoding"
                        );
                        return Ok(enc);
                    }
                    Err(e) => {
                        tracing::debug!(device = %name, d3d11, "Media Foundation encoder failed: {e:#}");
                        errors.push(format!("{name}{}: {e:#}", if d3d11 { " (Direct3D 11)" } else { "" }));
                    }
                }
            }
        }
        bail!("no Media Foundation H.264 encoder works: {}", errors.join("; "))
    }

    /// The encoder's name, e.g. "Intel® Quick Sync Video H.264 Encoder MFT".
    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    #[allow(clippy::too_many_arguments)]
    fn open(
        activate: &IMFActivate,
        name: &str,
        d3d11: bool,
        width: u32,
        height: u32,
        fps: f32,
        kbps: u32,
        format: PixelFormat,
    ) -> Result<Self> {
        // SAFETY: activation per the IMFActivate contract; whoever
        // activates shuts the object down again, here on failure and
        // otherwise in Drop.
        let (transform, codec) = unsafe {
            let transform: IMFTransform = match activate.ActivateObject() {
                Ok(t) => t,
                Err(e) => {
                    let _ = activate.ShutdownObject();
                    return Err(e).context("activating it");
                }
            };
            match transform.cast::<ICodecAPI>() {
                Ok(codec) => (transform, codec),
                Err(e) => {
                    drop(transform);
                    let _ = activate.ShutdownObject();
                    return Err(e).context("it has no ICodecAPI");
                }
            }
        };
        let mut enc = Self {
            activate: activate.clone(),
            transform,
            codec,
            events: None,
            d3d: None,
            input_id: 0,
            output_id: 0,
            input_size: 0,
            output_info: MFT_OUTPUT_STREAM_INFO::default(),
            need_input: 0,
            device: name.to_owned(),
            width,
            height,
            format,
            time: 0,
            duration: 0,
            first: true,
            lenient: software_allowed() && string(activate, &MFT_ENUM_HARDWARE_URL_Attribute).is_none(),
            nv12: Vec::new(),
        };
        enc.setup(d3d11, fps, kbps)?;
        // Without keyframe requests, the first frame has to be the
        // stream's own first.
        if !enc.lenient {
            enc.trial()?;
        }
        Ok(enc)
    }

    fn setup(&mut self, d3d11: bool, fps: f32, kbps: u32) -> Result<()> {
        // SAFETY: IMFTransform/IMFAttributes calls on a live transform, in
        // the order asynchronous MFTs require (unlock first, types before
        // streaming).
        unsafe {
            let attrs = self.transform.GetAttributes().ok();
            let flag = |key: &GUID| attrs.as_ref().is_some_and(|a| a.GetUINT32(key).is_ok_and(|v| v != 0));
            if flag(&MF_TRANSFORM_ASYNC) {
                let attrs = attrs.as_ref().expect("attributes");
                attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).context("unlocking the asynchronous MFT")?;
                self.events = Some(self.transform.cast().context("it has no event generator")?);
            }
            ensure!(!d3d11 || flag(&MF_SA_D3D11_AWARE), "it takes no Direct3D 11 input");
            if let Some(attrs) = &attrs {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let (mut input, mut output) = ([0u32], [0u32]);
            match self.transform.GetStreamIDs(&mut input, &mut output) {
                Ok(()) => (self.input_id, self.output_id) = (input[0], output[0]),
                // Fixed streams, numbered from 0.
                Err(e) if e.code() == E_NOTIMPL => {}
                Err(e) => return Err(e).context("GetStreamIDs"),
            }

            self.configure(fps, kbps)?;
            self.set_types(fps, kbps)?;

            if d3d11 {
                let d3d = D3d::new(&self.activate)?;
                self.transform
                    .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, d3d.manager.as_raw() as usize)
                    .context("setting the Direct3D device")?;
                self.d3d = Some(d3d);
            }

            self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0).context("flushing")?;
            self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).context("beginning streaming")?;
            self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).context("starting the stream")?;
            self.output_info = self.transform.GetOutputStreamInfo(self.output_id).context("GetOutputStreamInfo")?;
            let mut info = MFT_INPUT_STREAM_INFO::default();
            if self.transform.GetInputStreamInfo(self.input_id, &mut info).is_ok() {
                self.input_size = info.cbSize;
            }
        }
        Ok(())
    }

    /// Sets a codec property, reporting whether the encoder took it.
    fn set(&self, api: &GUID, value: impl Into<VARIANT>) -> windows::core::Result<()> {
        // SAFETY: a live ICodecAPI and a VARIANT of the property's type.
        unsafe { self.codec.SetValue(api, &value.into()) }
    }

    /// Like `set`, for properties the stream can't do without (unless
    /// `lenient`).
    fn need(&self, what: &str, api: &GUID, value: impl Into<VARIANT>) -> Result<()> {
        match self.set(api, value) {
            Err(e) if self.lenient => {
                tracing::warn!(device = %self.device, "{what} not set (software MFT): {e}");
                Ok(())
            }
            r => r.with_context(|| what.to_owned()),
        }
    }

    /// Like `set`, for properties the stream does without.
    fn try_set(&self, what: &str, api: &GUID, value: impl Into<VARIANT>) -> bool {
        match self.set(api, value) {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!(device = %self.device, "{what} not set: {e}");
                false
            }
        }
    }

    /// Encoder settings, made before the media types as some MFTs need.
    fn configure(&mut self, fps: f32, kbps: u32) -> Result<()> {
        let cbr = eAVEncCommonRateControlMode_CBR.0 as u32;
        self.need("setting constant bitrate", &CODECAPI_AVEncCommonRateControlMode, cbr)?;
        self.set_rate(fps, kbps)?;
        self.try_set("low latency", &CODECAPI_AVLowLatencyMode, true);
        self.try_set("no B-frames", &CODECAPI_AVEncMPVDefaultBPictureCount, 0u32);
        self.try_set("CAVLC", &CODECAPI_AVEncH264CABACEnable, false);
        let _ = GOP_SIZES.iter().any(|&n| self.try_set("GOP size", &CODECAPI_AVEncMPVGOPSize, n));
        // Intel's encoder wants its layer count stated, even if it's one,
        // and has a mode for screen content (as Chromium does).
        let vendor = string(&self.activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).unwrap_or_default();
        if vendor.eq_ignore_ascii_case("VEN_8086") {
            self.try_set("temporal layers", &CODECAPI_AVEncVideoTemporalLayerCount, 1u32);
            self.try_set("display remoting", &CODECAPI_AVScenarioInfo, eAVScenarioInfo_DisplayRemoting.0 as u32);
        }
        Ok(())
    }

    /// Constant bitrate with a one-frame buffer, as NVENC: each frame stays
    /// about the size the link can carry in one frame interval.
    fn set_rate(&mut self, fps: f32, kbps: u32) -> Result<()> {
        let fps = fps.clamp(1.0, 240.0);
        let bps = kbps.saturating_mul(1000).max(100_000);
        self.need("setting the bitrate", &CODECAPI_AVEncCommonMeanBitRate, bps)?;
        // Bytes for H.264 (the hypothetical reference decoder's buffer).
        self.try_set("buffer size", &CODECAPI_AVEncCommonBufferSize, (bps as f32 / 8.0 / fps) as u32);
        self.duration = (SECOND / f64::from(fps)) as i64;
        Ok(())
    }

    fn set_types(&mut self, fps: f32, kbps: u32) -> Result<()> {
        let (num, den) = rate(fps);
        let size = pack(self.width, self.height);
        let bps = kbps.saturating_mul(1000).max(100_000);
        // SAFETY: media type attributes set on fresh types; the output type
        // goes first, as encoders require.
        unsafe {
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bps)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(num, den))?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            self.transform.SetOutputType(self.output_id, &out, 0).context("setting the output type (H.264 Baseline)")?;

            let describe = |t: &IMFMediaType| -> windows::core::Result<()> {
                t.SetUINT64(&MF_MT_FRAME_RATE, pack(num, den))?;
                t.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
                t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
                t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                t.SetUINT32(&MF_MT_DEFAULT_STRIDE, self.width)
            };
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            describe(&input)?;
            if let Err(e) = self.transform.SetInputType(self.input_id, &input, 0) {
                // Some want one of their own offered types, filled in.
                let offered = (0..)
                    .map_while(|i| self.transform.GetInputAvailableType(self.input_id, i).ok())
                    .find(|t| t.GetGUID(&MF_MT_SUBTYPE).is_ok_and(|s| s == MFVideoFormat_NV12));
                let Some(t) = offered else { return Err(e).context("setting the input type (NV12)") };
                describe(&t)?;
                self.transform.SetInputType(self.input_id, &t, 0).context("setting the input type (NV12)")?;
            }
        }
        Ok(())
    }

    /// Two black frames, the second a requested keyframe: proves the MFT
    /// takes our input, answers in time, and honours keyframe requests,
    /// so a broken one is skipped here rather than failing mid-stream.
    fn trial(&mut self) -> Result<()> {
        let black = |nv12: &mut [u8], luma: usize| {
            nv12[..luma].fill(16);
            nv12[luma..].fill(128);
            Ok(())
        };
        let luma = self.width as usize * self.height as usize;
        let first = self.encode_nv12(|b| black(b, luma), false, FIRST_TIMEOUT).context("first trial frame")?;
        ensure!(first.keyframe, "its first frame isn't a keyframe");
        self.encode_nv12(|b| black(b, luma), true, FIRST_TIMEOUT).context("second trial frame")?;
        Ok(())
    }

    /// Changes the bitrate in place, without a keyframe.
    pub fn set_bitrate(&mut self, fps: f32, kbps: u32) -> Result<()> {
        com();
        self.set_rate(fps, kbps)
    }

    /// Encodes the top-left `width`×`height` of `frame`.
    pub fn encode(&mut self, frame: &RawFrame, force_keyframe: bool) -> Result<EncodedFrame> {
        ensure!(frame.format == self.format, "pixel format changed");
        ensure!(frame.width >= self.width && frame.height >= self.height, "frame smaller than the encoder");
        frame.validate()?;
        com();
        let (w, h) = (self.width, self.height);
        let key = force_keyframe || self.first;
        let encoded = self.encode_nv12(|nv12| convert(frame, w, h, nv12), key, TIMEOUT)?;
        self.first = false;
        Ok(encoded)
    }

    /// Encodes one NV12 picture that `fill` writes (`width` bytes a row,
    /// luma then interleaved chroma), returning its access unit.
    fn encode_nv12(
        &mut self,
        fill: impl FnOnce(&mut [u8]) -> Result<()>,
        key: bool,
        timeout: Duration,
    ) -> Result<EncodedFrame> {
        let deadline = Instant::now() + timeout;
        let sample = self.input(fill)?;
        if key {
            self.need("requesting a keyframe", &CODECAPI_AVEncVideoForceKeyFrame, 1u32)?;
        }
        let output =
            if self.events.is_some() { self.run_async(&sample, deadline)? } else { self.run_sync(&sample, deadline)? };
        let encoded = self.finish(&output)?;
        ensure!(!key || encoded.keyframe || self.lenient, "the encoder ignored a keyframe request");
        Ok(encoded)
    }

    /// A timed sample holding the next picture.
    fn input(&mut self, fill: impl FnOnce(&mut [u8]) -> Result<()>) -> Result<IMFSample> {
        let len = self.width as usize * self.height as usize * 3 / 2;
        // SAFETY: the memory buffer is locked for exactly the slice's
        // lifetime and holds at least `size` bytes (checked).
        unsafe {
            let sample = MFCreateSample()?;
            sample.SetSampleTime(self.time)?;
            sample.SetSampleDuration(self.duration)?;
            self.time += self.duration;
            let buffer = if let Some(d3d) = &self.d3d {
                self.nv12.resize(len, 0);
                fill(&mut self.nv12)?;
                d3d.buffer(&self.nv12, self.width, self.height)?
            } else {
                // As big as the MFT asks (some round the height up), with
                // the planes packed at the start and the rest zeroed.
                let size = (len as u32).max(self.input_size);
                let buffer = MFCreateMemoryBuffer(size).context("MFCreateMemoryBuffer")?;
                let (mut data, mut max) = (ptr::null_mut(), 0u32);
                buffer.Lock(&mut data, Some(&mut max), None)?;
                let filled = if data.is_null() || max < size {
                    Err(anyhow!("input buffer too small"))
                } else {
                    let all = std::slice::from_raw_parts_mut(data, size as usize);
                    all[len..].fill(0);
                    fill(&mut all[..len])
                };
                buffer.Unlock()?;
                filled?;
                buffer.SetCurrentLength(size)?;
                buffer
            };
            sample.AddBuffer(&buffer)?;
            Ok(sample)
        }
    }

    /// Asynchronous MFTs: input when asked, then wait for the output.
    fn run_async(&mut self, sample: &IMFSample, deadline: Instant) -> Result<IMFSample> {
        loop {
            while self.need_input == 0 {
                match self.next_event(deadline)? {
                    Event::NeedInput => self.need_input += 1,
                    // Left from an earlier frame: nothing waits for it now.
                    Event::HaveOutput => {
                        if let Output::Sample(_) = self.output()? {
                            tracing::debug!("dropped a late encoded frame");
                        }
                    }
                }
            }
            self.need_input -= 1;
            // SAFETY: a live transform that asked for input.
            match unsafe { self.transform.ProcessInput(self.input_id, sample, 0) } {
                Ok(()) => break,
                // It changed its mind: offer the frame at the next request.
                Err(e) if e.code() == MF_E_NOTACCEPTING => {}
                Err(e) => return Err(e).context("ProcessInput"),
            }
        }
        loop {
            match self.next_event(deadline)? {
                Event::NeedInput => self.need_input += 1,
                Event::HaveOutput => {
                    if let Output::Sample(s) = self.output()? {
                        return Ok(s);
                    }
                    // A format change: the frame follows with another event.
                }
            }
        }
    }

    /// Synchronous MFTs (software): input, then output straight away.
    fn run_sync(&mut self, sample: &IMFSample, deadline: Instant) -> Result<IMFSample> {
        // SAFETY: a live transform.
        let taken = unsafe { self.transform.ProcessInput(self.input_id, sample, 0) };
        if let Err(e) = taken {
            ensure!(e.code() == MF_E_NOTACCEPTING, "ProcessInput: {e}");
            // Output from before is in the way: collect it and retry.
            while !matches!(self.output()?, Output::NeedMore) {}
            // SAFETY: as above.
            unsafe { self.transform.ProcessInput(self.input_id, sample, 0) }.context("ProcessInput")?;
        }
        loop {
            match self.output()? {
                Output::Sample(s) => return Ok(s),
                Output::Changed => {}
                // Wine's encoders work on a thread of their own behind the
                // synchronous interface.
                Output::NeedMore if self.lenient && Instant::now() < deadline => std::thread::sleep(POLL),
                Output::NeedMore => bail!("the encoder held the frame back (no low-latency mode?)"),
            }
        }
    }

    /// The next event that matters from an asynchronous MFT.
    fn next_event(&self, deadline: Instant) -> Result<Event> {
        let events = self.events.as_ref().expect("asynchronous MFT");
        loop {
            // SAFETY: a live event generator; NO_WAIT never blocks.
            match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    // SAFETY: a live event.
                    let (kind, status) = unsafe { (event.GetType()?, event.GetStatus()?) };
                    if status.is_err() || kind == MEError.0 as u32 {
                        bail!("the encoder failed: {}", windows::core::Error::from_hresult(status));
                    }
                    if kind == METransformNeedInput.0 as u32 {
                        return Ok(Event::NeedInput);
                    }
                    if kind == METransformHaveOutput.0 as u32 {
                        return Ok(Event::HaveOutput);
                    }
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        bail!("the encoder gave no frame in time");
                    }
                    std::thread::sleep(POLL);
                }
                Err(e) => return Err(e).context("IMFMediaEventGenerator::GetEvent"),
            }
        }
    }

    /// Takes one output sample, renegotiating the type if it changed.
    fn output(&mut self) -> Result<Output> {
        let provides = self.output_info.dwFlags
            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
            != 0;
        // SAFETY: the output buffer holds either no sample (the MFT
        // provides one, which we then own) or one of ours; both it and any
        // event collection are taken out exactly once.
        unsafe {
            let ours = if provides {
                None
            } else {
                let size = self.output_info.cbSize.max(self.width * self.height * 3 / 2 + 4096);
                let sample = MFCreateSample()?;
                sample.AddBuffer(&MFCreateMemoryBuffer(size)?)?;
                Some(sample)
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.output_id,
                pSample: ManuallyDrop::new(ours),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0;
            let result = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let sample = ManuallyDrop::take(&mut buffers[0].pSample);
            drop(ManuallyDrop::take(&mut buffers[0].pEvents));
            match result {
                Ok(()) => Ok(Output::Sample(sample.context("the encoder returned no sample")?)),
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Output::NeedMore),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    let t = self.transform.GetOutputAvailableType(self.output_id, 0).context("new output type")?;
                    self.transform.SetOutputType(self.output_id, &t, 0).context("setting the new output type")?;
                    self.output_info = self.transform.GetOutputStreamInfo(self.output_id)?;
                    tracing::debug!(device = %self.device, "encoder output type renegotiated");
                    Ok(Output::Changed)
                }
                Err(e) => Err(e).context("ProcessOutput"),
            }
        }
    }

    /// The access unit in an output sample, with SPS/PPS in front of IDRs.
    fn finish(&self, sample: &IMFSample) -> Result<EncodedFrame> {
        // SAFETY: the buffer is locked while copied, for its current length.
        let (mut data, clean) = unsafe {
            let clean = sample.GetUINT32(&MFSampleExtension_CleanPoint).is_ok_and(|v| v != 0);
            let buffer = sample.ConvertToContiguousBuffer()?;
            let (mut p, mut len) = (ptr::null_mut(), 0u32);
            buffer.Lock(&mut p, None, Some(&mut len))?;
            let data = if p.is_null() { Vec::new() } else { std::slice::from_raw_parts(p, len as usize).to_vec() };
            buffer.Unlock()?;
            (data, clean)
        };
        ensure!(!data.is_empty(), "the encoder returned an empty frame");
        let (idr, params) = scan(&data);
        if idr && !params {
            let mut header = self.sequence_header().context("keyframe without SPS/PPS")?;
            header.append(&mut data);
            data = header;
        }
        Ok(EncodedFrame { data, keyframe: idr || clean, width: self.width, height: self.height })
    }

    /// SPS and PPS from the negotiated output type, as Annex-B.
    fn sequence_header(&self) -> Result<Vec<u8>> {
        // SAFETY: a blob read into a buffer of its reported size.
        unsafe {
            let t = self.transform.GetOutputCurrentType(self.output_id)?;
            let size = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).context("no MF_MT_MPEG_SEQUENCE_HEADER")?;
            let mut blob = vec![0u8; size as usize];
            t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)?;
            annex_b(&blob).context("unreadable MF_MT_MPEG_SEQUENCE_HEADER")
        }
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        com();
        // SAFETY: ends streaming and shuts the MFT down through its
        // activation object (which owns it); our references go after.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activate.ShutdownObject();
        }
    }
}

/// BGRA/RGBA to NV12, BT.601 limited range like the rest of the app.
fn convert(frame: &RawFrame, width: u32, height: u32, nv12: &mut [u8]) -> Result<()> {
    let (y, uv) = nv12.split_at_mut(width as usize * height as usize);
    let mut image = YuvBiPlanarImageMut {
        y_plane: BufferStoreMut::Borrowed(y),
        y_stride: width,
        uv_plane: BufferStoreMut::Borrowed(uv),
        uv_stride: width,
        width,
        height,
    };
    let convert = match frame.format {
        PixelFormat::Bgra => yuv::bgra_to_yuv_nv12,
        PixelFormat::Rgba => yuv::rgba_to_yuv_nv12,
    };
    let (range, matrix) = (YuvRange::Limited, YuvStandardMatrix::Bt601);
    convert(&mut image, &frame.data, frame.stride, range, matrix, YuvConversionMode::Balanced)
        .map_err(|e| anyhow!("colour conversion: {e:?}"))
}

/// Whether an Annex-B access unit is an IDR, and whether SPS and PPS come
/// before its first slice.
fn scan(data: &[u8]) -> (bool, bool) {
    let (mut sps, mut pps) = (false, false);
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            match data[i + 3] & 0x1f {
                1 => return (false, sps && pps),
                5 => return (true, sps && pps),
                7 => sps = true,
                8 => pps = true,
                _ => {}
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    (false, sps && pps)
}

/// A sequence header as Annex-B: as it is if it has start codes, else read
/// as an `avcC` record.
fn annex_b(blob: &[u8]) -> Option<Vec<u8>> {
    if blob.starts_with(&[0, 0, 1]) || blob.starts_with(&[0, 0, 0, 1]) {
        return Some(blob.to_vec());
    }
    if blob.first() != Some(&1) {
        return None;
    }
    let mut out = Vec::new();
    let mut p = 5;
    for mask in [0x1f, 0xff] {
        let count = *blob.get(p)? as usize & mask;
        p += 1;
        for _ in 0..count {
            let len = u16::from_be_bytes([*blob.get(p)?, *blob.get(p + 1)?]) as usize;
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(blob.get(p + 2..p + 2 + len)?);
            p += 2 + len;
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decoder, Encoder, EncoderSettings, YuvFrame};

    /// The Direct3D 11 input path builds NV12 texture buffers (where a
    /// Direct3D 11 device can be made).
    #[test]
    fn mf_texture_input_builds() {
        let d3d = match runtime().and_then(|()| {
            com();
            D3d::on(None)
        }) {
            Ok(d3d) => d3d,
            Err(e) => {
                eprintln!("skipped: {e:#}");
                return;
            }
        };
        let (w, h) = (64, 48);
        let buffer = d3d.buffer(&vec![128; w * h * 3 / 2], w as u32, h as u32).unwrap();
        // SAFETY: a live buffer.
        assert!(unsafe { buffer.GetCurrentLength() }.unwrap() as usize >= w * h * 3 / 2);
    }

    #[test]
    fn sequence_headers_become_annex_b() {
        let avcc = [1, 66, 0, 30, 0xff, 0xe1, 0, 2, 0x67, 0x42, 1, 0, 2, 0x68, 0xce];
        assert_eq!(annex_b(&avcc).unwrap(), [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce]);
        assert_eq!(annex_b(&[0, 0, 0, 1, 0x67]).unwrap(), [0, 0, 0, 1, 0x67]);
        assert!(annex_b(&[1, 66, 0, 30, 0xff, 0xe1, 0, 9, 0x67]).is_none());
        assert_eq!(scan(&[0, 0, 0, 1, 0x67, 0, 0, 1, 0x68, 0, 0, 1, 0x65, 0x88]), (true, true));
        assert_eq!(scan(&[0, 0, 1, 0x65, 0x88]), (true, false));
        assert_eq!(scan(&[0, 0, 1, 0x41, 0x9a]), (false, false));
    }

    fn test_frame(w: u32, h: u32, t: u32) -> RawFrame {
        let stride = w * 4 + 16;
        let mut data = vec![0u8; (stride * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let o = (y * stride + x * 4) as usize;
                data[o] = ((x + t) % 256) as u8; // B
                data[o + 1] = (y % 256) as u8; // G
                data[o + 2] = if (x / 32 + y / 32) % 2 == 0 { 220 } else { 30 }; // R
                data[o + 3] = 255;
            }
        }
        RawFrame { width: w, height: h, stride, format: PixelFormat::Bgra, data }
    }

    /// Mean absolute difference per channel between the BGRA source and a
    /// decoded picture.
    fn colour_error(raw: &RawFrame, decoded: &YuvFrame) -> f64 {
        let rgba = decoded.to_rgba();
        let (w, h) = (decoded.width as usize, decoded.height as usize);
        let mut sum = 0u64;
        for y in 0..h {
            for x in 0..w {
                let o = y * raw.stride as usize + x * 4;
                let d = (y * w + x) * 4;
                for (a, b) in [(raw.data[o + 2], rgba[d]), (raw.data[o + 1], rgba[d + 1]), (raw.data[o], rgba[d + 2])] {
                    sum += a.abs_diff(b) as u64;
                }
            }
        }
        sum as f64 / (w * h * 3) as f64
    }

    /// Where a Media Foundation H.264 encoder opens (hardware, or software
    /// with `SLIPSTREAM_MF_SOFTWARE=1`): its output decodes with the
    /// viewer's decoder, colours match the software path, keyframes come
    /// on request only, and bitrate changes keep the stream.
    #[test]
    fn mf_output_decodes_with_matching_colours() {
        let (w, h) = (640, 480);
        let mut hw = match MfEncoder::new(w, h, 60.0, 8000, PixelFormat::Bgra) {
            Ok(hw) => hw,
            Err(e) => {
                eprintln!("skipped: {e:#}");
                return;
            }
        };
        let mut sw = Encoder::with_hardware(EncoderSettings { bitrate_kbps: 8000, max_fps: 60.0 }, false);
        let (mut hd, mut sd) = (Decoder::new().unwrap(), Decoder::new().unwrap());
        let mut kinds = Vec::new();
        let (mut herr, mut serr) = (0.0, 0.0);
        // A frame larger than the encoder: only its top-left is encoded.
        let big = test_frame(w + 32, h + 16, 0);
        for t in 0..12 {
            let frame = test_frame(w, h, t * 5);
            if t == 6 {
                hw.set_bitrate(60.0, 4000).unwrap();
            }
            let e = if t == 3 { hw.encode(&big, false).unwrap() } else { hw.encode(&frame, t == 9).unwrap() };
            kinds.push(e.keyframe);
            let decoded = hd.decode(&e.data).unwrap().expect("hardware frame decodes");
            assert_eq!((decoded.width, decoded.height), (w, h));
            if t == 3 {
                continue;
            }
            herr = colour_error(&frame, &decoded);
            let s = sw.encode(&frame, t == 0).unwrap().unwrap();
            serr = colour_error(&frame, &sd.decode(&s.data).unwrap().unwrap());
        }
        eprintln!("{}: mean colour error {herr:.2}, OpenH264 {serr:.2}, keyframes {kinds:?}", hw.device());
        if hw.lenient {
            // A software MFT without keyframe control: its own GOP.
            assert!(kinds[0]);
        } else {
            assert_eq!(kinds, [true, false, false, false, false, false, false, false, false, true, false, false]);
        }
        assert!(herr < serr + 3.0, "hardware colours are off: {herr:.2} vs {serr:.2}");
    }
}
