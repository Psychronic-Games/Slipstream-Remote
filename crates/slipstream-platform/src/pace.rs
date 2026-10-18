//! Pace logging for the video pipeline.

/// Logs a capture or encoding stage's pace now and then, so a slow stage
/// shows in the log (the service's is readable from Config).
pub struct Pace {
    what: &'static str,
    since: std::time::Instant,
    frames: u32,
    busy: std::time::Duration,
}

impl Pace {
    pub fn new(what: &'static str) -> Self {
        Self { what, since: std::time::Instant::now(), frames: 0, busy: std::time::Duration::ZERO }
    }

    /// One frame that took `took`.
    pub fn add(&mut self, took: std::time::Duration) {
        self.frames += 1;
        self.busy += took;
        let span = self.since.elapsed();
        if span >= std::time::Duration::from_secs(30) {
            tracing::info!(
                "{}: {:.1} frames a second, {:.1} ms each",
                self.what,
                self.frames as f64 / span.as_secs_f64(),
                self.busy.as_secs_f64() * 1000.0 / self.frames.max(1) as f64
            );
            *self = Self::new(self.what);
        }
    }
}
