//! Bounded Android capture handoff. Kept separate from JNI for regression tests.
use crate::{FrameSlot, FrameStream, PixelFormat, RawFrame};
use anyhow::{Result, bail};
use std::sync::{Arc, Mutex, Weak};

#[derive(Default)]
struct State {
    active: bool,
    width: u32,
    height: u32,
    latest: Option<RawFrame>,
    viewers: Vec<Weak<FrameSlot>>,
}

#[derive(Default)]
pub(crate) struct CaptureHub {
    state: Mutex<State>,
}

impl CaptureHub {
    pub fn start(&self, width: u32, height: u32) {
        self.stop("Phone started a new screen-sharing session");
        let mut s = self.state.lock().unwrap();
        s.active = true;
        s.width = width;
        s.height = height;
    }
    pub fn stop(&self, reason: &str) {
        let mut s = self.state.lock().unwrap();
        s.active = false;
        s.latest = None;
        for viewer in s.viewers.drain(..).filter_map(|v| v.upgrade()) {
            viewer.fail(reason);
        }
    }
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let s = self.state.lock().unwrap();
        s.active.then_some((s.width, s.height))
    }
    pub fn has_viewers(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .viewers
            .iter()
            .filter_map(Weak::upgrade)
            .any(|v| !v.is_closed())
    }
    pub fn subscribe(&self) -> Result<FrameStream> {
        let mut s = self.state.lock().unwrap();
        if !s.active {
            bail!("On the phone, tap Start sharing and approve Android's screen-sharing prompt");
        }
        s.viewers
            .retain(|v| v.upgrade().is_some_and(|s| !s.is_closed()));
        if s.viewers.len() >= 4 {
            bail!("Phone already has four screen viewers");
        }
        tracing::info!(
            cached = s.latest.is_some(),
            width = s.width,
            height = s.height,
            "Android screen subscription"
        );
        let slot = FrameSlot::new();
        if let Some(f) = &s.latest {
            slot.put(f.clone());
        }
        s.viewers.push(Arc::downgrade(&slot));
        Ok(FrameStream::new(slot))
    }
    pub fn publish(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
        row_stride: usize,
        pixel_stride: usize,
    ) -> Result<()> {
        if width < 2
            || height < 2
            || width > 2048
            || height > 2048
            || u64::from(width) * u64::from(height) > 2_097_152
            || !(4..=16).contains(&pixel_stride)
        {
            bail!("Invalid Android frame dimensions");
        }
        let row_bytes = (width as usize - 1) * pixel_stride + 4;
        if row_stride < row_bytes
            || data.len()
                < row_stride
                    .saturating_mul(height as usize - 1)
                    .saturating_add(row_bytes)
        {
            bail!("Android image plane is shorter than its strides");
        }
        let mut s = self.state.lock().unwrap();
        if !s.active {
            return Ok(());
        }
        if s.latest.is_none() {
            tracing::info!(width, height, "Android first capture frame");
        }
        s.width = width;
        s.height = height;
        let mut frame = s.latest.take().unwrap_or(RawFrame {
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba,
            data: Vec::new(),
        });
        frame.width = width;
        frame.height = height;
        frame.stride = width * 4;
        frame.data.clear();
        frame.data.reserve(width as usize * height as usize * 4);
        for row in 0..height as usize {
            let source = &data[row * row_stride..row * row_stride + row_bytes];
            if pixel_stride == 4 {
                frame.data.extend_from_slice(source);
            } else {
                for x in 0..width as usize {
                    frame
                        .data
                        .extend_from_slice(&source[x * pixel_stride..x * pixel_stride + 4]);
                }
            }
        }
        s.viewers
            .retain(|v| v.upgrade().is_some_and(|v| !v.is_closed()));
        for slot in s.viewers.iter().filter_map(Weak::upgrade) {
            let mut data = slot.buffer(frame.data.len());
            data.extend_from_slice(&frame.data);
            slot.put(RawFrame {
                data,
                ..frame.clone_without_data()
            });
        }
        s.latest = Some(frame);
        Ok(())
    }
}

// Avoid cloning a phone-sized buffer only to overwrite it.
trait FrameMetadata {
    fn clone_without_data(&self) -> RawFrame;
}
impl FrameMetadata for RawFrame {
    fn clone_without_data(&self) -> RawFrame {
        RawFrame {
            width: self.width,
            height: self.height,
            stride: self.stride,
            format: self.format,
            data: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn padded_planes_late_viewers_rotation_and_revocation() {
        let hub = CaptureHub::default();
        assert!(hub.subscribe().is_err());
        hub.start(2, 2);
        let bytes = [
            1, 2, 3, 255, 4, 5, 6, 255, 99, 99, 99, 99, 7, 8, 9, 255, 10, 11, 12, 255,
        ];
        hub.publish(&bytes, 2, 2, 12, 4).unwrap();
        let stream = hub.subscribe().unwrap();
        assert!(hub.has_viewers());
        let frame = stream.next(Duration::ZERO).unwrap().unwrap();
        assert_eq!(
            frame.data,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        assert!(frame.validate().is_ok());
        hub.publish(&[20; 24], 3, 2, 12, 4).unwrap();
        assert_eq!(stream.next(Duration::ZERO).unwrap().unwrap().width, 3);
        assert_eq!(hub.dimensions(), Some((3, 2)));
        assert!(hub.publish(&[0; 4], 2, 2, 8, 4).is_err());
        assert!(hub.publish(&[], u32::MAX, 2, 8, 4).is_err());
        hub.stop("Revoked by phone user");
        assert!(hub.dimensions().is_none());
        assert!(stream.next(Duration::ZERO).unwrap_err().contains("Revoked"));
        assert!(!hub.has_viewers());
        hub.start(2, 2);
        let stream = hub.subscribe().unwrap();
        assert!(
            stream.next(Duration::ZERO).unwrap().is_none(),
            "Old capture must not leak into a new permission grant"
        );
    }
    #[test]
    fn viewer_limit_and_cleanup() {
        let hub = CaptureHub::default();
        hub.start(2, 2);
        let mut viewers: Vec<_> = (0..4).map(|_| hub.subscribe().unwrap()).collect();
        assert!(hub.subscribe().is_err());
        viewers.pop();
        assert!(hub.subscribe().is_ok());
        hub.stop("Stopped");
    }
}
