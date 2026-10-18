//! Synthetic backend: an animated test card plus a crosshair that follows
//! injected pointer events. Used for automated tests and demos.

use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::{DisplayInfo, InputEvent};

use crate::{Backend, FrameSlot, FrameStream};

#[derive(Debug, Default, Clone)]
pub struct InputLog {
    pub events: Vec<InputEvent>,
    pub pointer: (f32, f32),
}

pub struct TestPattern {
    width: u32,
    height: u32,
    log: Arc<Mutex<InputLog>>,
}

impl TestPattern {
    pub fn new(width: u32, height: u32) -> Self {
        // A cursor appears after input arrives; an initial (0,0) crosshair
        // looks like a white frame around the streamed picture in previews.
        Self { width, height, log: Arc::new(Mutex::new(InputLog { pointer: (-1.0, -1.0), ..Default::default() })) }
    }

    /// Shared record of every injected event (for tests).
    pub fn input_log(&self) -> Arc<Mutex<InputLog>> {
        self.log.clone()
    }
}

impl Backend for TestPattern {
    fn name(&self) -> &'static str {
        "test-pattern"
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        // SLIPSTREAM_TEST_SCREENS=N pretends to have N screens side by side.
        let n = std::env::var("SLIPSTREAM_TEST_SCREENS").ok().and_then(|v| v.parse().ok()).unwrap_or(1u32).clamp(1, 8);
        (0..n)
            .map(|i| DisplayInfo {
                index: i,
                name: format!("Test Pattern {}", i + 1),
                width: self.width,
                height: self.height,
                x: (i * self.width) as i32,
                y: 0,
                primary: i == 0,
            })
            .collect()
    }

    fn capture(&mut self, _display: u32, max_fps: u32) -> Result<FrameStream> {
        let slot = FrameSlot::new();
        let producer = slot.clone();
        let (w, h) = (self.width, self.height);
        let log = self.log.clone();
        let interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);
        thread::Builder::new().name("test-pattern".into()).spawn(move || {
            let start = Instant::now();
            let mut next = start;
            while !producer.is_closed() {
                let t = start.elapsed().as_secs_f32();
                let pointer = log.lock().unwrap().pointer;
                producer.put(render(w, h, t, pointer));
                next += interval;
                match next.checked_duration_since(Instant::now()) {
                    Some(wait) => thread::sleep(wait),
                    None => next = Instant::now(), // fell behind; don't burst
                }
            }
        })?;
        Ok(FrameStream::new(slot))
    }

    fn inject(&mut self, _display: u32, event: InputEvent) -> Result<()> {
        let mut log = self.log.lock().unwrap();
        if let InputEvent::MouseMove { x, y } = event {
            log.pointer = (x, y);
        }
        log.events.push(event);
        Ok(())
    }
}

fn render(w: u32, h: u32, t: f32, pointer: (f32, f32)) -> RawFrame {
    let stride = w * 4;
    let mut data = vec![0u8; (stride * h) as usize];
    let sweep = ((t * 0.25).fract() * w as f32) as u32;
    let (px, py) = ((pointer.0 * w as f32) as i64, (pointer.1 * h as f32) as i64);
    for y in 0..h {
        let row = &mut data[(y * stride) as usize..((y + 1) * stride) as usize];
        for x in 0..w {
            // Dark field, cyan grid, amber sweep bar.
            let grid = x % 64 == 0 || y % 64 == 0;
            let (mut r, mut g, mut b) = if grid { (20u8, 90u8, 110u8) } else { (6, 10, 18) };
            if x.abs_diff(sweep) < 6 {
                (r, g, b) = (255, 153, 0);
            }
            if (x as i64 - px).abs() < 2 || (y as i64 - py).abs() < 2 {
                (r, g, b) = (255, 255, 255);
            }
            let o = (x * 4) as usize;
            row[o] = b;
            row[o + 1] = g;
            row[o + 2] = r;
            row[o + 3] = 255;
        }
    }
    RawFrame { width: w, height: h, stride, format: PixelFormat::Bgra, data }
}
