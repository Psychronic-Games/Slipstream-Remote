//! The Linux service's backend: the screen as the graphics card shows it
//! ([`kms`](crate::kms)) and input as kernel devices
//! ([`uinput`](crate::uinput)). It follows whatever is on the displays,
//! with no desktop session, permission prompt or display server of its own.

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use slipstream_core::proto::{DisplayInfo, InputEvent};

use crate::{Backend, FrameStream, HeldInputs, kms, uinput};

/// New input devices take the desktop a moment to adopt; events sent
/// before that are lost.
const SETTLE: Duration = Duration::from_millis(500);
/// How long a blanked screen gets to come on after being woken.
const WAKE: Duration = Duration::from_secs(4);

pub struct SystemBackend {
    screens: Arc<Mutex<Vec<kms::Screen>>>,
    input: Option<mpsc::Sender<(u32, InputEvent)>>,
}

impl SystemBackend {
    pub fn open() -> Result<Self> {
        let mut devices = match uinput::Devices::create() {
            Ok(devices) => Some(devices),
            Err(e) => {
                tracing::warn!("remote input unavailable: {e:#}");
                None
            }
        };
        let mut screens = kms::screens()?;
        if screens.is_empty()
            && let Some(devices) = &mut devices
        {
            // Displays off (blanked, or asleep): touch the keyboard.
            thread::sleep(SETTLE);
            let _ = devices.nudge();
            let deadline = Instant::now() + WAKE;
            while screens.is_empty() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(250));
                screens = kms::screens()?;
            }
        }
        for line in kms::describe() {
            tracing::info!("screen: {line}");
        }
        if screens.is_empty() {
            bail!("that computer isn't showing a picture on any display (none is connected or switched on)");
        }
        let screens = Arc::new(Mutex::new(screens));
        let input = match devices {
            Some(devices) => {
                let (tx, rx) = mpsc::channel();
                let shared = screens.clone();
                let created = Instant::now();
                thread::Builder::new().name("uinput".into()).spawn(move || input_worker(devices, rx, shared, created)).context("starting the input thread")?;
                Some(tx)
            }
            None => None,
        };
        Ok(Self { screens, input })
    }

    /// The displays that are on now (the last known ones if the card can't
    /// be asked this moment).
    fn refresh(&self) -> Vec<kms::Screen> {
        let mut known = self.screens.lock().unwrap();
        if let Ok(now) = kms::screens() {
            *known = now;
        }
        known.clone()
    }
}

/// Where a position on one display (0..1) lies on the whole desktop (0..1).
fn on_desktop(displays: &[DisplayInfo], display: u32, x: f32, y: f32) -> (f32, f32) {
    let Some(d) = displays.iter().find(|d| d.index == display).or(displays.first()) else { return (x, y) };
    let left = displays.iter().map(|d| d.x).min().unwrap_or(0);
    let top = displays.iter().map(|d| d.y).min().unwrap_or(0);
    let right = displays.iter().map(|d| d.x + d.width as i32).max().unwrap_or(1);
    let bottom = displays.iter().map(|d| d.y + d.height as i32).max().unwrap_or(1);
    let (width, height) = ((right - left).max(1) as f32, (bottom - top).max(1) as f32);
    (((d.x - left) as f32 + x * d.width as f32) / width, ((d.y - top) as f32 + y * d.height as f32) / height)
}

fn input_worker(mut devices: uinput::Devices, rx: mpsc::Receiver<(u32, InputEvent)>, screens: Arc<Mutex<Vec<kms::Screen>>>, created: Instant) {
    let mut held = HeldInputs::default();
    let mut failed = false;
    let mut apply = |devices: &mut uinput::Devices, display: u32, event: InputEvent| {
        let displays: Vec<DisplayInfo> = screens.lock().unwrap().iter().map(|s| s.info.clone()).collect();
        match devices.inject(event, |x, y| on_desktop(&displays, display, x, y)) {
            Err(e) if !failed => {
                failed = true;
                tracing::warn!("remote input: {e:#}");
            }
            Err(_) => {}
            Ok(()) => failed = false,
        }
    };
    while let Ok((display, event)) = rx.recv() {
        if let Some(wait) = SETTLE.checked_sub(created.elapsed()) {
            thread::sleep(wait);
        }
        for event in held.process(event) {
            apply(&mut devices, display, event);
        }
    }
    // Session over: never leave keys or buttons stuck down.
    for event in held.release_all() {
        apply(&mut devices, 0, event);
    }
}

impl Backend for SystemBackend {
    fn name(&self) -> &'static str {
        "kms"
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.refresh().into_iter().map(|s| s.info).collect()
    }

    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream> {
        let screen = self.refresh().into_iter().find(|s| s.info.index == display).context("no such display")?;
        kms::capture(screen, max_fps)
    }

    fn inject(&mut self, display: u32, event: InputEvent) -> Result<()> {
        let input = self.input.as_ref().context("remote input isn't available on that computer")?;
        input.send((display, event)).ok().context("the input thread stopped")
    }

    fn input_supported(&self) -> bool {
        self.input.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(index: u32, x: i32, width: u32, height: u32) -> DisplayInfo {
        DisplayInfo { index, name: String::new(), width, height, x, y: 0, primary: index == 0 }
    }

    #[test]
    fn pointer_positions_span_the_whole_desktop() {
        let one = [display(0, 0, 1920, 1080)];
        assert_eq!(on_desktop(&one, 0, 0.25, 0.5), (0.25, 0.5));
        // A second, smaller display to the right.
        let two = [display(0, 0, 1920, 1080), display(1, 1920, 1280, 720)];
        let (x, y) = on_desktop(&two, 1, 0.5, 1.0);
        assert!((x - 2560.0 / 3200.0).abs() < 1e-6);
        assert!((y - 720.0 / 1080.0).abs() < 1e-6);
    }
}
