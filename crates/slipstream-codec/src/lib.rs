//! Video encoding and decoding.
//!
//! Today this is software H.264 via OpenH264 with SIMD colour conversion.
//! The [`Encoder`]/[`Decoder`] types are the seam where hardware codecs
//! (VA-API, NVENC, VideoToolbox, Media Foundation) plug in later.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use openh264::{
    OpenH264API, Timestamp,
    decoder::{Decoder as OhDecoder, DecoderConfig},
    encoder::{
        BitRate, Complexity, EncoderConfig, FrameRate, FrameType, RateControlMode, SpsPpsStrategy, UsageType,
    },
    formats::YUVSource,
};
use yuv::{YuvChromaSubsampling, YuvConversionMode, YuvPlanarImageMut, YuvRange, YuvStandardMatrix};

/// Byte order of a captured frame's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// B, G, R, A/x in memory (PipeWire BGRx, Windows DXGI, macOS default).
    Bgra,
    /// R, G, B, A/x in memory.
    Rgba,
}

/// An uncompressed captured frame.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row (may exceed `width * 4`).
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

impl RawFrame {
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            bail!("empty frame");
        }
        if self.stride < self.width * 4 || self.data.len() < (self.stride * (self.height - 1) + self.width * 4) as usize {
            bail!("frame buffer too small for {}x{} stride {}", self.width, self.height, self.stride);
        }
        Ok(())
    }
}

/// One encoded access unit (Annex-B H.264).
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderSettings {
    /// 0 = choose from resolution and frame rate.
    pub bitrate_kbps: u32,
    pub max_fps: f32,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self { bitrate_kbps: 0, max_fps: 60.0 }
    }
}

/// Picks a bitrate suited to screen content at this size.
pub fn auto_bitrate_kbps(width: u32, height: u32, fps: f32) -> u32 {
    let pixels_per_sec = width as f64 * height as f64 * fps.max(1.0) as f64;
    ((pixels_per_sec * 0.07 / 1000.0) as u32).clamp(1_500, 25_000)
}

/// OpenH264 refuses pictures larger than 4K by area (3840x2160 and
/// 2160x3840 both work; 4096x2160 and 5120x1440 don't).
const MAX_PIXELS: f64 = 3840.0 * 2160.0;
const MAX_SIDE: f64 = 3840.0;

/// The size a `w`×`h` frame is encoded at: its own, or scaled down to fit
/// the encoder (ultrawide and 5K screens). Always even, for 4:2:0.
pub fn encode_size(w: u32, h: u32) -> (u32, u32) {
    let (fw, fh) = (w as f64, h as f64);
    let s = (MAX_PIXELS / (fw * fh)).sqrt().min(MAX_SIDE / fw.max(fh)).min(1.0);
    if s >= 1.0 {
        return (w & !1, h & !1);
    }
    ((fw * s) as u32 & !1, (fh * s) as u32 & !1)
}

/// Planar I420 buffer that OpenH264 can read directly.
struct I420 {
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl I420 {
    fn new(width: usize, height: usize) -> Self {
        let (cw, ch) = (width / 2, height / 2);
        Self { width, height, y: vec![0; width * height], u: vec![128; cw * ch], v: vec![128; cw * ch] }
    }
}

impl YUVSource for I420 {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (self.width, self.width / 2, self.width / 2)
    }
    fn y(&self) -> &[u8] {
        &self.y
    }
    fn u(&self) -> &[u8] {
        &self.u
    }
    fn v(&self) -> &[u8] {
        &self.v
    }
}

pub struct Encoder {
    settings: EncoderSettings,
    inner: Option<openh264::encoder::Encoder>,
    planes: I420,
    dirty: bool,
    /// Frames are timestamped from here, so rate control sees the real
    /// spacing of frames (capture delivers them unevenly).
    epoch: Instant,
    /// Whether rate control may drop frames to hold the bitrate (see
    /// [`Encoder::set_frame_skip`]).
    skip: bool,
    /// For frames too big to encode as they are (reused buffers).
    resizer: fast_image_resize::Resizer,
    scaled: Vec<u8>,
    packed: Vec<u8>,
    /// Encode at this fraction of the frame's size (see [`Encoder::set_scale`]).
    scale: f32,
}

impl Encoder {
    pub fn new(settings: EncoderSettings) -> Self {
        Self {
            settings,
            inner: None,
            planes: I420::new(2, 2),
            dirty: true,
            epoch: Instant::now(),
            skip: false,
            resizer: fast_image_resize::Resizer::new(),
            scaled: Vec::new(),
            packed: Vec::new(),
            scale: 1.0,
        }
    }

    /// Encodes frames at `scale` (0.25–1) of their size from the next one
    /// on, for a processor that can't keep up at full size. A change of
    /// size starts with a keyframe.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.25, 1.0);
    }

    /// Applies new settings. A bitrate change takes effect in place; other
    /// changes rebuild the encoder (and so start with a keyframe).
    pub fn set_settings(&mut self, settings: EncoderSettings) {
        if settings == self.settings {
            return;
        }
        let rate_only = settings.max_fps == self.settings.max_fps;
        self.settings = settings;
        if rate_only && !self.dirty && self.set_bitrate() {
            return;
        }
        self.dirty = true;
    }

    /// Lets rate control drop frames to hold the bitrate (on), or keep every
    /// frame and lower quality instead (off). Off suits a fast link: motion
    /// stays smooth, and the viewer's frame acks still pace us. On suits a
    /// congested or deliberately limited one, where big frames mean lag.
    pub fn set_frame_skip(&mut self, on: bool) {
        if on == self.skip {
            return;
        }
        self.skip = on;
        if let Some(enc) = self.inner.as_mut() {
            let mut v = on;
            // SAFETY: a valid encoder handle; RC_FRAME_SKIP takes a bool*.
            unsafe { enc.raw_api().set_option(openh264_sys2::ENCODER_OPTION_RC_FRAME_SKIP, (&raw mut v).cast()) };
        }
    }

    fn kbps(&self, width: usize, height: usize) -> u32 {
        match self.settings.bitrate_kbps {
            0 => auto_bitrate_kbps(width as u32, height as u32, self.settings.max_fps.clamp(1.0, 240.0)),
            n => n,
        }
    }

    /// Changes the running encoder's bitrate. False if there is none or
    /// OpenH264 refused.
    fn set_bitrate(&mut self) -> bool {
        let kbps = self.kbps(self.planes.width, self.planes.height);
        let Some(enc) = self.inner.as_mut() else { return false };
        let mut info = openh264_sys2::SBitrateInfo {
            iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
            iBitrate: (kbps as i32).saturating_mul(1000),
        };
        // SAFETY: a valid encoder handle and a live SBitrateInfo, which is
        // what ENCODER_OPTION_BITRATE expects.
        let rc = unsafe {
            enc.raw_api().set_option(openh264_sys2::ENCODER_OPTION_BITRATE, (&raw mut info).cast())
        };
        if rc == 0 {
            tracing::debug!(kbps, "bitrate changed");
        }
        rc == 0
    }

    fn rebuild(&mut self, width: usize, height: usize) -> Result<()> {
        let fps = self.settings.max_fps.clamp(1.0, 240.0);
        let kbps = self.kbps(width, height);
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 8) as u16;
        // Camera mode, though this is a screen: OpenH264's screen-content
        // mode is about 3x slower, ignores extra threads, and is no
        // sharper. Measured at 1280x800 on 2 cores, scrolling text: 3.25 vs
        // 1.17 ms a frame, 32.3 vs 29.8 dB while it moves, and 35.6 vs 49.0
        // dB once it's still (examples/encode_bench.rs). On a slow virtual
        // machine that's the difference between ~35 and 60 frames a second.
        let config = EncoderConfig::new()
            .usage_type(UsageType::CameraVideoRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(kbps * 1000))
            .max_frame_rate(FrameRate::from_hz(fps))
            .skip_frames(self.skip)
            .complexity(Complexity::Low)
            .adaptive_quantization(false)
            .background_detection(false)
            .sps_pps_strategy(SpsPpsStrategy::ConstantId)
            // Size-limited slices let OpenH264 encode on several threads
            // (~3x faster than a single slice at 1080p).
            .max_slice_len(60_000)
            .num_threads(threads);
        self.inner = Some(
            openh264::encoder::Encoder::with_api_config(OpenH264API::from_source(), config)
                .context("creating H.264 encoder")?,
        );
        self.planes = I420::new(width, height);
        self.dirty = false;
        tracing::debug!(width, height, kbps, fps, "encoder configured");
        Ok(())
    }

    /// Scales `frame` down to `w`×`h` into a reused buffer.
    fn scale(&mut self, frame: &RawFrame, w: u32, h: u32) -> Result<RawFrame> {
        use fast_image_resize as fr;
        let row = frame.width as usize * 4;
        // The resizer wants tightly packed rows.
        let src: &[u8] = if frame.stride as usize == row {
            &frame.data
        } else {
            self.packed.clear();
            for y in 0..frame.height as usize {
                let o = y * frame.stride as usize;
                self.packed.extend_from_slice(&frame.data[o..o + row]);
            }
            &self.packed
        };
        let src = fr::images::ImageRef::new(frame.width, frame.height, src, fr::PixelType::U8x4)?;
        let mut buf = std::mem::take(&mut self.scaled);
        buf.resize(w as usize * h as usize * 4, 0);
        let mut dst = fr::images::Image::from_slice_u8(w, h, &mut buf, fr::PixelType::U8x4)?;
        // Not use_alpha: BGRx's padding byte is no alpha, often 0.
        let opts = fr::ResizeOptions::new()
            .use_alpha(false)
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear));
        self.resizer.resize(&src, &mut dst, &opts)?;
        Ok(RawFrame { width: w, height: h, stride: w * 4, format: frame.format, data: buf })
    }

    /// Encodes a frame. Returns `None` if the encoder chose to skip it.
    pub fn encode(&mut self, frame: &RawFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>> {
        frame.validate()?;
        // H.264 4:2:0 needs even dimensions (a trailing odd row/column is
        // dropped), and OpenH264 a picture no bigger than 4K.
        let (ew, eh) = encode_size(frame.width, frame.height);
        let (ew, eh) = if self.scale < 1.0 {
            (((ew as f32 * self.scale) as u32 & !1).max(2), ((eh as f32 * self.scale) as u32 & !1).max(2))
        } else {
            (ew, eh)
        };
        let (w, h) = (ew as usize, eh as usize);
        if w < 2 || h < 2 {
            bail!("frame too small");
        }
        let resized = self.planes.width != w || self.planes.height != h;
        if self.dirty || resized || self.inner.is_none() {
            self.rebuild(w, h)?;
        }
        if (ew, eh) != (frame.width & !1, frame.height & !1) {
            let scaled = self.scale(frame, ew, eh)?;
            let r = self.convert(&scaled, w, h);
            self.scaled = scaled.data;
            r?;
        } else {
            self.convert(frame, w, h)?;
        }

        let enc = self.inner.as_mut().expect("encoder built");
        if force_keyframe {
            enc.force_intra_frame();
        }
        let ts = Timestamp::from_millis(self.epoch.elapsed().as_millis() as u64);
        let bs = enc.encode_at(&self.planes, ts).context("H.264 encode")?;
        let keyframe = match bs.frame_type() {
            FrameType::Skip | FrameType::Invalid => return Ok(None),
            FrameType::IDR | FrameType::I => true,
            _ => false,
        };
        let data = bs.to_vec();
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(EncodedFrame { data, keyframe, width: w as u32, height: h as u32 }))
    }

    /// Converts `frame` (at least `w`×`h`) to I420 in our planes.
    fn convert(&mut self, frame: &RawFrame, w: usize, h: usize) -> Result<()> {
        let mut image = YuvPlanarImageMut::<u8>::alloc(w as u32, h as u32, YuvChromaSubsampling::Yuv420);
        // Write straight into our planes by swapping buffers in and out.
        image.y_plane = yuv::BufferStoreMut::Owned(std::mem::take(&mut self.planes.y));
        image.u_plane = yuv::BufferStoreMut::Owned(std::mem::take(&mut self.planes.u));
        image.v_plane = yuv::BufferStoreMut::Owned(std::mem::take(&mut self.planes.v));
        image.y_stride = w as u32;
        image.u_stride = (w / 2) as u32;
        image.v_stride = (w / 2) as u32;
        let convert = match frame.format {
            PixelFormat::Bgra => yuv::bgra_to_yuv420,
            PixelFormat::Rgba => yuv::rgba_to_yuv420,
        };
        let res = convert(
            &mut image,
            &frame.data,
            frame.stride,
            YuvRange::Limited,
            YuvStandardMatrix::Bt601,
            YuvConversionMode::Balanced,
        );
        let take = |b: yuv::BufferStoreMut<'_, u8>| match b {
            yuv::BufferStoreMut::Owned(v) => v,
            yuv::BufferStoreMut::Borrowed(_) => unreachable!(),
        };
        self.planes.y = take(image.y_plane);
        self.planes.u = take(image.u_plane);
        self.planes.v = take(image.v_plane);
        res.map_err(|e| anyhow::anyhow!("colour conversion: {e:?}"))
    }
}

/// A decoded frame in planar I420, ready for GPU upload.
#[derive(Debug, Clone)]
pub struct YuvFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub y_stride: u32,
    pub uv_stride: u32,
}

impl YuvFrame {
    /// CPU conversion to RGBA, for thumbnails/screenshots and tests.
    pub fn to_rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 4];
        for row in 0..h {
            for col in 0..w {
                let y = self.y[row * self.y_stride as usize + col] as f32 - 16.0;
                let ci = (row / 2) * self.uv_stride as usize + col / 2;
                let u = self.u[ci] as f32 - 128.0;
                let v = self.v[ci] as f32 - 128.0;
                let r = 1.164 * y + 1.596 * v;
                let g = 1.164 * y - 0.392 * u - 0.813 * v;
                let b = 1.164 * y + 2.017 * u;
                let o = (row * w + col) * 4;
                out[o] = r.clamp(0.0, 255.0) as u8;
                out[o + 1] = g.clamp(0.0, 255.0) as u8;
                out[o + 2] = b.clamp(0.0, 255.0) as u8;
                out[o + 3] = 255;
            }
        }
        out
    }
}

pub struct Decoder {
    inner: OhDecoder,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        let inner = OhDecoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())
            .context("creating H.264 decoder")?;
        Ok(Self { inner })
    }

    /// Decodes one access unit. `Ok(None)` means the decoder needs more data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<YuvFrame>> {
        let Some(img) = self.inner.decode(data).context("H.264 decode")? else { return Ok(None) };
        let (w, h) = img.dimensions();
        let (ys, us, _vs) = img.strides();
        let ch = h.div_ceil(2);
        Ok(Some(YuvFrame {
            width: w as u32,
            height: h as u32,
            y: img.y()[..ys * h].to_vec(),
            u: img.u()[..us * ch].to_vec(),
            v: img.v()[..us * ch].to_vec(),
            y_stride: ys as u32,
            uv_stride: us as u32,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test --release -p slipstream-codec bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_encode_decode() {
        for (w, h) in [(1920u32, 1080u32), (2560, 1440), (3840, 2160)] {
            let mut enc = Encoder::new(EncoderSettings::default());
            let mut dec = Decoder::new().unwrap();
            let frames: Vec<RawFrame> = (0..4).map(|t| test_frame(w, h, t * 7)).collect();
            let n = 60;
            let (mut te, mut td, mut bytes) = (0.0, 0.0, 0usize);
            for i in 0..n {
                let t = std::time::Instant::now();
                let e = enc.encode(&frames[i % 4], i == 0).unwrap();
                te += t.elapsed().as_secs_f64();
                if let Some(e) = e {
                    bytes += e.data.len();
                    let t = std::time::Instant::now();
                    dec.decode(&e.data).unwrap();
                    td += t.elapsed().as_secs_f64();
                }
            }
            println!(
                "{w}x{h}: encode {:.1} ms/frame, decode {:.1} ms/frame, {:.0} KB/frame",
                te * 1000.0 / n as f64,
                td * 1000.0 / n as f64,
                bytes as f64 / n as f64 / 1024.0
            );
        }
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

    /// Adaptive bitrate must not cost a keyframe (a burst of 10x the bytes
    /// exactly when the link is congested).
    #[test]
    fn bitrate_changes_in_place() {
        let mut enc = Encoder::new(EncoderSettings { bitrate_kbps: 8000, max_fps: 30.0 });
        let mut frames = (0..10).map(|t| test_frame(640, 480, t * 3));
        assert!(enc.encode(&frames.next().unwrap(), true).unwrap().unwrap().keyframe);
        for f in frames.by_ref().take(3) {
            enc.encode(&f, false).unwrap();
        }
        enc.set_settings(EncoderSettings { bitrate_kbps: 4000, max_fps: 30.0 });
        assert!(!enc.dirty, "bitrate change rebuilt the encoder");
        for f in frames {
            if let Some(e) = enc.encode(&f, false).unwrap() {
                assert!(!e.keyframe, "bitrate change produced a keyframe");
            }
        }
        // A frame-rate change still rebuilds.
        enc.set_settings(EncoderSettings { bitrate_kbps: 4000, max_fps: 60.0 });
        assert!(enc.dirty);
    }

    #[test]
    fn oversized_screens_are_scaled_to_fit() {
        assert_eq!(encode_size(1920, 1080), (1920, 1080));
        assert_eq!(encode_size(1921, 1081), (1920, 1080));
        assert_eq!(encode_size(3840, 2160), (3840, 2160));
        for (w, h) in [(4096, 2160), (5120, 1440), (5120, 2880), (7680, 2160), (3440, 2880), (2160, 5120)] {
            let (ew, eh) = encode_size(w, h);
            assert!(ew as u64 * eh as u64 <= 3840 * 2160 && ew.max(eh) <= 3840, "{w}x{h} -> {ew}x{eh}");
            assert!(((ew as f64 / eh as f64) - (w as f64 / h as f64)).abs() < 0.01, "aspect kept");
        }
        // And the encoder takes them (with padded rows, as PipeWire sends).
        let mut enc = Encoder::new(EncoderSettings::default());
        let mut dec = Decoder::new().unwrap();
        let mut src = test_frame(5120, 1440, 0);
        src.data.iter_mut().skip(3).step_by(4).for_each(|x| *x = 0); // BGRx with X = 0
        let e = enc.encode(&src, true).unwrap().expect("encoded");
        assert_eq!((e.width, e.height), encode_size(5120, 1440));
        let d = dec.decode(&e.data).unwrap().expect("decoded");
        // The red checkerboard survives (padding byte isn't treated as alpha).
        let rgba = d.to_rgba();
        assert!(rgba.chunks(4).any(|p| p[0] > 180), "picture went dark");
    }

    #[test]
    fn roundtrip_preserves_image() {
        let mut enc = Encoder::new(EncoderSettings { bitrate_kbps: 8000, max_fps: 30.0 });
        let mut dec = Decoder::new().unwrap();
        let mut decoded = None;
        for t in 0..5 {
            let src = test_frame(321, 241, t);
            if let Some(f) = enc.encode(&src, t == 0).unwrap() {
                assert_eq!((f.width, f.height), (320, 240));
                if t == 0 {
                    assert!(f.keyframe);
                }
                decoded = dec.decode(&f.data).unwrap().or(decoded);
            }
        }
        let d = decoded.expect("decoded a frame");
        assert_eq!((d.width, d.height), (320, 240));
        // Compare the red channel of the checkerboard (robust to compression).
        let rgba = d.to_rgba();
        let src = test_frame(321, 241, 4);
        let mut err = 0u64;
        for y in (8..232).step_by(16) {
            for x in (8..312).step_by(16) {
                let s = src.data[(y * src.stride + x * 4 + 2) as usize] as i64;
                let o = rgba[((y * 320 + x) * 4) as usize] as i64;
                err += (s - o).unsigned_abs();
            }
        }
        let samples = (224 / 16) * (304 / 16);
        assert!(err / samples < 25, "mean red error {} too high", err / samples);
    }
}


