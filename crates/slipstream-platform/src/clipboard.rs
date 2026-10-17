//! Clipboard sync: notices local changes and applies remote ones. Text,
//! and lists of copied files (which the session turns into transfers).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use tokio::sync::mpsc::UnboundedSender;

/// What's on a clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    Text(String),
    /// Files or folders copied in a file manager.
    Files(Vec<PathBuf>),
}

/// Which clipboard a session syncs.
#[derive(Debug, Clone, Default)]
pub enum ClipboardSource {
    /// The real system clipboard.
    #[default]
    System,
    /// An in-memory clipboard (tests): the shared value *is* the clipboard.
    Memory(Arc<Mutex<Option<Clip>>>),
}

trait Backend {
    fn get(&mut self) -> Option<Clip>;
    fn set(&mut self, clip: Clip);
}

struct MemoryBackend(Arc<Mutex<Option<Clip>>>);

impl Backend for MemoryBackend {
    fn get(&mut self) -> Option<Clip> {
        self.0.lock().unwrap().clone()
    }
    fn set(&mut self, clip: Clip) {
        *self.0.lock().unwrap() = Some(clip);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
struct SystemBackend(arboard::Clipboard);

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Backend for SystemBackend {
    fn get(&mut self) -> Option<Clip> {
        read(&mut self.0)
    }
    fn set(&mut self, clip: Clip) {
        let r = match clip {
            Clip::Text(t) => self.0.set_text(t),
            Clip::Files(f) => self.0.set().file_list(&f),
        };
        if let Err(e) = r {
            tracing::debug!("clipboard set failed: {e}");
        }
    }
}

/// Files first: file managers often put the paths up as text as well.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn read(cb: &mut arboard::Clipboard) -> Option<Clip> {
    match cb.get().file_list() {
        Ok(files) if !files.is_empty() => Some(Clip::Files(files)),
        _ => cb.get_text().ok().map(Clip::Text),
    }
}

/// What's on this computer's clipboard right now (for pasting into a
/// remote session).
pub fn read_local() -> Option<Clip> {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    return arboard::Clipboard::new().ok().as_mut().and_then(read);
    #[allow(unreachable_code)]
    None
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
#[derive(Clone)]
pub struct Clipboard {
    tx: mpsc::Sender<Clip>,
}

impl Clipboard {
    /// Starts watching the clipboard. Local changes are sent to
    /// `on_change`. Returns `None` if there is no clipboard access.
    pub fn start(on_change: UnboundedSender<Clip>, source: &ClipboardSource) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Clip>();
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
        let _ = self.tx.send(Clip::Text(text));
    }

    /// Puts files on the local clipboard, ready to paste.
    pub fn set_files(&self, files: Vec<PathBuf>) {
        let _ = self.tx.send(Clip::Files(files));
    }
}

fn worker(mut cb: Box<dyn Backend>, rx: mpsc::Receiver<Clip>, on_change: UnboundedSender<Clip>) {
    // Whatever is on the clipboard at start is not "new", and neither is
    // what we put there ourselves (no echo back to the remote side).
    let mut last = cb.get();
    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(clip) => {
                if last.as_ref() != Some(&clip) {
                    cb.set(clip.clone());
                    last = Some(clip);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(clip) = cb.get()
                    && last.as_ref() != Some(&clip)
                {
                    last = Some(clip.clone());
                    if on_change.send(clip).is_err() {
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
            if let Ok(Clip::Text(t)) = rx.try_recv() {
                noticed = Some(t);
                break;
            }
        }
        println!("session noticed local copy: {noticed:?}");
        // Files, both ways (what a file manager would paste / has copied).
        let file = std::env::temp_dir().join("slipstream clipboard test.txt");
        std::fs::write(&file, b"x").unwrap();
        cb.set_files(vec![file.clone()]);
        thread::sleep(Duration::from_millis(600));
        let seen_files = sys.get().file_list().ok();
        println!("OS clipboard after remote file set: {seen_files:?}");
        let other = std::env::temp_dir().join("slipstream clipboard test 2.txt");
        std::fs::write(&other, b"y").unwrap();
        sys.set().file_list(std::slice::from_ref(&other)).unwrap();
        let mut noticed_files = None;
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(100));
            if let Ok(Clip::Files(f)) = rx.try_recv() {
                noticed_files = Some(f);
                break;
            }
        }
        println!("session noticed local file copy: {noticed_files:?}");
        if let Some(b) = before {
            let _ = sys.set_text(b);
        }
        assert_eq!(seen.as_deref(), Some("slipstream remote→local"));
        assert_eq!(noticed.as_deref(), Some("slipstream local→remote"));
        assert_eq!(seen_files, Some(vec![file.clone()]));
        assert_eq!(noticed_files, Some(vec![other.clone()]));
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_file(other);
    }
}
