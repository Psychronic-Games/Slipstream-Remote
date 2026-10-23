//! Hardware H.264 encoding through VA-API on Linux (AMD with Mesa's
//! radeonsi, Intel with iHD or i965).
//!
//! libva and its DRM backend are loaded when first needed (`libva.so.2`,
//! `libva-drm.so.2`), like PipeWire, so the portable binary needs neither.
//! The first render node whose driver offers Constrained Baseline H.264
//! encoding is used (`SLIPSTREAM_VAAPI_DEVICE=/dev/dri/renderD129` picks
//! one). Frames are converted to NV12 on the processor (BT.601 limited, as
//! everywhere else) straight into the driver's surface. The H.264 headers
//! are written here and handed over as packed headers (see `headers`). One
//! reference frame, P-frames after an IDR that comes only on request, CAVLC,
//! constant bitrate.
mod headers;
mod sys;

use std::{
    ffi::{CStr, c_char, c_int, c_uint, c_void},
    fs::File,
    os::fd::AsRawFd,
    ptr,
    sync::OnceLock,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use sys::*;
use yuv::{BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix};

use crate::{EncodedFrame, PixelFormat, RawFrame};

const ENTRYPOINT_ENC_SLICE_LP: VAEntrypoint = 8;
const SLICE_P: u8 = 0;
const SLICE_I: u8 = 2;
/// frame_num wraps at 2^16 (log2_max_frame_num_minus4 = 12).
const MAX_FRAME_NUM: u32 = 1 << 16;

type MessageCallback = Option<unsafe extern "C" fn(*mut c_void, *const c_char)>;

/// libva's functions, loaded once per process.
struct Api {
    _va: libloading::Library,
    _drm: libloading::Library,
    get_display_drm: unsafe extern "C" fn(c_int) -> VADisplay,
    initialize: unsafe extern "C" fn(VADisplay, *mut c_int, *mut c_int) -> VAStatus,
    terminate: unsafe extern "C" fn(VADisplay) -> VAStatus,
    error_str: unsafe extern "C" fn(VAStatus) -> *const c_char,
    vendor: unsafe extern "C" fn(VADisplay) -> *const c_char,
    set_info_callback: unsafe extern "C" fn(VADisplay, MessageCallback, *mut c_void) -> MessageCallback,
    set_error_callback: unsafe extern "C" fn(VADisplay, MessageCallback, *mut c_void) -> MessageCallback,
    max_entrypoints: unsafe extern "C" fn(VADisplay) -> c_int,
    query_entrypoints: unsafe extern "C" fn(VADisplay, VAProfile, *mut VAEntrypoint, *mut c_int) -> VAStatus,
    get_config_attributes: unsafe extern "C" fn(VADisplay, VAProfile, VAEntrypoint, *mut VAConfigAttrib, c_int) -> VAStatus,
    create_config: unsafe extern "C" fn(VADisplay, VAProfile, VAEntrypoint, *mut VAConfigAttrib, c_int, *mut VAConfigID) -> VAStatus,
    destroy_config: unsafe extern "C" fn(VADisplay, VAConfigID) -> VAStatus,
    create_surfaces: unsafe extern "C" fn(VADisplay, c_uint, c_uint, c_uint, *mut VASurfaceID, c_uint, *mut c_void, c_uint) -> VAStatus,
    destroy_surfaces: unsafe extern "C" fn(VADisplay, *mut VASurfaceID, c_int) -> VAStatus,
    create_context: unsafe extern "C" fn(VADisplay, VAConfigID, c_int, c_int, c_int, *mut VASurfaceID, c_int, *mut VAContextID) -> VAStatus,
    destroy_context: unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus,
    create_buffer: unsafe extern "C" fn(VADisplay, VAContextID, VABufferType, c_uint, c_uint, *mut c_void, *mut VABufferID) -> VAStatus,
    destroy_buffer: unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus,
    map_buffer: unsafe extern "C" fn(VADisplay, VABufferID, *mut *mut c_void) -> VAStatus,
    unmap_buffer: unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus,
    begin_picture: unsafe extern "C" fn(VADisplay, VAContextID, VASurfaceID) -> VAStatus,
    render_picture: unsafe extern "C" fn(VADisplay, VAContextID, *mut VABufferID, c_int) -> VAStatus,
    end_picture: unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus,
    sync_surface: unsafe extern "C" fn(VADisplay, VASurfaceID) -> VAStatus,
    derive_image: unsafe extern "C" fn(VADisplay, VASurfaceID, *mut VAImage) -> VAStatus,
    destroy_image: unsafe extern "C" fn(VADisplay, VAImageID) -> VAStatus,
}

// SAFETY: plain function pointers into libraries that stay loaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

impl Api {
    fn load() -> Result<Self> {
        // SAFETY: loading the system's libva; its initialisers are libva's.
        let va = unsafe { libloading::Library::new("libva.so.2") }.context("libva.so.2 isn't installed")?;
        // SAFETY: as above.
        let drm = unsafe { libloading::Library::new("libva-drm.so.2") }.context("libva-drm.so.2 isn't installed")?;
        // SAFETY: these symbols have these signatures in libva 2.x (va.h, va_drm.h).
        unsafe {
            macro_rules! sym {
                ($lib:expr, $name:literal) => {
                    *$lib.get(concat!($name, "\0").as_bytes()).context(concat!("libva has no ", $name))?
                };
            }
            Ok(Self {
                get_display_drm: sym!(drm, "vaGetDisplayDRM"),
                initialize: sym!(va, "vaInitialize"),
                terminate: sym!(va, "vaTerminate"),
                error_str: sym!(va, "vaErrorStr"),
                vendor: sym!(va, "vaQueryVendorString"),
                set_info_callback: sym!(va, "vaSetInfoCallback"),
                set_error_callback: sym!(va, "vaSetErrorCallback"),
                max_entrypoints: sym!(va, "vaMaxNumEntrypoints"),
                query_entrypoints: sym!(va, "vaQueryConfigEntrypoints"),
                get_config_attributes: sym!(va, "vaGetConfigAttributes"),
                create_config: sym!(va, "vaCreateConfig"),
                destroy_config: sym!(va, "vaDestroyConfig"),
                create_surfaces: sym!(va, "vaCreateSurfaces"),
                destroy_surfaces: sym!(va, "vaDestroySurfaces"),
                create_context: sym!(va, "vaCreateContext"),
                destroy_context: sym!(va, "vaDestroyContext"),
                create_buffer: sym!(va, "vaCreateBuffer"),
                destroy_buffer: sym!(va, "vaDestroyBuffer"),
                map_buffer: sym!(va, "vaMapBuffer"),
                unmap_buffer: sym!(va, "vaUnmapBuffer"),
                begin_picture: sym!(va, "vaBeginPicture"),
                render_picture: sym!(va, "vaRenderPicture"),
                end_picture: sym!(va, "vaEndPicture"),
                sync_surface: sym!(va, "vaSyncSurface"),
                derive_image: sym!(va, "vaDeriveImage"),
                destroy_image: sym!(va, "vaDestroyImage"),
                _va: va,
                _drm: drm,
            })
        }
    }

    fn get() -> Result<&'static Api> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        API.get_or_init(|| Api::load().map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| anyhow!("{e}"))
    }

    fn check(&self, status: VAStatus, what: &str) -> Result<()> {
        if status == VA_STATUS_SUCCESS as VAStatus {
            return Ok(());
        }
        // SAFETY: vaErrorStr returns a static string for any status.
        let text = unsafe { CStr::from_ptr((self.error_str)(status)) }.to_string_lossy();
        bail!("{what} failed: {text}")
    }
}

/// libva writes informational messages to stderr; keep them out of it.
unsafe extern "C" fn quiet(_: *mut c_void, message: *const c_char) {
    // SAFETY: libva passes a NUL-terminated message.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    tracing::trace!("libva: {}", message.trim_end());
}

/// An initialised VA display on one render node.
struct Display {
    api: &'static Api,
    dpy: VADisplay,
    entrypoint: VAEntrypoint,
    /// The packed headers the driver takes (`VA_ENC_PACKED_HEADER_*`).
    packed: u32,
    vendor: String,
    _node: File,
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: a display this struct initialised.
        unsafe { (self.api.terminate)(self.dpy) };
    }
}

impl Display {
    /// The first render node able to encode Constrained Baseline H.264.
    fn open() -> Result<Self> {
        let api = Api::get()?;
        let nodes: Vec<String> = match std::env::var("SLIPSTREAM_VAAPI_DEVICE") {
            Ok(node) => vec![node],
            // NVIDIA's VA-API driver can't encode (NVENC covers those cards),
            // and loading it beside CUDA is best avoided: skip by PCI vendor.
            Err(_) => (128..192)
                .filter(|n| std::path::Path::new(&format!("/dev/dri/renderD{n}")).exists())
                .filter(|n| std::fs::read_to_string(format!("/sys/class/drm/renderD{n}/device/vendor")).is_ok_and(|v| v.trim() != "0x10de"))
                .map(|n| format!("/dev/dri/renderD{n}"))
                .collect(),
        };
        let mut why = Vec::new();
        for node in nodes {
            match Self::try_node(api, &node) {
                Ok(display) => return Ok(display),
                Err(e) => why.push(format!("{node}: {e:#}")),
            }
        }
        if why.is_empty() {
            bail!("no graphics render nodes (/dev/dri/renderD*)");
        }
        bail!("no VA-API H.264 encoder ({})", why.join("; "))
    }

    fn try_node(api: &'static Api, node: &str) -> Result<Self> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(node).with_context(|| format!("opening {node}"))?;
        // SAFETY: libva calls on a display obtained from an open render
        // node; the display is terminated (Drop) on every exit path once
        // created.
        unsafe {
            let dpy = (api.get_display_drm)(file.as_raw_fd());
            ensure!(!dpy.is_null(), "no VA display");
            (api.set_info_callback)(dpy, Some(quiet), ptr::null_mut());
            (api.set_error_callback)(dpy, Some(quiet), ptr::null_mut());
            let (mut major, mut minor) = (0, 0);
            let mut display = Self { api, dpy, entrypoint: 0, packed: 0, vendor: String::new(), _node: file };
            api.check((api.initialize)(dpy, &mut major, &mut minor), "vaInitialize")?;
            let vendor = (api.vendor)(dpy);
            if !vendor.is_null() {
                display.vendor = CStr::from_ptr(vendor).to_string_lossy().into_owned();
            }
            let mut entrypoints = vec![0 as VAEntrypoint; (api.max_entrypoints)(dpy).max(1) as usize];
            let mut count = 0;
            api.check(
                (api.query_entrypoints)(dpy, VAProfile_VAProfileH264ConstrainedBaseline, entrypoints.as_mut_ptr(), &mut count),
                "vaQueryConfigEntrypoints",
            )?;
            entrypoints.truncate(count.max(0) as usize);
            display.entrypoint = [VAEntrypoint_VAEntrypointEncSlice, ENTRYPOINT_ENC_SLICE_LP]
                .into_iter()
                .find(|e| entrypoints.contains(e))
                .with_context(|| format!("{} can't encode Constrained Baseline H.264", display.vendor))?;
            let mut attributes = [
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRTFormat, value: 0 },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRateControl, value: 0 },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribEncPackedHeaders, value: 0 },
            ];
            api.check(
                (api.get_config_attributes)(dpy, VAProfile_VAProfileH264ConstrainedBaseline, display.entrypoint, attributes.as_mut_ptr(), 3),
                "vaGetConfigAttributes",
            )?;
            ensure!(attributes[0].value & VA_RT_FORMAT_YUV420 != 0, "no 4:2:0 encoding");
            ensure!(attributes[1].value & VA_RC_CBR != 0, "no constant-bitrate encoding");
            if attributes[2].value != VA_ATTRIB_NOT_SUPPORTED {
                display.packed = attributes[2].value & (VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE | VA_ENC_PACKED_HEADER_SLICE);
            }
            Ok(display)
        }
    }
}

/// One VA-API encoding session for one picture size.
pub struct VaEncoder {
    display: Display,
    config: VAConfigID,
    context: VAContextID,
    /// [input, reconstructed A, reconstructed B]
    surfaces: [VASurfaceID; 3],
    coded: VABufferID,
    width: u32,
    height: u32,
    format: PixelFormat,
    fps: f32,
    kbps: u32,
    /// Frames since the last IDR.
    since_idr: u32,
    idr_id: u16,
    /// Which reconstructed surface holds the reference (the other is written next).
    reference: usize,
    started: bool,
    /// Send rate control with the next frame (a bitrate change).
    rate_changed: bool,
}

// SAFETY: libva displays may be used from any thread, one call at a time;
// the encoder is used by one thread at a time (&mut self).
unsafe impl Send for VaEncoder {}

fn align16(n: u32) -> u32 {
    (n + 15) & !15
}

impl VaEncoder {
    pub fn new(width: u32, height: u32, fps: f32, kbps: u32, format: PixelFormat) -> Result<Self> {
        ensure!(width >= 16 && height >= 16 && width.is_multiple_of(2) && height.is_multiple_of(2), "picture too small or odd");
        let display = Display::open()?;
        let api = display.api;
        let mut enc = Self {
            display,
            config: VA_INVALID_ID,
            context: VA_INVALID_ID,
            surfaces: [VA_INVALID_SURFACE; 3],
            coded: VA_INVALID_ID,
            width,
            height,
            format,
            fps: fps.clamp(1.0, 240.0),
            kbps: kbps.max(100),
            since_idr: 0,
            idr_id: 0,
            reference: 1,
            started: false,
            rate_changed: false,
        };
        let dpy = enc.display.dpy;
        let (aw, ah) = (align16(width), align16(height));
        // SAFETY: libva calls on our initialised display; everything created
        // here is released in Drop, also when a later step fails.
        unsafe {
            let mut attributes = vec![
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRTFormat, value: VA_RT_FORMAT_YUV420 },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRateControl, value: VA_RC_CBR },
            ];
            if enc.display.packed != 0 {
                attributes.push(VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribEncPackedHeaders, value: enc.display.packed });
            }
            api.check(
                (api.create_config)(dpy, VAProfile_VAProfileH264ConstrainedBaseline, enc.display.entrypoint, attributes.as_mut_ptr(), attributes.len() as c_int, &mut enc.config),
                "vaCreateConfig",
            )?;
            api.check(
                (api.create_surfaces)(dpy, VA_RT_FORMAT_YUV420, aw, ah, enc.surfaces.as_mut_ptr(), 3, ptr::null_mut(), 0),
                "vaCreateSurfaces",
            )?;
            api.check(
                (api.create_context)(dpy, enc.config, aw as c_int, ah as c_int, VA_PROGRESSIVE as c_int, enc.surfaces.as_mut_ptr(), 3, &mut enc.context),
                "vaCreateContext",
            )?;
            // Room for an uncompressed frame: more than any coded one.
            let size = (aw * ah * 3 / 2).max(1 << 20);
            api.check(
                (api.create_buffer)(dpy, enc.context, VABufferType_VAEncCodedBufferType, size, 1, ptr::null_mut(), &mut enc.coded),
                "vaCreateBuffer (coded)",
            )?;
        }
        tracing::info!(device = %enc.display.vendor, width, height, kbps, "VA-API hardware encoding");
        Ok(enc)
    }

    /// The driver, e.g. "Mesa Gallium driver … (radeonsi, …)".
    pub fn device(&self) -> &str {
        &self.display.vendor
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Changes the bitrate from the next frame on, without a keyframe.
    pub fn set_bitrate(&mut self, fps: f32, kbps: u32) -> Result<()> {
        ensure!((fps.clamp(1.0, 240.0) - self.fps).abs() < 0.5, "frame rate changed");
        self.kbps = kbps.max(100);
        self.rate_changed = true;
        Ok(())
    }

    /// Creates a parameter buffer holding `value`.
    fn buffer<T>(&self, kind: VABufferType, value: &T) -> Result<VABufferID> {
        let mut id = VA_INVALID_ID;
        // SAFETY: libva copies size_of::<T>() bytes from `value`.
        let status = unsafe {
            (self.display.api.create_buffer)(
                self.display.dpy,
                self.context,
                kind,
                size_of::<T>() as c_uint,
                1,
                (value as *const T).cast_mut().cast(),
                &mut id,
            )
        };
        self.display.api.check(status, "vaCreateBuffer")?;
        Ok(id)
    }

    /// A misc parameter buffer: its type, then `value`.
    fn misc<T>(&self, kind: VAEncMiscParameterType, value: &T) -> Result<VABufferID> {
        let header = size_of::<VAEncMiscParameterBuffer>();
        let mut bytes = vec![0u8; header + size_of::<T>()];
        bytes[..4].copy_from_slice(&kind.to_ne_bytes());
        // SAFETY: copies a plain C struct into the space after the header.
        unsafe { ptr::copy_nonoverlapping((value as *const T).cast::<u8>(), bytes[header..].as_mut_ptr(), size_of::<T>()) };
        let mut id = VA_INVALID_ID;
        // SAFETY: libva copies `bytes`.
        let status = unsafe {
            (self.display.api.create_buffer)(
                self.display.dpy,
                self.context,
                VABufferType_VAEncMiscParameterBufferType,
                bytes.len() as c_uint,
                1,
                bytes.as_mut_ptr().cast(),
                &mut id,
            )
        };
        self.display.api.check(status, "vaCreateBuffer (misc)")?;
        Ok(id)
    }

    /// A packed header: its parameter buffer and its data buffer.
    fn packed(&self, kind: VAEncPackedHeaderType, header: headers::Bits) -> Result<[VABufferID; 2]> {
        // SAFETY: a plain C struct; all-zero is a valid start.
        let mut parameter: VAEncPackedHeaderParameterBuffer = unsafe { std::mem::zeroed() };
        parameter.type_ = kind;
        parameter.bit_length = header.len_bits();
        parameter.has_emulation_bytes = 0;
        let parameter = self.buffer(VABufferType_VAEncPackedHeaderParameterBufferType, &parameter)?;
        let mut bytes = header.into_bytes();
        let mut id = VA_INVALID_ID;
        // SAFETY: libva copies `bytes`.
        let status = unsafe {
            (self.display.api.create_buffer)(
                self.display.dpy,
                self.context,
                VABufferType_VAEncPackedHeaderDataBufferType,
                bytes.len() as c_uint,
                1,
                bytes.as_mut_ptr().cast(),
                &mut id,
            )
        };
        if let Err(e) = self.display.api.check(status, "vaCreateBuffer (packed header)") {
            // SAFETY: the parameter buffer we just created.
            unsafe { (self.display.api.destroy_buffer)(self.display.dpy, parameter) };
            return Err(e);
        }
        Ok([parameter, id])
    }

    fn stream(&self) -> headers::Stream {
        headers::Stream { width: self.width, height: self.height, fps: self.fps.round() as u32 }
    }

    /// Converts the top-left `width`×`height` of `frame` to NV12 in the input surface.
    fn upload(&mut self, frame: &RawFrame) -> Result<()> {
        let api = self.display.api;
        let dpy = self.display.dpy;
        // SAFETY: the derived image maps the input surface; both planes lie
        // within its data_size (checked), and it's unmapped and destroyed
        // before returning.
        unsafe {
            let mut image: VAImage = std::mem::zeroed();
            api.check((api.derive_image)(dpy, self.surfaces[0], &mut image), "vaDeriveImage")?;
            let result = (|| {
                ensure!(image.format.fourcc == VA_FOURCC_NV12, "the driver's surfaces aren't NV12");
                let mut base = ptr::null_mut();
                api.check((api.map_buffer)(dpy, image.buf, &mut base), "vaMapBuffer (image)")?;
                let (y_pitch, uv_pitch) = (image.pitches[0] as usize, image.pitches[1] as usize);
                let (y_len, uv_len) = (y_pitch * self.height as usize, uv_pitch * (self.height as usize / 2));
                let converted = if image.offsets[0] as usize + y_len <= image.data_size as usize
                    && image.offsets[1] as usize + uv_len <= image.data_size as usize
                {
                    let y = std::slice::from_raw_parts_mut(base.cast::<u8>().add(image.offsets[0] as usize), y_len);
                    let uv = std::slice::from_raw_parts_mut(base.cast::<u8>().add(image.offsets[1] as usize), uv_len);
                    let mut nv12 = YuvBiPlanarImageMut {
                        y_plane: BufferStoreMut::Borrowed(y),
                        y_stride: y_pitch as u32,
                        uv_plane: BufferStoreMut::Borrowed(uv),
                        uv_stride: uv_pitch as u32,
                        width: self.width,
                        height: self.height,
                    };
                    let convert = match frame.format {
                        PixelFormat::Bgra => yuv::bgra_to_yuv_nv12,
                        PixelFormat::Rgba => yuv::rgba_to_yuv_nv12,
                    };
                    convert(&mut nv12, &frame.data, frame.stride, YuvRange::Limited, YuvStandardMatrix::Bt601, YuvConversionMode::Balanced)
                        .map_err(|e| anyhow!("colour conversion: {e:?}"))
                } else {
                    Err(anyhow!("the driver's image is smaller than the picture"))
                };
                api.check((api.unmap_buffer)(dpy, image.buf), "vaUnmapBuffer (image)")?;
                converted
            })();
            (api.destroy_image)(dpy, image.image_id);
            result
        }
    }

    fn picture(surface: VASurfaceID, frame_num: u32, poc: u32) -> VAPictureH264 {
        VAPictureH264 {
            picture_id: surface,
            frame_idx: frame_num,
            flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
            TopFieldOrderCnt: poc as i32,
            BottomFieldOrderCnt: poc as i32,
            va_reserved: [0; 4],
        }
    }

    fn invalid() -> VAPictureH264 {
        VAPictureH264 { picture_id: VA_INVALID_SURFACE, frame_idx: 0, flags: VA_PICTURE_H264_INVALID, TopFieldOrderCnt: 0, BottomFieldOrderCnt: 0, va_reserved: [0; 4] }
    }

    pub fn encode(&mut self, frame: &RawFrame, force_keyframe: bool) -> Result<EncodedFrame> {
        ensure!(frame.format == self.format, "pixel format changed");
        ensure!(frame.width >= self.width && frame.height >= self.height, "frame smaller than the encoder");
        frame.validate()?;
        self.upload(frame)?;
        let idr = force_keyframe || !self.started;
        if idr {
            self.since_idr = 0;
            self.idr_id = self.idr_id.wrapping_add(1);
        }
        let frame_num = self.since_idr % MAX_FRAME_NUM;
        // Picture order follows frame order (type 2): twice the frame number.
        let poc = (frame_num * 2) % MAX_FRAME_NUM;
        let current = 3 - self.reference;
        let (mbw, mbh) = (align16(self.width) / 16, align16(self.height) / 16);
        let bps = self.kbps.saturating_mul(1000);

        let mut buffers = Vec::new();
        let result = (|| {
            if idr || self.rate_changed {
                // SAFETY: plain C structs; all-zero is a valid start.
                let mut seq: VAEncSequenceParameterBufferH264 = unsafe { std::mem::zeroed() };
                seq.level_idc = 51;
                seq.intra_period = 0;
                seq.intra_idr_period = 0;
                seq.ip_period = 1;
                seq.bits_per_second = bps;
                seq.max_num_ref_frames = 1;
                seq.picture_width_in_mbs = mbw as u16;
                seq.picture_height_in_mbs = mbh as u16;
                // SAFETY: writing the bitfield view of a plain union.
                unsafe {
                    let bits = &mut seq.seq_fields.bits;
                    bits.set_chroma_format_idc(1);
                    bits.set_frame_mbs_only_flag(1);
                    bits.set_direct_8x8_inference_flag(1);
                    bits.set_log2_max_frame_num_minus4(12);
                    bits.set_pic_order_cnt_type(2);
                    seq.vui_fields.bits.set_timing_info_present_flag(1);
                    seq.vui_fields.bits.set_fixed_frame_rate_flag(1);
                }
                let (crop_w, crop_h) = (mbw * 16 - self.width, mbh * 16 - self.height);
                if crop_w > 0 || crop_h > 0 {
                    seq.frame_cropping_flag = 1;
                    seq.frame_crop_right_offset = crop_w / 2;
                    seq.frame_crop_bottom_offset = crop_h / 2;
                }
                seq.vui_parameters_present_flag = 1;
                seq.num_units_in_tick = 1;
                seq.time_scale = (self.fps.round() as u32) * 2;
                buffers.push(self.buffer(VABufferType_VAEncSequenceParameterBufferType, &seq)?);

                // SAFETY: as above.
                let mut rate: VAEncMiscParameterRateControl = unsafe { std::mem::zeroed() };
                rate.bits_per_second = bps;
                rate.target_percentage = 100;
                rate.window_size = 500;
                rate.initial_qp = 26;
                rate.min_qp = 10;
                rate.max_qp = 51;
                buffers.push(self.misc(VAEncMiscParameterType_VAEncMiscParameterTypeRateControl, &rate)?);
                // SAFETY: as above.
                let mut hrd: VAEncMiscParameterHRD = unsafe { std::mem::zeroed() };
                hrd.buffer_size = bps / 2;
                hrd.initial_buffer_fullness = bps / 4;
                buffers.push(self.misc(VAEncMiscParameterType_VAEncMiscParameterTypeHRD, &hrd)?);
                // SAFETY: as above.
                let mut rate_of_frames: VAEncMiscParameterFrameRate = unsafe { std::mem::zeroed() };
                rate_of_frames.framerate = self.fps.round() as u32;
                buffers.push(self.misc(VAEncMiscParameterType_VAEncMiscParameterTypeFrameRate, &rate_of_frames)?);
            }

            // SAFETY: as above.
            let mut pic: VAEncPictureParameterBufferH264 = unsafe { std::mem::zeroed() };
            pic.CurrPic = Self::picture(self.surfaces[current], frame_num, poc);
            pic.ReferenceFrames = [Self::invalid(); 16];
            if !idr {
                let previous = (frame_num + MAX_FRAME_NUM - 1) % MAX_FRAME_NUM;
                pic.ReferenceFrames[0] = Self::picture(self.surfaces[self.reference], previous, (previous * 2) % MAX_FRAME_NUM);
            }
            pic.coded_buf = self.coded;
            pic.frame_num = frame_num as u16;
            pic.pic_init_qp = 26;
            // SAFETY: bitfield view of a plain union.
            unsafe {
                let bits = &mut pic.pic_fields.bits;
                bits.set_idr_pic_flag(idr as u32);
                bits.set_reference_pic_flag(1);
                bits.set_entropy_coding_mode_flag(0);
                bits.set_deblocking_filter_control_present_flag(1);
            }
            buffers.push(self.buffer(VABufferType_VAEncPictureParameterBufferType, &pic)?);
            let packed = self.display.packed;
            if idr && packed & VA_ENC_PACKED_HEADER_SEQUENCE != 0 {
                buffers.extend(self.packed(VAEncPackedHeaderType_VAEncPackedHeaderSequence, headers::sps(&self.stream()))?);
            }
            if idr && packed & VA_ENC_PACKED_HEADER_PICTURE != 0 {
                buffers.extend(self.packed(VAEncPackedHeaderType_VAEncPackedHeaderPicture, headers::pps())?);
            }
            if packed & VA_ENC_PACKED_HEADER_SLICE != 0 {
                buffers.extend(self.packed(VAEncPackedHeaderType_VAEncPackedHeaderSlice, headers::slice(idr, frame_num, self.idr_id))?);
            }

            // SAFETY: as above.
            let mut slice: VAEncSliceParameterBufferH264 = unsafe { std::mem::zeroed() };
            slice.num_macroblocks = mbw * mbh;
            slice.macroblock_info = VA_INVALID_ID;
            slice.slice_type = if idr { SLICE_I } else { SLICE_P };
            slice.idr_pic_id = self.idr_id;
            slice.RefPicList0 = [Self::invalid(); 32];
            slice.RefPicList1 = [Self::invalid(); 32];
            if !idr {
                slice.RefPicList0[0] = pic.ReferenceFrames[0];
            }
            buffers.push(self.buffer(VABufferType_VAEncSliceParameterBufferType, &slice)?);

            let (api, dpy) = (self.display.api, self.display.dpy);
            // SAFETY: rendering our buffers into our context, then reading the
            // coded segments while the coded buffer is mapped.
            unsafe {
                api.check((api.begin_picture)(dpy, self.context, self.surfaces[0]), "vaBeginPicture")?;
                api.check((api.render_picture)(dpy, self.context, buffers.as_mut_ptr(), buffers.len() as c_int), "vaRenderPicture")?;
                api.check((api.end_picture)(dpy, self.context), "vaEndPicture")?;
                api.check((api.sync_surface)(dpy, self.surfaces[0]), "vaSyncSurface")?;
                let mut segment = ptr::null_mut::<c_void>();
                api.check((api.map_buffer)(dpy, self.coded, &mut segment), "vaMapBuffer (coded)")?;
                let mut data = Vec::new();
                let mut next = segment.cast::<VACodedBufferSegment>();
                while !next.is_null() {
                    let s = &*next;
                    data.extend_from_slice(std::slice::from_raw_parts(s.buf.cast::<u8>(), s.size as usize));
                    next = s.next.cast();
                }
                api.check((api.unmap_buffer)(dpy, self.coded), "vaUnmapBuffer (coded)")?;
                ensure!(!data.is_empty(), "the driver produced no data");
                // A driver that takes no packed parameter sets may still
                // leave them out: put ours in front of each IDR.
                if idr && packed & VA_ENC_PACKED_HEADER_SEQUENCE == 0 && !data.windows(5).any(|w| w[..4] == [0, 0, 0, 1] && w[4] & 31 == 7) {
                    let mut with = headers::escape(&headers::sps(&self.stream()).into_bytes());
                    with.extend(headers::escape(&headers::pps().into_bytes()));
                    with.extend(data);
                    return Ok(with);
                }
                Ok(data)
            }
        })();
        for id in buffers {
            // SAFETY: buffers we created for this picture.
            unsafe { (self.display.api.destroy_buffer)(self.display.dpy, id) };
        }
        let data = result?;
        self.started = true;
        self.rate_changed = false;
        self.since_idr += 1;
        self.reference = current;
        Ok(EncodedFrame { data, keyframe: idr, width: self.width, height: self.height })
    }
}

impl Drop for VaEncoder {
    fn drop(&mut self) {
        let (api, dpy) = (self.display.api, self.display.dpy);
        // SAFETY: releases what `new` created, each only if it was created.
        unsafe {
            if self.coded != VA_INVALID_ID {
                (api.destroy_buffer)(dpy, self.coded);
            }
            if self.context != VA_INVALID_ID {
                (api.destroy_context)(dpy, self.context);
            }
            if self.surfaces[0] != VA_INVALID_SURFACE {
                (api.destroy_surfaces)(dpy, self.surfaces.as_mut_ptr(), 3);
            }
            if self.config != VA_INVALID_ID {
                (api.destroy_config)(dpy, self.config);
            }
        }
    }
}

/// Whether VA-API encoding can be used here.
#[cfg(test)]
pub fn available() -> Result<()> {
    Display::open().map(|_| ())
}
