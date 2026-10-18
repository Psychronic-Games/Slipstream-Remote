//! Encoder speed on screen-like content: `cargo run --release -p slipstream-codec --example encode_bench`
//! (with `taskset -c 0,1` to mimic a small VM).
use std::time::Instant;

use openh264::{
    OpenH264API,
    encoder::{BitRate, Complexity, Encoder, EncoderConfig, FrameRate, RateControlMode, SpsPpsStrategy, UsageType},
    formats::YUVBuffer,
};

fn desktop(w: usize, h: usize) -> Vec<u8> {
    // Text-like rows of glyph blocks on white, a few colour panels.
    let mut rgb = vec![255u8; w * h * 3];
    let mut seed = 12345u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            if (y % 18) < 12 && (x % 9) < 6 && (rnd() % 3 == 0) {
                rgb[i..i + 3].copy_from_slice(&[20, 20, 20]);
            }
            if x < 200 {
                rgb[i..i + 3].copy_from_slice(&[40, 60, 120]);
            }
        }
    }
    rgb
}

fn psnr(a: &[u8], b: &[u8], b_stride: usize, w: usize, h: usize) -> f64 {
    let mut se = 0f64;
    for y in 0..h {
        for x in 0..w {
            let d = a[y * w + x] as f64 - b[y * b_stride + x] as f64;
            se += d * d;
        }
    }
    let mse = se / (w * h) as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

/// `scroll`: px a frame (0 = a still screen after one change).
fn run(name: &str, cfg: EncoderConfig, w: usize, h: usize, page: &[u8], scroll: usize) {
    use openh264::formats::YUVSource;
    let mut enc = Encoder::with_api_config(OpenH264API::from_source(), cfg).unwrap();
    let mut dec = openh264::decoder::Decoder::new().unwrap();
    let frames = 120;
    let (mut total, mut bytes, mut q, mut last_q) = (0.0, 0, 0.0, 0.0);
    for f in 0..frames {
        let off = if scroll == 0 { if f < 2 { f * 7 } else { 7 } } else { (f * scroll) % h };
        let rgb = &page[off * w * 3..(off + h) * w * 3];
        let yuv = YUVBuffer::from_rgb_source(openh264::formats::RgbSliceU8::new(rgb, (w, h)));
        let t = Instant::now();
        let bs = enc.encode(&yuv).unwrap();
        if f >= 10 {
            total += t.elapsed().as_secs_f64();
        }
        let data = bs.to_vec();
        bytes += data.len();
        if let Ok(Some(img)) = dec.decode(&data) {
            let p = psnr(yuv.y(), img.y(), img.strides().0, w, h);
            if f >= 10 {
                q += p;
            }
            last_q = p;
        }
    }
    let n = (frames - 10) as f64;
    println!("{name:<28} {:>5.2} ms/frame {:>6} kbit/frame  PSNR avg {:>5.1} dB, last {:>5.1} dB", total * 1000.0 / n, bytes * 8 / 1000 / frames, q / n, last_q);
}

fn base(w: u32, h: u32, threads: u16) -> EncoderConfig {
    let kbps = ((w as f64 * h as f64 * 60.0 * 0.07 / 1000.0) as u32).clamp(1500, 25000);
    EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(kbps * 1000))
        .max_frame_rate(FrameRate::from_hz(60.0))
        .skip_frames(false)
        .complexity(Complexity::Low)
        .adaptive_quantization(false)
        .background_detection(false)
        .sps_pps_strategy(SpsPpsStrategy::ConstantId)
        .max_slice_len(60_000)
        .num_threads(threads)
}

fn main() {
    let (w, h) = (1280usize, 800usize);
    let page = desktop(w, h * 2);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4) as u16;
    println!("{threads} threads");
    for (label, scroll) in [("scrolling", 6), ("still", 0)] {
        println!("-- {label}");
        let b = || base(w as u32, h as u32, threads);
        run("screen (current)", b(), w, h, &page, scroll);
        run("camera", b().usage_type(UsageType::CameraVideoRealTime), w, h, &page, scroll);
        run("camera, medium complexity", b().usage_type(UsageType::CameraVideoRealTime).complexity(Complexity::Medium), w, h, &page, scroll);
    }
}
