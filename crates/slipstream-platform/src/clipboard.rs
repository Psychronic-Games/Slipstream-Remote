//! Clipboard sync: notices local changes and applies remote ones.

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use tokio::sync::mpsc::UnboundedSender;

/// Which clipboard a session syncs.
#[derive(Debug, Clone, Default)]
pub enum ClipboardSource {
    /// The real system clipboard.
    #[default]
    System,
    /// An in-memory clipboard (tests): the shared string *is* the clipboard.
    Memory(Arc<Mutex<Option<String>>>),
}

trait Backend {
    fn get_text(&mut self) -> Option<String>;
    fn set_text(&mut self, text: String);
}

struct MemoryBackend(Arc<Mutex<Option<String>>>);

impl Backend for MemoryBackend {
    fn get_text(&mut self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
    fn set_text(&mut self, text: String) {
        *self.0.lock().unwrap() = Some(text);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
struct SystemBackend(arboard::Clipboard);

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Backend for SystemBackend {
    fn get_text(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }
    fn set_text(&mut self, text: String) {
        if let Err(e) = self.0.set_text(text) {
            tracing::debug!("clipboard set failed: {e}");
        }
    }
}

fn open(source: &ClipboardSource) -> Option<Box<dyn Backend>> {
    match source {
        ClipboardSource::Memory(m) => Some(Box::new(MemoryBackend(m.clone()))),
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        ClipboardSource::System => match arboard::Clipboard::new() {
            Ok(c) => Some(Box::new(SystemBackend(c))),
            Err(e) => {
                tracing::warn!("clipboard unavailable: {e}");
                None
            }
        },
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        ClipboardSource::System => None,
    }
}

/// Handle to a background clipboard worker.
pub struct Clipboard {
    tx: mpsc::Sender<String>,
}

impl Clipboard {
    /// Starts watching the clipboard. Local text changes are sent to
    /// `on_change`. Returns `None` if there is no clipboard access.
    pub fn start(on_change: UnboundedSender<String>, source: &ClipboardSource) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<String>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let source = source.clone();
        thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || match open(&source) {
                Some(backend) => {
                    let _ = ready_tx.send(true);
                    worker(backend, rx, on_change);
                }
                None => {
                    let _ = ready_tx.send(false);
                }
            })
            .ok()?;
        ready_rx.recv().ok()?.then_some(Self { tx })
    }

    /// Replaces the local clipboard text (from the remote side).
    pub fn set_text(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

fn worker(mut cb: Box<dyn Backend>, rx: mpsc::Receiver<String>, on_change: UnboundedSender<String>) {
    // Whatever is on the clipboard at start is not "new".
    let mut last = cb.get_text();
    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(text) => {
                if last.as_deref() != Some(text.as_str()) {
                    cb.set_text(text.clone());
                    last = Some(text);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(text) = cb.get_text()
                    && last.as_deref() != Some(text.as_str())
                {
                    last = Some(text.clone());
                    if on_change.send(text).is_err() {
                        return;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if on_change.is_closed() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real system clipboard round trip (touches your clipboard; restores it).
    /// `cargo test -p slipstream-platform system_clipboard -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn system_clipboard_roundtrip() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cb = Clipboard::start(tx, &ClipboardSource::System).expect("clipboard available");
        let mut sys = arboard::Clipboard::new().unwrap();
        let before = sys.get_text().ok();
        // Remote → local: the session writes; the OS sees it.
        cb.set_text("slipstream remote→local".into());
        thread::sleep(Duration::from_millis(600));
        let seen = sys.get_text().ok();
        println!("OS clipboard after remote set: {seen:?}");
        // Local → remote: the OS changes; the session notices.
        sys.set_text("slipstream local→remote").unwrap();
        let mut noticed = None;
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(100));
            if let Ok(t) = rx.try_recv() {
                noticed = Some(t);
                break;
            }
        }
        println!("session noticed local copy: {noticed:?}");
        if let Some(b) = before {
            let _ = sys.set_text(b);
        }
        assert_eq!(seen.as_deref(), Some("slipstream remote→local"));
        assert_eq!(noticed.as_deref(), Some("slipstream local→remote"));
    }
}
