//! A single-frame "latest wins" handoff between a capture thread and the
//! encoder. Old frames are overwritten, never queued, so a slow consumer
//! always gets the freshest picture.
//!
//! Frame buffers are recycled: overwritten frames and frames the encoder is
//! done with go back to a small pool that the producer copies the next
//! frame into. A fresh 4K buffer per frame would cost 33 MB of page faults
//! every time.

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
    spare: Vec<Vec<u8>>,
}

/// Spare buffers kept for reuse.
const POOL: usize = 3;

impl State {
    fn give_back(&mut self, buf: Vec<u8>) {
        if self.spare.len() < POOL && buf.capacity() > 0 {
            self.spare.push(buf);
        }
    }
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
        if let Some(old) = s.frame.replace(frame) {
            s.give_back(old.data);
        }
        self.cv.notify_one();
    }

    /// Producer: an empty buffer to copy the next frame into, reused when
    /// possible (keeps its capacity, so filling it doesn't fault in pages).
    pub fn buffer(&self, len: usize) -> Vec<u8> {
        let mut s = self.state.lock().unwrap();
        let mut buf = match s.spare.iter().position(|b| b.capacity() >= len) {
            Some(i) => s.spare.swap_remove(i),
            None => Vec::with_capacity(len),
        };
        buf.clear();
        buf
    }

    /// Consumer: hand a finished frame's buffer back for reuse.
    pub fn recycle(&self, frame: RawFrame) {
        self.reuse(frame.data);
    }

    /// Returns a buffer from [`buffer`](Self::buffer) that wasn't sent.
    pub fn reuse(&self, buf: Vec<u8>) {
        self.state.lock().unwrap().give_back(buf);
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

    /// Returns a frame that's no longer needed, so capture can reuse it.
    pub fn recycle(&self, frame: RawFrame) {
        self.slot.recycle(frame);
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

#[cfg(test)]
mod tests {
    use super::*;
    use slipstream_codec::PixelFormat;

    fn frame(data: Vec<u8>) -> RawFrame {
        RawFrame { width: 1, height: 1, stride: 4, format: PixelFormat::Bgra, data }
    }

    #[test]
    fn buffers_are_reused() {
        let slot = FrameSlot::new();
        let mut a = slot.buffer(1 << 20);
        a.resize(1 << 20, 7);
        let ptr = a.as_ptr();
        slot.put(frame(a));
        // Overwritten before the consumer took it: its buffer comes back.
        slot.put(frame(slot.buffer(16)));
        let b = slot.buffer(1 << 20);
        assert_eq!(b.as_ptr(), ptr);
        assert!(b.is_empty());
        // Consumed frames come back through recycle().
        let taken = slot.take(Duration::ZERO).unwrap().unwrap();
        slot.recycle(taken);
        assert!(slot.buffer(8).capacity() >= 8);
    }
}
