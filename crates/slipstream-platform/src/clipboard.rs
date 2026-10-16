//! System clipboard sync: notices local changes and applies remote ones.

use std::{sync::mpsc, thread, time::Duration};

use tokio::sync::mpsc::UnboundedSender;

/// Handle to a background clipboard worker.
pub struct Clipboard {
    tx: mpsc::Sender<String>,
}

impl Clipboard {
    /// Starts watching the clipboard. Local text changes are sent to
    /// `on_change`. Returns `None` if the platform has no clipboard access.
    pub fn start(on_change: UnboundedSender<String>) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<String>();
        let (ready_tx, ready_rx) = mpsc::channel();
        thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || worker(rx, on_change, ready_tx))
            .ok()?;
        ready_rx.recv().ok()?.then_some(Self { tx })
    }

    /// Replaces the local clipboard text (from the remote side).
    pub fn set_text(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn worker(rx: mpsc::Receiver<String>, on_change: UnboundedSender<String>, ready: mpsc::Sender<bool>) {
    let mut cb = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("clipboard unavailable: {e}");
            let _ = ready.send(false);
            return;
        }
    };
    let _ = ready.send(true);
    // Whatever is on the clipboard at start is not "new".
    let mut last = cb.get_text().ok();
    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(text) => {
                if last.as_deref() != Some(text.as_str()) {
                    if let Err(e) = cb.set_text(text.clone()) {
                        tracing::debug!("clipboard set failed: {e}");
                    }
                    last = Some(text);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(text) = cb.get_text()
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

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn worker(_rx: mpsc::Receiver<String>, _on_change: UnboundedSender<String>, ready: mpsc::Sender<bool>) {
    let _ = ready.send(false);
}
