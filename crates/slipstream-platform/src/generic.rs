//! Cross-platform backend: `xcap` for capture (DXGI/WGC on Windows,
//! ScreenCaptureKit on macOS, XShm on X11) and `enigo` for input.

use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::{DisplayInfo, InputEvent, MouseButton};
use xcap::Monitor;

use crate::{Backend, FrameSlot, FrameStream, HeldInputs, keymap};

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
        "xcap"
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.displays.clone()
    }

    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream> {
        let monitors = Monitor::all()?;
        let monitor = monitors
            .into_iter()
            .nth(display as usize)
            .with_context(|| format!("display {display} not found"))?;
        let slot = FrameSlot::new();
        let producer = slot.clone();
        let min_interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);

        match monitor.video_recorder() {
            Ok((recorder, frames)) => {
                recorder.start().map_err(|e| anyhow!("starting capture: {e}"))?;
                thread::Builder::new().name("capture".into()).spawn(move || {
                    let mut last = Instant::now() - min_interval;
                    while !producer.is_closed() {
                        match frames.recv_timeout(Duration::from_millis(200)) {
                            Ok(f) => {
                                if last.elapsed() < min_interval.mul_f32(0.8) {
                                    continue;
                                }
                                last = Instant::now();
                                producer.put(RawFrame {
                                    width: f.width,
                                    height: f.height,
                                    stride: f.width * 4,
                                    format: PixelFormat::Rgba,
                                    data: f.raw,
                                });
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                producer.fail("capture ended");
                                break;
                            }
                        }
                    }
                    let _ = recorder.stop();
                })?;
            }
            Err(e) => {
                // Fall back to polling screenshots.
                tracing::warn!("video recorder unavailable ({e}), polling screenshots");
                // Monitor handles aren't Send on every OS: look it up in the thread.
                let index = display as usize;
                thread::Builder::new().name("capture".into()).spawn(move || {
                    let Some(monitor) = Monitor::all().ok().and_then(|m| m.into_iter().nth(index)) else {
                        producer.fail("display disappeared");
                        return;
                    };
                    while !producer.is_closed() {
                        let t = Instant::now();
                        match monitor.capture_image() {
                            Ok(img) => {
                                let (w, h) = img.dimensions();
                                producer.put(RawFrame {
                                    width: w,
                                    height: h,
                                    stride: w * 4,
                                    format: PixelFormat::Rgba,
                                    data: img.into_raw(),
                                });
                            }
                            Err(e) => {
                                producer.fail(format!("screenshot failed: {e}"));
                                break;
                            }
                        }
                        if let Some(rest) = min_interval.checked_sub(t.elapsed()) {
                            thread::sleep(rest);
                        }
                    }
                })?;
            }
        }
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
