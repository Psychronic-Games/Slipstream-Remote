//! Single-instance guard: the first Slipstream (per settings profile)
//! listens on a loopback port derived from its profile path; later launches
//! connect to it, ask it to show its window (or pass on a `slipstream://`
//! link they were opened with), and exit. Android has its own [`inbox`].
#![cfg_attr(target_os = "android", allow(dead_code, unused_imports))]

use std::{
    hash::{Hash, Hasher},
    io::{BufRead, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};

const SHOW: &[u8; 16] = b"slipstream:show\n";
/// Followed by the URL and a newline.
const LINK: &[u8; 16] = b"slipstream:link\n";

/// What a later launch asked for.
pub enum Request {
    Show,
    Link(String),
}

pub struct Instance {
    listener: TcpListener,
}

fn port_for(profile: &Path) -> u16 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    profile.hash(&mut h);
    40_000 + (h.finish() % 20_000) as u16
}

impl Instance {
    /// Returns `None` if another instance is running (it has been asked to
    /// show itself, and handed `link`; `quiet`: left alone, for a launch
    /// that starts hidden). If the port is taken by something else, carries
    /// on.
    pub fn acquire(profile: &Path, link: Option<&str>, quiet: bool) -> Option<Self> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port_for(profile)));
        // Relaunched after an update: the old copy is still exiting, so wait
        // for it to release the instance port instead of handing off to it.
        if std::env::var_os("SLIPSTREAM_RESTARTED").is_some() {
            for _ in 0..150 {
                if let Ok(listener) = TcpListener::bind(addr) {
                    return Some(Self { listener });
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        match TcpListener::bind(addr) {
            Ok(listener) => Some(Self { listener }),
            // Started hidden (at sign-in, possibly twice: the user's own
            // autostart and the service's): nothing to bring up.
            Err(_) if quiet && link.is_none() => None,
            Err(_) => {
                if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
                    let msg = match link {
                        Some(url) => [&LINK[..], url.as_bytes(), b"\n"].concat(),
                        None => SHOW.to_vec(),
                    };
                    if s.write_all(&msg).is_ok() {
                        let mut ack = [0u8; 2];
                        if s.read_exact(&mut ack).is_ok() && &ack == b"ok" {
                            return None;
                        }
                    }
                }
                // Port in use by something that isn't us: run anyway.
                let fallback = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
                Some(Self { listener: fallback })
            }
        }
    }

    /// Serves requests from later launches; `wake` is called after each.
    pub fn listen(self, wake: impl Fn() + Send + 'static) -> mpsc::Receiver<Request> {
        let (tx, rx) = mpsc::channel();
        let _ = thread::Builder::new().name("single-instance".into()).spawn(move || {
            for stream in self.listener.incoming().flatten() {
                let mut s = stream;
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 16];
                if s.read_exact(&mut buf).is_err() {
                    continue;
                }
                let req = if &buf == SHOW {
                    Request::Show
                } else if &buf == LINK {
                    // The sender waits for our "ok", so read up to the newline.
                    let mut url = String::new();
                    let _ = std::io::BufReader::new((&s).take(4096)).read_line(&mut url);
                    match url.trim() {
                        "" => continue,
                        u => Request::Link(u.to_string()),
                    }
                } else {
                    continue;
                };
                let _ = s.write_all(b"ok");
                if tx.send(req).is_err() {
                    return;
                }
                wake();
            }
        });
        rx
    }
}

/// Android: the activity is the one instance, so there are no later
/// launches to listen for, and loopback ports are open to every app on the
/// device. Links from the activity's intents come straight in here instead.
#[cfg(target_os = "android")]
pub mod inbox {
    use std::sync::{Mutex, mpsc};

    use super::Request;

    type Wake = Box<dyn Fn() + Send>;

    struct Inbox {
        /// Links that came before the app was ready for them.
        pending: Vec<String>,
        open: Option<(mpsc::Sender<Request>, Wake)>,
    }

    static INBOX: Mutex<Inbox> = Mutex::new(Inbox { pending: Vec::new(), open: None });

    /// Hands a `slipstream://` link to the app, now or once it's running.
    pub fn deliver(url: String) {
        tracing::info!("opened with a station link");
        let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
        match &inbox.open {
            Some((tx, wake)) => {
                let _ = tx.send(Request::Link(url));
                wake();
            }
            None => inbox.pending.push(url),
        }
    }

    /// Where the app reads links from, starting with any that are waiting;
    /// `wake` is called after each new one.
    pub fn open(wake: impl Fn() + Send + 'static) -> mpsc::Receiver<Request> {
        let (tx, rx) = mpsc::channel();
        let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
        for url in inbox.pending.drain(..) {
            let _ = tx.send(Request::Link(url));
        }
        inbox.open = Some((tx, Box::new(wake)));
        rx
    }
}
