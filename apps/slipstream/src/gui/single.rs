//! Single-instance guard: the first Slipstream (per settings profile)
//! listens on a loopback port derived from its profile path; later launches
//! connect to it, ask it to show its window, and exit.

use std::{
    hash::{Hash, Hasher},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};

use eframe::egui;

const MAGIC: &[u8] = b"slipstream:show\n";

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
    /// show itself). If the port is taken by something else, carries on.
    pub fn acquire(profile: &Path) -> Option<Self> {
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
            Err(_) => {
                if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
                    if s.write_all(MAGIC).is_ok() {
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

    /// Serves "show" requests from later launches.
    pub fn listen(self, ctx: egui::Context) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        let _ = thread::Builder::new().name("single-instance".into()).spawn(move || {
            for stream in self.listener.incoming().flatten() {
                let mut s = stream;
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 16];
                if s.read_exact(&mut buf).is_ok() && buf == MAGIC {
                    let _ = s.write_all(b"ok");
                    if tx.send(()).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        });
        rx
    }
}
