//! Slipstream sessions: the long-running [`Client`] that keeps this device
//! reachable by ID, hosts incoming sessions, and opens outgoing ones.

mod desktop_shell;
mod meeting_preview;
mod public_ip;
mod reconnect;
mod stations;
pub use stations::StationRef;
mod accounts;
pub mod approval;
pub mod files;
pub mod fsops;
pub mod preview;
mod host;
mod incoming;
pub mod meetings;
pub mod network;
pub mod support;
mod network_names;
pub mod tunnel;
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
pub use accounts::{AccountRef, Presence};
pub use files::RemoteFs;
pub use viewer::{AudioOut, FrameShared, ViewerCmd, ViewerEvent, ViewerSession, ViewerStats};

/// Connection state to the directory server.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    PublicDirectory(DirStatus),
    Password(String),
    /// Server message of the day.
    Motd(String),
    /// Name of the organisation running the station, if any.
    Org(Option<String>),
    HostSessionStarted { session: u64, peer: String },
    HostSessionEnded { session: u64, reason: String },
    HostChat { session: u64, text: String },
    HostStatus { session: u64, text: String },
    FileReceived { session: u64, path: std::path::PathBuf },
    Notice(String),
    CallChanged,
    /// A one-time-password link is waiting for Allow, or stopped waiting
    /// (see [`Client::approvals`]).
    LinkApproval,
    /// The station offers a newer version of the app.
    UpdateAvailable(update::UpdateInfo),
}

#[derive(Debug, Clone, Default)]
pub struct ClientOptions {
    /// Production opts into the default public station explicitly. Fixtures
    /// remain local unless they supply their own isolated public station.
    pub public_station: Option<slipstream_core::config::ServerSettings>,
    /// Stream a synthetic test pattern instead of the real screen.
    pub test_pattern: bool,
    /// Accept incoming remote-control sessions.
    pub accept_sessions: bool,
    /// Which clipboard to sync (the system one, or an in-memory one for tests).
    pub clipboard: slipstream_platform::clipboard::ClipboardSource,
    /// Where viewing sessions play sound (the speakers, or memory for tests).
    pub audio_out: viewer::AudioOut,
}

impl ClientOptions {
    pub fn production() -> Self {
        Self { public_station: Some(slipstream_core::config::ServerSettings::public_station()), ..Self::default() }
    }
}

pub(crate) enum HostSessionCmd {
    Chat(String),
    Disconnect,
    Restart,
}

pub(crate) struct Inner {
    pub calls: Arc<meetings::calls::Broker>,
    pub approvals: Arc<approval::Approvals>,
    pub endpoint: Endpoint,
    pub public_station: Option<stations::PublicStation>,
    pub store: Store,
    pub settings: Mutex<Settings>,
    save_lock: Mutex<()>,
    pub opts: ClientOptions,
    events: mpsc::UnboundedSender<ClientEvent>,
    dir_status: watch::Sender<DirStatus>,
    dir_client: watch::Sender<Option<DirectoryClient>>,
    server_changed: tokio::sync::Notify,
    stopping: watch::Sender<bool>,
    pub password: Mutex<String>,
    /// Held for the whole password exchange, so checks run one at a time
    /// (see `host::run_session`).
    pub throttle: tokio::sync::Mutex<AuthThrottle>,
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
        // Serialize snapshots as well as writes. A delayed older snapshot
        // must not replace settings saved after a concurrent mutation.
        let _save=self.save_lock.lock().unwrap();
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
    pub fn call_broker(&self) -> Arc<meetings::calls::Broker> {
        self.inner.calls.clone()
    }

    /// One-time-password links waiting for the person at this computer.
    pub fn approvals(&self) -> Arc<approval::Approvals> {
        self.inner.approvals.clone()
    }

    pub fn camera_for_meeting(
        &self,
        index: u32,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<slipstream_platform::FrameStream> {
        if self.inner.opts.test_pattern {
            use slipstream_platform::Backend;
            return slipstream_platform::test_pattern::TestPattern::new(640, 360).capture(0, 15);
        }
        slipstream_platform::camera::start_cancellable(index, cancelled)
    }

    pub fn meeting_cameras(&self)->Result<Vec<(u32,String)>> {
        if self.inner.opts.test_pattern {return Ok(vec![(0,"Test camera 1".into()),(1,"Test camera 2".into())]);}
        slipstream_platform::camera::devices()
    }

    /// Sharing a screen into a meeting does not inject input or create a viewer.
    pub async fn meeting_displays(&self) -> Result<Vec<slipstream_core::proto::DisplayInfo>> {
        let backend = host::screen_backend(&self.inner).await?;
        Ok(backend.as_ref().context("No screen is available")?.displays())
    }
    pub async fn capture_for_meeting(&self, index:u32) -> Result<slipstream_platform::FrameStream> {
        self.meeting_capture(index,15).await
    }
    async fn meeting_capture(&self,index:u32,fps:u32)->Result<slipstream_platform::FrameStream> {
        let mut backend = host::screen_backend(&self.inner).await?;
        let backend = backend.as_mut().context("No screen is available")?;
        let displays = backend.displays();
        let display = displays.iter().find(|d| d.index==index)
            .context("The selected screen is no longer available; choose a screen again")?;
        backend.capture(display.index, fps)
    }
    /// Loads settings and identity from `store`, binds the network endpoint
    /// and starts background tasks. Must be called inside a tokio runtime.
    pub async fn start(store: Store, opts: ClientOptions) -> Result<(Self, mpsc::UnboundedReceiver<ClientEvent>)> {
        // Bind the persistent profile before loading settings: clone detection
        // may clear a stale station ID. Viewers still use a throwaway network key.
        let persistent = store.identity()?;
        let settings = store.load_settings();
        // The stored identity is this computer's ID, and only the one
        // process that hosts it may use it: two endpoints with one key push
        // each other off the relay. A client that only connects out (the
        // app while the Windows service hosts, command-line tools) gets a
        // throwaway key; nothing relies on a viewer's key staying the same.
        let secret = if opts.accept_sessions { persistent } else { iroh::SecretKey::generate() };
        let mut primary_server = settings.server.clone();
        if opts.public_station.is_some() && !stations::has_custom(&settings) {
            primary_server.relays.clear();
            primary_server.use_public_relays = false;
        }
        let endpoint = net::bind_client(secret, &primary_server, opts.accept_sessions).await?;
        let public_station = match opts.public_station.as_ref() {
            Some(server) => {
                let persistent = store.identity()?;
                let secret = if opts.accept_sessions {
                    slipstream_core::config::public_identity(&persistent)
                } else { iroh::SecretKey::generate() };
                Some(stations::PublicStation::bind(secret, server.clone(), opts.accept_sessions).await?)
            }
            None => None,
        };
        info!(endpoint = %endpoint.id(), "endpoint bound");
        let (events, rx) = mpsc::unbounded_channel();
        let password = generate_password(settings.host.one_time_password_len.clamp(4, 32));
        let inner = Arc::new(Inner {
            calls: Arc::new(meetings::calls::Broker::default()),
            approvals: Arc::new(approval::Approvals::default()),
            endpoint,
            public_station,
            store,
            settings: Mutex::new(settings),
            save_lock: Mutex::new(()),
            opts,
            events,
            dir_status: watch::Sender::new(DirStatus::Connecting),
            dir_client: watch::Sender::new(None),
            server_changed: tokio::sync::Notify::new(),
            stopping: watch::channel(false).0,
            password: Mutex::new(password.clone()),
            throttle: tokio::sync::Mutex::new(AuthThrottle::new()),
            backend: tokio::sync::Mutex::new(None),
            host_sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(0),
        });
        #[cfg(target_os = "android")]
        {
            let broker = Arc::downgrade(&inner.calls);
            slipstream_platform::android::install_call_handler(move |id, answer| {
                if let Some(broker) = broker.upgrade() {
                    broker.answer(id, answer);
                }
            });
        }
        inner.emit(ClientEvent::Password(password));
        spawn_until_shutdown(&inner, directory_task(inner.clone()));
        if inner.public_station.is_some() {
            spawn_until_shutdown(&inner, stations::directory_task(inner.clone()));
        }
        spawn_until_shutdown(&inner, update_task(Client { inner: inner.clone() }));
        if inner.opts.accept_sessions {
            tokio::spawn(accept_task(inner.clone(), inner.endpoint.clone(), false));
            if let Some(public) = &inner.public_station {
                tokio::spawn(accept_task(inner.clone(), public.endpoint.clone(), true));
            }
        }
        Ok((Self { inner }, rx))
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.default_endpoint().id()
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings.lock().unwrap().clone()
    }

    /// Approve screen/input access locally before any viewer connects. The
    /// backend stays available while the host runs, including in the tray.
    /// Returns whether the desktop supplied a permission token for later runs.
    pub async fn prepare_screen_access(&self) -> Result<bool> {
        if !self.inner.opts.accept_sessions || !self.settings().host.enabled {
            bail!("Enable incoming connections before approving screen access");
        }
        let backend = host::screen_backend(&self.inner).await?;
        let backend = backend.as_ref().context("No screen is available")?;
        if backend.displays().is_empty() {
            bail!("No screen is available");
        }
        Ok(backend.restore_token().is_some())
    }

    /// Forget a portal grant and its cached session. Do not interrupt a viewer.
    pub async fn reset_screen_access(&self) -> Result<()> {
        let mut backend = self.inner.backend.lock().await;
        if !self.inner.host_sessions.lock().unwrap().is_empty() {
            bail!("End incoming sessions before changing screen permission");
        }
        *backend = None;
        self.update_settings(|s| s.host.portal_restore_token = None);
        Ok(())
    }

    /// Changes settings and saves them. Reconnects to the directory if the
    /// server changed.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let server_changed = {
            let mut s = self.inner.settings.lock().unwrap();
            let before = (s.server.url.clone(), s.server.key.clone());
            f(&mut s);
            let changed = before != (s.server.url.clone(), s.server.key.clone());
            if changed { s.host.last_device_id = None; }
            changed
        };
        self.inner.save_settings();
        if server_changed {
            self.inner.server_changed.notify_waiters();
        }
    }

    pub fn dir_status(&self) -> DirStatus {
        if !stations::has_custom(&self.settings()) && let Some(public) = &self.inner.public_station {
            return public.status.borrow().clone();
        }
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

    /// Tell connected viewers to wait for the updated host before replacing
    /// its binary. Graceful shutdown must not look like a deliberate End Link.
    pub async fn notify_restarting(&self) {
        for tx in self.inner.host_sessions.lock().unwrap().values() {
            let _=tx.send(HostSessionCmd::Restart);
        }
        let _=tokio::time::timeout(Duration::from_secs(1),async {
            while !self.inner.host_sessions.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
    }

    async fn directory(&self) -> Result<DirectoryClient> {
        if !stations::has_custom(&self.settings()) && self.inner.public_station.is_some() {
            return self.public_directory().await;
        }
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
        let directory = self.directory().await?;
        let station = Some(directory.base_url().to_string());
        let found = directory.lookup(id).await?;
        self.connect_found(id, found, password, on_progress, station).await
    }

    /// The rest of [`connect`](Self::connect), once `id` is looked up (on
    /// this app's station or any other, see `accounts`).
    pub(crate) async fn connect_found(
        &self,
        id: DeviceId,
        found: Lookup,
        password: &str,
        on_progress: impl Fn(String) + Send + Sync + 'static,
        station: Option<String>,
    ) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
        let addr = match found {
            Lookup::Online { addr, .. } => addr,
            Lookup::Offline { info } => bail!("{} ({id}) is offline", info.name),
            Lookup::NotFound => bail!("no device has the ID {id}"),
        };
        let endpoint = self.endpoint_for_address(station.as_deref());
        if addr.id == self.inner.endpoint.id() || self.inner.public_station.as_ref().is_some_and(|p| addr.id == p.endpoint.id()) {
            bail!("that's this device's own ID");
        }
        let (name, viewer_settings) = {
            let s = self.inner.settings.lock().unwrap();
            (s.device_name.clone(), s.viewer.clone())
        };
        let (session, events) = viewer::connect(
            &endpoint,
            addr,
            id,
            password,
            name,
            viewer_settings,
            self.inner.opts.clipboard.clone(),
            self.inner.opts.audio_out.clone(),
            Box::new(on_progress),
        )
        .await?;
        let info = session.info().clone();
        self.update_settings(|s| s.remember_connection(id, &info.host_name, &info.os, station));
        Ok((session, events))
    }

    /// This device's full network address (key + relay + IPs), for
    /// serverless direct connections.
    pub fn endpoint_addr(&self) -> iroh::EndpointAddr {
        self.default_endpoint().addr()
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
            &self.default_endpoint(),
            addr,
            DeviceId(0),
            password,
            name,
            viewer_settings,
            self.inner.opts.clipboard.clone(),
            self.inner.opts.audio_out.clone(),
            Box::new(on_progress),
        )
        .await
    }

    /// Asks our station for a newer app build. `Ok(None)` = up to date.
    pub async fn check_update(&self) -> Result<Option<update::UpdateInfo>> {
        Ok(self.available_update().await?.filter(update::UpdateInfo::is_newer))
    }

    /// The signed station release, also used to bring an older installed
    /// Windows service up to the already-running app's version.
    pub async fn available_update(&self) -> Result<Option<update::UpdateInfo>> {
        update::available(&self.directory().await?, &slipstream_core::release::trusted_keys()).await
    }

    async fn download_latest_update(&self, requested: &update::UpdateInfo, staged: &std::path::Path, progress: impl FnMut(u64, u64)) -> Result<update::UpdateInfo> {
        update::download_latest(&self.directory().await?, requested, staged, &slipstream_core::release::trusted_keys(), progress).await
    }

    /// Downloads into a temporary directory for a privileged installer. The
    /// guard removes staging data on cancellation or any installation failure.
    pub async fn stage_update(&self, info: &update::UpdateInfo, progress: impl FnMut(u64, u64)) -> Result<(update::UpdateInfo, tempfile::TempPath)> {
        let staged = tempfile::NamedTempFile::with_prefix("slipstream-update-")?.into_temp_path();
        let info = self.download_latest_update(info, &staged, progress).await?;
        Ok((info, staged))
    }

    pub async fn install_update(&self, info: &update::UpdateInfo, progress: impl FnMut(u64, u64)) -> Result<std::path::PathBuf> {
        let staged = update::staging_path()?;
        self.download_latest_update(info, &staged, progress).await?;
        self.notify_restarting().await;
        let staged2 = staged.clone();
        tokio::task::spawn_blocking(move || update::install(&staged2)).await?
    }

    pub async fn shutdown(&self) {
        self.inner.stopping.send_replace(true);
        self.disconnect_all_host_sessions();
        // Show as offline right away instead of after the heartbeat window.
        let dc = self.inner.dir_client.borrow().clone();
        if let Some(dc) = dc
            && self.inner.opts.accept_sessions
        {
            let secret = self.inner.endpoint.secret_key().clone();
            let _ = tokio::time::timeout(Duration::from_secs(2), dc.unregister(&secret)).await;
        }
        // Closing waits for peers to acknowledge; one that doesn't mustn't
        // keep the app from exiting (or an update from restarting it).
        if let Some(public) = &self.inner.public_station {
            let dc = public.directory.borrow().clone();
            if self.inner.opts.accept_sessions && let Some(dc) = dc {
                let _ = tokio::time::timeout(Duration::from_secs(2), dc.unregister(public.endpoint.secret_key())).await;
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), public.endpoint.close()).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), self.inner.endpoint.close()).await;
    }
}

fn spawn_until_shutdown(inner: &Arc<Inner>, future: impl std::future::Future<Output = ()> + Send + 'static) {
    let mut stop = inner.stopping.subscribe();
    tokio::spawn(async move {
        tokio::select! { _ = future => {}, _ = stop.wait_for(|value| *value) => {} }
    });
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
            (s.server.url.clone().filter(|u| !u.trim().is_empty()
                && !(inner.public_station.is_some() && slipstream_core::config::is_public_station(u))), s.server.key.clone())
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
    let has_private_key = key.as_deref().is_some_and(|k| !k.is_empty());
    let mut dc = DirectoryClient::new(server, key)?;
    let hello = dc.hello().await?;
    let inventory_enabled = hello.inventory && has_private_key && !slipstream_core::config::is_public_station(server);
    let mut inventory_sent = !inventory_enabled;
    if let Some(m) = hello.motd.filter(|m| !m.trim().is_empty()) {
        inner.emit(ClientEvent::Motd(m));
    }
    inner.emit(ClientEvent::Org(hello.org.clone()));
    let relays = hello.relays;
    let (new_relays, drop_public) = {
        let mut s = inner.settings.lock().unwrap();
        let fresh = s.server.relays != relays
            || s.server.org_name != hello.org
            || s.server.station_allows_public_relays != hello.public_relays
            || s.server.station_approves_links != hello.approve_links;
        let drop_public = s.server.public_relays_enabled() && !hello.public_relays;
        s.server.relays = relays.clone();
        s.server.org_name = hello.org.clone();
        s.server.station_allows_public_relays = hello.public_relays;
        s.server.station_approves_links = hello.approve_links;
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
        if !inventory_sent {
            match tokio::task::spawn_blocking(slipstream_platform::machine::station_inventory).await {
                Ok(inventory) => match dc.publish_inventory(&secret, inventory).await {
                    Ok(()) => inventory_sent = true,
                    Err(e) => debug!("station inventory: {e:#}"),
                },
                Err(e) => debug!("collecting station inventory: {e:#}"),
            }
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

/// Why a connection went away, in words for the UI. Code 0 is how both
/// sides close a session normally.
pub(crate) fn closed_reason(e: &iroh::endpoint::ConnectionError, peer: &str) -> String {
    use iroh::endpoint::ConnectionError;
    match e {
        ConnectionError::ApplicationClosed(c) if c.error_code == 0u32.into() => {
            format!("{peer} ended the session")
        }
        ConnectionError::TimedOut => "connection lost (no response)".into(),
        other => format!("connection lost: {other}"),
    }
}

/// Lets a final message on `send` (a `Bye`) reach the peer before the
/// connection is closed: closing discards anything still unsent.
pub(crate) async fn flush(send: &mut iroh::endpoint::SendStream) {
    send.finish().ok();
    let _ = tokio::time::timeout(Duration::from_millis(500), send.stopped()).await;
}

async fn accept_task(inner: Arc<Inner>, endpoint: Endpoint, public: bool) {
    while let Some(incoming) = endpoint.accept().await {
        let inner = inner.clone();
        let endpoint_id = endpoint.id();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    debug!("incoming connection failed: {e:#}");
                    return;
                }
            };
            if conn.alpn() == slipstream_core::meeting::CALL_ALPN {
                if let Err(error) = meetings::calls::receive(inner, conn, endpoint_id, public).await {
                    debug!("incoming call ended: {error:#}");
                }
                return;
            }
            if conn.alpn() != ALPN_REMOTE {
                conn.close(0u32.into(), b"unsupported protocol");
                return;
            }
            let linger = conn.clone();
            if let Err(e) = host::run_session(inner, conn, endpoint_id, public).await {
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
        // Tests never play through the speakers.
        client_full(name, host, clipboard, AudioOut::Memory(Default::default()), tweak).await
    }

    async fn client_full(
        name: &str,
        host: bool,
        clipboard: slipstream_platform::clipboard::ClipboardSource,
        audio_out: AudioOut,
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
        let opts = ClientOptions { test_pattern: host, accept_sessions: host, clipboard, audio_out, ..Default::default() };
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

    #[tokio::test(flavor="multi_thread")]
    async fn explicit_end_stays_closed_but_update_restart_requests_recovery() {
        let (host,_hev,hdir)=client("deliberate-host",true).await;
        let (viewer,_vev,vdir)=client("deliberate-viewer",false).await;
        for mode in 0..3 {
            let (session,mut events)=viewer.connect_direct(host.endpoint_addr(),"correct horse battery",|_|{}).await.unwrap();
            match mode {0=>host.disconnect_all_host_sessions(),1=>session.send(ViewerCmd::Disconnect),_=>host.notify_restarting().await}
            let retryable=tokio::time::timeout(Duration::from_secs(8),async {
                while let Some(event)=events.recv().await {
                    if let ViewerEvent::Disconnected{retryable,..}=event {return retryable;}
                }
                panic!("missing disconnect event");
            }).await.unwrap();
            assert_eq!(retryable,mode==2,"wrong recovery policy for session end mode {mode}");drop(session);
        }
        viewer.shutdown().await;host.shutdown().await;
        let _=std::fs::remove_dir_all(hdir);let _=std::fs::remove_dir_all(vdir);
    }

    /// Big control messages (clipboard) arrive intact while video and frame
    /// acks are flowing, and the session survives them.
    #[tokio::test(flavor = "multi_thread")]
    async fn large_clipboard_during_video() {
        use slipstream_platform::clipboard::{Clip, ClipboardSource};
        let host_clip = Arc::new(Mutex::new(None::<Clip>));
        let viewer_clip = Arc::new(Mutex::new(None::<Clip>));
        let (host, mut hev, _) = client_with("bhost", true, ClipboardSource::Memory(host_clip.clone()), |_| {}).await;
        let (viewer, _vev, _) = client_with("bviewer", false, ClipboardSource::Memory(viewer_clip.clone()), |_| {}).await;
        let (session, mut events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        assert!(wait_for(&mut events, |e| matches!(e, ViewerEvent::Stats(s) if s.fps > 10.0)).await, "video flowing");

        let poll = |clip: &Arc<Mutex<Option<Clip>>>, want: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if matches!(&*clip.lock().unwrap(), Some(Clip::Text(t)) if t == want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        for round in 0..4 {
            let text: String = (0..600_000).map(|i| char::from(b'a' + ((i * 7 + round) % 26) as u8)).collect();
            *viewer_clip.lock().unwrap() = Some(Clip::Text(text.clone()));
            assert!(tokio::task::block_in_place(|| poll(&host_clip, &text)), "viewer → host, round {round}");
            let back = text.to_uppercase();
            *host_clip.lock().unwrap() = Some(Clip::Text(back.clone()));
            assert!(tokio::task::block_in_place(|| poll(&viewer_clip, &back)), "host → viewer, round {round}");
        }
        session.send(ViewerCmd::Chat("still here".into()));
        assert!(wait_for(&mut hev, |e| matches!(e, ClientEvent::HostChat { text, .. } if text == "still here")).await);

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
    }

    /// The host's sound reaches the viewer (the test pattern host sends a
    /// 440 Hz tone), starts with the link when asked to, and stops on request.
    #[tokio::test(flavor = "multi_thread")]
    async fn audio_end_to_end() {
        let heard = Arc::new(Mutex::new(Vec::<i16>::new()));
        let (host, _hev, _) = client("ahost", true).await;
        let (viewer, _vev, _) =
            client_full("aviewer", false, Default::default(), AudioOut::Memory(heard.clone()), |s| s.viewer.play_audio = true).await;
        let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        assert!(session.info().capabilities.audio);

        let wait = |want: usize| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while heard.lock().unwrap().len() < want && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            heard.lock().unwrap().len()
        };
        // About half a second of audio arrives…
        let got = tokio::task::block_in_place(|| wait(48_000));
        assert!(got >= 48_000, "only {got} samples arrived");
        // …and it's the tone: count zero crossings of the left channel.
        let samples = heard.lock().unwrap().clone();
        let left: Vec<i16> = samples.iter().step_by(2).skip(4800).take(24_000).copied().collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        let hz = crossings as f64 / 2.0 / (left.len() as f64 / 48_000.0);
        assert!((400.0..480.0).contains(&hz), "heard {hz:.0} Hz, not 440");

        // Switching it off stops the flow.
        session.send(ViewerCmd::Audio(false));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let before = heard.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(heard.lock().unwrap().len(), before, "audio kept coming after switching it off");

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
    }

    /// Copied files: pasted from the viewer onto the host's clipboard, and
    /// fetched from the host's clipboard onto the viewer's.
    #[tokio::test(flavor = "multi_thread")]
    async fn clipboard_files_both_ways() {
        use slipstream_platform::clipboard::{Clip, ClipboardSource};
        let host_clip = Arc::new(Mutex::new(None::<Clip>));
        let viewer_clip = Arc::new(Mutex::new(None::<Clip>));
        let (host, _hev, hdir) = client_with("cfhost", true, ClipboardSource::Memory(host_clip.clone()), |_| {}).await;
        let (viewer, _vev, vdir) =
            client_with("cfviewer", false, ClipboardSource::Memory(viewer_clip.clone()), |_| {}).await;
        let (session, mut events) =
            viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();

        let src = vdir.join("to-paste");
        std::fs::create_dir_all(src.join("Folder/sub")).unwrap();
        std::fs::write(src.join("Folder/sub/a.txt"), b"alpha").unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(src.join("big.bin"), &big).unwrap();

        let poll_files = |clip: &Arc<Mutex<Option<Clip>>>| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Some(Clip::Files(f)) = &*clip.lock().unwrap() {
                    return f.clone();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Vec::new()
        };
        let read = |files: &[std::path::PathBuf], name: &str| {
            let p = files
                .iter()
                .find(|p| p.file_name().is_some_and(|n| n == name))
                .unwrap_or_else(|| panic!("{name} in {files:?}"));
            p.to_path_buf()
        };

        // Viewer → host.
        let last = Arc::new(Mutex::new((0, 0)));
        let l = last.clone();
        let n = session
            .paste_files(vec![src.join("Folder"), src.join("big.bin")], move |a, b| *l.lock().unwrap() = (a, b))
            .await
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(*last.lock().unwrap(), (300_005, 300_005), "progress");
        let pasted = tokio::task::block_in_place(|| poll_files(&host_clip));
        assert_eq!(pasted.len(), 2, "{pasted:?}");
        assert_eq!(std::fs::read(read(&pasted, "big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(read(&pasted, "Folder").join("sub/a.txt")).unwrap(), b"alpha");
        assert!(viewer_clip.lock().unwrap().is_none(), "the paste didn't echo back");

        // Host → viewer.
        let there = hdir.join("copied.txt");
        std::fs::write(&there, b"from the host").unwrap();
        *host_clip.lock().unwrap() = Some(Clip::Files(vec![there]));
        let fetched = tokio::task::block_in_place(|| poll_files(&viewer_clip));
        assert_eq!(fetched.len(), 1, "{fetched:?}");
        assert_eq!(std::fs::read(read(&fetched, "copied.txt")).unwrap(), b"from the host");
        assert!(wait_for(&mut events, |e| matches!(e, ViewerEvent::Notice(n) if n.contains("Copied 1 file"))).await);

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(hdir);
        let _ = std::fs::remove_dir_all(vdir);
    }

    /// The viewer can ask what the remote computer is.
    #[tokio::test(flavor = "multi_thread")]
    async fn system_info_end_to_end() {
        let (host, _hev, _) = client("sihost", true).await;
        let (viewer, _vev, _) = client("siviewer", false).await;
        let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        let info = session.system_info().await.unwrap();
        let computer = info.iter().find(|s| s.title == "Computer").expect("a Computer section");
        assert!(computer.rows.iter().any(|(k, v)| k == "Serial number" && !v.is_empty()), "{computer:?}");
        let processor = info.iter().find(|s| s.title == "Processor").expect("a Processor section");
        assert!(processor.rows.iter().any(|(k, v)| k == "Cores" && !v.is_empty()), "{processor:?}");
        assert!(info.iter().any(|s| s.title == "Memory"));
        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
    }

    /// Scan a single address on this test machine through a real authenticated
    /// QUIC session; no other LAN devices are probed by the test.
    #[tokio::test(flavor = "multi_thread")]
    async fn network_scan_end_to_end_and_permissions() {
        use slipstream_core::network::{ScanEvent, ScanRequest};
        let (host, _hev, hdir) = client("nhost", true).await;
        let (viewer, _vev, vdir) = client("nviewer", false).await;
        let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        assert!(session.info().capabilities.network_scan);
        let interfaces = session.fs().network_interfaces().await.unwrap();
        if let Some(interface) = interfaces.first() {
            let listener = tokio::net::TcpListener::bind((interface.address, 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let request = ScanRequest { interface: interface.address, cidr: format!("{}/32", interface.address), ports: vec![port] };
            let mut scan = session.fs().network_scan(request.clone());
            let mut got_device = false;
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    match scan.events.recv().await.expect("scan event").unwrap() {
                        ScanEvent::Device(d) if d.address == interface.address => { assert_eq!(d.open_ports, vec![port]); got_device = true; }
                        ScanEvent::Finished => break,
                        _ => {},
                    }
                }
            }).await.unwrap();
            assert!(got_device);
            let names=session.fs().network_names(interface.address,vec![interface.address]).await.unwrap();
            assert!(names.iter().any(|n|n.address==interface.address&&!n.name.is_empty()));
            let mut invalid = session.fs().network_scan(ScanRequest { cidr: "0.0.0.0/0".into(), ..request.clone() });
            assert!(tokio::time::timeout(Duration::from_secs(5), invalid.events.recv()).await.unwrap().unwrap().is_err());

            let (strict, _sev, sdir) = client_with("nstrict", true, Default::default(), |s| s.host.allow_network_scan = false).await;
            let (restricted, _events) = viewer.connect_direct(strict.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
            assert!(!restricted.info().capabilities.network_scan);
            assert!(restricted.fs().network_interfaces().await.unwrap_err().to_string().contains("not allowed"));
            assert!(restricted.fs().network_names(interface.address,vec![interface.address]).await.unwrap_err().to_string().contains("not allowed"));
            let mut denied = restricted.fs().network_scan(request);
            assert!(tokio::time::timeout(Duration::from_secs(5), denied.events.recv()).await.unwrap().unwrap().unwrap_err().contains("not allowed"));
            drop(restricted);
            strict.shutdown().await;
            let _ = std::fs::remove_dir_all(sdir);
        }
        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(hdir);
        let _ = std::fs::remove_dir_all(vdir);
    }

    /// Invalid requests and view-only sessions never reach a browser launcher.
    #[tokio::test(flavor = "multi_thread")]
    async fn remote_browser_validates_host_requests_and_permissions() {
        use slipstream_core::proto::StreamOpen;
        let (host, _hev, hdir) = client("webhost", true).await;
        let (viewer, _vev, vdir) = client("webviewer", false).await;
        let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        for (address,port) in [(std::net::Ipv4Addr::LOCALHOST,22),(std::net::Ipv4Addr::BROADCAST,443)] {
            assert!(session.fs().ask(StreamOpen::OpenNetworkWeb { address,port }).await.is_err());
        }
        drop(session);
        host.shutdown().await;
        for (name, scan, input) in [("webnoscan",false,true),("webviewonly",true,false)] {
            let (host, _events, dir) = client_with(name,true,Default::default(),|s| {
                s.host.allow_network_scan=scan;s.host.allow_input=input;
            }).await;
            let (session, _events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
            let err = session.fs().ask(StreamOpen::OpenNetworkWeb { address:std::net::Ipv4Addr::LOCALHOST,port:80 }).await.unwrap_err();
            assert!(format!("{err:#}").contains("not allowed"),"{err:#}");
            drop(session);host.shutdown().await;let _=std::fs::remove_dir_all(dir);
        }
        viewer.shutdown().await;
        let _=std::fs::remove_dir_all(hdir);let _=std::fs::remove_dir_all(vdir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn support_requests_enforce_permissions_and_keep_sessions_usable() {
        use slipstream_core::proto::StreamOpen;
        use std::net::Ipv4Addr;
        let (viewer, _ev, vd) = client("supportviewer", false).await;
        for (name, control, scan, tunnels, wake) in [
            ("supportfull", true, true, true, true),
            ("supportnocontrol", false, true, true, true),
            ("supportnoscan", true, false, true, true),
            ("supportnowake", true, true, true, false),
            ("supportnotunnels", true, true, false, true),
            ("supportreadonly", false, false, false, false),
        ] {
            let (host, mut events, hd) = client_with(name, true, Default::default(), |s| {
                s.host.allow_input = control;s.host.allow_network_scan=scan;s.host.allow_tunnels=tunnels;s.host.allow_wake_on_lan=wake;
            }).await;
            let (session, _ev)=viewer.connect_direct(host.endpoint_addr(),"correct horse battery", |_|{}).await.unwrap();
            let fs=session.fs();
            if control {
                fs.support_status("Checking the printer".into()).await.unwrap();
                assert!(wait_for(&mut events,|e| matches!(e,ClientEvent::HostStatus{text,..} if text=="Checking the printer")).await);
                fs.support_status(String::new()).await.unwrap();
                assert!(wait_for(&mut events,|e| matches!(e,ClientEvent::HostStatus{text,..} if text.is_empty())).await);
                assert!(fs.ask(StreamOpen::SupportStatus{text:"bad\nstatus".into()}).await.is_err());
            } else {
                assert!(format!("{:#}",fs.support_status("Checking".into()).await.unwrap_err()).contains("not allowed"));
            }
            let wake_error=fs.wake_on_lan(Ipv4Addr::LOCALHOST,"ff:ff:ff:ff:ff:ff".into()).await.unwrap_err();
            assert!(format!("{wake_error:#}").contains(if wake && scan && control {"unicast"}else{"not allowed"}));
            let diagnostic=fs.network_diagnostics(Ipv4Addr::LOCALHOST,"invalid name!".into(),None).await.unwrap_err();
            assert!(format!("{diagnostic:#}").contains(if scan {"DNS name"}else{"not allowed"}));
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port=listener.local_addr().unwrap().port();
            let server=tokio::spawn(async move {
                while let Ok((mut stream,_))=listener.accept().await {
                    tokio::spawn(async move {let (mut r,mut w)=stream.split();let _=tokio::io::copy(&mut r,&mut w).await;});
                }
            });
            if tunnels {
                use tokio::io::{AsyncReadExt,AsyncWriteExt};
                let tunnel=fs.connect_through(Ipv4Addr::LOCALHOST,port).await.unwrap();
                let mut socket=tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST,tunnel.local_port)).await.unwrap();
                socket.write_all(b"support tunnel").await.unwrap();let mut echoed=[0;14];socket.read_exact(&mut echoed).await.unwrap();assert_eq!(&echoed,b"support tunnel");
                drop(tunnel);
            } else {assert!(format!("{:#}",fs.connect_through(Ipv4Addr::LOCALHOST,port).await.err().unwrap()).contains("not allowed"));}
            server.abort();
            assert!(!fs.support_inventory().await.unwrap().is_empty());
            drop(session);host.shutdown().await;let _=std::fs::remove_dir_all(hd);
        }
        viewer.shutdown().await;let _=std::fs::remove_dir_all(vd);
    }

    /// Port forwarding carries a TCP connection both ways, and is refused
    /// unless the host allows it.
    #[tokio::test(flavor = "multi_thread")]
    async fn tunnel_end_to_end() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // An echo server "on the host's network".
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });

        let (host, _hev, _) = client_with("thost", true, Default::default(), |s| s.host.allow_tunnels = true).await;
        let (viewer, _vev, _) = client("tviewer", false).await;
        let (session, _events) =
            viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        let tunnel = session.tunnel(0, "127.0.0.1".into(), echo_port).await.unwrap();

        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", tunnel.local_port)).await.unwrap();
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
        c.write_all(&payload).await.unwrap();
        c.shutdown().await.unwrap();
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut back)).await.unwrap().unwrap();
        assert_eq!(back, payload, "echo through the tunnel");

        // Closing a forward must close already-connected sockets as well.
        let mut live=tokio::net::TcpStream::connect(("127.0.0.1",tunnel.local_port)).await.unwrap();
        live.write_all(b"live").await.unwrap();let mut echoed=[0u8;4];
        live.read_exact(&mut echoed).await.unwrap();assert_eq!(&echoed,b"live");
        drop(tunnel);
        let result=tokio::time::timeout(Duration::from_secs(3),live.read(&mut echoed)).await.expect("closed tunnel left a connection alive");
        assert!(matches!(result,Ok(0)|Err(_)));

        // IPv6 literals must not be formatted as an ambiguous host:port string.
        if let Ok(echo6)=tokio::net::TcpListener::bind(("::1",0)).await {
            let port=echo6.local_addr().unwrap().port();
            let job=tokio::spawn(async move {
                let (mut socket,_)=echo6.accept().await.unwrap();
                let (mut read,mut write)=socket.split();let _=tokio::io::copy(&mut read,&mut write).await;
            });
            let tunnel6=session.tunnel(0,"::1".into(),port).await.unwrap();
            let mut socket=tokio::net::TcpStream::connect(("127.0.0.1",tunnel6.local_port)).await.unwrap();
            socket.write_all(b"ipv6").await.unwrap();
            tokio::time::timeout(Duration::from_secs(3),socket.read_exact(&mut echoed)).await.unwrap().unwrap();
            assert_eq!(&echoed,b"ipv6");drop(tunnel6);job.abort();
        }

        // A target the host can't reach: the error comes back.
        let dead = session.tunnel(0, "127.0.0.1".into(), 1).await.unwrap();
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", dead.local_port)).await.unwrap();
        let mut buf = [0u8; 1];
        let _ = tokio::time::timeout(Duration::from_secs(10), c.read(&mut buf)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(dead.status().last_error.is_some_and(|e| e.contains("can't reach")), "{:?}", dead.status());
        drop(session);

        // Forwarding off (the default): refused before any port opens.
        let (strict, _sev, _) = client("tstrict", true).await;
        let (session2, _e2) =
            viewer.connect_direct(strict.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        assert!(!session2.info().capabilities.tunnels);
        assert!(session2.tunnel(0, "127.0.0.1".into(), echo_port).await.is_err());

        drop(session2);
        viewer.shutdown().await;
        host.shutdown().await;
        strict.shutdown().await;
    }

    /// Guesses sent in parallel are still counted one by one: only the free
    /// attempts get a verdict, the rest hit the lockout.
    #[tokio::test(flavor = "multi_thread")]
    async fn parallel_guesses_hit_the_lockout() {
        let (host, _hev, hdir) = client("phost", true).await;
        let (viewer, _vev, vdir) = client("pviewer", false).await;
        let addr = host.endpoint_addr();

        let attempts = (0..8).map(|i| {
            let (viewer, addr) = (viewer.clone(), addr.clone());
            tokio::spawn(async move {
                let r = viewer.connect_direct(addr, &format!("guess {i}"), |_| {}).await;
                format!("{:#}", r.err().expect("a guess got in"))
            })
        });
        let errors: Vec<String> = futures_join(attempts).await;
        let locked = errors.iter().filter(|e| e.contains("too many wrong passwords")).count();
        let wrong = errors.iter().filter(|e| e.starts_with("wrong password")).count();
        assert_eq!(wrong + locked, errors.len(), "unexpected errors: {errors:?}");
        assert!(wrong <= 3, "{wrong} guesses were checked: {errors:?}");

        viewer.shutdown().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(hdir);
        let _ = std::fs::remove_dir_all(vdir);
    }

    async fn futures_join<T>(tasks: impl Iterator<Item = tokio::task::JoinHandle<T>>) -> Vec<T> {
        let mut out = Vec::new();
        for t in tasks.collect::<Vec<_>>() {
            out.push(t.await.unwrap());
        }
        out
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

    #[tokio::test(flavor="multi_thread")]
    async fn concurrent_settings_changes_match_the_persisted_snapshot() {
        let (client,_events,dir)=client("settings-writers",false).await;
        let expected=std::thread::scope(|scope| {
            let jobs:Vec<_>=(0..8).map(|_| {
                let client=&client;
                scope.spawn(move || {for _ in 0..10 {client.update_settings(|s|s.device_name.push('x'));}})
            }).collect();
            for job in jobs {job.join().unwrap();}
            client.settings()
        });
        assert_eq!(client.inner.store.load_settings().device_name,expected.device_name);
        client.shutdown().await;let _=std::fs::remove_dir_all(dir);
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
        // A drop resolves once, without needing an input-stream cursor move.
        // Non-Windows hosts use their default receive folder.
        #[cfg(not(windows))]
        assert_eq!(fs.drop_target(500, 300).await.unwrap(), UploadTarget::Folder(crate::fsops::display(&crate::fsops::default_upload_dir())));
        fs.make_dir(remote_dir.join("Empty folder/nested").display().to_string()).await.unwrap();
        assert!(remote_dir.join("Empty folder/nested").is_dir());

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

        // Two uploads can finish into the same requested name concurrently.
        // Neither the staging file nor the final file may be shared/overwritten.
        let request = slipstream_core::proto::StreamOpen::Upload {
            target: target.clone(), rel_path: "concurrent.bin".into(), size: 4,
        };
        let (mut a,mut ar,_) = fs.ask(request.clone()).await.unwrap();
        let (mut b,mut br,_) = fs.ask(request).await.unwrap();
        a.write_all(b"aaaa").await.unwrap(); a.finish().unwrap();
        b.write_all(b"bbbb").await.unwrap(); b.finish().unwrap();
        let saved_a=slipstream_core::framing::expect_msg::<slipstream_core::proto::StreamReply>(&mut ar).await.unwrap();
        let saved_b=slipstream_core::framing::expect_msg::<slipstream_core::proto::StreamReply>(&mut br).await.unwrap();
        let (slipstream_core::proto::StreamReply::Saved {path:a},slipstream_core::proto::StreamReply::Saved {path:b})=(saved_a,saved_b) else {panic!("uploads were not saved")};
        assert_ne!(a,b,"simultaneous uploads overwrote the same destination");
        assert_eq!(std::fs::read(a).unwrap(),b"aaaa");
        assert_eq!(std::fs::read(b).unwrap(),b"bbbb");

        // Path traversal is refused.
        assert!(fs.upload(&local_dir.join("big.bin"), target.clone(), "../escape.bin".into(), |_| {}).await.is_err());
        assert!(!work.join("escape.bin").exists());

        // Browse it back and download the whole folder.
        let listing = fs.list(Some(remote_dir.display().to_string())).await.unwrap();
        assert!(listing.entries.iter().any(|entry| entry.name == "Photos" && entry.is_dir));
        let photos = format!("{}{}Photos", listing.path, listing.sep);
        let files = fs.walk(photos, true, "Photos".into(), 0).await.unwrap();
        assert_eq!(files.len(), 2);
        for (remote, rel, _) in files {
            let dest = local_dir.join("down").join(&rel);
            fs.download(remote, &dest, |_| {}).await.unwrap();
        }
        assert_eq!(std::fs::read(local_dir.join("down/Photos/2026/big.bin")).unwrap(), payload);

        // Passive previews are small, use the same authenticated session,
        // and unsupported files do not break subsequent browsing/transfers.
        let image=image::RgbaImage::from_pixel(600,300,image::Rgba([10,50,200,255]));
        image.save(remote_dir.join("photo.png")).unwrap();
        let preview=fs.preview(remote_dir.join("photo.png").display().to_string()).await.unwrap().unwrap();
        assert!(preview.valid());
        assert!(matches!(preview,slipstream_core::proto::FilePreview::Image {width:128,height:64,..}));
        std::fs::write(remote_dir.join("notes.txt"),"Remote text preview\nSecond line").unwrap();
        assert!(matches!(fs.preview(remote_dir.join("notes.txt").display().to_string()).await.unwrap(),
            Some(slipstream_core::proto::FilePreview::Text(t)) if t.contains("Second line")));
        assert!(fs.preview(remote_dir.join("Photos/2026/big.bin").display().to_string()).await.unwrap().is_none());
        assert!(fs.preview(remote_dir.join("missing.png").display().to_string()).await.unwrap().is_none());
        assert!(!fs.list(Some(remote_dir.display().to_string())).await.unwrap().entries.is_empty());

        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
        std::fs::remove_dir_all(work).ok();
    }

    #[tokio::test(flavor="multi_thread")]
    async fn interrupted_and_oversized_uploads_leave_no_partial_files() {
        use slipstream_core::{proto::{StreamOpen,StreamReply,UploadTarget},framing::expect_msg};
        let (host,_events,hdir)=client("broken-upload-host",true).await;
        let (viewer,_events,vdir)=client("broken-upload-viewer",false).await;
        let (session,_events)=viewer.connect_direct(host.endpoint_addr(),"correct horse battery",|_|{}).await.unwrap();
        let work=tempfile::tempdir().unwrap();let fs=session.fs();
        for mode in ["reset","short","excess"] {
            let (mut send,mut recv,_)=fs.ask(StreamOpen::Upload {
                target:UploadTarget::Folder(work.path().display().to_string()),rel_path:format!("{mode}.bin"),size:if mode=="excess" {1}else{4},
            }).await.unwrap();
            send.write_all(b"xx").await.unwrap();
            if mode=="reset" {send.reset(0u32.into()).unwrap();}else{send.finish().unwrap();}
            let response=tokio::time::timeout(Duration::from_secs(3),expect_msg::<StreamReply>(&mut recv)).await.unwrap().unwrap();
            assert!(matches!(response,StreamReply::Error(_)),"{mode}: {response:?}");
            assert_eq!(std::fs::read_dir(work.path()).unwrap().count(),0,"{mode} left staging files");
        }
        // Different extensions previously used the same staging filename.
        let source=work.path().join("source");std::fs::write(&source,vec![42;1_000_000]).unwrap();
        let target=UploadTarget::Folder(work.path().display().to_string());
        let a=fs.upload(&source,target.clone(),"image.one".into(),|_|{}).await.unwrap();
        let b=fs.upload(&source,target,"image.two".into(),|_|{}).await.unwrap();
        let dest=work.path().join("downloads");std::fs::create_dir(&dest).unwrap();
        let (dest_a,dest_b)=(dest.join("image.one"),dest.join("image.two"));
        let (a,b)=tokio::join!(fs.download(a,&dest_a,|_|{}),fs.download(b,&dest_b,|_|{}));
        for path in [a.unwrap(),b.unwrap()] {assert_eq!(std::fs::read(path).unwrap(),vec![42;1_000_000]);}
        #[cfg(unix)] {
            std::os::unix::fs::symlink(work.path(),work.path().join("cycle")).unwrap();
            let files=tokio::time::timeout(Duration::from_secs(3),fs.walk(work.path().display().to_string(),true,"tree".into(),0)).await.unwrap().unwrap();
            assert_eq!(files.len(),5,"remote symlink traversal repeated a directory");
        }
        drop(session);viewer.shutdown().await;host.shutdown().await;
        let _=std::fs::remove_dir_all(hdir);let _=std::fs::remove_dir_all(vdir);
    }

    #[tokio::test(flavor="multi_thread",worker_threads=2)]
    async fn a_stopped_video_receiver_ends_the_session_instead_of_freezing() {
        use slipstream_core::{auth::ViewerAuth,framing::{expect_msg,write_msg},proto::{HostMsg,ViewerMsg,VideoFrame,StreamSettings,PROTOCOL_VERSION,ALPN_REMOTE}};
        let (host,mut events,hdir)=client("video-stop-host",true).await;
        let (viewer,_events,vdir)=client("video-stop-viewer",false).await;
        let conn=viewer.inner.endpoint.connect(host.endpoint_addr(),ALPN_REMOTE).await.unwrap();
        let (mut send,mut recv)=conn.open_bi().await.unwrap();
        write_msg(&mut send,&ViewerMsg::Hello {version:PROTOCOL_VERSION,viewer_name:"video-stop-test".into()}).await.unwrap();
        let HostMsg::AuthChallenge {pakes}=expect_msg(&mut recv).await.unwrap() else {panic!("no challenge")};
        let (auth,pakes,confirms)=ViewerAuth::respond("correct horse battery",&pakes,&host.inner.endpoint.id(),&viewer.inner.endpoint.id()).unwrap();
        write_msg(&mut send,&ViewerMsg::AuthResponse {pakes,confirms}).await.unwrap();
        let HostMsg::AuthOk {confirm,..}=expect_msg(&mut recv).await.unwrap() else {panic!("not authenticated")};
        auth.verify_host(&confirm).unwrap();
        write_msg(&mut send,&ViewerMsg::StartStream {display:0,settings:StreamSettings {max_in_flight:1,..Default::default()}}).await.unwrap();
        let mut video=tokio::time::timeout(Duration::from_secs(3),conn.accept_uni()).await.unwrap().unwrap();
        // Consume the sole frame without granting more credit. The writer is
        // now waiting for the encoder, so only STOP_SENDING can wake it.
        let _:VideoFrame=tokio::time::timeout(Duration::from_secs(3),expect_msg(&mut video)).await.unwrap().unwrap();
        video.stop(0u32.into()).unwrap();
        let bye=tokio::time::timeout(Duration::from_secs(3),async {
            loop {if let HostMsg::Bye {reason}=expect_msg(&mut recv).await.unwrap() {break reason;}}
        }).await.unwrap();
        assert!(bye.contains("Video stream"),"{bye}");
        conn.close(0u32.into(),b"test complete");
        assert!(wait_for(&mut events,|e|matches!(e,ClientEvent::HostSessionEnded {..})).await);
        viewer.shutdown().await;host.shutdown().await;
        let _=std::fs::remove_dir_all(hdir);let _=std::fs::remove_dir_all(vdir);
    }

    #[tokio::test(flavor="multi_thread",worker_threads=2)]
    async fn meeting_screen_selection_uses_explicit_sparse_id_without_input_or_automatic_capture() {
        use slipstream_platform::{Backend,FrameStream,test_pattern::TestPattern};
        struct Screens {seen:Arc<Mutex<Vec<u32>>>}
        impl Backend for Screens {
            fn name(&self)->&'static str {"meeting-screen-selection"}
            fn displays(&self)->Vec<slipstream_core::proto::DisplayInfo> {
                [7,27].into_iter().map(|index|slipstream_core::proto::DisplayInfo {index,name:format!("Screen {index}"),width:320,height:180,x:0,y:0,primary:index==7}).collect()
            }
            fn capture(&mut self,index:u32,fps:u32)->Result<FrameStream> {self.seen.lock().unwrap().push(index);TestPattern::new(320,180).capture(0,fps)}
            fn inject(&mut self,_index:u32,_event:InputEvent)->Result<()> {panic!("meeting capture must not inject input")}
        }
        let (client,_events,dir)=client("meeting-screen-choice",false).await;
        let seen=Arc::new(Mutex::new(Vec::new()));
        *client.inner.backend.lock().await=Some(Box::new(Screens {seen:seen.clone()}));
        let choices=client.meeting_displays().await.unwrap();
        assert_eq!(choices.iter().map(|d|d.index).collect::<Vec<_>>(),vec![7,27]);
        assert!(seen.lock().unwrap().is_empty(),"listing screens started capture");
        assert!(client.capture_for_meeting(99).await.is_err());
        assert!(seen.lock().unwrap().is_empty(),"invalid choice fell back to another screen");
        let capture=client.capture_for_meeting(27).await.unwrap();
        assert_eq!(*seen.lock().unwrap(),vec![27]);
        let frame=capture.next(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!((frame.width,frame.height),(320,180));
        assert!(client.inner.host_sessions.lock().unwrap().is_empty());
        drop(capture);client.shutdown().await;let _=std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor="multi_thread")]
    async fn abrupt_disconnect_releases_input_and_sparse_display_ids_work() {
        use slipstream_platform::{Backend,FrameStream,test_pattern::TestPattern};
        struct Offset(TestPattern);
        impl Backend for Offset {
            fn name(&self)->&'static str {"offset-test-pattern"}
            fn displays(&self)->Vec<slipstream_core::proto::DisplayInfo> {
                let mut displays=self.0.displays();displays[0].index=7;displays
            }
            fn capture(&mut self,index:u32,fps:u32)->anyhow::Result<FrameStream> {assert_eq!(index,7);self.0.capture(0,fps)}
            fn inject(&mut self,index:u32,event:InputEvent)->anyhow::Result<()> {assert_eq!(index,7);self.0.inject(0,event)}
        }
        let (host,mut events,hdir)=client("release-host",true).await;
        let (viewer,_events,vdir)=client("release-viewer",false).await;
        let backend=TestPattern::new(64,64);let log=backend.input_log();
        *host.inner.backend.lock().await=Some(Box::new(Offset(backend)));
        let (session,mut video)=viewer.connect_direct(host.endpoint_addr(),"correct horse battery",|_|{}).await.unwrap();
        assert!(wait_for(&mut video,|e|matches!(e,ViewerEvent::Stats(stats) if stats.width==64)).await,"non-contiguous display ID did not start video");
        session.input(InputEvent::Key {code:"ControlLeft".into(),down:true});
        let deadline=tokio::time::Instant::now()+Duration::from_secs(3);
        while !log.lock().unwrap().events.iter().any(|event|matches!(event,InputEvent::Key {down:true,..})) {
            assert!(tokio::time::Instant::now()<deadline);tokio::time::sleep(Duration::from_millis(10)).await;
        }
        viewer.inner.endpoint.close().await;
        assert!(wait_for(&mut events,|e|matches!(e,ClientEvent::HostSessionEnded {..})).await);
        assert!(matches!(log.lock().unwrap().events.last(),Some(InputEvent::ReleaseAll)));
        drop(session);viewer.shutdown().await;host.shutdown().await;
        let _=std::fs::remove_dir_all(hdir);let _=std::fs::remove_dir_all(vdir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn temporary_and_unattended_passwords_work_together_and_only_the_code_rotates() {
        let (host, mut events, hdir) = client("dual-password-host", true).await;
        let (first, _events, adir) = client("dual-password-a", false).await;
        let (second, _events, bdir) = client("dual-password-b", false).await;
        let code = host.one_time_password();
        let (permanent, _events) = first.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        let (temporary, _events) = second.connect_direct(host.endpoint_addr(), &code, |_| {}).await.unwrap();
        temporary.send(ViewerCmd::Disconnect);
        assert!(wait_for(&mut events, |e| matches!(e, ClientEvent::Password(password) if password != &code)).await);
        // The first controller remains connected when another code user leaves.
        assert!(!permanent.system_info().await.unwrap().is_empty());
        assert!(second.connect_direct(host.endpoint_addr(), &code, |_| {}).await.is_err());
        let new_code = host.regenerate_password();
        let (new_session, _events) = second.connect_direct(host.endpoint_addr(), &new_code, |_| {}).await.unwrap();
        new_session.send(ViewerCmd::Disconnect);
        permanent.send(ViewerCmd::Disconnect);
        first.shutdown().await;
        second.shutdown().await;
        host.shutdown().await;
        for dir in [hdir, adir, bdir] { let _ = std::fs::remove_dir_all(dir); }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_screen_approval_is_reused_and_reset_does_not_interrupt_viewers() {
        let (host, mut events, hdir) = client("preapproved-host", true).await;
        let (viewer, _events, vdir) = client("preapproved-viewer", false).await;
        assert!(viewer.prepare_screen_access().await.is_err());
        let (first, second) = tokio::join!(host.prepare_screen_access(), host.prepare_screen_access());
        assert!(!first.unwrap()); // Synthetic backend has no desktop restore token.
        assert!(!second.unwrap());
        let backend_address = {
            let backend = host.inner.backend.lock().await;
            std::ptr::from_ref(&**backend.as_ref().unwrap()).cast::<()>() as usize
        };
        let (session, mut video) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();
        assert!(wait_for(&mut video, |e| matches!(e, ViewerEvent::Stats(_))).await);
        assert!(host.reset_screen_access().await.is_err());
        host.prepare_screen_access().await.unwrap();
        {
            let backend = host.inner.backend.lock().await;
            assert_eq!(std::ptr::from_ref(&**backend.as_ref().unwrap()).cast::<()>() as usize, backend_address);
        }
        session.send(ViewerCmd::Disconnect);
        assert!(wait_for(&mut events, |e| matches!(e, ClientEvent::HostSessionEnded { .. })).await);
        host.update_settings(|s| s.host.portal_restore_token = Some("old-desktop-grant".into()));
        host.reset_screen_access().await.unwrap();
        assert!(host.inner.backend.lock().await.is_none());
        assert!(host.inner.store.load_settings().host.portal_restore_token.is_none());
        host.prepare_screen_access().await.unwrap();
        host.update_settings(|s| s.host.enabled = false);
        assert!(host.prepare_screen_access().await.is_err());
        viewer.shutdown().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(hdir);
        let _ = std::fs::remove_dir_all(vdir);
    }

    /// A projection can be revoked after its last viewer leaves. No encoder
    /// then exists to report capture failure, so do not trust the cached backend.
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_cached_screen_backend_is_reopened() {
        struct Revoked;
        impl slipstream_platform::Backend for Revoked {
            fn name(&self)->&'static str {"revoked-screen"}
            fn displays(&self)->Vec<slipstream_core::proto::DisplayInfo> {Vec::new()}
            fn capture(&mut self,_display:u32,_fps:u32)->anyhow::Result<slipstream_platform::FrameStream> {anyhow::bail!("revoked")}
            fn inject(&mut self,_display:u32,_event:InputEvent)->anyhow::Result<()> {Ok(())}
        }
        let (host,_events,hdir)=client("revokedhost",true).await;
        let (viewer,_events,vdir)=client("revokedviewer",false).await;
        *host.inner.backend.lock().await=Some(Box::new(Revoked));
        let (session,_events)=viewer.connect_direct(host.endpoint_addr(),"correct horse battery",|_|{}).await.unwrap();
        assert_eq!(session.info().displays.len(),1);
        assert_eq!(session.info().displays[0].width,1280);
        drop(session);viewer.shutdown().await;host.shutdown().await;
        let _=std::fs::remove_dir_all(hdir);let _=std::fs::remove_dir_all(vdir);
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
        let preview_err=session.fs().preview("unused.png".into()).await.unwrap_err();
        assert!(format!("{preview_err:#}").contains("not allowed"));
        let drop_err = session.fs().drop_target(500, 300).await.unwrap_err();
        assert!(format!("{drop_err:#}").contains("not allowed"), "{drop_err:#}");
        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
    }

    /// Chat and clipboard text flow both ways.
    #[tokio::test(flavor = "multi_thread")]
    async fn chat_and_clipboard_both_ways() {
        use slipstream_platform::clipboard::{Clip, ClipboardSource};
        let host_clip = Arc::new(Mutex::new(Some(Clip::Text("host start".into()))));
        let viewer_clip = Arc::new(Mutex::new(Some(Clip::Text("viewer start".into()))));
        let (host, mut hev, _) = client_with("chost", true, ClipboardSource::Memory(host_clip.clone()), |_| {}).await;
        let (viewer, _vev, _) = client_with("cviewer", false, ClipboardSource::Memory(viewer_clip.clone()), |_| {}).await;
        let (session, mut events) = viewer.connect_direct(host.endpoint_addr(), "correct horse battery", |_| {}).await.unwrap();

        session.send(ViewerCmd::Chat("hello host".into()));
        assert!(wait_for(&mut hev, |e| matches!(e, ClientEvent::HostChat { text, .. } if text == "hello host")).await);
        host.host_chat(1, "hello viewer".into());
        assert!(wait_for(&mut events, |e| matches!(e, ViewerEvent::Chat(t) if t == "hello viewer")).await);

        let poll = |clip: &Arc<Mutex<Option<Clip>>>, want: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if matches!(&*clip.lock().unwrap(), Some(Clip::Text(t)) if t == want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        // Copy on the viewer → appears on the host, and vice versa.
        *viewer_clip.lock().unwrap() = Some(Clip::Text("copied on viewer".into()));
        assert!(tokio::task::block_in_place(|| poll(&host_clip, "copied on viewer")), "viewer → host clipboard");
        *host_clip.lock().unwrap() = Some(Clip::Text("copied on host".into()));
        assert!(tokio::task::block_in_place(|| poll(&viewer_clip, "copied on host")), "host → viewer clipboard");

        // A normal disconnect reaches the host as such, not as a lost link.
        drop(session);
        let mut why = String::new();
        assert!(wait_for(&mut hev, |e| match e {
            ClientEvent::HostSessionEnded { reason, .. } => {
                why = reason.clone();
                true
            }
            _ => false,
        })
        .await);
        assert_eq!(why, "they ended the session");
        viewer.shutdown().await;
        host.shutdown().await;
    }

    /// One-time-password links wait for Allow when the host asks for it;
    /// the unattended password never does. A declined code is replaced.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_time_links_wait_for_approval_and_unattended_links_do_not() {
        let (host, _hev, hdir) = client_with("approve-host", true, Default::default(), |s| s.host.confirm_links = true).await;
        let (viewer, _vev, vdir) = client("approve-viewer", false).await;
        let addr = host.endpoint_addr();
        let approvals = host.approvals();

        // Nobody can answer: declined without waiting for the full minute.
        let code = host.one_time_password();
        let err = format!("{:#}", viewer.connect_direct(addr.clone(), &code, |_| {}).await.err().unwrap());
        assert!(err.contains("Nobody is at the remote computer"), "unexpected error: {err}");
        assert_ne!(host.one_time_password(), code, "a declined code must be replaced");

        // Declined by the person at the computer.
        let receiver = approvals.receiver();
        let answer = |accepted: bool| {
            let approvals = approvals.clone();
            tokio::spawn(async move {
                loop {
                    if let Some(request) = approvals.pending() {
                        assert_eq!(request.peer, "approve-viewer");
                        assert!(!request.public);
                        approvals.answer(request.id, accepted);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
        };
        let code = host.one_time_password();
        let declined = answer(false);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let seen = notices.clone();
        let err = format!("{:#}", viewer.connect_direct(addr.clone(), &code, move |n| seen.lock().unwrap().push(n)).await.err().unwrap());
        declined.await.unwrap();
        assert!(err.contains("declined"), "unexpected error: {err}");
        assert!(notices.lock().unwrap().iter().any(|n| n == approval::WAITING));
        assert_ne!(host.one_time_password(), code);

        // Allowed.
        let code = host.one_time_password();
        let allowed = answer(true);
        let (session, _events) = viewer.connect_direct(addr.clone(), &code, |_| {}).await.unwrap();
        allowed.await.unwrap();
        assert_eq!(session.info().host_name, "approve-host");
        drop(session);
        drop(receiver);

        // The unattended password connects with nobody there to ask.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(approvals.pending().is_none());
        let (session, _events) = viewer.connect_direct(addr, "correct horse battery", |_| {}).await.unwrap();
        assert_eq!(session.info().host_name, "approve-host");
        drop(session);
        viewer.shutdown().await;
        host.shutdown().await;
        let _ = std::fs::remove_dir_all(hdir);
        let _ = std::fs::remove_dir_all(vdir);
    }
}
