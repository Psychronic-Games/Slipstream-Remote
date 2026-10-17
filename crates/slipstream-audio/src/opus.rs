//! The vendored libopus, wrapped for one use: 48 kHz stereo in 20 ms frames.

use std::ffi::{CStr, c_char, c_int};

use anyhow::{Result, bail};

/// Samples per channel in one 20 ms frame.
pub const FRAME: usize = 960;
/// Interleaved stereo samples in one frame.
pub const FRAME_SAMPLES: usize = FRAME * 2;
pub const SAMPLE_RATE: u32 = 48_000;

#[repr(C)]
struct RawEncoder {
    _p: [u8; 0],
}
#[repr(C)]
struct RawDecoder {
    _p: [u8; 0],
}

const APPLICATION_AUDIO: c_int = 2049;
const SET_BITRATE: c_int = 4002;
const SET_PACKET_LOSS_PERC: c_int = 4014;

unsafe extern "C" {
    fn opus_encoder_create(fs: i32, channels: c_int, application: c_int, error: *mut c_int) -> *mut RawEncoder;
    fn opus_encoder_destroy(st: *mut RawEncoder);
    fn opus_encode(st: *mut RawEncoder, pcm: *const i16, frame_size: c_int, data: *mut u8, max: i32) -> i32;
    fn opus_encoder_ctl(st: *mut RawEncoder, request: c_int, ...) -> c_int;
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut RawDecoder;
    fn opus_decoder_destroy(st: *mut RawDecoder);
    fn opus_decode(st: *mut RawDecoder, data: *const u8, len: i32, pcm: *mut i16, frame_size: c_int, fec: c_int) -> c_int;
    fn opus_strerror(error: c_int) -> *const c_char;
}

fn error(code: c_int) -> String {
    // SAFETY: opus_strerror returns a static C string for any code.
    unsafe { CStr::from_ptr(opus_strerror(code)) }.to_string_lossy().into_owned()
}

pub struct Encoder {
    raw: *mut RawEncoder,
    packet: Vec<u8>,
}

// SAFETY: the encoder state is only touched through &mut self.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(bitrate: i32) -> Result<Self> {
        let mut err = 0;
        // SAFETY: valid arguments; the result is checked.
        let raw = unsafe { opus_encoder_create(SAMPLE_RATE as i32, 2, APPLICATION_AUDIO, &mut err) };
        if raw.is_null() {
            bail!("Opus encoder: {}", error(err));
        }
        // SAFETY: a live encoder; these requests take one opus_int32.
        unsafe {
            opus_encoder_ctl(raw, SET_BITRATE, bitrate);
            // Datagrams can get lost. (Not in-band FEC: it pushes Opus into
            // its speech mode, which costs music a lot of quality.)
            opus_encoder_ctl(raw, SET_PACKET_LOSS_PERC, 5i32);
        }
        Ok(Self { raw, packet: vec![0; 1500] })
    }

    /// Encodes one frame ([`FRAME_SAMPLES`] interleaved samples).
    pub fn encode(&mut self, pcm: &[i16]) -> Result<&[u8]> {
        assert_eq!(pcm.len(), FRAME_SAMPLES);
        // SAFETY: a live encoder, a full frame, and a buffer of the size given.
        let n = unsafe { opus_encode(self.raw, pcm.as_ptr(), FRAME as c_int, self.packet.as_mut_ptr(), self.packet.len() as i32) };
        if n < 0 {
            bail!("Opus encode: {}", error(n));
        }
        Ok(&self.packet[..n as usize])
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_encoder_create, destroyed once.
        unsafe { opus_encoder_destroy(self.raw) }
    }
}

pub struct Decoder {
    raw: *mut RawDecoder,
}

// SAFETY: the decoder state is only touched through &mut self.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self> {
        let mut err = 0;
        // SAFETY: valid arguments; the result is checked.
        let raw = unsafe { opus_decoder_create(SAMPLE_RATE as i32, 2, &mut err) };
        if raw.is_null() {
            bail!("Opus decoder: {}", error(err));
        }
        Ok(Self { raw })
    }

    /// Decodes a packet into `out` (one frame), or with `None` conceals a
    /// lost one. Returns the samples per channel written.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [i16; FRAME_SAMPLES]) -> Result<usize> {
        let (ptr, len) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        // SAFETY: a live decoder; `out` holds FRAME stereo samples.
        let n = unsafe { opus_decode(self.raw, ptr, len, out.as_mut_ptr(), FRAME as c_int, 0) };
        if n < 0 {
            bail!("Opus decode: {}", error(n));
        }
        Ok(n as usize)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_decoder_create, destroyed once.
        unsafe { opus_decoder_destroy(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chord on the left, a sweep on the right, with bursts of noise.
    fn signal(frames: usize) -> Vec<i16> {
        let total = frames * FRAME;
        let mut pcm = vec![0i16; total * 2];
        let mut seed = 1u32;
        for i in 0..total {
            let t = i as f32 / SAMPLE_RATE as f32;
            let tau = 2.0 * std::f32::consts::PI;
            let chord = [261.6f32, 329.6, 392.0].iter().map(|f| (tau * f * t).sin()).sum::<f32>() / 3.0;
            let sweep = (tau * (200.0 + 2000.0 * t) * t).sin();
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((seed >> 16) as f32 / 32768.0 - 1.0) * if ((t * 4.0) as u32).is_multiple_of(2) { 0.1 } else { 0.0 };
            pcm[i * 2] = ((chord * 0.5 + noise) * 12000.0) as i16;
            pcm[i * 2 + 1] = ((sweep * 0.4 + noise) * 12000.0) as i16;
        }
        pcm
    }

    #[test]
    fn round_trip_quality() {
        let frames = 100;
        let pcm = signal(frames);
        let mut enc = Encoder::new(96_000).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut out = vec![0i16; pcm.len()];
        let mut bytes = 0;
        let mut frame = [0i16; FRAME_SAMPLES];
        for f in 0..frames {
            let packet = enc.encode(&pcm[f * FRAME_SAMPLES..(f + 1) * FRAME_SAMPLES]).unwrap().to_vec();
            bytes += packet.len();
            assert_eq!(dec.decode(Some(&packet), &mut frame).unwrap(), FRAME);
            out[f * FRAME_SAMPLES..(f + 1) * FRAME_SAMPLES].copy_from_slice(&frame);
        }
        // Opus delays the signal by 312 samples at 48 kHz.
        let (mut s, mut e) = (0f64, 0f64);
        for i in 4800..(frames * FRAME - 1300) {
            for ch in 0..2 {
                let (a, b) = (pcm[i * 2 + ch] as f64, out[(i + 312) * 2 + ch] as f64);
                s += a * a;
                e += (a - b) * (a - b);
            }
        }
        let snr = 10.0 * (s / e).log10();
        let kbps = bytes as f64 * 8.0 / 2.0 / 1000.0;
        println!("SNR {snr:.1} dB at {kbps:.0} kbps");
        // The reference libopus gets ~14 dB on this (perceptual coding
        // doesn't keep the noise's waveform); a broken build gets ~0.
        assert!(snr > 12.0, "SNR {snr:.1} dB");
        assert!((80.0..110.0).contains(&kbps), "{kbps} kbps");
        // Losing a packet: concealment still yields a frame.
        assert_eq!(dec.decode(None, &mut frame).unwrap(), FRAME);
    }
}
