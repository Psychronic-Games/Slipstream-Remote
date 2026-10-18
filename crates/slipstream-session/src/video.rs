//! Host-side video pipeline: capture → encode → send, paced by viewer acks.
//!
//! The encoder only starts a frame when fewer than `limit` frames are
//! unacknowledged, where `limit` grows with the link's base round-trip time
//! so that high-latency links still get full frame rates. (The base, not the
//! smoothed RTT: that includes queueing, and letting queueing raise the
//! limit would only queue more.) Frames are skipped
//! *before* encoding, so the H.264 reference chain is never broken.
//!
//! Bitrate adapts AIMD-style: if frame round trips stretch well beyond the
//! best seen (queueing somewhere), the bitrate drops; while things are calm
//! it creeps back up toward the target.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use slipstream_codec::{Encoder, EncoderSettings, RawFrame, auto_bitrate_kbps};
use slipstream_core::proto::{StreamSettings, VideoFrame};
use slipstream_platform::FrameStream;
use tokio::sync::mpsc;

/// Per-session frame accounting shared by the encoder and the ack handler.
pub struct Flow {
    state: Mutex<FlowState>,
    cv: Condvar,
}

struct FlowState {
    next_seq: u64,
    acked: u64,
    /// (seq, sent at) for frames in flight.
    sent: VecDeque<(u64, Instant)>,
    srtt: Option<Duration>,
    min_rtt: Option<Duration>,
}

impl Flow {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FlowState {
                next_seq: 1,
                acked: 0,
                sent: VecDeque::new(),
                srtt: None,
                min_rtt: None,
            }),
            cv: Condvar::new(),
        })
    }

    pub fn ack(&self, seq: u64) {
        let mut s = self.state.lock().unwrap();
        if seq <= s.acked {
            return;
        }
        s.acked = seq;
        while let Some(&(sq, at)) = s.sent.front() {
            if sq > seq {
                break;
            }
            s.sent.pop_front();
            if sq == seq {
                let sample = at.elapsed();
                s.srtt = Some(match s.srtt {
                    None => sample,
                    Some(prev) => (prev * 7 + sample) / 8,
                });
                s.min_rtt = Some(s.min_rtt.map_or(sample, |m| m.min(sample)));
            }
        }
        self.cv.notify_all();
    }

    fn in_flight(s: &FlowState) -> u64 {
        s.next_seq - 1 - s.acked
    }

    /// Blocks until another frame may be sent, or `timeout` passes.
    fn wait_for_credit(&self, base_limit: u32, frame_interval: Duration, timeout: Duration) -> bool {
        let s = self.state.lock().unwrap();
        let (s, _) = self
            .cv
            .wait_timeout_while(s, timeout, |s| Flow::in_flight(s) >= limit(s, base_limit, frame_interval))
            .unwrap();
        Flow::in_flight(&s) < limit(&s, base_limit, frame_interval)
    }

    fn take_seq(&self) -> u64 {
        let mut s = self.state.lock().unwrap();
        let seq = s.next_seq;
        s.next_seq += 1;
        s.sent.push_back((seq, Instant::now()));
        seq
    }

    fn rtts(&self) -> (Option<Duration>, Option<Duration>) {
        let s = self.state.lock().unwrap();
        (s.srtt, s.min_rtt)
    }
}

fn limit(s: &FlowState, base: u32, frame_interval: Duration) -> u64 {
    let base = base.max(1) as u64;
    match s.min_rtt {
        Some(rtt) if !frame_interval.is_zero() => {
            let needed = (rtt.as_secs_f64() / frame_interval.as_secs_f64()).ceil() as u64 + 1;
            needed.clamp(base, 8)
        }
        _ => base,
    }
}

/// Controls for a running pipeline.
pub struct Pipeline {
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    settings: Arc<Mutex<StreamSettings>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Pipeline {
    /// Starts encoding frames from `capture` into `out`. Fatal errors are
    /// reported on `errors`.
    pub fn start(
        capture: FrameStream,
        flow: Arc<Flow>,
        settings: StreamSettings,
        out: mpsc::Sender<VideoFrame>,
        errors: mpsc::UnboundedSender<String>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let settings = Arc::new(Mutex::new(settings));
        let thread = {
            let (stop, keyframe, settings) = (stop.clone(), keyframe.clone(), settings.clone());
            thread::Builder::new()
                .name("encoder".into())
                .spawn(move || encode_loop(capture, flow, settings, keyframe, stop, out, errors))
                .expect("spawn encoder thread")
        };
        Self { stop, keyframe, settings, thread: Some(thread) }
    }

    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    pub fn set_settings(&self, s: StreamSettings) {
        *self.settings.lock().unwrap() = s;
    }

    pub fn settings(&self) -> StreamSettings {
        *self.settings.lock().unwrap()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            // The encoder notices `stop` within ~100 ms.
            let _ = t.join();
        }
    }
}

struct RateControl {
    target_kbps: u32,
    current_kbps: u32,
    last_change: Instant,
}

impl RateControl {
    fn new(target: u32) -> Self {
        Self { target_kbps: target, current_kbps: target, last_change: Instant::now() }
    }

    /// Returns a new bitrate if it should change.
    fn update(&mut self, srtt: Option<Duration>, min_rtt: Option<Duration>) -> Option<u32> {
        let (Some(srtt), Some(min_rtt)) = (srtt, min_rtt) else { return None };
        let since = self.last_change.elapsed();
        let queueing = srtt > min_rtt * 2 + Duration::from_millis(40);
        let floor = (self.target_kbps / 8).max(500);
        if queueing && since > Duration::from_millis(500) && self.current_kbps > floor {
            self.current_kbps = (self.current_kbps * 3 / 4).max(floor);
        } else if !queueing && since > Duration::from_secs(2) && self.current_kbps < self.target_kbps {
            self.current_kbps = (self.current_kbps + self.target_kbps / 10).min(self.target_kbps);
        } else {
            return None;
        }
        self.last_change = Instant::now();
        Some(self.current_kbps)
    }
}

fn encode_loop(
    capture: FrameStream,
    flow: Arc<Flow>,
    settings: Arc<Mutex<StreamSettings>>,
    keyframe: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    out: mpsc::Sender<VideoFrame>,
    errors: mpsc::UnboundedSender<String>,
) {
    let start = Instant::now();
    let mut applied = *settings.lock().unwrap();
    let mut encoder = Encoder::new(EncoderSettings { bitrate_kbps: applied.bitrate_kbps, max_fps: applied.max_fps as f32 });
    let mut rate: Option<RateControl> = None;
    let mut last_frame: Option<RawFrame> = None;
    // The newest frame rate control skipped. Damage-driven capture may send
    // nothing more once the screen goes still, so it's retried then.
    let mut skipped: Option<RawFrame> = None;
    // When the last frame was *taken* (not sent), so encode time doesn't
    // stretch the frame interval.
    let mut last_taken = Instant::now() - Duration::from_secs(1);
    let mut pace = slipstream_platform::pace::Pace::new("video encoding");
    let mut waited_viewer = slipstream_platform::pace::Pace::new("waiting for the viewer");
    let mut waited_screen = slipstream_platform::pace::Pace::new("waiting for a new screen frame");

    while !stop.load(Ordering::Relaxed) {
        let current = *settings.lock().unwrap();
        if current != applied {
            applied = current;
            rate = None;
            encoder.set_settings(EncoderSettings { bitrate_kbps: applied.bitrate_kbps, max_fps: applied.max_fps as f32 });
        }
        let frame_interval = Duration::from_secs_f64(1.0 / applied.max_fps.clamp(1, 240) as f64);

        let t = Instant::now();
        if !flow.wait_for_credit(applied.max_in_flight, frame_interval, Duration::from_millis(100)) {
            continue;
        }
        waited_viewer.add(t.elapsed());
        // Pace to max_fps (with slack for capture jitter).
        if let Some(rest) = frame_interval.mul_f32(0.9).checked_sub(last_taken.elapsed()) {
            thread::sleep(rest);
        }

        let t = Instant::now();
        let frame = match capture.next(Duration::from_millis(100)) {
            Ok(Some(f)) => {
                waited_screen.add(t.elapsed());
                f
            }
            Ok(None) => {
                // Screen is static: send a skipped frame, or re-encode the
                // last one if a keyframe is owed.
                if let Some(f) = skipped.take() {
                    f
                } else if keyframe.load(Ordering::Relaxed)
                    && let Some(f) = last_frame.clone()
                {
                    f
                } else {
                    continue;
                }
            }
            Err(e) => {
                let _ = errors.send(e);
                return;
            }
        };
        last_taken = Instant::now();

        // Adaptive bitrate (only in automatic mode).
        if applied.bitrate_kbps == 0 {
            let rc = rate.get_or_insert_with(|| {
                RateControl::new(auto_bitrate_kbps(frame.width, frame.height, applied.max_fps as f32))
            });
            let (srtt, min_rtt) = flow.rtts();
            if let Some(kbps) = rc.update(srtt, min_rtt) {
                tracing::debug!(kbps, ?srtt, ?min_rtt, "bitrate adjusted");
                encoder.set_settings(EncoderSettings { bitrate_kbps: kbps, max_fps: applied.max_fps as f32 });
            }
        }
        // Keep every frame while the link keeps up; drop frames to hold the
        // rate once it's congested, or when the viewer chose a fixed rate.
        let congested = rate.as_ref().is_some_and(|r| r.current_kbps < r.target_kbps);
        encoder.set_frame_skip(applied.bitrate_kbps != 0 || congested);

        let force = keyframe.swap(false, Ordering::Relaxed);
        let t = Instant::now();
        let encoded = match encoder.encode(&frame, force) {
            Ok(Some(e)) => {
                pace.add(t.elapsed());
                e
            }
            Ok(None) => {
                if force {
                    keyframe.store(true, Ordering::Relaxed);
                }
                if let Some(old) = skipped.replace(frame) {
                    capture.recycle(old);
                }
                continue;
            }
            Err(e) => {
                let _ = errors.send(format!("encoder: {e:#}"));
                return;
            }
        };
        if let Some(old) = skipped.take() {
            capture.recycle(old);
        }
        if let Some(old) = last_frame.replace(frame) {
            capture.recycle(old);
        }
        let seq = flow.take_seq();
        let msg = VideoFrame {
            seq,
            keyframe: encoded.keyframe,
            width: encoded.width,
            height: encoded.height,
            timestamp_us: start.elapsed().as_micros() as u64,
            data: encoded.data,
        };
        if out.blocking_send(msg).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_limits_in_flight() {
        let flow = Flow::new();
        let iv = Duration::from_millis(16);
        assert!(flow.wait_for_credit(2, iv, Duration::ZERO));
        flow.take_seq();
        flow.take_seq();
        assert!(!flow.wait_for_credit(2, iv, Duration::from_millis(5)));
        flow.ack(1);
        assert!(flow.wait_for_credit(2, iv, Duration::ZERO));
        flow.ack(2);
        assert!(flow.rtts().0.is_some());
    }

    /// `cargo test --release -p slipstream-session pipeline_rate -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn pipeline_rate() {
        use slipstream_codec::PixelFormat;
        use slipstream_platform::{FrameSlot, FrameStream};
        // Replays pre-rendered, fully moving frames at 60 fps through a real
        // frame slot: a worst case for the encoder (every pixel changes).
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (2560, 1440), (3840, 2160)] {
            let frames: Vec<Vec<u8>> = (0..8u32)
                .map(|t| {
                    let mut d = vec![255u8; (w * h * 4) as usize];
                    for (i, px) in d.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let (x, y) = (i as u32 % w, i as u32 / w);
                        px[0] = ((x + t * 9) % 256) as u8;
                        px[1] = ((y + t * 5) % 256) as u8;
                        px[2] = if ((x + t * 9) / 40 + y / 40) % 2 == 0 { 220 } else { 30 };
                    }
                    d
                })
                .collect();
            let slot = FrameSlot::new();
            let producer = slot.clone();
            std::thread::spawn(move || {
                let mut i = 0;
                while !producer.is_closed() {
                    let mut data = producer.buffer(frames[0].len());
                    data.extend_from_slice(&frames[i % frames.len()]);
                    producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Bgra, data });
                    i += 1;
                    std::thread::sleep(Duration::from_micros(16_667));
                }
            });
            let capture = FrameStream::new(slot);
            let flow = Flow::new();
            let (tx, mut rx) = mpsc::channel(4);
            let (etx, _erx) = mpsc::unbounded_channel();
            let _p = Pipeline::start(capture, flow.clone(), StreamSettings::default(), tx, etx);
            let start = Instant::now();
            let (mut n, mut bytes) = (0, 0);
            while start.elapsed() < Duration::from_secs(3) {
                if let Some(f) = rx.blocking_recv() {
                    n += 1;
                    bytes += f.data.len();
                    flow.ack(f.seq);
                }
            }
            println!("{w}x{h}: {:.1} fps through the pipeline, {:.0} KB/frame", n as f64 / 3.0, bytes as f64 / n.max(1) as f64 / 1024.0);
        }
    }

    #[test]
    fn rate_control_backs_off_and_recovers() {
        let mut rc = RateControl::new(8000);
        rc.last_change -= Duration::from_secs(3);
        let base = Some(Duration::from_millis(20));
        assert_eq!(rc.update(Some(Duration::from_millis(200)), base), Some(6000));
        rc.last_change -= Duration::from_secs(3);
        assert_eq!(rc.update(Some(Duration::from_millis(25)), base), Some(6800));
    }
}
