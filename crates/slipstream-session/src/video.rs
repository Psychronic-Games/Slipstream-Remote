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
    /// The connection's own round-trip time (QUIC), when there is one.
    path_rtt: Option<Box<dyn Fn() -> Option<Duration> + Send + Sync>>,
}

struct Sent {
    seq: u64,
    at: Instant,
    bytes: usize,
    keyframe: bool,
}

struct FlowState {
    next_seq: u64,
    acked: u64,
    /// Frames in flight.
    sent: VecDeque<Sent>,
    /// Round trips of delivered frames (keyframes aside), smoothed.
    srtt: Option<Duration>,
    min_rtt: Option<Duration>,
    /// Bytes the viewer acknowledged, and when, over the last second.
    delivered: VecDeque<(Instant, usize)>,
    /// The connection's lowest round trip lately, and when it was seen.
    path_min: Option<(Duration, Instant)>,
    path_polled: Option<Instant>,
    /// When the first frame was acknowledged (throughput needs a while).
    first_delivery: Option<Instant>,
    /// The automatic bitrate last in use, for the next pipeline on this
    /// link (another display or settings): it starts there, not at the top.
    last_kbps: Option<u32>,
}

/// How long the connection's lowest round trip stands for the base (a
/// route can change); a newer sample replaces it after this.
const PATH_MIN_WINDOW: Duration = Duration::from_secs(30);
/// The most frames in flight, however long the round trip: at 60 fps a
/// 250 ms link needs 16 to keep the frame rate.
const MAX_IN_FLIGHT: u64 = 16;

impl Flow {
    /// Without a connection (tests): the base round trip comes from frames.
    #[cfg(test)]
    pub fn new() -> Arc<Self> {
        Self::build(None)
    }

    /// Takes the link's base round trip from `rtt` (the connection's
    /// measured path), rather than from frame acknowledgements: those
    /// include the time it takes to send a frame, which a big keyframe on
    /// a slow link stretches to seconds.
    pub fn with_path_rtt(rtt: impl Fn() -> Option<Duration> + Send + Sync + 'static) -> Arc<Self> {
        Self::build(Some(Box::new(rtt)))
    }

    fn build(path_rtt: Option<Box<dyn Fn() -> Option<Duration> + Send + Sync>>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FlowState {
                next_seq: 1,
                acked: 0,
                sent: VecDeque::new(),
                srtt: None,
                min_rtt: None,
                delivered: VecDeque::new(),
                path_min: None,
                path_polled: None,
                first_delivery: None,
                last_kbps: None,
            }),
            cv: Condvar::new(),
            path_rtt,
        })
    }

    pub fn ack(&self, seq: u64) {
        let mut s = self.state.lock().unwrap();
        if seq <= s.acked || seq >= s.next_seq {
            return;
        }
        s.acked = seq;
        let now = Instant::now();
        s.first_delivery.get_or_insert(now);
        while let Some(front) = s.sent.front() {
            if front.seq > seq {
                break;
            }
            let sent = s.sent.pop_front().unwrap();
            s.delivered.push_back((now, sent.bytes));
            // A keyframe's round trip is mostly the time to send it, not
            // queueing: leave it out, or one would set the base for good.
            if sent.seq == seq && !sent.keyframe {
                let sample = now - sent.at;
                s.srtt = Some(match s.srtt {
                    None => sample,
                    Some(prev) => (prev * 7 + sample) / 8,
                });
                s.min_rtt = Some(s.min_rtt.map_or(sample, |m| m.min(sample)));
            }
        }
        while s.delivered.front().is_some_and(|(at, _)| now - *at > DELIVERY_WINDOW) {
            s.delivered.pop_front();
        }
        self.cv.notify_all();
    }

    fn in_flight(s: &FlowState) -> u64 {
        s.next_seq - 1 - s.acked
    }

    /// Samples the connection's round trip now and then, keeping its
    /// lowest value of the last [`PATH_MIN_WINDOW`].
    fn poll_path(&self, s: &mut FlowState) {
        let Some(rtt) = &self.path_rtt else { return };
        if s.path_polled.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        s.path_polled = Some(Instant::now());
        if let Some(sample) = rtt().filter(|d| !d.is_zero()) {
            let stale = s.path_min.is_none_or(|(min, at)| sample <= min || at.elapsed() > PATH_MIN_WINDOW);
            if stale {
                s.path_min = Some((sample, Instant::now()));
            }
        }
    }

    /// Blocks until another frame may be sent, or `timeout` passes.
    fn wait_for_credit(&self, base_limit: u32, frame_interval: Duration, timeout: Duration) -> bool {
        let mut s = self.state.lock().unwrap();
        self.poll_path(&mut s);
        let (s, _) = self.cv.wait_timeout_while(s, timeout, |s| !may_send(s, base_limit, frame_interval)).unwrap();
        may_send(&s, base_limit, frame_interval)
    }

    fn take_seq(&self, bytes: usize, keyframe: bool) -> u64 {
        let mut s = self.state.lock().unwrap();
        let seq = s.next_seq;
        s.next_seq += 1;
        s.sent.push_back(Sent { seq, at: Instant::now(), bytes, keyframe });
        seq
    }

    /// The smoothed frame round trip, and the link's base round trip.
    fn rtts(&self) -> (Option<Duration>, Option<Duration>) {
        let mut s = self.state.lock().unwrap();
        self.poll_path(&mut s);
        (s.srtt, base_rtt(&s))
    }

    /// What reached the viewer lately, in kbit/s; `None` until there's
    /// enough of it to tell.
    fn delivered_kbps(&self) -> Option<u32> {
        delivered_bps(&self.state.lock().unwrap()).map(|bps| (bps * 8.0 / 1000.0) as u32)
    }

    fn remember_kbps(&self, kbps: u32) {
        self.state.lock().unwrap().last_kbps = Some(kbps);
    }

    fn last_kbps(&self) -> Option<u32> {
        self.state.lock().unwrap().last_kbps
    }
}

/// Throughput is averaged over this long: acknowledgements arrive in
/// bursts (after a loss is repaired, say), and a shorter window overstates it.
const DELIVERY_WINDOW: Duration = Duration::from_secs(2);

/// Bytes a second the viewer acknowledged over the last [`DELIVERY_WINDOW`].
fn delivered_bps(s: &FlowState) -> Option<f64> {
    let since = s.first_delivery?.elapsed();
    if since < Duration::from_secs(1) || s.delivered.len() < 4 {
        return None;
    }
    let bytes: usize = s.delivered.iter().map(|(_, b)| b).sum();
    Some(bytes as f64 / since.min(DELIVERY_WINDOW).as_secs_f64())
}

/// Whether another frame may go out. At least `base` frames always may (a
/// picture gets through however slow the link). Beyond that, frames go out
/// while the round trip needs them, and while what's in flight would reach
/// the viewer within about a round trip at the rate it's been arriving:
/// counting frames alone let a dozen big ones queue up on a slow link,
/// seconds of lag.
fn may_send(s: &FlowState, base: u32, frame_interval: Duration) -> bool {
    let frames = Flow::in_flight(s);
    if frames < base.max(1) as u64 {
        return true;
    }
    if frames >= limit(s, base, frame_interval) {
        return false;
    }
    let (Some(rate), Some(rtt)) = (delivered_bps(s), base_rtt(s)) else { return false };
    let budget = rate * (rtt.as_secs_f64() * 1.25 + 0.1);
    let in_flight: usize = s.sent.iter().map(|f| f.bytes).sum();
    (in_flight as f64) < budget
}

fn base_rtt(s: &FlowState) -> Option<Duration> {
    s.path_min.map(|(rtt, _)| rtt).or(s.min_rtt)
}

fn limit(s: &FlowState, base: u32, frame_interval: Duration) -> u64 {
    let base = base.max(1) as u64;
    match base_rtt(s) {
        Some(rtt) if !frame_interval.is_zero() => {
            let needed = (rtt.as_secs_f64() / frame_interval.as_secs_f64()).ceil() as u64 + 1;
            needed.clamp(base, MAX_IN_FLIGHT.max(base))
        }
        _ => base,
    }
}

/// Controls for a running pipeline.
pub struct Pipeline {
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    /// Keyframes the viewer asked for, so far.
    requests: Arc<std::sync::atomic::AtomicU32>,
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
        // Tests: pretend encoding a full-size frame takes this long (scaled
        // by the pixel count), as on a slow virtual machine.
        let slow = std::env::var("SLIPSTREAM_TEST_SLOW_ENCODE_MS").ok().and_then(|v| v.parse().ok());
        Self::start_with(capture, flow, settings, out, errors, slow)
    }

    fn start_with(
        capture: FrameStream,
        flow: Arc<Flow>,
        settings: StreamSettings,
        out: mpsc::Sender<VideoFrame>,
        errors: mpsc::UnboundedSender<String>,
        slow_encode_ms: Option<f32>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let requests = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let settings = Arc::new(Mutex::new(settings));
        let thread = {
            let (stop, keyframe, settings, requests) =
                (stop.clone(), keyframe.clone(), settings.clone(), requests.clone());
            thread::Builder::new()
                .name("encoder".into())
                .spawn(move || {
                    encode_loop(capture, flow, settings, keyframe, requests, stop, out, errors, slow_encode_ms)
                })
                .expect("spawn encoder thread")
        };
        Self { stop, keyframe, requests, settings, thread: Some(thread) }
    }

    /// The viewer asks for a keyframe (it lost its place, or can't decode).
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
        self.requests.fetch_add(1, Ordering::Relaxed);
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
    /// Below the target since last there.
    backed_off: bool,
    /// Since when the bitrate has been near the target (a fast link).
    full_since: Option<Instant>,
    /// Still finding out how fast the link is (no backing off yet): the
    /// bitrate doubles every second while frames don't queue up.
    probing: bool,
}

/// Automatic bitrate starts here on a link of unknown speed, and doubles
/// while it keeps up: OpenH264 sizes the first keyframe by the bitrate, and
/// at the full target (or even 1.5 Mbit/s) a slow link took seconds to
/// show the first picture. A fast one reaches 4 Mbit/s in three seconds.
const START_KBPS: u32 = 1_500;
/// The lowest automatic bitrate: a picture still gets through on a link of
/// a couple of hundred kbit/s.
const FLOOR_KBPS: u32 = 150;

impl RateControl {
    /// `base` is the link's round trip if known, and `last` the bitrate a
    /// previous pipeline on this link ended at.
    fn new(target: u32, base: Option<Duration>, last: Option<u32>) -> Self {
        let (start, probing) = match (last, base) {
            (Some(last), _) => (last, false),
            // A local network: the full target straight away.
            (None, Some(rtt)) if rtt < Duration::from_millis(20) => (target, false),
            _ => (START_KBPS, true),
        };
        let current = start.clamp(FLOOR_KBPS, target.max(FLOOR_KBPS));
        let mut rc = Self {
            target_kbps: target,
            current_kbps: current,
            last_change: Instant::now(),
            backed_off: current < target,
            full_since: None,
            probing,
        };
        rc.note_level();
        rc
    }

    fn note_level(&mut self) {
        if self.current_kbps.saturating_mul(10) >= self.target_kbps.saturating_mul(9) {
            self.full_since.get_or_insert_with(Instant::now);
        } else {
            self.full_since = None;
        }
    }

    /// Whether software encoding should be in screen-content mode (see
    /// `Encoder::set_screen_content`): once the link holds the bitrate
    /// under 40% of the target, until it has carried 90% for 5 s. Apart,
    /// so a link near either mark doesn't switch (each switch is a
    /// keyframe) back and forth.
    fn screen_content(&self, now: bool) -> bool {
        if now {
            !self.full_since.is_some_and(|t| t.elapsed() >= Duration::from_secs(5))
        } else {
            self.current_kbps.saturating_mul(5) < self.target_kbps.saturating_mul(2)
        }
    }

    /// Returns a new bitrate if it should change. `srtt` is the frames'
    /// smoothed round trip, `base` the link's own, and `delivered_kbps`
    /// what actually got through lately.
    fn update(&mut self, srtt: Option<Duration>, base: Option<Duration>, delivered_kbps: Option<u32>) -> Option<u32> {
        let (Some(srtt), Some(base)) = (srtt, base) else { return None };
        let since = self.last_change.elapsed();
        // Frames queueing up behind each other: well over the link's own
        // round trip. An absolute margin, mostly: on a long link, twice
        // the round trip would allow seconds of lag before reacting.
        let queueing = srtt > base + (base / 4).max(Duration::from_millis(60));
        if queueing && since > Duration::from_millis(500) && self.current_kbps > FLOOR_KBPS {
            // Back off at least to what's getting through.
            let mut next = self.current_kbps * 3 / 4;
            if let Some(delivered) = delivered_kbps {
                next = next.min(delivered.saturating_mul(85) / 100);
            }
            self.current_kbps = next.max(FLOOR_KBPS);
            self.backed_off = true;
            self.probing = false;
        } else if !queueing && since > Duration::from_secs(1) && self.current_kbps < self.target_kbps {
            let step =
                if self.probing { self.current_kbps } else { (self.target_kbps / 20).max(self.current_kbps / 4) };
            self.current_kbps = (self.current_kbps + step).min(self.target_kbps);
            if self.current_kbps == self.target_kbps {
                self.backed_off = false;
            }
        } else {
            return None;
        }
        self.last_change = Instant::now();
        self.note_level();
        Some(self.current_kbps)
    }
}

/// Keeps encoding inside the frame budget on a slow processor: while the
/// picture moves it's encoded smaller (down to half size), and once it's
/// still a full-size frame follows, so text at rest stays sharp.
struct Speed {
    scale: f32,
    /// Encode time at the current scale, smoothed.
    avg_ms: Option<f32>,
    changed: Instant,
    roomy_since: Option<Instant>,
    /// When the last reduced-size frame went out (cleared by a sharp one).
    last_small: Option<Instant>,
}

const MIN_SCALE: f32 = 0.5;

impl Speed {
    fn new() -> Self {
        Self { scale: 1.0, avg_ms: None, changed: Instant::now(), roomy_since: None, last_small: None }
    }

    /// Records a frame that took `ms` to encode at the current scale, and
    /// adjusts the scale to fit `budget_ms` a frame.
    fn record(&mut self, ms: f32, budget_ms: f32) {
        let avg = self.avg_ms.map_or(ms, |a| a * 0.9 + ms * 0.1);
        self.avg_ms = Some(avg);
        if self.scale < 1.0 {
            self.last_small = Some(Instant::now());
        }
        if self.changed.elapsed() < Duration::from_millis(1500) {
            return;
        }
        if avg > budget_ms * 0.9 && self.scale > MIN_SCALE {
            // Encode time follows the pixel count, so the side's square root.
            let s = self.scale * (budget_ms * 0.7 / avg).sqrt();
            self.set((s.clamp(MIN_SCALE, 1.0) * 20.0).floor() / 20.0);
        } else if avg < budget_ms * 0.4 && self.scale < 1.0 {
            if self.roomy_since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(3) {
                self.set(((self.scale * 1.25).min(1.0) * 20.0).round() / 20.0);
            }
        } else {
            self.roomy_since = None;
        }
    }

    fn set(&mut self, scale: f32) {
        if scale != self.scale {
            tracing::info!("video: encoding moving pictures at {:.0}% size to keep the frame rate", scale * 100.0);
            self.scale = scale;
            self.avg_ms = None;
            self.changed = Instant::now();
            self.roomy_since = None;
        }
    }

    /// Reduced-size frames went out and the picture has been still for
    /// `after`: time for a full-size one. Each change of size is a keyframe,
    /// so not after every short pause (typing paused every 300 ms or so,
    /// two keyframes each time).
    fn wants_sharp(&mut self, after: Duration) -> bool {
        if self.last_small.is_some_and(|t| t.elapsed() >= after) {
            self.last_small = None;
            return true;
        }
        false
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_loop(
    capture: FrameStream,
    flow: Arc<Flow>,
    settings: Arc<Mutex<StreamSettings>>,
    keyframe: Arc<AtomicBool>,
    requests: Arc<std::sync::atomic::AtomicU32>,
    stop: Arc<AtomicBool>,
    out: mpsc::Sender<VideoFrame>,
    errors: mpsc::UnboundedSender<String>,
    slow_encode_ms: Option<f32>,
) {
    // When the viewer last asked for keyframes (see below).
    let mut asked: VecDeque<Instant> = VecDeque::new();
    let mut requests_seen = 0;
    let start = Instant::now();
    let mut applied = *settings.lock().unwrap();
    let encoder_settings = EncoderSettings { bitrate_kbps: applied.bitrate_kbps, max_fps: applied.max_fps as f32 };
    // A simulated slow encoder stands for a slow processor: keep it there.
    let mut encoder = if slow_encode_ms.is_some() {
        Encoder::with_hardware(encoder_settings, false)
    } else {
        Encoder::new(encoder_settings)
    };
    let mut rate: Option<RateControl> = None;
    let mut last_frame: Option<RawFrame> = None;
    // The newest frame rate control skipped. Damage-driven capture may send
    // nothing more once the screen goes still, so it's retried then.
    let mut skipped: Option<RawFrame> = None;
    // When the last frame was *taken* (not sent), so encode time doesn't
    // stretch the frame interval.
    let mut last_taken = Instant::now() - Duration::from_secs(1);
    // Named after what encodes (graphics card or processor), for the viewer's statistics.
    let mut pace = slipstream_platform::pace::Pace::new(encoder.backend());
    let mut waited_viewer = slipstream_platform::pace::Pace::new("waiting for the viewer");
    let mut waited_screen = slipstream_platform::pace::Pace::new("waiting for a new screen frame");
    let mut speed = Speed::new();
    let mut screen_content = false;

    while !stop.load(Ordering::Relaxed) {
        let current = *settings.lock().unwrap();
        if current != applied {
            applied = current;
            rate = None;
            // A quality change must restore full size on the very next frame.
            speed = Speed::new();
            keyframe.store(true, Ordering::Relaxed);
            encoder
                .set_settings(EncoderSettings { bitrate_kbps: applied.bitrate_kbps, max_fps: applied.max_fps as f32 });
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
        // A full-size frame of a picture that came out reduced while moving.
        let mut sharp = false;
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
                } else if speed.wants_sharp(if applied.bitrate_kbps != 0 && applied.bitrate_kbps <= 2_000 {
                    // A low fixed bitrate (Low bandwidth): keyframes cost most.
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(1)
                }) && let Some(f) = last_frame.clone()
                {
                    sharp = true;
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
                let target = auto_bitrate_kbps(frame.width, frame.height, applied.max_fps as f32);
                let rc = RateControl::new(target, flow.rtts().1, flow.last_kbps());
                encoder
                    .set_settings(EncoderSettings { bitrate_kbps: rc.current_kbps, max_fps: applied.max_fps as f32 });
                rc
            });
            let (srtt, base) = flow.rtts();
            let delivered = flow.delivered_kbps();
            if let Some(kbps) = rc.update(srtt, base, delivered) {
                tracing::debug!(kbps, ?srtt, ?base, ?delivered, "bitrate adjusted");
                flow.remember_kbps(kbps);
                encoder.set_settings(EncoderSettings { bitrate_kbps: kbps, max_fps: applied.max_fps as f32 });
            }
        }
        // Automatic bitrate: the encoder keeps every frame it's given, and
        // the frame window above holds frames back while the link is full
        // (before encoding, so nothing is wasted). The encoder's own
        // skipping, after a big keyframe on a slow link, went on paying it
        // back for many seconds of frozen picture. A fixed bitrate (the
        // viewer's choice) still has the encoder hold it.
        encoder.set_frame_skip(applied.bitrate_kbps != 0);
        // Screen-content mode on a slow link: an automatic bitrate held well
        // under its target, or a low one the viewer chose.
        screen_content = match (applied.bitrate_kbps, &rate) {
            (0, Some(rc)) => rc.screen_content(screen_content),
            (0, None) => false,
            (fixed, _) => fixed <= 2_000,
        };
        encoder.set_screen_content(screen_content);

        // Three keyframe requests within 10 s: the viewer can't decode what
        // the graphics card makes (it asks again after each failure, and the
        // picture never came). The processor takes over for a while.
        let total = requests.load(Ordering::Relaxed);
        for _ in requests_seen..total {
            asked.push_back(Instant::now());
        }
        requests_seen = total;
        while asked.front().is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
            asked.pop_front();
        }
        if asked.len() >= 3 && encoder.distrust_hardware() {
            asked.clear();
            keyframe.store(true, Ordering::Relaxed);
        }
        let force = keyframe.swap(false, Ordering::Relaxed);
        let scale = if applied.adaptive_resolution && !sharp { speed.scale } else { 1.0 };
        encoder.set_scale(scale);
        let t = Instant::now();
        let encoded = match encoder.encode(&frame, force) {
            Ok(Some(e)) => {
                if pace.what() != encoder.backend() {
                    pace = slipstream_platform::pace::Pace::new(encoder.backend());
                }
                if let Some(ms) = slow_encode_ms {
                    thread::sleep(Duration::from_secs_f32(ms * scale * scale / 1000.0));
                }
                if !sharp {
                    pace.add(t.elapsed());
                    if applied.adaptive_resolution {
                        speed.record(t.elapsed().as_secs_f32() * 1000.0, frame_interval.as_secs_f32() * 1000.0);
                    }
                }
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
        if encoded.keyframe {
            tracing::debug!(bytes = encoded.data.len(), width = encoded.width, height = encoded.height, "keyframe");
        }
        let seq = flow.take_seq(encoded.data.len(), encoded.keyframe);
        let msg = VideoFrame {
            seq,
            keyframe: encoded.keyframe,
            width: encoded.width,
            height: encoded.height,
            timestamp_us: start.elapsed().as_micros() as u64,
            data: encoded.data,
        };
        if !send_interruptibly(&out, msg, &stop) {
            return;
        }
    }
}

/// Blocking sends cannot observe `stop` when the video writer is stalled.
/// Bound this wait too so a quality change/disconnect can join the encoder.
fn send_interruptibly(out: &mpsc::Sender<VideoFrame>, mut frame: VideoFrame, stop: &AtomicBool) -> bool {
    while !stop.load(Ordering::Relaxed) {
        match out.try_send(frame) {
            Ok(()) => return true,
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
            Err(mpsc::error::TrySendError::Full(pending)) => {
                frame = pending;
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow host must keep text at native resolution in the default mode,
    /// and switching back from a speed preset must undo its previous scale.
    #[test]
    fn native_resolution_survives_motion_and_a_preset_change() {
        use slipstream_codec::PixelFormat;
        use slipstream_platform::FrameSlot;
        let (w, h) = (320u32, 180u32);
        let slot = FrameSlot::new();
        let producer = slot.clone();
        let producer_thread = std::thread::spawn(move || {
            let mut i = 0u8;
            while !producer.is_closed() {
                let mut data = producer.buffer((w * h * 4) as usize);
                for p in 0..w * h {
                    data.extend_from_slice(&[(p as u8).wrapping_add(i), i, 128, 255]);
                }
                producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Bgra, data });
                i = i.wrapping_add(1);
                std::thread::sleep(Duration::from_millis(16));
            }
        });
        let flow = Flow::new();
        let (tx, mut rx) = mpsc::channel(4);
        let (etx, _erx) = mpsc::unbounded_channel();
        let pipeline = Pipeline::start_with(
            FrameStream::new(slot.clone()),
            flow.clone(),
            StreamSettings::default(),
            tx,
            etx,
            Some(30.0),
        );
        let start = Instant::now();
        let mut native_frames = 0;
        while start.elapsed() < Duration::from_secs(2) {
            if let Ok(frame) = rx.try_recv() {
                flow.ack(frame.seq);
                assert_eq!(frame.width, w, "a native-resolution preset shrank a moving frame");
                native_frames += 1;
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(native_frames > 5);
        pipeline.set_settings(StreamSettings { adaptive_resolution: true, ..Default::default() });
        let start = Instant::now();
        let mut shrank = false;
        while start.elapsed() < Duration::from_secs(3) {
            if let Ok(frame) = rx.try_recv() {
                flow.ack(frame.seq);
                shrank |= frame.width < w;
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(shrank, "speed preset never reduced resolution on a slow host");
        pipeline.set_settings(StreamSettings { bitrate_kbps: 20_000, ..Default::default() });
        let start = Instant::now();
        let mut restored = false;
        while start.elapsed() < Duration::from_secs(1) {
            if let Ok(frame) = rx.try_recv() {
                flow.ack(frame.seq);
                if restored {
                    assert_eq!(frame.width, w, "Quality fell back to its old reduced scale");
                }
                restored |= frame.width == w;
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(restored, "Quality failed to restore full size during motion");
        drop(pipeline);
        slot.close();
        producer_thread.join().unwrap();
    }

    /// A processor too slow for full size: moving frames shrink until 60 fps
    /// fits, and once the picture stops a full-size frame follows.
    #[test]
    fn slow_encoder_shrinks_then_sharpens() {
        use slipstream_codec::PixelFormat;
        use slipstream_platform::{FrameSlot, FrameStream};
        let (w, h) = (1280u32, 720u32);
        let slot = FrameSlot::new();
        let producer = slot.clone();
        let frames: Vec<Vec<u8>> = (0..8u32)
            .map(|t| {
                (0..w * h).flat_map(|i| [((i % w + t * 7) % 256) as u8, ((i / w) % 256) as u8, 128, 255]).collect()
            })
            .collect();
        let moving = Arc::new(AtomicBool::new(true));
        let motion = moving.clone();
        std::thread::spawn(move || {
            // 3 s of motion, then a still picture (nothing more captured).
            for t in 0..180usize {
                let mut data = producer.buffer((w * h * 4) as usize);
                data.extend_from_slice(&frames[t % frames.len()]);
                producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Bgra, data });
                std::thread::sleep(Duration::from_micros(16_667));
            }
            motion.store(false, Ordering::Relaxed);
        });
        let flow = Flow::new();
        let (tx, mut rx) = mpsc::channel(4);
        let (etx, _erx) = mpsc::unbounded_channel();
        let _p = Pipeline::start_with(
            FrameStream::new(slot),
            flow.clone(),
            StreamSettings { adaptive_resolution: true, ..Default::default() },
            tx,
            etx,
            Some(30.0),
        );
        let start = Instant::now();
        let (mut widths, mut late) = (Vec::new(), 0);
        // Slow machines (CI's Macs) take longer than 3 s to play the motion:
        // wait for it to end, then a second and a half for the sharp frame.
        let mut still_since: Option<Instant> = None;
        while still_since.is_none_or(|t| t.elapsed() < Duration::from_millis(1500))
            && start.elapsed() < Duration::from_secs(30)
        {
            if still_since.is_none() && !moving.load(Ordering::Relaxed) {
                still_since = Some(Instant::now());
            }
            // Nothing comes once the picture is still: poll, don't block.
            let Ok(f) = rx.try_recv() else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            {
                flow.ack(f.seq);
                if start.elapsed() > Duration::from_millis(2500) && start.elapsed() < Duration::from_secs(3) {
                    late += 1;
                }
                widths.push(f.width);
            }
        }
        assert!(widths.iter().any(|&fw| fw < w), "moving frames never shrank: {widths:?}");
        // Unoptimised builds are too slow to hold any frame rate.
        if !cfg!(debug_assertions) {
            assert!(late >= 20, "only {late} frames in half a second once it had adapted");
        }
        assert_eq!(widths.last(), Some(&w), "the still picture didn't get a full-size frame: {widths:?}");
    }

    #[test]
    fn a_full_video_queue_can_be_stopped() {
        let (tx, _rx) = mpsc::channel(1);
        let frame = VideoFrame { seq: 1, keyframe: true, width: 2, height: 2, timestamp_us: 0, data: vec![0] };
        tx.try_send(frame.clone()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(send_interruptibly(&tx, frame, &flag)).unwrap();
        });
        assert!(finished.recv_timeout(Duration::from_millis(50)).is_err(), "full queue should apply backpressure");
        stop.store(true, Ordering::Relaxed);
        assert!(!finished.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn flow_limits_in_flight() {
        let flow = Flow::new();
        let iv = Duration::from_millis(16);
        assert!(flow.wait_for_credit(2, iv, Duration::ZERO));
        flow.take_seq(1000, true);
        flow.take_seq(1000, false);
        assert!(!flow.wait_for_credit(2, iv, Duration::from_millis(5)));
        flow.ack(u64::MAX);
        flow.ack(3);
        assert!(
            !flow.wait_for_credit(2, iv, Duration::ZERO),
            "future acknowledgements must not grant credit or underflow"
        );
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
            println!(
                "{w}x{h}: {:.1} fps through the pipeline, {:.0} KB/frame",
                n as f64 / 3.0,
                bytes as f64 / n.max(1) as f64 / 1024.0
            );
        }
    }

    #[test]
    fn rate_control_backs_off_and_recovers() {
        let mut rc = RateControl::new(8000, Some(Duration::from_millis(80)), None);
        assert_eq!(rc.current_kbps, START_KBPS, "starts low enough for a slow link's first keyframe");
        assert_eq!(RateControl::new(8000, Some(Duration::from_millis(2)), None).current_kbps, 8000, "LAN: full rate");
        assert_eq!(RateControl::new(8000, None, Some(900)).current_kbps, 900, "a new pipeline resumes the link's rate");
        rc.last_change -= Duration::from_secs(3);
        let base = Some(Duration::from_millis(20));
        assert_eq!(rc.update(Some(Duration::from_millis(200)), base, None), Some(START_KBPS * 3 / 4));
        assert!(rc.backed_off);
        rc.last_change -= Duration::from_secs(3);
        let up = rc.update(Some(Duration::from_millis(25)), base, None).unwrap();
        assert!(up > START_KBPS * 3 / 4);
        // Climbs to the target on a fast link, and frame skipping ends there.
        for _ in 0..20 {
            rc.last_change -= Duration::from_secs(3);
            rc.update(Some(Duration::from_millis(25)), base, None);
        }
        assert_eq!(rc.current_kbps, 8000);
        assert!(!rc.backed_off);
    }

    /// Screen-content mode while the link holds the rate well down; back to
    /// camera mode only after a while near the target.
    #[test]
    fn screen_content_mode_follows_the_link() {
        let lan = RateControl::new(8000, Some(Duration::from_millis(2)), None);
        assert!(!lan.screen_content(false), "a local network starts in camera mode");
        let mut rc = RateControl::new(8000, Some(Duration::from_millis(80)), None);
        assert!(rc.screen_content(false), "a link of unknown speed starts low, in screen-content mode");
        rc.current_kbps = 4000;
        rc.note_level();
        assert!(rc.screen_content(true), "stays until the link carries near the target");
        assert!(!rc.screen_content(false), "but doesn't switch to it between the marks");
        rc.current_kbps = 7500;
        rc.note_level();
        assert!(rc.screen_content(true));
        rc.full_since = Some(Instant::now() - Duration::from_secs(6));
        assert!(!rc.screen_content(true), "back to camera mode after 5 s near the target");
    }

    /// A slow link: back off to what gets through, below the old 500 kbit/s
    /// floor, and react on a long link before seconds of lag build up.
    #[test]
    fn rate_control_follows_a_slow_long_link() {
        let mut rc = RateControl::new(4000, None, Some(4000));
        rc.last_change -= Duration::from_secs(3);
        let base = Some(Duration::from_millis(300));
        // 100 ms of queueing over a 300 ms link is enough to react.
        assert_eq!(rc.update(Some(Duration::from_millis(400)), base, Some(400)), Some(340));
        for _ in 0..10 {
            rc.last_change -= Duration::from_secs(3);
            rc.update(Some(Duration::from_millis(900)), base, Some(180));
        }
        assert_eq!(rc.current_kbps, FLOOR_KBPS);
        // Queueing below the margin isn't congestion.
        rc.last_change -= Duration::from_secs(3);
        assert!(rc.update(Some(Duration::from_millis(350)), base, Some(180)).unwrap() > FLOOR_KBPS);
    }

    /// The base round trip comes from the connection, not a keyframe that
    /// took seconds to send; a long link gets enough frames in flight; and
    /// what's in flight stays within about a round trip of throughput.
    #[test]
    fn flow_base_ignores_keyframes_and_scales_the_window() {
        let flow = Flow::with_path_rtt(|| Some(Duration::from_millis(250)));
        let iv = Duration::from_micros(16_667);
        let seq = flow.take_seq(200_000, true);
        std::thread::sleep(Duration::from_millis(30));
        flow.ack(seq);
        let (srtt, base) = flow.rtts();
        assert_eq!(srtt, None, "a keyframe's send time isn't a round-trip sample");
        assert_eq!(base, Some(Duration::from_millis(250)));
        // Two frames go out before anything is known about throughput.
        for _ in 0..2 {
            assert!(flow.wait_for_credit(2, iv, Duration::ZERO));
            flow.take_seq(1000, false);
        }
        assert!(!flow.wait_for_credit(2, iv, Duration::ZERO), "no throughput known yet");
        // 50 KB/s arriving: room for 50 KB/s x (1.25 x 250 ms + 100 ms) ≈ 20 KB.
        {
            let mut s = flow.state.lock().unwrap();
            let now = Instant::now();
            s.first_delivery = Some(now - Duration::from_secs(3));
            s.delivered = (0..10).map(|i| (now - Duration::from_millis(i * 190), 10_000)).collect();
        }
        assert_eq!(flow.delivered_kbps(), Some(400));
        let mut small = 2;
        while flow.wait_for_credit(2, iv, Duration::ZERO) {
            flow.take_seq(1000, false);
            small += 1;
        }
        assert_eq!(small, 16, "250 ms at 60 fps: 16 small frames in flight");
        let flow = Flow::with_path_rtt(|| Some(Duration::from_millis(250)));
        flow.rtts();
        {
            let mut s = flow.state.lock().unwrap();
            let now = Instant::now();
            s.first_delivery = Some(now - Duration::from_secs(3));
            s.delivered = (0..10).map(|i| (now - Duration::from_millis(i * 190), 10_000)).collect();
        }
        let mut big = 0;
        while flow.wait_for_credit(2, iv, Duration::ZERO) {
            flow.take_seq(6000, false);
            big += 1;
        }
        assert_eq!(big, 4, "big frames stop at the byte budget, not the frame count");
    }
}
