//! Video encoding and decoding.
//!
//! Today this is software H.264 via OpenH264 with SIMD colour conversion.
//! The [`Encoder`]/[`Decoder`] types are the seam where hardware codecs
//! (VA-API, NVENC, VideoToolbox, Media Foundation) plug in later.

use anyhow::{Context, Result, bail};
use openh264::{
    OpenH264API,
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
}

impl Encoder {
    pub fn new(settings: EncoderSettings) -> Self {
        Self { settings, inner: None, planes: I420::new(2, 2), dirty: true }
    }

    pub fn set_settings(&mut self, settings: EncoderSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.dirty = true;
        }
    }

    fn rebuild(&mut self, width: usize, height: usize) -> Result<()> {
        let fps = self.settings.max_fps.clamp(1.0, 240.0);
        let kbps = match self.settings.bitrate_kbps {
            0 => auto_bitrate_kbps(width as u32, height as u32, fps),
            n => n,
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4) as u16;
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(kbps * 1000))
            .max_frame_rate(FrameRate::from_hz(fps))
            .skip_frames(true)
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

    /// Encodes a frame. Returns `None` if the encoder chose to skip it.
    pub fn encode(&mut self, frame: &RawFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>> {
        frame.validate()?;
        // H.264 4:2:0 needs even dimensions; drop a trailing odd row/column.
        let (w, h) = ((frame.width & !1) as usize, (frame.height & !1) as usize);
        if w < 2 || h < 2 {
            bail!("frame too small");
        }
        let resized = self.planes.width != w || self.planes.height != h;
        if self.dirty || resized || self.inner.is_none() {
            self.rebuild(w, h)?;
        }

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
        res.map_err(|e| anyhow::anyhow!("colour conversion: {e:?}"))?;

        let enc = self.inner.as_mut().expect("encoder built");
        if force_keyframe {
            enc.force_intra_frame();
        }
        let bs = enc.encode(&self.planes).context("H.264 encode")?;
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

