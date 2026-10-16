//! Platform backends: screen capture, input injection and clipboard.
//!
//! [`open`] picks the best backend for the current system:
//!
//! | System | Backend |
//! |---|---|
//! | Linux, Wayland session | xdg-desktop-portal RemoteDesktop + PipeWire ([`wayland`]) |
//! | Linux X11, Windows, macOS | xcap capture + enigo input ([`generic`]) |
//! | anything (tests, demos) | synthetic test pattern ([`test_pattern`]) |

pub mod clipboard;
pub mod keymap;
mod slot;
pub mod test_pattern;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub mod generic;
#[cfg(target_os = "linux")]
pub mod wayland;

use std::collections::HashSet;

use anyhow::Result;
pub use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::{DisplayInfo, InputEvent, MouseButton};
pub use slot::{FrameSlot, FrameStream};

/// A live capture + input session on the host machine.
pub trait Backend: Send {
    /// Short name for logs and the UI ("wayland-portal", "xcap", …).
    fn name(&self) -> &'static str;

    /// Displays that can be streamed.
    fn displays(&self) -> Vec<DisplayInfo>;

    /// Starts capturing `display`. Capture stops when the stream is dropped.
    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream>;

    /// Injects one input event. Pointer coordinates are normalized to `display`.
    /// Must not block for long; implementations queue to a worker.
    fn inject(&mut self, display: u32, event: InputEvent) -> Result<()>;

    /// Token that lets the next session skip the permission prompt (Wayland).
    fn restore_token(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Use a synthetic animated test pattern instead of the real screen.
    pub test_pattern: bool,
    /// Wayland portal restore token from a previous session.
    pub restore_token: Option<String>,
}

/// Opens the best backend for this system. On Wayland this shows the
/// desktop's screen-sharing permission dialog (unless a valid restore token
/// is supplied), so it can take as long as the user takes to click.
pub async fn open(opts: &OpenOptions) -> Result<Box<dyn Backend>> {
    if opts.test_pattern {
        return Ok(Box::new(test_pattern::TestPattern::new(1280, 720)));
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Ok(Box::new(wayland::WaylandBackend::open(opts.restore_token.as_deref()).await?));
    }
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        Ok(Box::new(generic::GenericBackend::open()?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        anyhow::bail!("screen capture is not supported on this platform yet")
    }
}

/// Remembers which keys and buttons are held so they can all be released
/// when the viewer loses focus or disconnects (no stuck Ctrl keys).
#[derive(Debug, Default)]
pub struct HeldInputs {
    keys: HashSet<String>,
    buttons: HashSet<MouseButtonKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MouseButtonKey(u8);

fn button_key(b: MouseButton) -> MouseButtonKey {
    MouseButtonKey(b as u8)
}

fn button_from_key(k: MouseButtonKey) -> MouseButton {
    match k.0 {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::Back,
        _ => MouseButton::Forward,
    }
}

impl HeldInputs {
    /// Expands an event into the concrete events to inject, tracking state.
    pub fn process(&mut self, event: InputEvent) -> Vec<InputEvent> {
        match event {
            InputEvent::Key { ref code, down } => {
                if down {
                    self.keys.insert(code.clone());
                } else if !self.keys.remove(code) {
                    // Release of a key we never pressed: harmless, pass through.
                }
                vec![event]
            }
            InputEvent::MouseButton { button, down } => {
                if down {
                    self.buttons.insert(button_key(button));
                } else {
                    self.buttons.remove(&button_key(button));
                }
                vec![event]
            }
            InputEvent::ReleaseAll => self.release_all(),
            other => vec![other],
        }
    }

    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let mut out: Vec<InputEvent> = self
            .keys
            .drain()
            .map(|code| InputEvent::Key { code, down: false })
            .collect();
        out.extend(
            self.buttons
                .drain()
                .map(|b| InputEvent::MouseButton { button: button_from_key(b), down: false }),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_all_releases_held() {
        let mut h = HeldInputs::default();
        h.process(InputEvent::Key { code: "ControlLeft".into(), down: true });
        h.process(InputEvent::Key { code: "KeyC".into(), down: true });
        h.process(InputEvent::Key { code: "KeyC".into(), down: false });
        h.process(InputEvent::MouseButton { button: MouseButton::Left, down: true });
        let released = h.process(InputEvent::ReleaseAll);
        assert_eq!(released.len(), 2);
        assert!(released.contains(&InputEvent::Key { code: "ControlLeft".into(), down: false }));
        assert!(released.contains(&InputEvent::MouseButton { button: MouseButton::Left, down: false }));
        assert!(h.process(InputEvent::ReleaseAll).is_empty());
    }
}
