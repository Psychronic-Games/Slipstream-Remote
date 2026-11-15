//! Hosting from the signed-in desktop while the Linux service is installed.
//!
//! The service reads the screen below the desktop, which reaches the login
//! screen but has no clipboard or sound, and can't read every graphics
//! card. A desktop session has all of those. So while someone is signed in
//! and their session is in front and unlocked, the service lends its place
//! to a **desktop host**: a helper the app starts in that session
//! (`slipstream service desktop`), which hosts under the same identity
//! through the desktop's own screen sharing, as the app did before the
//! service was installed. The service takes over again when the session
//! locks, moves to the background or ends.
//!
//! One computer, one identity, one host at a time: the two never host
//! together. They talk over a socket in `/run/slipstream-remote`, in lines:
//!
//! ```text
//! helper   hello 1 <login session>
//! service  prove <nonce>            helper   proof <signature>
//! service  host | wait | refused …  helper   released   (after wait)
//! ```
//!
//! The proof is a signature with the computer's identity key: only a
//! helper holding it could host under this identity anyway, and anyone
//! else must not be able to make the service step aside.

use std::{
    future::Future,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    pin::Pin,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use slipstream_core::{
    config::Store,
    iroh::{PublicKey, SecretKey, Signature},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot},
};

use crate::cli::{self, HostEnd};

const PROOF_CONTEXT: &[u8] = b"slipstream desktop host 1\n";
/// How long a choice must hold before hosting moves (a lock screen that
/// flickers past, a session still starting).
const SETTLE: Duration = Duration::from_secs(2);
/// How long the helper gets to let go before the service hosts regardless.
const RELEASE: Duration = Duration::from_secs(6);

fn socket_path() -> PathBuf {
    // Tests run both sides without root.
    std::env::var_os("SLIPSTREAM_DESKTOP_SOCKET").map(PathBuf::from).unwrap_or_else(|| "/run/slipstream-remote/desktop.sock".into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn proof_message(nonce: &[u8; 32]) -> Vec<u8> {
    [PROOF_CONTEXT, nonce].concat()
}

/// A login session as logind describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Session {
    /// In front: on the displays, not switched away from.
    active: bool,
    locked: bool,
}

/// Reads `loginctl show-session` output; `None` unless it is a graphical
/// session of `uid`'s own.
fn parse_session(text: &str, uid: u32) -> Option<Session> {
    let get = |key: &str| text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='));
    if get("User")?.trim().parse::<u32>().ok()? != uid || get("Class")?.trim() != "user" || !matches!(get("Type")?.trim(), "wayland" | "x11" | "mir") {
        return None;
    }
    Some(Session { active: get("Active")?.trim() == "yes", locked: get("LockedHint").is_some_and(|v| v.trim() == "yes") })
}

fn session_state(uid: u32, session: &str) -> Option<Session> {
    let out = std::process::Command::new("loginctl").args(["show-session", session, "-p", "User", "-p", "Class", "-p", "Type", "-p", "Active", "-p", "LockedHint"]).output().ok()?;
    parse_session(&String::from_utf8_lossy(&out.stdout), uid)
}

/// Whether a desktop should host now. A locked session goes back to the
/// service, which shows the lock screen (a Wayland desktop hides it from
/// screen sharing), unless the service can't read this computer's screen:
/// then the desktop is still the better of the two.
fn desktop_should_host(session: Option<Session>, service_reads_screen: bool) -> bool {
    session.is_some_and(|s| s.active && (!s.locked || !service_reads_screen))
}

// ---------------------------------------------------------------- service

struct Desk {
    id: u64,
    uid: u32,
    session: String,
    tell: mpsc::UnboundedSender<&'static str>,
}

enum Event {
    Joined(Desk),
    Released(u64),
    Left(u64),
}

type Hosting = (Pin<Box<dyn Future<Output = Result<HostEnd>>>>, oneshot::Sender<()>);

fn start_service_host() -> Hosting {
    let (stop, stopped) = oneshot::channel::<()>();
    let until = async move {
        if stopped.await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    (Box::pin(cli::host_until(false, None, true, until)), stop)
}

/// Says who hosts, for signed-in users' apps to show.
fn publish_host(desktop: bool) {
    let path = crate::service::host_file();
    if std::fs::write(&path, if desktop { "desktop" } else { "service" }).is_ok() {
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    }
}

/// `slipstream service run`: hosts this computer, lending the place to a
/// signed-in desktop's helper while that is the better host.
pub async fn run_service() -> Result<()> {
    let key = Store::open_default()?.identity()?.public();
    let (events_tx, mut events) = mpsc::unbounded_channel();
    match listen() {
        Ok(listener) => {
            tokio::spawn(accept(listener, key, events_tx));
        }
        // Without it the service hosts by itself, as it did before.
        Err(e) => tracing::warn!("desktop hosting unavailable: {e:#}"),
    }
    let mut desks: Vec<Desk> = Vec::new();
    let mut hosting = Some(start_service_host());
    // The desk hosting in the service's place.
    let mut lent: Option<u64> = None;
    let mut choice: (Option<u64>, Instant) = (None, Instant::now());
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    publish_host(false);
    loop {
        tokio::select! {
            // The host ended by itself: asked to stop, or updated.
            ended = async { hosting.as_mut().expect("checked").0.as_mut().await }, if hosting.is_some() => {
                return ended.map(|_| ());
            }
            _ = cli::shutdown_signal(), if hosting.is_none() => {
                if let Some(desk) = desks.iter().find(|d| Some(d.id) == lent) {
                    let _ = desk.tell.send("wait\n");
                }
                return Ok(());
            }
            Some(event) = events.recv() => match event {
                Event::Joined(desk) => {
                    tracing::info!(uid = desk.uid, session = %desk.session, "a desktop host is ready");
                    desks.push(desk);
                }
                Event::Released(_) => {}
                Event::Left(id) => {
                    desks.retain(|d| d.id != id);
                    if lent == Some(id) {
                        // Its place is empty: host again at once.
                        tracing::info!("the desktop host left; the service hosts");
                        lent = None;
                        hosting = Some(start_service_host());
                        publish_host(false);
                    }
                }
            },
            _ = tick.tick() => {}
        }
        // Who should host: the desk that has the place while it still
        // should, else the first that should, else the service.
        let reads = crate::service::screen_readable();
        let candidates: Vec<(u64, u32, String)> = desks.iter().map(|d| (d.id, d.uid, d.session.clone())).collect();
        let holder = lent;
        let wanted = tokio::task::spawn_blocking(move || {
            let fit = |(_, uid, session): &&(u64, u32, String)| desktop_should_host(session_state(*uid, session), reads);
            candidates.iter().filter(|c| Some(c.0) == holder).find(fit).or_else(|| candidates.iter().find(fit)).map(|c| c.0)
        })
        .await
        .unwrap_or(None);
        if wanted != choice.0 {
            choice = (wanted, Instant::now());
        }
        if wanted == lent || choice.1.elapsed() < SETTLE {
            continue;
        }
        // Take the place back from whoever has it.
        if let Some(old) = lent.take() {
            if let Some(desk) = desks.iter().find(|d| d.id == old) {
                let _ = desk.tell.send("wait\n");
            }
            let deadline = tokio::time::Instant::now() + RELEASE;
            loop {
                match tokio::time::timeout_at(deadline, events.recv()).await {
                    Ok(Some(Event::Released(id))) if id == old => break,
                    Ok(Some(Event::Left(id))) => {
                        desks.retain(|d| d.id != id);
                        if id == old {
                            break;
                        }
                    }
                    Ok(Some(Event::Joined(desk))) => desks.push(desk),
                    Ok(Some(Event::Released(_))) => {}
                    Ok(None) | Err(_) => {
                        tracing::warn!("the desktop host didn't let go in time");
                        break;
                    }
                }
            }
        }
        match wanted.and_then(|id| desks.iter().find(|d| d.id == id)) {
            Some(desk) => {
                if let Some((mut host, stop)) = hosting.take() {
                    let _ = stop.send(());
                    match tokio::time::timeout(Duration::from_secs(12), host.as_mut()).await {
                        // It stopped for its own reasons at that moment.
                        Ok(Ok(HostEnd::Stopped)) => return Ok(()),
                        Ok(Err(e)) => return Err(e),
                        Ok(Ok(HostEnd::Yielded)) => {}
                        Err(_) => tracing::warn!("the service's host was slow to stop"),
                    }
                }
                tracing::info!(uid = desk.uid, session = %desk.session, "the desktop hosts (its own screen sharing, clipboard and sound)");
                let _ = desk.tell.send("host\n");
                lent = Some(desk.id);
                publish_host(true);
            }
            None => {
                if hosting.is_none() {
                    tracing::info!("the service hosts (login screen, locked or background session)");
                    hosting = Some(start_service_host());
                    publish_host(false);
                }
            }
        }
    }
}

fn listen() -> Result<UnixListener> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("listening on {}", path.display()))?;
    // Any signed-in user's helper may knock; the proof decides.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
    Ok(listener)
}

async fn accept(listener: UnixListener, key: PublicKey, events: mpsc::UnboundedSender<Event>) {
    // Few helpers exist (one a signed-in user); the limit is for strangers.
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    let mut next = 0u64;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
        next += 1;
        let (id, events) = (next, events.clone());
        tokio::spawn(async move {
            let _slot = slot;
            if let Err(e) = serve_desk(stream, key, id, events).await {
                tracing::debug!("desktop host connection: {e:#}");
            }
        });
    }
}

async fn serve_desk(stream: UnixStream, key: PublicKey, id: u64, events: mpsc::UnboundedSender<Event>) -> Result<()> {
    let uid = stream.peer_cred()?.uid();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let soon = Duration::from_secs(5);
    let hello = tokio::time::timeout(soon, lines.next_line()).await??.context("closed")?;
    let session = hello.strip_prefix("hello 1 ").context("unknown greeting")?.trim().to_string();
    if session.is_empty() || session.len() > 32 || !session.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("not a login session name");
    }
    let nonce: [u8; 32] = rand::random();
    write.write_all(format!("prove {}\n", hex(&nonce)).as_bytes()).await?;
    let proof = tokio::time::timeout(soon, lines.next_line()).await??.context("closed")?;
    let signature = proof.strip_prefix("proof ").and_then(unhex::<64>).context("no proof")?;
    if key.verify(&proof_message(&nonce), &Signature::from_bytes(&signature)).is_err() {
        // Another user's app, or one from before a reinstall.
        write.write_all(b"refused this app has a different identity from the service's\n").await?;
        return Ok(());
    }
    let (tell, mut told) = mpsc::unbounded_channel();
    if events.send(Event::Joined(Desk { id, uid, session, tell })).is_err() {
        return Ok(());
    }
    let served = async {
        loop {
            tokio::select! {
                line = told.recv() => match line {
                    Some(line) => write.write_all(line.as_bytes()).await?,
                    None => break,
                },
                line = lines.next_line() => match line? {
                    Some(line) if line.trim() == "released" => { let _ = events.send(Event::Released(id)); }
                    Some(_) => {}
                    None => break,
                },
            }
        }
        anyhow::Ok(())
    }
    .await;
    let _ = events.send(Event::Left(id));
    served
}

// ----------------------------------------------------------------- helper

/// Makes the helper's own profile from the app's (`from`): the same
/// identity, station, unattended password and permissions. It is separate
/// because two programs can't share one settings file; what the helper
/// learned (the desktop's screen-sharing approval, its IDs) is kept.
///
/// `settings`: copy the settings too. Done only when hosting starts, not
/// while waiting: the app may still be writing its own (a screen-sharing
/// approval it has just renewed), and a stale approval here makes the
/// desktop ask again.
fn sync_profile(from: &Path, settings: bool) -> Result<SecretKey> {
    let own = Store::open_default()?;
    if own.dir() == from {
        bail!("the desktop host needs a profile of its own");
    }
    let key = std::fs::read(from.join("identity.key")).with_context(|| format!("reading this app's identity in {}", from.display()))?;
    let dest = own.dir().join("identity.key");
    if std::fs::read(&dest).ok().as_deref() != Some(&key[..]) {
        let tmp = dest.with_extension("new");
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, &key)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, &dest)?;
    }
    if !settings {
        return own.identity();
    }
    let old = own.load_settings();
    let mut settings = Store::open(from)?.load_settings();
    settings.host.enabled = true;
    if old.host.portal_restore_token.is_some() {
        settings.host.portal_restore_token = old.host.portal_restore_token;
    }
    if old.server.url == settings.server.url && old.host.last_device_id.is_some() {
        settings.host.last_device_id = old.host.last_device_id;
    }
    if old.host.public_device_id.is_some() {
        settings.host.public_device_id = old.host.public_device_id;
    }
    // The helper only hosts: the technician's sign-ins stay with the app.
    settings.accounts.clear();
    settings.recent.clear();
    own.save_settings(&settings)?;
    own.identity()
}

fn login_session() -> Option<String> {
    if let Some(id) = std::env::var("XDG_SESSION_ID").ok().filter(|id| !id.is_empty()) {
        return Some(id);
    }
    // SAFETY: a plain call.
    let uid = unsafe { libc::getuid() };
    let out = std::process::Command::new("loginctl").args(["show-user", &uid.to_string(), "-p", "Display", "--value"]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|id| !id.is_empty())
}

enum Attended {
    /// Asked to stop (the app closed, the session is ending).
    Stop,
    /// The service went away or wouldn't have us: try again later.
    Retry(Duration),
}

/// `slipstream service desktop --from <the app's profile>`: started by
/// the app in a desktop session while the service is installed.
pub async fn run_desktop(from: &Path) -> Result<()> {
    loop {
        let wait = match attend(from).await {
            Ok(Attended::Stop) => return Ok(()),
            Ok(Attended::Retry(wait)) => wait,
            Err(e) => {
                tracing::debug!("desktop host: {e:#}");
                Duration::from_secs(5)
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = cli::shutdown_signal() => return Ok(()),
        }
    }
}

async fn attend(from: &Path) -> Result<Attended> {
    let key = sync_profile(from, false)?;
    let session = login_session().context("not in a login session")?;
    let stream = UnixStream::connect(socket_path()).await.context("the Slipstream service isn't listening")?;
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(format!("hello 1 {session}\n").as_bytes()).await?;
    let ask = lines.next_line().await?.context("closed")?;
    let nonce = ask.strip_prefix("prove ").and_then(unhex::<32>).context("unexpected answer")?;
    write.write_all(format!("proof {}\n", hex(&key.sign(&proof_message(&nonce)).to_bytes())).as_bytes()).await?;
    tracing::info!("ready to host from this desktop when the service asks");

    let mut hosting: Option<Hosting> = None;
    let start = || {
        let (stop, stopped) = oneshot::channel::<()>();
        let until = async move {
            if stopped.await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        let host: Pin<Box<dyn Future<Output = Result<HostEnd>>>> = Box::pin(cli::host_until(false, None, false, until));
        (host, stop)
    };
    // Stops hosting and waits until the identity is free again.
    async fn halt(hosting: &mut Option<Hosting>) {
        if let Some((mut host, stop)) = hosting.take() {
            let _ = stop.send(());
            let _ = tokio::time::timeout(Duration::from_secs(5), host.as_mut()).await;
        }
    }
    loop {
        tokio::select! {
            ended = async { hosting.as_mut().expect("checked").0.as_mut().await }, if hosting.is_some() => {
                return match ended {
                    Ok(_) => Ok(Attended::Stop),
                    // Couldn't host here (no screen sharing, no station):
                    // give the place back by leaving, and try again later.
                    Err(e) => {
                        tracing::warn!("hosting from this desktop failed: {e:#}");
                        Ok(Attended::Retry(Duration::from_secs(60)))
                    }
                };
            }
            _ = cli::shutdown_signal(), if hosting.is_none() => return Ok(Attended::Stop),
            line = lines.next_line() => match line?.as_deref().map(str::trim) {
                Some("host") => {
                    if hosting.is_none() {
                        // Settings changed in the app since the last time.
                        sync_profile(from, true)?;
                        tracing::info!("hosting from this desktop");
                        hosting = Some(start());
                    }
                }
                Some("wait") => {
                    if hosting.is_some() {
                        tracing::info!("handing hosting back to the service");
                    }
                    halt(&mut hosting).await;
                    write.write_all(b"released\n").await?;
                }
                Some(line) if line.starts_with("refused") => {
                    tracing::info!("the service keeps hosting: {}", line.trim_start_matches("refused").trim());
                    return Ok(Attended::Retry(Duration::from_secs(300)));
                }
                Some(_) => {}
                None => {
                    // The service restarted (an update) or was removed.
                    halt(&mut hosting).await;
                    return Ok(Attended::Retry(Duration::from_secs(5)));
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "User=1000\nClass=user\nType=wayland\nActive=yes\nLockedHint=no\n";

    #[test]
    fn reads_a_login_session() {
        assert_eq!(parse_session(SESSION, 1000), Some(Session { active: true, locked: false }));
        let locked = SESSION.replace("LockedHint=no", "LockedHint=yes").replace("Active=yes", "Active=no");
        assert_eq!(parse_session(&locked, 1000), Some(Session { active: false, locked: true }));
        // Someone else's session, a text login and a greeter never host.
        assert_eq!(parse_session(SESSION, 1001), None);
        assert_eq!(parse_session(&SESSION.replace("wayland", "tty"), 1000), None);
        assert_eq!(parse_session(&SESSION.replace("Class=user", "Class=greeter"), 1000), None);
        assert_eq!(parse_session("", 1000), None);
    }

    #[test]
    fn the_desktop_hosts_while_in_front_and_unlocked() {
        let session = |active, locked| Some(Session { active, locked });
        assert!(desktop_should_host(session(true, false), true));
        assert!(!desktop_should_host(session(true, true), true));
        assert!(!desktop_should_host(session(false, false), true));
        assert!(!desktop_should_host(None, true));
        // Where the service can't read the screen, a locked desktop is
        // still the better host.
        assert!(desktop_should_host(session(true, true), false));
        assert!(!desktop_should_host(session(false, true), false));
    }

    #[test]
    fn proofs_are_tied_to_the_key_and_the_nonce() {
        let key = SecretKey::generate();
        let nonce = [7u8; 32];
        let proof = hex(&key.sign(&proof_message(&nonce)).to_bytes());
        let signature = Signature::from_bytes(&unhex::<64>(&proof).unwrap());
        assert!(key.public().verify(&proof_message(&nonce), &signature).is_ok());
        assert!(key.public().verify(&proof_message(&[8u8; 32]), &signature).is_err());
        assert!(SecretKey::generate().public().verify(&proof_message(&nonce), &signature).is_err());
        assert_eq!(unhex::<2>("0aff"), Some([10, 255]));
        assert_eq!(unhex::<2>("0af"), None);
        assert_eq!(unhex::<2>("zzzz"), None);
    }

    /// The whole handshake over a real socket: a helper with the right
    /// key is accepted and told to host; one with another key is refused.
    #[tokio::test]
    async fn the_service_accepts_only_its_own_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let key = SecretKey::generate();
        let (events_tx, mut events) = mpsc::unbounded_channel();
        tokio::spawn(accept(listener, key.public(), events_tx));

        let knock = |key: SecretKey| {
            let path = path.clone();
            async move {
                let (read, mut write) = UnixStream::connect(&path).await.unwrap().into_split();
                let mut lines = BufReader::new(read).lines();
                write.write_all(b"hello 1 42\n").await.unwrap();
                let ask = lines.next_line().await.unwrap().unwrap();
                let nonce = unhex::<32>(ask.strip_prefix("prove ").unwrap()).unwrap();
                write.write_all(format!("proof {}\n", hex(&key.sign(&proof_message(&nonce)).to_bytes())).as_bytes()).await.unwrap();
                (lines, write)
            }
        };
        let (mut lines, _write) = knock(SecretKey::generate()).await;
        assert!(lines.next_line().await.unwrap().unwrap().starts_with("refused"));

        let (mut lines, mut write) = knock(key).await;
        let Some(Event::Joined(desk)) = events.recv().await else { panic!("not joined") };
        assert_eq!(desk.session, "42");
        desk.tell.send("host\n").unwrap();
        assert_eq!(lines.next_line().await.unwrap().unwrap(), "host");
        write.write_all(b"released\n").await.unwrap();
        assert!(matches!(events.recv().await, Some(Event::Released(id)) if id == desk.id));
        drop((lines, write));
        assert!(matches!(events.recv().await, Some(Event::Left(id)) if id == desk.id));
    }
}
