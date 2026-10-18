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
fn draw_pointer(_: &mut [u8], _: u32, _: u32, _: u32, _: (i32, i32), _: (), _: bool) {}

pub struct GenericBackend {
    displays: Vec<DisplayInfo>,
    input: mpsc::Sender<InputCmd>,
    #[cfg(target_os = "windows")]
    curtain: Option<crate::privacy::Curtain>,
}

/// Work for the input thread.
enum InputCmd {
    Event(DisplayInfo, InputEvent),
    /// Block (or unblock) the local keyboard and mouse: privacy mode.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    BlockLocal(bool),
}

impl GenericBackend {
    pub fn open() -> Result<Self> {
        let displays = list_displays()?;
        let (tx, rx) = mpsc::channel();
        thread::Builder::new().name("input".into()).spawn(move || input_worker(rx))?;
        Ok(Self {
            displays,
            input: tx,
            #[cfg(target_os = "windows")]
            curtain: None,
        })
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

    #[cfg(target_os = "windows")]
    fn privacy_supported(&self) -> bool {
        crate::privacy::supported()
    }

    #[cfg(target_os = "windows")]
    fn set_privacy(&mut self, on: bool, who: &str) -> Result<()> {
        if on {
            if self.curtain.is_none() {
                self.curtain = Some(crate::privacy::Curtain::show(who)?);
            }
        } else {
            self.curtain = None;
        }
        let _ = self.input.send(InputCmd::BlockLocal(on));
        Ok(())
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
        #[cfg(target_os = "windows")]
        let size = (d.width, d.height);
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
            // Service mode follows the input desktop (login screen, UAC).
            #[cfg(target_os = "windows")]
            if crate::windesk::following() {
                follow_desktops(&producer, &monitor, min_interval, origin, size);
                return;
            }
            match monitor.video_recorder() {
                Ok((recorder, frames)) => match recorder.start() {
                    Ok(()) => {
                        if record(&producer, &frames, min_interval, origin) {
                            producer.fail("capture ended");
                        }
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
        let d = self.displays.get(display as usize).cloned().with_context(|| format!("display {display} not found"))?;
        self.input.send(InputCmd::Event(d, event)).map_err(|_| anyhow!("input worker stopped"))
    }
}

/// Streams the recorder's frames. It only delivers frames when the picture
/// changes, and they carry no pointer. So the latest clean frame is kept,
/// and pointer moves on a still screen (and changes that arrive while
/// throttled) still go out. Returns true if the recorder stopped (on
/// Windows, a desktop switch ends DXGI duplication), false once the
/// session closed the stream.
#[cfg(not(target_os = "linux"))]
fn record(producer: &FrameSlot, frames: &mpsc::Receiver<xcap::Frame>, min_interval: Duration, origin: (i32, i32)) -> bool {
    let poll = (min_interval / 2).max(Duration::from_millis(4));
    let mut last = Instant::now() - min_interval;
    let mut clean = None;
    let mut dirty = false;
    let mut shown = None;
    let mut pace = crate::pace::Pace::new(if cfg!(windows) { "screen capture (DXGI)" } else { "screen capture" });
    while !producer.is_closed() {
        match frames.recv_timeout(poll) {
            Ok(f) => {
                clean = Some(frames.try_iter().last().unwrap_or(f));
                dirty = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return true,
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
            draw_pointer(&mut data, f.width, f.height, f.width * 4, origin, p, false);
        }
        producer.put(RawFrame { width: f.width, height: f.height, stride: f.width * 4, format: PixelFormat::Rgba, data });
        pace.add(last.elapsed());
    }
    false
}

/// Service mode: DXGI duplication (fast, and free while nothing changes)
/// while the normal desktop has input; GDI screenshots on the login screen
/// and UAC's secure desktop, which duplication can't follow. A desktop
/// switch ends duplication, and the loop picks again.
#[cfg(target_os = "windows")]
fn follow_desktops(producer: &FrameSlot, monitor: &Monitor, min_interval: Duration, origin: (i32, i32), size: (u32, u32)) {
    // When duplication isn't available on the normal desktop either (no
    // display driver that supports it), stay on GDI and retry now and then.
    let mut try_dxgi_at = Instant::now();
    while !producer.is_closed() {
        crate::windesk::follow();
        if Instant::now() >= try_dxgi_at && crate::windesk::on_default_desktop() {
            // Duplication only delivers a frame once something changes:
            // show the screen as it is meanwhile.
            gdi_once(producer, origin, size);
            match monitor.video_recorder().map_err(|e| e.to_string()).and_then(|(r, f)| r.start().map(|()| (r, f)).map_err(|e| e.to_string())) {
                Ok((recorder, frames)) => {
                    tracing::info!("screen capture: DXGI duplication");
                    let ended = record(producer, &frames, min_interval, origin);
                    let _ = recorder.stop();
                    if !ended {
                        return;
                    }
                    tracing::info!("screen capture: duplication ended (desktop switch?)");
                    continue;
                }
                Err(e) => {
                    tracing::info!("screen capture: DXGI duplication unavailable ({e}), using GDI");
                    try_dxgi_at = Instant::now() + Duration::from_secs(60);
                }
            }
        }
        if !gdi_until(producer, min_interval, origin, size, || Instant::now() >= try_dxgi_at && crate::windesk::on_default_desktop()) {
            return;
        }
    }
}

/// One GDI frame of the screen as it is now.
#[cfg(target_os = "windows")]
fn gdi_once(producer: &FrameSlot, origin: (i32, i32), (w, h): (u32, u32)) {
    let Some(mut gdi) = crate::win_gdi::GdiCapture::new(origin.0, origin.1, w, h) else { return };
    let mut data = producer.buffer(w as usize * h as usize * 4);
    if gdi.grab(&mut data) {
        if let Some(p) = pointer_now() {
            draw_pointer(&mut data, w, h, w * 4, origin, p, true);
        }
        producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Bgra, data });
    }
}

/// GDI capture following the input desktop, until `switch` says to try
/// something else (true) or the stream closes or fails (false).
#[cfg(target_os = "windows")]
fn gdi_until(producer: &FrameSlot, min_interval: Duration, origin: (i32, i32), size: (u32, u32), switch: impl Fn() -> bool) -> bool {
    let mut gdi: Option<crate::win_gdi::GdiCapture> = None;
    let mut failures = 0;
    let mut pace = crate::pace::Pace::new("screen capture (GDI)");
    let mut checked = Instant::now();
    while !producer.is_closed() {
        let t = Instant::now();
        if crate::windesk::follow() {
            gdi = None; // its screen DC belongs to the old desktop
        }
        if checked.elapsed() >= Duration::from_millis(500) {
            checked = Instant::now();
            if switch() {
                return true;
            }
        }
        if gdi.is_none() {
            gdi = crate::win_gdi::GdiCapture::new(origin.0, origin.1, size.0, size.1);
        }
        let (w, h) = size;
        let mut data = producer.buffer(w as usize * h as usize * 4);
        if gdi.as_mut().is_some_and(|g| g.grab(&mut data)) {
            failures = 0;
            if let Some(p) = pointer_now() {
                draw_pointer(&mut data, w, h, w * 4, origin, p, true);
            }
            producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Bgra, data });
            pace.add(t.elapsed());
        } else {
            // A desktop switch can fail a few captures; keep trying a while.
            gdi = None;
            failures += 1;
            if failures > 50 {
                producer.fail("screen capture failed");
                return false;
            }
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        if let Some(rest) = min_interval.checked_sub(t.elapsed()) {
            thread::sleep(rest);
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn following_desktops() -> bool {
    crate::windesk::following()
}
#[cfg(target_os = "macos")]
fn following_desktops() -> bool {
    false
}

/// Fallback: polls screenshots at the frame rate.
#[cfg(not(target_os = "linux"))]
fn poll_screenshots(producer: &FrameSlot, monitor: &Monitor, min_interval: Duration, origin: (i32, i32)) {
    let mut failures = 0;
    while !producer.is_closed() {
        let t = Instant::now();
        #[cfg(target_os = "windows")]
        crate::windesk::follow();
        match monitor.capture_image() {
            Ok(img) => {
                let (w, h) = img.dimensions();
                let mut data = img.into_raw();
                if let Some(p) = pointer_now() {
                    draw_pointer(&mut data, w, h, w * 4, origin, p, false);
                }
                producer.put(RawFrame { width: w, height: h, stride: w * 4, format: PixelFormat::Rgba, data });
            }
            Err(e) => {
                // A desktop switch (UAC prompt, login screen) can fail a
                // few captures; keep trying for a while in service mode.
                failures += 1;
                if !following_desktops() || failures > 50 {
                    producer.fail(format!("screenshot failed: {e}"));
                    return;
                }
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
        failures = 0;
        if let Some(rest) = min_interval.checked_sub(t.elapsed()) {
            thread::sleep(rest);
        }
    }
}

fn input_worker(rx: mpsc::Receiver<InputCmd>) {
    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("input injection unavailable: {e}");
            return;
        }
    };
    let mut held = HeldInputs::default();
    let mut wheel = (0.0f32, 0.0f32);
    let mut refused = false;
    let mut last_display = None;
    while let Ok(cmd) = rx.recv() {
        let (display, event) = match cmd {
            InputCmd::Event(d, e) => (d, e),
            InputCmd::BlockLocal(on) => {
                // Only an elevated process may (the service's host is SYSTEM);
                // otherwise the local user can still type, but sees the curtain.
                #[cfg(target_os = "windows")]
                {
                    crate::windesk::follow();
                    // SAFETY: plain call; the blocking thread's own SendInput still works.
                    let ok = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::BlockInput(on as i32) } != 0;
                    tracing::info!(on, ok, "local input blocking");
                }
                let _ = on;
                continue;
            }
        };
        // Service mode: inject on whichever desktop has input (login screen,
        // UAC prompt), and turn Ctrl+Alt+Del into the real thing.
        #[cfg(target_os = "windows")]
        {
            crate::windesk::follow();
            if let InputEvent::Key { code, down: true } = &event
                && code == "Delete"
                && crate::windesk::following()
                && ["ControlLeft", "ControlRight"].iter().any(|k| held.key_down(k))
                && ["AltLeft", "AltRight"].iter().any(|k| held.key_down(k))
                && crate::windesk::request_sas()
            {
                continue;
            }
        }
        for ev in held.process(event) {
            match apply(&mut enigo, &display, ev, &mut wheel) {
                // Once per run of refusals, not per mouse move.
                Err(e) if !refused => {
                    refused = true;
                    tracing::warn!(
                        "Windows refused the remote input ({e}): the window in front is probably running as administrator, which only the Slipstream service can control"
                    );
                }
                Err(_) => {}
                Ok(()) => refused = false,
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
