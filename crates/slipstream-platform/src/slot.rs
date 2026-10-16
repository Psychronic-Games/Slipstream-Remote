//! A single-frame "latest wins" handoff between a capture thread and the
//! encoder. Old frames are overwritten, never queued, so a slow consumer
//! always gets the freshest picture.

use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use slipstream_codec::RawFrame;

#[derive(Default)]
struct State {
    frame: Option<RawFrame>,
    closed: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct FrameSlot {
    state: Mutex<State>,
    cv: Condvar,
}

impl FrameSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Producer: publish a frame, replacing any unconsumed one.
    pub fn put(&self, frame: RawFrame) {
        let mut s = self.state.lock().unwrap();
        s.frame = Some(frame);
        self.cv.notify_one();
    }

    /// Producer: report a fatal capture error and close.
    pub fn fail(&self, error: impl Into<String>) {
        let mut s = self.state.lock().unwrap();
        s.error = Some(error.into());
        s.closed = true;
        self.cv.notify_all();
    }

    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.cv.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    /// Consumer: wait up to `timeout` for a frame. `Err` once closed.
    pub fn take(&self, timeout: Duration) -> Result<Option<RawFrame>, String> {
        let s = self.state.lock().unwrap();
        let (mut s, _) = self
            .cv
            .wait_timeout_while(s, timeout, |s| s.frame.is_none() && !s.closed)
            .unwrap();
        if let Some(f) = s.frame.take() {
            return Ok(Some(f));
        }
        if s.closed {
            return Err(s.error.clone().unwrap_or_else(|| "capture stopped".into()));
        }
        Ok(None)
    }
}

/// Consumer handle for a running capture. Dropping it stops the capture.
pub struct FrameStream {
    slot: Arc<FrameSlot>,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl FrameStream {
    pub fn new(slot: Arc<FrameSlot>) -> Self {
        Self { slot, on_drop: None }
    }

    /// Runs `f` when the stream is dropped (after closing the slot).
    pub fn with_cleanup(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.on_drop = Some(Box::new(f));
        self
    }

    /// Waits up to `timeout` for the next frame. `Err` if capture died.
    pub fn next(&self, timeout: Duration) -> Result<Option<RawFrame>, String> {
        self.slot.take(timeout)
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.slot.close();
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}
