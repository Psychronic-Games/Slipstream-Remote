//! Cross-platform backend: screen capture via `xcap` (DXGI/WGC on Windows,
//! ScreenCaptureKit on macOS) or our pure-Rust X11 capture on Linux, and
//! `enigo` for input.

use std::{sync::mpsc, thread};
#[cfg(not(target_os = "linux"))]
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
#[cfg(not(target_os = "linux"))]
use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::{DisplayInfo, InputEvent, MouseButton};
#[cfg(not(target_os = "linux"))]
use xcap::Monitor;

#[cfg(not(target_os = "linux"))]
use crate::FrameSlot;
use crate::{Backend, FrameStream, HeldInputs, keymap};
#[cfg(target_os = "windows")]
use crate::win_cursor::{current as pointer_now, draw as draw_pointer};

// macOS capture already shows the pointer.
#[cfg(target_os = "macos")]
fn pointer_now() -> Option<()> {
    None
}
#[cfg(target_os = "macos")]
fn draw_pointer(_: &mut [u8], _: u32, _: u32, _: u32, _: (i32, i32), _: ()) {}

pub struct GenericBackend {
    displays: Vec<DisplayInfo>,
    input: mpsc::Sender<(DisplayInfo, InputEvent)>,
}

impl GenericBackend {
    pub fn open() -> Result<Self> {
        let displays = list_displays()?;
        let (tx, rx) = mpsc::channel();
        thread::Builder::new().name("input".into()).spawn(move || input_worker(rx))?;
        Ok(Self { displays, input: tx })
    }
}

#[cfg(target_os = "linux")]
fn list_displays() -> Result<Vec<DisplayInfo>> {
    crate::x11::list_displays()
}

#[cfg(not(target_os = "linux"))]
fn list_displays() -> Result<Vec<DisplayInfo>> {
    let monitors = Monitor::all().context("listing monitors")?;
    let mut out: Vec<DisplayInfo> = monitors
        .iter()
        .enumerate()
        .map(|(i, m)| DisplayInfo {
            index: i as u32,
            name: m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| format!("Display {}", i + 1)),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            x: m.x().unwrap_or(0),
            y: m.y().unwrap_or(0),
            primary: m.is_primary().unwrap_or(i == 0),
        })
        .collect();
    if out.is_empty() {
        return Err(anyhow!("no displays found"));
    }
    if !out.iter().any(|d| d.primary) {
        out[0].primary = true;
    }
    Ok(out)
}

impl Backend for GenericBackend {
    fn name(&self) -> &'static str {
        if cfg!(target_os = "linux") { "x11" } else { "xcap" }
    }

    fn cursor_in_video(&self) -> bool {
        // Composited by us on X11 and Windows; macOS is unverified.
        cfg!(any(target_os = "linux", target_os = "windows"))
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.displays.clone()
    }

    #[cfg(target_os = "linux")]
    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream> {
        let d = self.displays.get(display as usize).cloned().with_context(|| format!("display {display} not found"))?;
        crate::x11::capture(d, max_fps)
    }

    #[cfg(not(target_os = "linux"))]
    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream> {
        let d = self.displays.get(display as usize).cloned().with_context(|| format!("display {display} not found"))?;
        let origin = (d.x, d.y);
        let index = display as usize;
        Monitor::all()?.into_iter().nth(index).with_context(|| format!("display {display} not found"))?;
        let slot = FrameSlot::new();
        let producer = slot.clone();
        let min_interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);
        // Monitor handles and the recorder (an AVCaptureSession on macOS)
        // aren't Send: create them on the capture thread.
        thread::Builder::new().name("capture".into()).spawn(move || {
            let Some(monitor) = Monitor::all().ok().and_then(|m| m.into_iter().nth(index)) else {
                producer.fail("display disappeared");
                return;
            };
            match monitor.video_recorder() {
                Ok((recorder, frames)) => match recorder.start() {
                    Ok(()) => {
                        record(&producer, &frames, min_interval, origin);
                        let _ = recorder.stop();
                    }
                    Err(e) => {
                        tracing::warn!("video recorder didn't start ({e}), polling screenshots");
                        poll_screenshots(&producer, &monitor, min_interval, origin);
                    }
                },
                Err(e) => {
                    tracing::warn!("video recorder unavailable ({e}), polling screenshots");
                    poll_screenshots(&producer, &monitor, min_interval, origin);
                }
            }
        })?;
        Ok(FrameStream::new(slot))
    }

    fn inject(&mut self, display: u32, event: InputEvent) -> Result<()> {
        let d = self
            .displays
            .get(display as usize)
            .cloned()
            .with_context(|| format!("display {display} not found"))?;
        self.input.send((d, event)).map_err(|_| anyhow!("input worker stopped"))
    }
}

/// Streams the recorder's frames. It only delivers frames when the picture
/// changes, and they carry no pointer. So the latest clean frame is kept,
/// and pointer moves on a still screen (and changes that arrive while
/// throttled) still go out.
#[cfg(not(target_os = "linux"))]
fn record(producer: &FrameSlot, frames: &mpsc::Receiver<xcap::Frame>, min_interval: Duration, origin: (i32, i32)) {
    let poll = (min_interval / 2).max(Duration::from_millis(4));
    let mut last = Instant::now() - min_interval;
    let mut clean = None;
    let mut dirty = false;
    let mut shown = None;
    while !producer.is_closed() {
        match frames.recv_timeout(poll) {
            Ok(f) => {
                clean = Some(frames.try_iter().last().unwrap_or(f));
                dirty = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                producer.fail("capture ended");
                return;
            }
        }
        let Some(f) = &clean else { continue };
        let pointer = pointer_now();
        if !(dirty || pointer != shown) || last.elapsed() < min_interval.mul_f32(0.8) {
            continue;
        }
        last = Instant::now();
        dirty = false;
        shown = pointer;
        let mut data = producer.buffer(f.raw.len());
        data.extend_from_slice(&f.raw);
        if let Some(p) = pointer {
            draw_pointer(&mut data, f.width, f.height, f.width * 4, origin, p);
        }
        producer.put(RawFrame { width: f.width, height: f.height, stride: f.width * 4, format: PixelFormat::Rgba, data });
    }
}

/// Fallback: polls screenshots at the frame rate.
#[cfg(not(target_os = "linux"))]
fn poll_screenshots(producer: &FrameSlot, monitor: &Monitor, min_interval: Duration, origin: (i32, i32)) {
    while !producer.is_closed() {
        let t = Instant::now();
        match monitor.capture_image() {
            Ok(img) => {
                let (w, h) = img.dimensions();
                let mut data = img.into_raw();
                if let Some(p) = pointer_now() {
                    draw_pointer(&mut data, w, h, w * 4, origin, p);
                }
                producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Rgba, data });
            }
            Err(e) => {
                producer.fail(format!("screenshot failed: {e}"));
                return;
            }
        }
        if let Some(rest) = min_interval.checked_sub(t.elapsed()) {
            thread::sleep(rest);
        }
    }
}

fn input_worker(rx: mpsc::Receiver<(DisplayInfo, InputEvent)>) {
    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("input injection unavailable: {e}");
            return;
        }
    };
    let mut held = HeldInputs::default();
    let mut wheel = (0.0f32, 0.0f32);
    let mut last_display = None;
    while let Ok((display, event)) = rx.recv() {
        for ev in held.process(event) {
            if let Err(e) = apply(&mut enigo, &display, ev, &mut wheel) {
                tracing::debug!("input event failed: {e}");
            }
        }
        last_display = Some(display);
    }
    // Session over: never leave keys or buttons stuck down.
    if let Some(display) = last_display {
        for ev in held.release_all() {
            let _ = apply(&mut enigo, &display, ev, &mut wheel);
        }
    }
}

fn apply(enigo: &mut Enigo, d: &DisplayInfo, ev: InputEvent, wheel: &mut (f32, f32)) -> Result<()> {
    match ev {
        InputEvent::MouseMove { x, y } => {
            let px = d.x + (x.clamp(0.0, 1.0) * d.width.saturating_sub(1) as f32).round() as i32;
            let py = d.y + (y.clamp(0.0, 1.0) * d.height.saturating_sub(1) as f32).round() as i32;
            enigo.move_mouse(px, py, Coordinate::Abs)?;
        }
        InputEvent::MouseButton { button, down } => {
            let b = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
                MouseButton::Back => Button::Back,
                MouseButton::Forward => Button::Forward,
            };
            enigo.button(b, if down { Direction::Press } else { Direction::Release })?;
        }
        InputEvent::Wheel { dx, dy } => {
            // Accumulate fractional (touchpad) scrolling into whole notches.
            wheel.0 += dx;
            wheel.1 += dy;
            let (sx, sy) = (wheel.0.trunc() as i32, wheel.1.trunc() as i32);
            wheel.0 -= sx as f32;
            wheel.1 -= sy as f32;
            // Protocol: positive = up/left; enigo: positive = down/right.
            if sy != 0 {
                enigo.scroll(-sy, Axis::Vertical)?;
            }
            if sx != 0 {
                enigo.scroll(-sx, Axis::Horizontal)?;
            }
        }
        InputEvent::Key { code, down } => {
            let Some(k) = keymap::lookup(&code) else { return Ok(()) };
            #[cfg(target_os = "windows")]
            let (raw, valid) = (k.win, k.win != 0);
            #[cfg(target_os = "macos")]
            let (raw, valid) = (k.mac, k.mac != 0xffff);
            #[cfg(target_os = "linux")]
            let (raw, valid) = (k.xkb, k.xkb != 0);
            if valid {
                enigo.raw(raw, if down { Direction::Press } else { Direction::Release })?;
            }
        }
        InputEvent::ReleaseAll => {}
    }
    Ok(())
}
