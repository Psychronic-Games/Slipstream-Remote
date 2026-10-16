//! Slipstream sessions: the long-running [`Client`] that keeps this device
//! reachable by ID, hosts incoming sessions, and opens outgoing ones.

mod cursor_folder;
pub mod files;
pub mod fsops;
mod host;
pub mod update;
mod video;
pub mod viewer;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, EndpointId, Watcher};
pub use slipstream_core;
use slipstream_core::{
    auth::{AuthThrottle, generate_password},
    config::{Settings, Store},
    directory::{DeviceId, DeviceInfo, DirectoryClient, Lookup},
    net,
    proto::ALPN_REMOTE,
};
pub use slipstream_codec::YuvFrame;
use slipstream_platform::Backend;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};
pub use files::RemoteFs;
pub use viewer::{FrameShared, ViewerCmd, ViewerEvent, ViewerSession, ViewerStats};

/// Connection state to the directory server.
#[derive(Debug, Clone, PartialEq)]
pub enum DirStatus {
    NotConfigured,
    Connecting,
    Online { id: DeviceId },
    /// Server reachable (view-only client; not registered).
    Ready,
    Error(String),
}

/// Things the UI should know about, in order.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Directory(DirStatus),
    Password(String),
    /// Server message of the day.
    Motd(String),
    /// Name of the organisation running the station, if any.
    Org(Option<String>),
    HostSessionStarted { session: u64, peer: String },
    HostSessionEnded { session: u64, reason: String },
    HostChat { session: u64, text: String },
    FileReceived { session: u64, path: std::path::PathBuf },
    Notice(String),
    /// The station offers a newer version of the app.
    UpdateAvailable(update::UpdateInfo),
}

#[derive(Debug, Clone, Default)]
pub struct ClientOptions {
    /// Stream a synthetic test pattern instead of the real screen.
    pub test_pattern: bool,
    /// Accept incoming remote-control sessions.
    pub accept_sessions: bool,
    /// Which clipboard to sync (the system one, or an in-memory one for tests).
    pub clipboard: slipstream_platform::clipboard::ClipboardSource,
}

pub(crate) enum HostSessionCmd {
    Chat(String),
    Disconnect,
}

pub(crate) struct Inner {
    pub endpoint: Endpoint,
    pub store: Store,
    pub settings: Mutex<Settings>,
    pub opts: ClientOptions,
    events: mpsc::UnboundedSender<ClientEvent>,
    dir_status: watch::Sender<DirStatus>,
    dir_client: watch::Sender<Option<DirectoryClient>>,
    server_changed: tokio::sync::Notify,
    pub password: Mutex<String>,
    pub throttle: Mutex<AuthThrottle>,
    pub backend: tokio::sync::Mutex<Option<Box<dyn Backend>>>,
    pub host_sessions: Mutex<HashMap<u64, mpsc::UnboundedSender<HostSessionCmd>>>,
    next_session: AtomicU64,
}

impl Inner {
    pub fn emit(&self, e: ClientEvent) {
        let _ = self.events.send(e);
    }

    pub fn one_time_password(&self) -> String {
        self.password.lock().unwrap().clone()
    }

    pub fn next_session_id(&self) -> u64 {
        self.next_session.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn save_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        if let Err(e) = self.store.save_settings(&s) {
            warn!("saving settings failed: {e:#}");
        }
    }
}

/// The long-lived Slipstream client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Loads settings and identity from `store`, binds the network endpoint
    /// and starts background tasks. Must be called inside a tokio runtime.
    pub async fn start(store: Store, opts: ClientOptions) -> Result<(Self, mpsc::UnboundedReceiver<ClientEvent>)> {
        let settings = store.load_settings();
        let secret = store.identity()?;
        let endpoint = net::bind_client(secret, &settings.server, opts.accept_sessions).await?;
        info!(endpoint = %endpoint.id(), "endpoint bound");
        let (events, rx) = mpsc::unbounded_channel();
        let password = generate_password(settings.host.one_time_password_len.clamp(4, 32));
        let inner = Arc::new(Inner {
            endpoint,
            store,
            settings: Mutex::new(settings),
            opts,
            events,
            dir_status: watch::Sender::new(DirStatus::Connecting),
            dir_client: watch::Sender::new(None),
            server_changed: tokio::sync::Notify::new(),
            password: Mutex::new(password.clone()),
            throttle: Mutex::new(AuthThrottle::new()),
            backend: tokio::sync::Mutex::new(None),
            host_sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(0),
        });
        inner.emit(ClientEvent::Password(password));
        tokio::spawn(directory_task(inner.clone()));
        tokio::spawn(update_task(Client { inner: inner.clone() }));
        if inner.opts.accept_sessions {
            tokio::spawn(accept_task(inner.clone()));
        }
        Ok((Self { inner }, rx))
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.inner.endpoint.id()
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings.lock().unwrap().clone()
    }

    /// Changes settings and saves them. Reconnects to the directory if the
    /// server changed.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let server_changed = {
            let mut s = self.inner.settings.lock().unwrap();
            let before = (s.server.url.clone(), s.server.key.clone());
            f(&mut s);
            before != (s.server.url.clone(), s.server.key.clone())
        };
        self.inner.save_settings();
        if server_changed {
            self.inner.server_changed.notify_waiters();
        }
    }

    pub fn dir_status(&self) -> DirStatus {
        self.inner.dir_status.borrow().clone()
    }

    pub fn one_time_password(&self) -> String {
        self.inner.one_time_password()
    }

    pub fn regenerate_password(&self) -> String {
        let len = self.settings().host.one_time_password_len.clamp(4, 32);
        let pw = generate_password(len);
        *self.inner.password.lock().unwrap() = pw.clone();
        self.inner.emit(ClientEvent::Password(pw.clone()));
        pw
    }

    pub fn host_chat(&self, session: u64, text: String) {
        if let Some(tx) = self.inner.host_sessions.lock().unwrap().get(&session) {
            let _ = tx.send(HostSessionCmd::Chat(text));
        }
    }

    pub fn disconnect_host_session(&self, session: u64) {
        if let Some(tx) = self.inner.host_sessions.lock().unwrap().get(&session) {
            let _ = tx.send(HostSessionCmd::Disconnect);
        }
    }

    pub fn disconnect_all_host_sessions(&self) {
        for tx in self.inner.host_sessions.lock().unwrap().values() {
            let _ = tx.send(HostSessionCmd::Disconnect);
        }
    }

    async fn directory(&self) -> Result<DirectoryClient> {
        let mut rx = self.inner.dir_client.subscribe();
        let wait = rx.wait_for(|c| c.is_some());
        match tokio::time::timeout(Duration::from_secs(10), wait).await {
            Ok(Ok(c)) => Ok(c.clone().expect("checked")),
            _ => match self.dir_status() {
                DirStatus::NotConfigured => bail!("no server configured — set one in Settings"),
                DirStatus::Error(e) => bail!("{e}"),
                _ => bail!("server unreachable"),
            },
        }
    }

    /// Looks up a device by ID without connecting.
    pub async fn lookup(&self, id: DeviceId) -> Result<Lookup> {
        self.directory().await?.lookup(id).await
    }

    /// Opens a remote-control session. `on_progress` receives status lines
    /// ("Locating…", "Waiting for approval…") while connecting.
    pub async fn connect(
        &self,
        id: DeviceId,
        password: &str,
        on_progress: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
        on_progress(format!("Locating {id}…"));
        let addr = match self.lookup(id).await? {
            Lookup::Online { addr, .. } => addr,
            Lookup::Offline { info } => bail!("{} ({id}) is offline", info.name),
            Lookup::NotFound => bail!("no device has the ID {id}"),
        };
        if addr.id == self.endpoint_id() {
            bail!("that's this device's own ID");
        }
        let (name, viewer_settings) = {
            let s = self.inner.settings.lock().unwrap();
            (s.device_name.clone(), s.viewer.clone())
        };
        let (session, events) = viewer::connect(
            &self.inner.endpoint,
            addr,
            id,
            password,
            name,
            viewer_settings,
            self.inner.opts.clipboard.clone(),
            Box::new(on_progress),
        )
        .await?;
        let info = session.info().clone();
        self.update_settings(|s| s.remember_connection(id, &info.host_name, &info.os));
        Ok((session, events))
    }

    /// This device's full network address (key + relay + IPs), for
    /// serverless direct connections.
    pub fn endpoint_addr(&self) -> iroh::EndpointAddr {
        self.inner.endpoint.addr()
    }

    /// Connects straight to a known endpoint address, bypassing the
    /// directory (LAN use, or when no server is available).
    pub async fn connect_direct(
        &self,
        addr: iroh::EndpointAddr,
        password: &str,
        on_progress: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
        let (name, viewer_settings) = {
            let s = self.inner.settings.lock().unwrap();
            (s.device_name.clone(), s.viewer.clone())
        };
        viewer::connect(
            &self.inner.endpoint,
            addr,
            DeviceId(0),
            password,
            name,
            viewer_settings,
            self.inner.opts.clipboard.clone(),
            Box::new(on_progress),
        )
        .await
    }

    /// Asks our station for a newer app build. `Ok(None)` = up to date.
    pub async fn check_update(&self) -> Result<Option<update::UpdateInfo>> {
        let dc = self.directory().await?;
        let Some(v) = dc.update_check(&slipstream_core::platform()).await? else { return Ok(None) };
        let info: update::UpdateInfo = serde_json::from_value(v).context("unreadable update info")?;
        Ok(info.is_newer().then_some(info))
    }

    /// Downloads, verifies and installs an update. Returns the app path to
    /// relaunch; the caller restarts.
    pub async fn install_update(&self, info: &update::UpdateInfo, progress: impl FnMut(u64, u64)) -> Result<std::path::PathBuf> {
        let dc = self.directory().await?;
        let staged = update::download(dc.base_url(), dc.key(), info, progress).await?;
        let staged2 = staged.clone();
        tokio::task::spawn_blocking(move || update::install(&staged2)).await?
    }

    pub async fn shutdown(&self) {
        self.disconnect_all_host_sessions();
        // Show as offline right away instead of after the heartbeat window.
        let dc = self.inner.dir_client.borrow().clone();
        if let Some(dc) = dc
            && self.inner.opts.accept_sessions
        {
            let secret = self.inner.endpoint.secret_key().clone();
            let _ = tokio::time::timeout(Duration::from_secs(2), dc.unregister(&secret)).await;
        }
        self.inner.endpoint.close().await;
    }
}

fn set_status(inner: &Inner, status: DirStatus) {
    if *inner.dir_status.borrow() != status {
        inner.dir_status.send_replace(status.clone());
        inner.emit(ClientEvent::Directory(status));
    }
}

/// Keeps this device registered with the directory server (signed
/// heartbeats), reconnecting with backoff and re-registering whenever our
/// network address changes.
async fn directory_task(inner: Arc<Inner>) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let (server, key) = {
            let s = inner.settings.lock().unwrap();
            (s.server.url.clone().filter(|u| !u.trim().is_empty()), s.server.key.clone())
        };
        let Some(server) = server else {
            inner.dir_client.send_replace(None);
            set_status(&inner, DirStatus::NotConfigured);
            inner.server_changed.notified().await;
            continue;
        };
        set_status(&inner, DirStatus::Connecting);
        let changed = inner.server_changed.notified();
        tokio::pin!(changed);
        let result = tokio::select! {
            r = run_directory(&inner, &server, key) => r,
            _ = &mut changed => Ok(()),
        };
        inner.dir_client.send_replace(None);
        match result {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                debug!("directory: {e:#}");
                set_status(&inner, DirStatus::Error(short_error(&e)));
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = &mut changed => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

fn short_error(e: &anyhow::Error) -> String {
    let s = format!("{e}");
    if s.chars().count() > 120 { format!("{}…", s.chars().take(120).collect::<String>()) } else { s }
}

async fn run_directory(inner: &Inner, server: &str, key: Option<String>) -> Result<()> {
    let mut dc = DirectoryClient::new(server, key)?;
    let hello = dc.hello().await?;
    if let Some(m) = hello.motd.filter(|m| !m.trim().is_empty()) {
        inner.emit(ClientEvent::Motd(m));
    }
    inner.emit(ClientEvent::Org(hello.org.clone()));
    let relays = hello.relays;
    let (new_relays, drop_public) = {
        let mut s = inner.settings.lock().unwrap();
        let fresh = s.server.relays != relays
            || s.server.org_name != hello.org
            || s.server.station_allows_public_relays != hello.public_relays;
        let drop_public = s.server.public_relays_enabled() && !hello.public_relays;
        s.server.relays = relays.clone();
        s.server.org_name = hello.org.clone();
        s.server.station_allows_public_relays = hello.public_relays;
        (fresh, drop_public)
    };
    if new_relays {
        inner.save_settings();
        net::add_relays(&inner.endpoint, &relays).await;
    }
    if drop_public {
        net::remove_public_relays(&inner.endpoint).await;
    }
    // Lookups work from here on, even before we register.
    inner.dir_client.send_replace(Some(dc.clone()));
    if !inner.opts.accept_sessions {
        set_status(inner, DirStatus::Ready);
        std::future::pending::<()>().await;
    }

    // Give the endpoint a moment to pick a home relay so our address is useful.
    let _ = tokio::time::timeout(Duration::from_secs(5), inner.endpoint.online()).await;
    let secret = inner.endpoint.secret_key().clone();
    let mut addr_watch = inner.endpoint.watch_addr();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(slipstream_core::directory::HEARTBEAT_SECS));
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {}
            changed = addr_watch.updated() => {
                if changed.is_err() {
                    bail!("endpoint closed");
                }
            }
        }
        let info = DeviceInfo {
            name: inner.settings.lock().unwrap().device_name.clone(),
            os: slipstream_core::os_name(),
            app_version: slipstream_core::APP_VERSION.into(),
        };
        let id = dc.register(&secret, inner.endpoint.addr(), info).await.context("registering")?;
        if *inner.dir_status.borrow() != (DirStatus::Online { id }) {
            info!(%id, "registered with directory");
            let changed = {
                let mut s = inner.settings.lock().unwrap();
                let changed = s.host.last_device_id != Some(id);
                s.host.last_device_id = Some(id);
                changed
            };
            if changed {
                inner.save_settings();
            }
            set_status(inner, DirStatus::Online { id });
        }
    }
}

/// Checks the station for app updates shortly after start, then every 6 h.
async fn update_task(client: Client) {
    update::cleanup();
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut announced: Option<update::UpdateInfo> = None;
    loop {
        match client.check_update().await {
            Ok(Some(info)) if announced.as_ref() != Some(&info) => {
                info!(version = %info.label(), "update available");
                client.inner.emit(ClientEvent::UpdateAvailable(info.clone()));
                announced = Some(info);
            }
            Ok(_) => {}
            Err(e) => debug!("update check: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
    }
}

async fn accept_task(inner: Arc<Inner>) {
    while let Some(incoming) = inner.endpoint.accept().await {
        let inner = inner.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    debug!("incoming connection failed: {e:#}");
                    return;
                }
            };
            if conn.alpn() != ALPN_REMOTE {
                conn.close(0u32.into(), b"unsupported protocol");
                return;
            }
            let linger = conn.clone();
            if let Err(e) = host::run_session(inner, conn).await {
                debug!("host session ended with error: {e:#}");
            }
            // Let a final message (e.g. "wrong password") reach the viewer
            // before the connection is torn down.
            let _ = tokio::time::timeout(Duration::from_secs(3), linger.closed()).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use slipstream_core::proto::InputEvent;

    use super::*;

    async fn client(name: &str, host: bool) -> (Client, mpsc::UnboundedReceiver<ClientEvent>, std::path::PathBuf) {
        client_with(name, host, Default::default(), |_| {}).await
    }

    async fn client_with(
        name: &str,
        host: bool,
        clipboard: slipstream_platform::clipboard::ClipboardSource,
        tweak: impl FnOnce(&mut slipstream_core::config::Settings),
    ) -> (Client, mpsc::UnboundedReceiver<ClientEvent>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("slipstream-e2e-{}-{name}", std::process::id()));
        let store = Store::open(&dir).unwrap();
        let mut s = store.load_settings();
        s.server.use_public_relays = false;
        s.device_name = name.into();
        s.host.set_permanent_password(Some("correct horse battery"));
        tweak(&mut s);
        store.save_settings(&s).unwrap();
        let opts = ClientOptions { test_pattern: host, accept_sessions: host, clipboard };
        let (c, ev) = Client::start(store, opts).await.unwrap();
        (c, ev, dir)
    }

    /// Host and viewer in one process, connected directly: checks auth,
    /// video, and that injected input reaches the host (the test pattern
    /// draws a crosshair where the pointer is).
    #[tokio::test(flavor = "multi_thread")]
    async fn end_to_end_video_and_input() {
        let (host, _hev, hdir) = client("host", true).await;
        let (viewer, _vev, vdir) = client("viewer", false).await;
        let addr = host.endpoint_addr();

        let bad = viewer.connect_direct(addr.clone(), "wrong password", |_| {}).await;
        let err = format!("{:#}", bad.err().unwrap());
        assert!(err.contains("wrong password"), "unexpected error: {err}");

        let (session, _events) = viewer.connect_direct(addr, "correct horse battery", |_| {}).await.unwrap();
        assert_eq!(session.info().host_name, "host");
        let frames = session.frames();

        let wait_frame = |pred: &dyn Fn(&YuvFrame) -> bool| {
            let frames = frames.clone();
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let (_, Some(f)) = frames.latest()
                    && pred(&f)
                {
                    return true;
                }
                if Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let got_video = tokio::task::block_in_place(|| wait_frame(&|f| f.width == 1280 && f.height == 720));
        assert!(got_video, "no video frames arrived");

        // Move the remote pointer to (0.75, 0.25); the crosshair's vertical
        // line should then appear at x = 960 in a decoded frame.
        session.input(InputEvent::MouseMove { x: 0.75, y: 0.25 });
        let moved = tokio::task::block_in_place(|| {
            wait_frame(&|f| {
                let rgba = f.to_rgba();
                let px = |x: usize, y: usize| {
                    let o = (y * 1280 + x) * 4;
                    (rgba[o] as u32 + rgba[o + 1] as u32 + rgba[o + 2] as u32) / 3
                };
                // White crosshair column at x≈960, far from the horizontal line.
                px(960, 600) > 200 && px(900, 600) < 120
            })
        });
        assert!(moved, "pointer movement did not reach the host");

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(hdir);
        let _ = std::fs::remove_dir_all(vdir);
    }

    async fn wait_for<T>(rx: &mut mpsc::UnboundedReceiver<T>, mut pred: impl FnMut(&T) -> bool) -> bool {
        let deadline = tokio::time::sleep(Duration::from_secs(10));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return false,
                ev = rx.recv() => match ev {
                    Some(ev) if pred(&ev) => return true,
                    Some(_) => {}
                    None => return false,
                }
            }
        }
    }

    /// Remote file browsing, upload (incl. subfolders), download, and that
    /// a peer can't escape the target folder.
    #[tokio::test(flavor = "multi_thread")]
    async fn file_manager_end_to_end() {
        use slipstream_core::proto::UploadTarget;
        let (host, mut hev, _) = client("fhost", true).await;
        let (viewer, _vev, _) = client("fviewer", false).await;
        let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        let fs = session.fs();

        let work = std::env::temp_dir().join(format!("slipstream-fm-{}", std::process::id()));
        let remote_dir = work.join("remote");
        let local_dir = work.join("local");
        std::fs::create_dir_all(&remote_dir).unwrap();
        std::fs::create_dir_all(&local_dir).unwrap();
        let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(local_dir.join("big.bin"), &payload).unwrap();

        assert!(!fs.places().await.unwrap().is_empty());

        // Upload a file into a nested folder of the remote dir.
        let mut sent = 0u64;
        let target = UploadTarget::Folder(remote_dir.display().to_string());
        let saved = fs.upload(&local_dir.join("big.bin"), target.clone(), "Photos/2026/big.bin".into(), |n| sent += n).await.unwrap();
        assert_eq!(sent, payload.len() as u64);
        assert_eq!(std::fs::read(remote_dir.join("Photos/2026/big.bin")).unwrap(), payload);
        assert!(saved.ends_with("big.bin"));
        assert!(wait_for(&mut hev, |e| matches!(e, ClientEvent::FileReceived { .. })).await, "host notified");

        // Same name again: kept, not overwritten.
        fs.upload(&local_dir.join("big.bin"), target.clone(), "Photos/2026/big.bin".into(), |_| {}).await.unwrap();
        assert!(remote_dir.join("Photos/2026/big (1).bin").exists());

        // Path traversal is refused.
        assert!(fs.upload(&local_dir.join("big.bin"), target.clone(), "../escape.bin".into(), |_| {}).await.is_err());
        assert!(!work.join("escape.bin").exists());

        // Browse it back and download the whole folder.
        let listing = fs.list(Some(remote_dir.display().to_string())).await.unwrap();
        assert_eq!(listing.entries[0].name, "Photos");
        let photos = format!("{}{}Photos", listing.path, listing.sep);
        let files = fs.walk(photos, true, "Photos".into(), 0).await.unwrap();
        assert_eq!(files.len(), 2);
        for (remote, rel, _) in files {
            let dest = local_dir.join("down").join(&rel);
            fs.download(remote, &dest, |_| {}).await.unwrap();
        }
        assert_eq!(std::fs::read(local_dir.join("down/Photos/2026/big.bin")).unwrap(), payload);

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
        std::fs::remove_dir_all(work).ok();
    }

    /// With file transfer disabled on the host, nothing gets through.
    #[tokio::test(flavor = "multi_thread")]
    async fn file_transfer_can_be_disabled() {
        let (host, _hev, _) = client_with("nfhost", true, Default::default(), |s| s.host.allow_file_transfer = false).await;
        let (viewer, _vev, _) = client("nfviewer", false).await;
        let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        assert!(!session.info().capabilities.file_transfer);
        let err = session.fs().list(None).await.unwrap_err();
        assert!(format!("{err:#}").contains("not allowed"), "{err:#}");
        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
    }

    /// Chat and clipboard text flow both ways.
    #[tokio::test(flavor = "multi_thread")]
    async fn chat_and_clipboard_both_ways() {
        use slipstream_platform::clipboard::ClipboardSource;
        let host_clip = Arc::new(Mutex::new(Some("host start".to_string())));
        let viewer_clip = Arc::new(Mutex::new(Some("viewer start".to_string())));
        let (host, mut hev, _) = client_with("chost", true, ClipboardSource::Memory(host_clip.clone()), |_| {}).await;
        let (viewer, _vev, _) = client_with("cviewer", false, ClipboardSource::Memory(viewer_clip.clone()), |_| {}).await;
        let (session, mut events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();

        session.send(ViewerCmd::Chat("hello host".into()));
        assert!(wait_for(&mut hev, |e| matches!(e, ClientEvent::HostChat { text, .. } if text == "hello host")).await);
        host.host_chat(1, "hello viewer".into());
        assert!(wait_for(&mut events, |e| matches!(e, ViewerEvent::Chat(t) if t == "hello viewer")).await);

        let poll = |clip: &Arc<Mutex<Option<String>>>, want: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if clip.lock().unwrap().as_deref() == Some(want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        // Copy on the viewer → appears on the host, and vice versa.
        *viewer_clip.lock().unwrap() = Some("copied on viewer".into());
        assert!(tokio::task::block_in_place(|| poll(&host_clip, "copied on viewer")), "viewer → host clipboard");
        *host_clip.lock().unwrap() = Some("copied on host".into());
        assert!(tokio::task::block_in_place(|| poll(&viewer_clip, "copied on host")), "host → viewer clipboard");

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
    }
}
