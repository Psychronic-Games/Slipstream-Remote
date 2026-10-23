//! Pace of the host's video pipeline stages (capture, encoding, waiting):
//! logged now and then, so a slow stage shows in the log (the service's is
//! readable from Config), and kept live for the viewer to ask for.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// Each stage's latest figures: (name, frames a second, ms per frame, when).
static LIVE: Mutex<Vec<(&'static str, f32, f32, Instant)>> = Mutex::new(Vec::new());

/// The stages that reported in the last few seconds: (name, frames a
/// second, milliseconds per frame).
pub fn live() -> Vec<(String, f32, f32)> {
    let live = LIVE.lock().unwrap();
    live.iter().filter(|s| s.3.elapsed() < Duration::from_secs(5)).map(|s| (s.0.to_string(), s.1, s.2)).collect()
}

struct Window {
    since: Instant,
    frames: u32,
    busy: Duration,
}

impl Window {
    fn new() -> Self {
        Self { since: Instant::now(), frames: 0, busy: Duration::ZERO }
    }

    fn add(&mut self, took: Duration) {
        self.frames += 1;
        self.busy += took;
    }

    /// (frames a second, ms per frame) once `span` has passed.
    fn take(&mut self, span: Duration) -> Option<(f32, f32)> {
        let elapsed = self.since.elapsed();
        if elapsed < span {
            return None;
        }
        let r = (self.frames as f32 / elapsed.as_secs_f32(), self.busy.as_secs_f32() * 1000.0 / self.frames.max(1) as f32);
        *self = Self::new();
        Some(r)
    }
}

/// One stage's pace.
pub struct Pace {
    what: &'static str,
    log: Window,
    live: Window,
}

impl Pace {
    pub fn new(what: &'static str) -> Self {
        Self { what, log: Window::new(), live: Window::new() }
    }

    /// The stage's name.
    pub fn what(&self) -> &'static str {
        self.what
    }

    /// One frame that took `took` in this stage.
    pub fn add(&mut self, took: Duration) {
        self.log.add(took);
        self.live.add(took);
        if let Some((fps, ms)) = self.live.take(Duration::from_secs(2)) {
            let mut live = LIVE.lock().unwrap();
            live.retain(|s| s.0 != self.what);
            live.push((self.what, fps, ms, Instant::now()));
        }
        if let Some((fps, ms)) = self.log.take(Duration::from_secs(30)) {
            tracing::info!("{}: {fps:.1} frames a second, {ms:.1} ms each", self.what);
        }
    }
}
