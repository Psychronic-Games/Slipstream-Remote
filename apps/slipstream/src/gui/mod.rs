//! The desktop GUI.

#[cfg(target_os = "android")]
mod android;
mod autostart;
mod files;
mod file_previews;
#[cfg(target_os = "linux")]
mod desktop;
mod home;
mod network;
mod keys;
#[cfg(windows)]
mod lite;
mod meetings;
mod remote;
mod session_windows;
mod session_tabs;
mod support_report;
#[cfg(any(windows, test))]
mod support_status;
mod saved;
#[cfg(windows)]
mod scheme;
#[cfg(windows)]
mod softgl;
mod settings;
#[cfg(target_os = "linux")]
mod screen_access;
mod quick_connect;
mod single;
pub mod theme;
mod window_shape;
#[cfg(target_os = "linux")]
mod desktop_drop;
mod touch;
mod tray;
mod video;

use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use anyhow::Result;
use eframe::{
    egui::{self, Align2, Color32, Rect, Ui, UiBuilder, pos2, vec2},
    egui_wgpu,
};
use slipstream_core::{
    config::Store,
    directory::{DeviceId, DirectoryClient},
    provision::{StationConfig, StationLink},
};
use slipstream_session::{Client, ClientEvent, ClientOptions, DirStatus, ViewerEvent, ViewerSession};
use theme::{Corner, pal};
use tokio::sync::{mpsc as tmpsc, oneshot};

#[derive(Debug, Default)]
pub struct GuiOptions {
    pub test_pattern: bool,
    /// (ID, password) to connect to at startup.
    pub connect: Option<(String, String)>,
    /// Save a PNG of the window after N seconds, then exit.
    pub screenshot: Option<(std::path::PathBuf, f64)>,
    pub page: Option<String>,
    /// Start hidden in the tray.
    pub minimized: bool,
    #[cfg(windows)]
    pub service_chat: bool,
    /// Open the file transfer window when a session starts (testing).
    pub open_files: bool,
    /// Initial window size (testing, e.g. full-page screenshots).
    pub window_size: Option<[f32; 2]>,
    /// A `slipstream://link?…` URL the app was opened with.
    pub link: Option<String>,
    /// The Android activity the app runs in.
    #[cfg(target_os = "android")]
    pub android_app: Option<android_activity::AndroidApp>,
}

/// Android: a `slipstream://` link the activity was opened with.
#[cfg(target_os = "android")]
pub use single::inbox::deliver as open_link;
#[cfg(target_os = "android")]
pub use android::{editor_action, set_insets};

pub fn run(opts: GuiOptions) -> Result<()> {
    let store = Store::open_default()?;
    // Only one Slipstream per profile: a second launch just brings the
    // running one to the front.
    #[cfg(not(target_os = "android"))]
    let Some(instance) = single::Instance::acquire(store.dir(), opts.link.as_deref(), opts.minimized) else {
        tracing::info!("Slipstream is already running; showing its window");
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    if let Err(e) = desktop::integrate() {
        tracing::debug!("desktop integration skipped: {e:#}");
    }
    #[cfg(target_os = "linux")]
    if let Err(e) = autostart::refresh_enabled() {
        tracing::warn!("could not refresh autostart: {e:#}");
    }
    #[cfg(windows)]
    if let Err(e) = scheme::register() {
        tracing::debug!("link handler not registered: {e:#}");
    }
    let rt = Arc::new(tokio::runtime::Builder::new_multi_thread().enable_all().build()?);
    // With the Windows service installed, it is this computer's host (one
    // host per computer, one ID): the app then only controls others, and
    // keeps the service running.
    #[cfg(windows)]
    let via_service = crate::service::installed();
    #[cfg(windows)]
    if via_service {
        crate::service::keep_running();
    }
    // The service starts the app hidden at every sign-in, for its tray
    // icon; this user turned that off.
    #[cfg(windows)]
    if via_service && opts.minimized && !opts.service_chat && store.load_settings().app.hide_tray_with_service {
        return Ok(());
    }
    #[cfg(not(windows))]
    let via_service = false;
    // Android registers a persistent host identity too; capture still
    // requires an explicit MediaProjection grant on the phone.
    let accept_sessions = !via_service;
    let (client, events) = rt.block_on(Client::start(store, ClientOptions { test_pattern: opts.test_pattern, accept_sessions, public_station: (!opts.test_pattern).then(slipstream_core::config::ServerSettings::public_station), ..Default::default() }))?;
    #[cfg(windows)]
    let sync_service = via_service && !opts.test_pattern && opts.screenshot.is_none()
        && !crate::service::update_in_progress() && crate::service::needs_app_update().unwrap_or(false);

    // Graphics wgpu can't use (Windows Server 2016's Direct3D 12 can't run
    // the shaders it needs, for one) get the built-in software renderer,
    // and if even that can't draw, the compact native window: the computer
    // can still be controlled.
    #[cfg(windows)]
    {
        let renderer = if std::env::var_os("SLIPSTREAM_LITE").is_some() { Renderer::Compact } else { pick_renderer(&rt) };
        match renderer {
            Renderer::Full => {}
            Renderer::Compact => {
                tracing::warn!("no usable graphics: using the compact window");
                return lite::run(rt, client, events, instance, opts.screenshot, via_service, sync_service);
            }
            Renderer::Relaunch(exe) => {
                // The copy waits for this one to let go of the instance port.
                tracing::info!(exe = %exe.display(), "relaunching beside the software renderer");
                std::process::Command::new(&exe).args(std::env::args_os().skip(1)).env("SLIPSTREAM_RESTARTED", "1").spawn()?;
                drop(instance);
                rt.block_on(client.shutdown());
                return Ok(());
            }
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(theme::transparent_window())
            .with_title("Slipstream Remote")
            .with_app_id("slipstream-remote") // must match the .desktop file name (Wayland icons)
            .with_inner_size(opts.window_size.unwrap_or([1080.0, 700.0]))
            .with_min_inner_size([720.0, 480.0])
            .with_icon(theme::icon(128))
            .with_drag_and_drop(true),
        // winit 0.30 has no native Wayland file-drop events. Keep capture
        // and input on the Wayland portal, but use Xwayland for our windows
        // when available so the desktop can deliver files and folders.
        #[cfg(target_os = "linux")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            if std::env::var_os("DISPLAY").is_some() {
                builder.with_x11();
            }
        })),
        renderer: eframe::Renderer::Wgpu,
        // Ctrl+C/X/V are handled in `raw_input_hook`, which can see files
        // on the clipboard (egui only knows text).
        clipboard_shortcuts: false,
        // Queue one frame, not two: a remote screen is all about latency
        // (this saves a refresh interval, ~17 ms at 60 Hz).
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            surface: eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY,
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(eframe::egui_wgpu::WgpuSetupCreateNew {
                native_adapter_selector: Some(Arc::new(video::pick_adapter)),
                // eframe fills in the display handle.
                ..eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle()
            }),
            ..Default::default()
        },
        #[cfg(target_os = "android")]
        android_app: opts.android_app.clone(),
        ..Default::default()
    };
    let rt2 = rt.clone();
    let client2 = client.clone();
    eframe::run_native(
        "Slipstream Remote",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            #[cfg(target_os = "android")]
            if let Some(app) = &opts.android_app {
                android::install(app.clone(), &cc.egui_ctx);
            }
            let render_state = cc.wgpu_render_state.clone().ok_or("the wgpu renderer is required")?;
            video::install(&render_state);
            let mut app = App::new(cc.egui_ctx.clone(), rt2, client2, events, render_state);
            app.page = match opts.page.as_deref() {
                Some("links") => Page::Links,
                Some("meetings") => Page::Meetings,
                Some("config") => Page::Settings,
                Some("about") => Page::About,
                _ => Page::Home,
            };
            if let Some((id, pw)) = opts.connect {
                app.connect_id = id;
                app.connect_pw = pw;
                app.start_connect(&cc.egui_ctx);
            }
            app.screenshot = opts.screenshot;
            app.via_service = via_service;
            #[cfg(windows)]
            if sync_service { app.check_service_update(&cc.egui_ctx); }
            #[cfg(windows)]
            if via_service { app.service_chat = Some(crate::service_chat::Gui::start(&app.rt,cc.egui_ctx.clone())); }
            let wake = cc.egui_ctx.clone();
            #[cfg(not(target_os = "android"))]
            let requests = instance.listen(move || wake.request_repaint());
            #[cfg(target_os = "android")]
            let requests = single::inbox::open(move || wake.request_repaint());
            app.show_requests = Some(requests);
            // Link to a station: as asked, or (no station yet) from where the
            // browser says this download came from.
            let link = opts.link.as_deref().and_then(StationLink::parse).map(|l| (l, true)).or_else(|| {
                app.client.settings().server.url.is_none().then(|| {
                    slipstream_core::provision::download_origins().iter().find_map(|u| StationLink::parse(u)).map(|l| (l, false))
                })?
            });
            if let Some((link, asked)) = link {
                app.start_link(&cc.egui_ctx, link, asked);
            }
            app.start_hidden = opts.minimized && app.tray.is_some();
            app.open_files = opts.open_files;
            if let Some(link) = opts
                .link
                .filter(|link| link.starts_with("slipstream://meeting"))
            {
                app.meetings.join_text = link;
                app.page = Page::Meetings;
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    rt.block_on(client.shutdown());
    Ok(())
}

/// Narrower than this (a phone held upright), the console is the handheld
/// layout (sections in a row along the bottom) and pages stack their parts.
pub const NARROW: f32 = 600.0;

/// The console's sections: label, page, colour.
const NAV: [(&str, Page, Color32); 5] = [
    ("Bridge", Page::Home, pal::PEACH),
    ("Links", Page::Links, pal::BLUE),
    ("Meetings", Page::Meetings, pal::GREEN),
    ("Config", Page::Settings, pal::VIOLET),
    ("About", Page::About, pal::GOLD),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Links,
    Meetings,
    Settings,
    About,
    Remote(u64),
}

pub struct Toast {
    text: String,
    color: Color32,
    until: Instant,
}

type ConnectResult = Result<(ViewerSession, tmpsc::UnboundedReceiver<ViewerEvent>), String>;
type SessionAction = (u64, remote::TabAction);
type SessionActionQueue = (mpsc::Sender<SessionAction>, mpsc::Receiver<SessionAction>);

pub struct Attempt {
    destination: Option<egui::ViewportId>,
    pub id: DeviceId,
    pub password: String,
    /// Found on this signed-in station (`None`: the app's own).
    pub via: slipstream_session::StationRef,
    pub progress: Arc<Mutex<String>>,
    pub rx: oneshot::Receiver<ConnectResult>,
    pub task: tokio::task::JoinHandle<()>,
}

/// An available app update and its installation progress.
pub struct UpdateState {
    pub info: slipstream_session::update::UpdateInfo,
    pub progress: Arc<Mutex<(u64, u64)>>,
    pub installing: bool,
    pub error: Option<String>,
    rx: Option<oneshot::Receiver<Result<std::path::PathBuf, String>>>,
}

impl UpdateState {
    fn new(info: slipstream_session::update::UpdateInfo) -> Self {
        Self { info, progress: Arc::new(Mutex::new((0, 0))), installing: false, error: None, rx: None }
    }
}

/// Chat with whoever is controlling this computer. Shared with its window,
/// which egui draws on that window's own schedule.
#[derive(Default)]
pub struct HostChat {
    pub log: Vec<(bool, String)>,
    pub input: String,
    pub open: bool,
    /// A message arrived: flash the chat window.
    pub ping: bool,
    pub status: String,
    pub status_open: bool,
}

pub struct IncomingSession {
    pub id: u64,
    pub peer: String,
    pub chat: Arc<Mutex<HostChat>>,
    pub started: Instant,
}

pub struct App {
    pub rt: Arc<tokio::runtime::Runtime>,
    pub client: Client,
    render_state: egui_wgpu::RenderState,
    events: mpsc::Receiver<ClientEvent>,
    t0: Instant,
    pub page: Page,
    pub dir_status: DirStatus,
    pub public_status: DirStatus,
    pub selected_public: bool,
    pub station_draft: (String, String),
    pub primary_station: Option<String>,
    pub service_public_status: Option<DirStatus>,
    pub service_station: Option<String>,
    pub password: String,
    pub service_password: Option<String>,
    pub connect_id: String,
    pub connect_pw: String,
    pub attempt: Option<Attempt>,
    pub tabs: Vec<Arc<Mutex<remote::RemoteTab>>>,
    session_actions: SessionActionQueue,
    session_tabs: Arc<Mutex<session_tabs::Tabs>>,
    quick_connect: quick_connect::QuickConnect,
    meetings: meetings::Meetings,
    next_tab: u64,
    next_window: u64,
    connect_destination: Option<egui::ViewportId>,
    pub incoming: Vec<IncomingSession>,
    toasts: Vec<Toast>,
    pub settings_form: settings::SettingsForm,
    #[cfg(target_os = "linux")]
    screen_access: screen_access::ScreenAccess,
    pub motd: Option<String>,
    /// Organisation running our station, shown in the frame.
    pub org: Option<String>,
    window_focused: bool,
    /// Drawing on the processor (Mesa's llvmpipe, a VM without graphics):
    /// every frame costs real CPU time, so animations run slowly. `None`
    /// until the first frame has looked.
    software: Option<bool>,
    screenshot: Option<(std::path::PathBuf, f64)>,
    pub tray: Option<tray::Tray>,
    tray_status: String,
    quitting: bool,
    start_hidden: bool,
    /// The window is hidden in the tray.
    hidden: bool,
    /// Another launch asked us to show the window (or to link).
    show_requests: Option<mpsc::Receiver<single::Request>>,
    /// Exchanging a setup code with a station: (asked by the user, result).
    link_job: Option<(bool, oneshot::Receiver<Result<StationConfig, String>>)>,
    /// The Windows service hosts this computer (see `crate::service`).
    pub via_service: bool,
    #[cfg(windows)]
    pub service_chat: Option<crate::service_chat::Gui>,
    /// Installing or removing the service: (restart afterwards, result).
    service_job: Option<ServiceJob>,
    /// The Bridge/Config "connect to your station" form.
    pub link_station: String,
    /// Station sign-ins and saved devices.
    saved: saved::Saved,
    pub link_code: String,
    window_title: String,
    logo: Option<egui::TextureHandle>,
    pub update: Option<UpdateState>,
    update_check: Option<oneshot::Receiver<Result<Option<slipstream_session::update::UpdateInfo>, String>>>,
    service_update_check: bool,
    /// Showing the "keeps running in the tray" explanation.
    tray_hint: bool,
    open_files: bool,
}

impl App {
    fn new(
        ctx: egui::Context,
        rt: Arc<tokio::runtime::Runtime>,
        client: Client,
        mut events: tmpsc::UnboundedReceiver<ClientEvent>,
        render_state: egui_wgpu::RenderState,
    ) -> Self {
        // Forward client events to the UI thread and wake it up.
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                if tx.send(ev).is_err() {
                    break;
                }
                wake.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
                wake.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        let settings_form = settings::SettingsForm::from(&client.settings());
        #[cfg(target_os = "linux")]
        let screen_access = {
            let mut access = screen_access::ScreenAccess::default();
            if std::env::var_os("WAYLAND_DISPLAY").is_some() && client.settings().host.enabled {
                access.start(&rt, &client, &ctx, false);
            }
            access
        };
        let tray = tray::Tray::new(&rt, &ctx);
        let org = client.settings().server.org_name.clone();
        Self {
            password: client.one_time_password(),
            service_password: None,
            dir_status: client.dir_status(),
            public_status: client.public_dir_status(),
            selected_public: !client.has_custom_station() && client.public_dir_status() != DirStatus::NotConfigured,
            station_draft: Default::default(),
            primary_station: client.settings().server.url.filter(|url| !slipstream_core::config::is_public_station(url)),
            service_public_status: None,
            service_station: None,
            rt,
            client,
            render_state,
            events: rx,
            t0: Instant::now(),
            page: Page::Home,
            connect_id: String::new(),
            connect_pw: String::new(),
            attempt: None,
            tabs: Vec::new(),
            session_actions: mpsc::channel(),
            session_tabs: Arc::new(Mutex::new(session_tabs::Tabs::default())),
            quick_connect: Default::default(),
            meetings: Default::default(),
            next_tab: 1,
            next_window: 1,
            connect_destination: None,
            incoming: Vec::new(),
            toasts: Vec::new(),
            settings_form,
            #[cfg(target_os = "linux")]
            screen_access,
            motd: None,
            org,
            window_focused: true,
            software: None,
            screenshot: None,
            tray,
            tray_status: String::new(),
            quitting: false,
            start_hidden: false,
            hidden: false,
            show_requests: None,
            link_job: None,
            via_service: false,
            #[cfg(windows)]
            service_chat: None,
            service_job: None,
            link_station: String::new(),
            saved: saved::Saved::default(),
            link_code: String::new(),
            logo: None,
            window_title: String::new(),
            update: None,
            update_check: None,
            service_update_check: false,
            tray_hint: false,
            open_files: false,
        }
    }

    fn set_visible(&mut self, ctx: &egui::Context, visible: bool) {
        self.hidden = !visible;
        if !visible {self.session_tabs.lock().unwrap().hide_target();}
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
    }

    /// How often to redraw for animations alone: the display's rate on a
    /// graphics card, a few times a second when the processor draws.
    pub fn frame_interval(&self) -> Duration {
        if self.software == Some(true) { Duration::from_millis(125) } else { Duration::from_millis(16) }
    }

    pub fn time(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    pub fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast { text: text.into(), color, until: Instant::now() + Duration::from_secs(5) });
    }

    pub fn regenerate_password(&mut self) {
        #[cfg(windows)]
        if self.via_service {
            if let Some(bridge) = &self.service_chat { bridge.sender().regenerate_password(); }
            return;
        }
        self.password = self.client.regenerate_password();
    }

    fn handle_events(&mut self) {
        #[cfg(windows)]
        if let Some(bridge)=&mut self.service_chat {
            use crate::service_chat::Event;
            while let Ok(event)=bridge.events.try_recv() {
                match event {
                    Event::Password(password) => self.service_password = password,
                    Event::Directories { primary, public, station } => {
                        self.service_station = station;
                        self.dir_status = primary.unwrap_or(DirStatus::Connecting);
                        self.service_public_status = public;
                    },
                    Event::MeetingCall(call) => self.meetings.incoming = call,
                    Event::Started { session, peer } => {
                        if !self.incoming.iter().any(|s| s.id == session) {
                            self.incoming.push(IncomingSession {
                                id: session,
                                peer,
                                chat: Default::default(),
                                started: Instant::now(),
                            });
                        }
                    }
                    Event::Status { session, text } => {
                        if let Some(s) = self.incoming.iter().find(|s| s.id == session) {
                            let mut chat = s.chat.lock().unwrap();
                            chat.status_open = !text.is_empty();
                            chat.status = text;
                        }
                    }
                    Event::Ended { session } => self.incoming.retain(|s| s.id != session),
                    Event::Message {
                        session,
                        ours,
                        text,
                    } => {
                        if let Some(session) = self.incoming.iter_mut().find(|s| s.id == session) {
                            let mut chat = session.chat.lock().expect("chat lock");
                            chat.log.push((ours, text));
                            if !ours {
                                chat.open = true;
                                chat.ping = true;
                            }
                        }
                    }
                }
            }
        }

        while let Ok(ev) = self.events.try_recv() {
            match ev {
                ClientEvent::Directory(s) => if self.client.has_custom_station() || self.client.public_dir_status() == DirStatus::NotConfigured { self.dir_status = s; },
                ClientEvent::PublicDirectory(s) => {
                    self.public_status = s.clone();
                    if !self.client.has_custom_station() { self.dir_status = s; }
                },
                ClientEvent::Password(p) => self.password = p,
                ClientEvent::Motd(m) => self.motd = Some(m),
                ClientEvent::Org(o) => self.org = o,
                ClientEvent::HostSessionStarted { session, peer } => {
                    self.toast(format!("{peer} is now connected to this device"), pal::RED);
                    self.incoming.push(IncomingSession {
                        id: session,
                        peer,
                        chat: Default::default(),
                        started: Instant::now(),
                    });
                }
                ClientEvent::HostSessionEnded { session, reason } => {
                    if let Some(s) = self.incoming.iter().find(|s| s.id == session) {
                        let peer = s.peer.clone();
                        self.toast(format!("Link with {peer} closed: {reason}"), pal::LAVENDER);
                    }
                    self.incoming.retain(|s| s.id != session);
                }
                ClientEvent::HostChat { session, text } => {
                    if let Some(s) = self.incoming.iter_mut().find(|s| s.id == session) {
                        let mut chat = s.chat.lock().expect("chat lock");
                        chat.log.push((false, text));
                        chat.open = true;
                        chat.ping = true;
                    }
                }
                ClientEvent::HostStatus { session, text } => if let Some(s) = self.incoming.iter().find(|s| s.id == session) {
                    let mut chat = s.chat.lock().unwrap();chat.status_open = !text.is_empty();chat.status = text;
                },
                ClientEvent::FileReceived { path, .. } => {
                    self.toast(format!("Received {}", path.display()), pal::GREEN);
                }
                ClientEvent::Notice(n) => self.toast(n, pal::GOLD),
                ClientEvent::CallChanged => {}
                ClientEvent::UpdateAvailable(info) => {
                    if self.update.as_ref().is_none_or(|u| !u.installing && u.info != info) {
                        self.update = Some(UpdateState::new(info));
                    }
                }
            }
        }
    }

    /// Starts connecting to `id` in the background.
    pub fn select_public(&mut self, public: bool) {
        if self.selected_public != public {
            std::mem::swap(&mut self.connect_id, &mut self.station_draft.0);
            std::mem::swap(&mut self.connect_pw, &mut self.station_draft.1);
            self.selected_public = public;
        }
    }

    pub fn service_station_matches(&self) -> bool {
        self.service_station.as_deref().is_none_or(|actual| {
            let norm = slipstream_core::directory::normalize_server_url;
            self.client.station_settings().url.as_deref().is_some_and(|expected| norm(expected).ok() == norm(actual).ok())
        })
    }

    pub fn selected_dir_status(&self) -> DirStatus {
        if self.selected_public {
            if self.via_service { self.service_public_status.clone().unwrap_or(DirStatus::Connecting) }
            else { self.public_status.clone() }
        } else if self.via_service && !self.service_station_matches() {
            DirStatus::Error("Apply the saved station settings to the Windows service: Config → Windows service → Apply settings".into())
        } else { self.dir_status.clone() }
    }

    pub fn start_connect(&mut self, ctx: &egui::Context) {
        let id: DeviceId = match self.connect_id.parse() {
            Ok(id) => id,
            Err(e) => {
                self.toast(format!("{e}"), pal::RED);
                return;
            }
        };
        if self.connect_pw.trim().is_empty() {
            self.toast("Enter the partner's password", pal::RED);
            return;
        }
        let password = self.connect_pw.clone();
        let station = if self.selected_public { slipstream_session::StationRef::Public } else { slipstream_session::StationRef::Primary };
        self.start_connect_at(ctx, id, password, station);
    }

    /// Connects to `id`, found on this app's station (`via: None`) or a
    /// signed-in one.
    pub fn start_connect_to(&mut self, ctx: &egui::Context, id: DeviceId, password: String, via: Option<slipstream_session::AccountRef>) {
        self.start_connect_at(ctx, id, password, via.map(slipstream_session::StationRef::Account).unwrap_or_default());
    }

    pub fn start_connect_at(&mut self, ctx: &egui::Context, id: DeviceId, password: String, via: slipstream_session::StationRef) {
        let via = if via == slipstream_session::StationRef::Primary && !self.client.has_custom_station()
            && self.client.public_dir_status() != DirStatus::NotConfigured {
            slipstream_session::StationRef::Public
        } else { via };
        let destination=self.connect_destination.take();
        if self.attempt.is_some() {
            return;
        }
        let progress = Arc::new(Mutex::new(format!("Locating {id}…")));
        let (tx, rx) = oneshot::channel();
        let client = self.client.clone();
        let p = progress.clone();
        let wake = ctx.clone();
        let (pw, station) = (password.clone(), via.clone());
        let task = self.rt.spawn(async move {
            let w2 = wake.clone();
            let progress = move |msg| {
                *p.lock().unwrap() = msg;
                w2.request_repaint();
            };
            let result = client.connect_on(&station, id, &pw, progress).await
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            wake.request_repaint();
        });
        self.attempt = Some(Attempt { destination, id, password, via, progress, rx, task });
    }

    fn poll_attempt(&mut self, ctx: &egui::Context) {
        let Some(attempt) = &mut self.attempt else { return };
        let result = match attempt.rx.try_recv() {
            Ok(r) => r,
            Err(oneshot::error::TryRecvError::Empty) => return,
            Err(oneshot::error::TryRecvError::Closed) => Err("connection attempt failed".into()),
        };
        let attempt = self.attempt.take().expect("checked");
        match result {
            Ok((session, events)) => {
                let id = self.next_tab;
                self.next_tab += 1;
                let play_audio = self.client.settings().viewer.play_audio;
                let mut tab = remote::RemoteTab::new(id, session, events, attempt.password, ctx, play_audio);
                tab.via = attempt.via;
                tab.files.lock().unwrap().open = self.open_files;
                self.toast(format!("Link established with {}", tab.info.host_name), pal::GREEN);
                self.tabs.push(Arc::new(Mutex::new(tab)));
                self.page = Page::Remote(id);
                if let Some(destination)=attempt.destination {
                    self.move_session_tab(ctx,id,destination,None);
                } else if self.client.settings().viewer.separate_windows && !ctx.embed_viewports() {
                    self.open_session_window(ctx, id);
                }
                self.connect_pw.clear();
                self.settings_form.refresh_recent(&self.client.settings());
            }
            Err(e) => {
                tracing::warn!("link to {} failed: {e}", attempt.id);
                self.toast(format!("Link failed: {e}"), pal::RED);
            }
        }
    }

    /// Downloads and installs the pending update in the background.
    pub fn start_update(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if self.service_busy() { self.toast("Wait for the Windows service operation to finish before updating", pal::GOLD);return; }
        let Some(u) = &mut self.update else { return };
        if u.installing {
            return;
        }
        u.installing = true;
        u.error = None;
        let (tx, rx) = oneshot::channel();
        u.rx = Some(rx);
        let (client, info, progress, wake) = (self.client.clone(), u.info.clone(), u.progress.clone(), ctx.clone());
        self.rt.spawn(async move {
            let w = wake.clone();
            let r = crate::update::install(&client, &info, move |done, total| {
                    *progress.lock().unwrap() = (done, total);
                    w.request_repaint_after(Duration::from_millis(100));
                })
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            wake.request_repaint();
        });
    }

    pub fn checking_update(&self) -> bool {
        self.update_check.is_some()
    }

    #[cfg(windows)]
    fn check_service_update(&mut self, ctx: &egui::Context) {
        self.service_update_check = true;
        let (tx, rx) = oneshot::channel();
        self.update_check = Some(rx);
        let (client, wake) = (self.client.clone(), ctx.clone());
        self.rt.spawn(async move {
            let _ = tx.send(client.available_update().await.map_err(|e| format!("{e:#}")));
            wake.request_repaint();
        });
    }

    /// Asks the station for an update right now (About page).
    pub fn check_update_now(&mut self, ctx: &egui::Context) {
        if self.update_check.is_some() || self.update.as_ref().is_some_and(|update| update.installing) {
            return;
        }
        let (client, wake) = (self.client.clone(), ctx.clone());
        let (tx, rx) = oneshot::channel();
        self.update_check = Some(rx);
        self.rt.spawn(async move {
            let _ = tx.send(client.check_update().await.map_err(|e| format!("{e:#}")));
            wake.request_repaint();
        });
    }

    /// Exchanges `link`'s setup code for its station's config, then switches
    /// to that station. `asked`: the user did this (a button, a typed code),
    /// so it replaces a server they typed in by hand and errors are shown.
    pub fn start_link(&mut self, ctx: &egui::Context, link: StationLink, asked: bool) {
        let wake = ctx.clone();
        let (tx, rx) = oneshot::channel();
        self.link_job = Some((asked, rx));
        self.rt.spawn(async move {
            let r = async { DirectoryClient::new(&link.station, None)?.link(&link.code).await }.await;
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            wake.request_repaint();
        });
    }

    pub fn linking(&self) -> bool {
        self.link_job.is_some()
    }

    /// Installs (or removes) the Windows service, through the administrator
    /// prompt; the app restarts afterwards so it hosts (or stops hosting).
    /// `start_first`: an installed, stopped service is just started if
    /// Windows lets this user, and reinstalled only if not.
    #[cfg(windows)]
    pub fn service_action(&mut self, ctx: &egui::Context, install: bool, start_first: bool) {
        if self.update.as_ref().is_some_and(|update| update.installing) {
            self.toast("Wait for the app update to finish before changing the service", pal::GOLD);return;
        }
        if install && self.client.settings().host.permanent_password_secret.is_none() {
            self.toast("Set and save an unattended password first: the service has no one-time passwords", pal::RED);
            return;
        }
        // The elevated process writes what went wrong here.
        let report = std::env::temp_dir().join(format!("slipstream-service-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&report);
        let args = if install {
            match Store::open_default() {
                Ok(store) => format!("service install --from \"{}\" --report \"{}\"", store.dir().display(), report.display()),
                Err(e) => return self.toast(format!("{e:#}"), pal::RED),
            }
        } else {
            format!("service uninstall --report \"{}\"", report.display())
        };
        let (tx, rx) = oneshot::channel();
        // Adding or removing the service changes who hosts: restart then.
        let done = match (install, start_first) {
            (true, true) => "The service is running",
            (true, false) => "The service is installed and running",
            (false, _) => "The service was removed",
        };
        self.service_job = Some((install != self.via_service, done, rx));
        let wake = ctx.clone();
        crate::service::set_busy(true);
        std::thread::spawn(move || {
            if start_first {
                match crate::service::start() {
                    Ok(()) => {
                        crate::service::set_problem(None);
                        crate::service::set_busy(false);
                        let _ = tx.send(Ok(()));
                        wake.request_repaint();
                        return;
                    }
                    Err(e) => tracing::info!("starting the service directly: {e:#}; reinstalling it"),
                }
            }
            let result = match crate::service::run_elevated(&args) {
                // Installed: make sure it's running (the install starts it,
                // but check from here too, and start it if it isn't).
                Ok(0) if install && !crate::service::running() => crate::service::start().map_err(|e| format!("{e:#}")),
                Ok(0) => Ok(()),
                Ok(code) => Err(match std::fs::read_to_string(&report) {
                    Ok(why) if !why.trim().is_empty() => why.trim().to_string(),
                    _ => format!("that didn't work (code {code})"),
                }),
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = std::fs::remove_file(&report);
            if let Err(e) = &result {
                tracing::warn!("service {}: {e}", if install { "install" } else { "removal" });
            }
            crate::service::set_problem(result.as_ref().err().cloned());
            crate::service::set_busy(false);
            let _ = tx.send(result);
            wake.request_repaint();
        });
    }

    #[cfg(windows)]
    pub fn service_busy(&self) -> bool {
        self.service_job.is_some()
    }

    fn poll_service(&mut self, ctx: &egui::Context) {
        let Some((restart, done, rx)) = &mut self.service_job else { return };
        let Ok(result) = rx.try_recv() else { return };
        let (restart, done) = (*restart, *done);
        self.service_job = None;
        match result {
            Ok(()) if !restart => self.toast(done, pal::GREEN),
            Ok(()) => {
                self.toast("Done: restarting Slipstream", pal::GREEN);
                if let Ok(exe) = std::env::current_exe()
                    && std::process::Command::new(exe).env("SLIPSTREAM_RESTARTED", "1").spawn().is_ok()
                {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            // The details stay on the Config page (and the Bridge).
            Err(e) => self.toast(format!("The service: {}", e.lines().next().unwrap_or("")), pal::RED),
        }
    }

    fn poll_link(&mut self) {
        let Some((asked, rx)) = &mut self.link_job else { return };
        let Ok(result) = rx.try_recv() else { return };
        let asked = *asked;
        self.link_job = None;
        match result {
            Ok(cfg) => {
                if slipstream_core::config::is_public_station(&cfg.server) && self.client.has_custom_station() {
                    if asked { self.select_public(true); self.toast("Slipstream Public is available on its tab", pal::GREEN); }
                    return;
                }
                self.client.update_settings(|s| {
                    if asked {
                        s.server.provisioned = true;
                    }
                    slipstream_core::provision::apply(s, &cfg);
                });
                self.settings_form = settings::SettingsForm::from(&self.client.settings());
                self.select_public(false);
                self.dir_status = self.client.dir_status();
                self.org = cfg.org.clone();
                self.link_code.clear();
                let name = cfg.org.unwrap_or_else(|| cfg.server.clone());
                self.toast(format!("Connected to {name}"), pal::GREEN);
            }
            Err(e) if asked => self.toast(format!("Couldn't connect to the station: {e}"), pal::RED),
            Err(e) => tracing::info!("linking from the download address failed: {e}"),
        }
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &mut self.update_check
            && let Ok(r) = rx.try_recv()
        {
            self.update_check = None;
            let sync_service = std::mem::take(&mut self.service_update_check);
            match r {
                Ok(Some(info)) => {
                    self.toast(format!("Slipstream {} is available", info.label()), pal::GREEN);
                    self.update = Some(UpdateState::new(info));
                    if sync_service { self.start_update(ctx); }
                }
                Ok(None) if sync_service => self.toast("The Windows service needs an update, but the station has no signed package available", pal::RED),
                Ok(None) => self.toast("Slipstream is up to date", pal::GREEN),
                Err(e) => self.toast(format!("Update check failed: {e}"), pal::RED),
            }
        }
        let Some(u) = &mut self.update else { return };
        let Some(rx) = &mut u.rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        u.rx = None;
        u.installing = false;
        match result {
            Ok(exe) => {
                // Relaunch the new version; it waits for us to exit first.
                let started = std::process::Command::new(&exe).env("SLIPSTREAM_RESTARTED", "1").spawn();
                match started {
                    Ok(_) => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => u.error = Some(format!("Updated — please restart Slipstream ({e})")),
                }
            }
            Err(e) => u.error = Some(e),
        }
    }

    pub fn cancel_attempt(&mut self) {
        if let Some(a) = self.attempt.take() {
            a.task.abort();
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        self.session_tabs.lock().unwrap().cancel(id);
        if let Some(tab) = self.tab(id) { tab.lock().unwrap().end_link(); }
        self.tabs.retain(|t| t.lock().unwrap().id != id);
        let alive: Vec<u64> = self.tabs.iter().map(|t| t.lock().unwrap().id).collect();
        video::retain_streams(&self.render_state, |s| {
            s >= meetings::STREAM_BASE || alive.contains(&s)
        });
        if self.page == Page::Remote(id) {
            self.page = self.tabs.iter().find_map(|tab| {
                let tab = tab.lock().unwrap();
                (!tab.detached).then_some(Page::Remote(tab.id))
            }).unwrap_or(if self.tabs.is_empty() {Page::Home} else {Page::Links});
        }
    }

    fn tab(&self, id: u64) -> Option<Arc<Mutex<remote::RemoteTab>>> {
        self.tabs.iter().find(|t| t.lock().unwrap().id == id).cloned()
    }

    pub fn open_session_window(&mut self, ctx: &egui::Context, id: u64) {
        let existing=self.tab(id).and_then(|tab|{let tab=tab.lock().unwrap();tab.detached.then(||tab.viewport_id())});
        if let Some(viewport)=existing {
            self.session_action(ctx,id,remote::TabAction::Activate);
            ctx.send_viewport_cmd_to(viewport,egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(viewport,egui::ViewportCommand::Focus);return;
        }
        self.detach_session_window(ctx,id);
    }

    fn detach_session_window(&mut self,ctx:&egui::Context,id:u64) {
        if ctx.embed_viewports() {self.page=Page::Remote(id);return;}
        let viewport=egui::ViewportId::from_hash_of(("session-group",self.next_window));
        self.next_window+=1;
        self.move_session_tab(ctx,id,viewport,None);
    }

    fn close_empty_session_window(&self,ctx:&egui::Context,viewport:egui::ViewportId) {
        if viewport!=egui::ViewportId::ROOT && !self.tabs.iter().any(|t|t.lock().unwrap().viewport_id()==viewport) {
            ctx.send_viewport_cmd_to(viewport,egui::ViewportCommand::Close);
        }
    }

    fn move_session_tab(&mut self,ctx:&egui::Context,id:u64,destination:egui::ViewportId,before:Option<u64>) {
        let Some(tab)=self.tab(id) else {return;};
        let old=tab.lock().unwrap().viewport_id();
        for other in &self.tabs {
            let mut other=other.lock().unwrap();other.release_keyboard();
            if other.viewport_id()==destination {other.window_active=false;}
        }
        {
            let mut tab=tab.lock().unwrap();tab.leave_fullscreen(ctx);
            tab.detached=destination!=egui::ViewportId::ROOT;
            tab.window_group=tab.detached.then_some(destination);tab.window_active=true;tab.window_position=None;tab.update_window_repaint(ctx);
        }
        if let Some(index)=self.tabs.iter().position(|t|Arc::ptr_eq(t,&tab)) {
            let tab=self.tabs.remove(index);
            let index=before.and_then(|other|self.tabs.iter().position(|t|t.lock().unwrap().id==other)).unwrap_or(self.tabs.len());
            self.tabs.insert(index,tab);
        }
        if destination==egui::ViewportId::ROOT {self.page=Page::Remote(id);self.set_visible(ctx,true);}
        else if self.page==Page::Remote(id) {self.page=Page::Links;}
        self.close_empty_session_window(ctx,old);
        ctx.send_viewport_cmd_to(destination,egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(destination,egui::ViewportCommand::Focus);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn session_action(&mut self, ctx: &egui::Context, id: u64, action: remote::TabAction) {
        match action {
            remote::TabAction::None => {},
            remote::TabAction::NewLink(destination) => {
                self.quick_connect.destination=Some(destination);
                for tab in &self.tabs {tab.lock().unwrap().release_keyboard();}
                self.quick_connect.open = true;
                self.quick_connect.focus = true;
                self.quick_connect.via = if self.selected_public { slipstream_session::StationRef::Public } else { slipstream_session::StationRef::Primary };
                self.set_visible(ctx, true);
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            },
            remote::TabAction::Bridge => {
                for tab in &self.tabs {tab.lock().unwrap().release_keyboard();}
                self.page = Page::Home;
                self.set_visible(ctx, true);
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            },
            remote::TabAction::Close => {
                let old=self.tab(id).map(|t|t.lock().unwrap().viewport_id());
                self.close_tab(id);
                if let Some(old)=old {self.close_empty_session_window(ctx,old);}
            },
            remote::TabAction::Detach => self.open_session_window(ctx, id),
            remote::TabAction::Activate => {
                for tab in &self.tabs { tab.lock().unwrap().release_keyboard(); }
                if let Some(tab) = self.tab(id) {
                    let viewport=tab.lock().unwrap().viewport_id();
                    if viewport!=egui::ViewportId::ROOT {
                        for other in &self.tabs {let mut other=other.lock().unwrap();if other.viewport_id()==viewport {other.window_active=other.id==id;}}
                        ctx.request_repaint_of(viewport);
                    } else {self.page=Page::Remote(id);}
                }
            },
            remote::TabAction::FloatAt(position) => {
                self.detach_session_window(ctx, id);
                if let Some(tab) = self.tab(id) {tab.lock().unwrap().window_position = Some(position);}
                if let Some(tab)=self.tab(id) {ctx.send_viewport_cmd_to(tab.lock().unwrap().viewport_id(),egui::ViewportCommand::OuterPosition(position));}
            },
            remote::TabAction::PlaceTab(destination,before) => self.move_session_tab(ctx,id,destination,before),
            remote::TabAction::Dock => self.move_session_tab(ctx,id,egui::ViewportId::ROOT,None),
        }
    }

    // ── Frame ──────────────────────────────────────────────────────────

    fn draw_frame(&mut self, ui: &mut Ui) -> Rect {
        let full = ui.max_rect();
        let t = self.time();
        let painter = ui.painter().clone();
        theme::backdrop(&painter, full, t);
        if full.width() < NARROW {
            return self.draw_frame_narrow(ui, full, t);
        }
        if !cfg!(target_os="android") && (full.width() < 1000.0 || full.height() < 600.0) {
            return self.draw_frame_compact(ui, full);
        }

        let m = 10.0;
        let (l, top, r, b) = (full.left() + m, full.top() + m, full.right() - m, full.bottom() - m);
        let arm_w = 150.0;
        let bar_h = 26.0;
        let gap = 6.0;
        // Short windows (a phone on its side) get slimmer elbows, so the
        // section buttons still fit between them.
        let short = full.height() < 600.0;
        let elbow_w = ((r - l) * 0.34).max(arm_w + 120.0);
        let top_elbow = Rect::from_min_max(pos2(l, top), pos2(l + elbow_w, top + if short { 64.0 } else { 92.0 }));
        let bot_elbow = Rect::from_min_max(pos2(l, b - if short { 48.0 } else { 70.0 }), pos2(l + elbow_w, b));
        theme::elbow(&painter, top_elbow, arm_w, bar_h, Corner::TopLeft, pal::ORANGE);
        theme::elbow(&painter, bot_elbow, arm_w, bar_h, Corner::BottomLeft, pal::LAVENDER);

        // Elbow labels.
        painter.text(
            pos2(l + arm_w - 10.0, top_elbow.bottom() - 6.0),
            Align2::RIGHT_BOTTOM,
            theme::readout(7, t, 6),
            theme::mono(13.0),
            Color32::BLACK,
        );
        painter.text(
            pos2(l + arm_w - 10.0, bot_elbow.top() + 10.0),
            Align2::RIGHT_TOP,
            format!("v{}", slipstream_core::APP_VERSION),
            theme::mono(13.0),
            Color32::BLACK,
        );

        // Title, right-aligned on the top bar.
        let title = "SLIPSTREAM REMOTE";
        let title_galley = painter.layout_no_wrap(title.into(), theme::display_bold(38.0), pal::ORANGE);
        let title_w = title_galley.size().x;
        let title_pos = pos2(r - title_w, top + bar_h / 2.0 - title_galley.size().y / 2.0);
        painter.galley(title_pos, title_galley, pal::ORANGE);
        // The mark, just left of the title.
        let logo_size = 40.0;
        let logo_rect = Rect::from_min_size(pos2(title_pos.x - logo_size - 8.0, top - 2.0), vec2(logo_size, logo_size));
        let tex = self.logo_texture(ui.ctx());
        painter.image(tex, logo_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let title_pos = pos2(logo_rect.left(), title_pos.y);
        if let Some(org) = &self.org {
            painter.text(
                pos2(r, top + bar_h + 14.0),
                Align2::RIGHT_TOP,
                org.to_uppercase(),
                theme::display(17.0),
                pal::LAVENDER,
            );
        }

        // Top bar segments between the elbow and the title.
        let seg_left = top_elbow.right() + gap;
        let seg_right = title_pos.x - 14.0;
        let segs = [(0.45, pal::PEACH), (0.18, pal::VIOLET), (0.37, pal::TAN)];
        bar_segments(&painter, seg_left, seg_right, top, bar_h, gap, &segs, t, true);

        // Bottom bar: segments and live status.
        let (status_text, status_color) = self.status_line();
        let status_galley = painter.layout_no_wrap(status_text, theme::display(18.0), status_color);
        let sw = status_galley.size().x;
        let bseg_left = bot_elbow.right() + gap;
        let status_x = r - sw;
        bar_segments(
            &painter,
            bseg_left,
            status_x - 14.0,
            b - bar_h,
            bar_h,
            gap,
            &[(0.25, pal::BLUE), (0.5, pal::PERIWINKLE), (0.25, pal::GOLD)],
            t,
            false,
        );
        painter.galley(pos2(status_x, b - bar_h / 2.0 - status_galley.size().y / 2.0), status_galley, status_color);

        // Sidebar navigation between the elbows.
        let nav_top = top_elbow.bottom() + gap;
        let nav_bottom = bot_elbow.top() - gap;
        let item_h = if short { 44.0 } else { 54.0 };
        let mut y = nav_top;
        for i in 0..NAV.len() {
            if y + item_h > nav_bottom {
                break;
            }
            self.nav_button(ui, i, Rect::from_min_size(pos2(l, y), vec2(arm_w, item_h)), false);
            y += item_h + gap;
        }
        // Filler block with ticking readouts.
        if nav_bottom - y > 20.0 {
            let filler = Rect::from_min_max(pos2(l, y), pos2(l + arm_w, nav_bottom));
            painter.rect_filled(filler, 2.0, pal::ORANGE.gamma_multiply(0.85));
            let mut ry = filler.top() + 8.0;
            let mut i = 0;
            while ry + 16.0 < filler.bottom() {
                painter.text(
                    pos2(filler.right() - 10.0, ry),
                    Align2::RIGHT_TOP,
                    theme::readout(100 + i, t, 7),
                    theme::mono(12.0),
                    Color32::BLACK.gamma_multiply(0.7),
                );
                ry += 18.0;
                i += 1;
            }
        }

        // Content area: inside the frame (below the organisation name, if shown).
        let content_top = top + bar_h + if self.org.is_some() { 44.0 } else { 22.0 };
        Rect::from_min_max(pos2(l + arm_w + 22.0, content_top), pos2(r, b - bar_h - 16.0))
    }

    /// A smaller console frame for short or narrow desktop windows.
    fn draw_frame_compact(&mut self, ui: &mut Ui, full: Rect) -> Rect {
        let frame = full.shrink(5.0);
        let p = ui.painter().clone();
        let top = frame.with_max_y(frame.top()+31.0);
        p.rect_filled(top, 10.0, pal::PANEL);
        p.rect_filled(top.with_max_x(top.left()+12.0), 6.0, pal::ORANGE);
        p.text(top.left_center()+vec2(21.0,0.0), Align2::LEFT_CENTER, "SLIPSTREAM", theme::display_bold(17.0), pal::ORANGE);
        let start = top.left()+128.0;
        let width = (top.right()-start-3.0*4.0) / NAV.len() as f32;
        for (i,(label,page,color)) in NAV.iter().enumerate() {
            let rect = Rect::from_min_size(pos2(start+i as f32*(width+4.0),top.top()),vec2(width,31.0));
            let active = self.page == *page || (*page == Page::Links && matches!(self.page, Page::Remote(_)));
            let response = ui.interact(rect, ui.scope_id().with(("compact-nav",i)),egui::Sense::click());
            p.rect_filled(rect, 8.0, if active || response.hovered() {*color} else {color.gamma_multiply(0.6)});
            let label = if *page == Page::Links && !self.tabs.is_empty() {format!("{label} · {}",self.tabs.len())} else {label.to_string()};
            p.text(rect.center(),Align2::CENTER_CENTER,label,theme::display(17.0),pal::BG);
            if response.clicked() { self.navigate(*page); }
        }
        let footer = frame.with_min_y(frame.bottom()-21.0);
        p.rect_filled(footer,6.0,pal::PANEL);
        p.rect_filled(footer.with_max_x(footer.left()+45.0),6.0,pal::LAVENDER);
        let (text,color) = self.status_line();
        p.with_clip_rect(footer.shrink(4.0)).text(footer.right_center()-vec2(6.0,0.0),Align2::RIGHT_CENTER,text,theme::display(14.0),color);
        Rect::from_min_max(pos2(frame.left()+3.0,top.bottom()+7.0),pos2(frame.right()-3.0,footer.top()-6.0))
    }

    fn navigate(&mut self, page: Page) {
        let single = self.tabs.first().filter(|_| self.tabs.len() == 1).and_then(|tab| {
            let tab = tab.lock().unwrap();
            (!tab.detached).then_some(tab.id)
        });
        self.page = if page == Page::Links { single.map(Page::Remote).unwrap_or(page) } else { page };
    }

    /// Section button `i` of [`NAV`]; `up`: in a row below the content.
    fn nav_button(&mut self, ui: &mut Ui, i: usize, rect: Rect, up: bool) {
        let (label, page, color) = NAV[i];
        let active = self.page == page || (page == Page::Links && matches!(self.page, Page::Remote(_)));
        let mut text = label.to_string();
        if page == Page::Links && !self.tabs.is_empty() {
            text = format!("{label} · {}", self.tabs.len());
        }
        if theme::block_button_pointing(ui, rect, &text, &format!("{:02}-{}", i + 1, 40 + i * 13), color, active, up).clicked() {
            self.navigate(page);
        }
    }

    /// The frame on a phone held upright, laid out like a handheld
    /// scanner: the title and a sensor sweep at the top, the sections as a
    /// row of buttons at the bottom (within reach of a thumb), the status
    /// along the bottom edge.
    fn draw_frame_narrow(&mut self, ui: &mut Ui, full: Rect, t: f64) -> Rect {
        let painter = ui.painter().clone();
        let m = 8.0;
        let (l, top, r, b) = (full.left() + m, full.top() + m, full.right() - m, full.bottom() - m);
        let (arm_w, bar_h, gap) = (48.0, 22.0, 5.0);

        // Header: an elbow, the title on its bar, a sweep and the
        // organisation under it.
        let elbow = Rect::from_min_max(pos2(l, top), pos2(l + arm_w + 36.0, top + 58.0));
        theme::elbow(&painter, elbow, arm_w, bar_h, Corner::TopLeft, pal::ORANGE);
        let title = painter.layout_no_wrap("SLIPSTREAM REMOTE".into(), theme::display_bold(26.0), pal::ORANGE);
        let title_pos = pos2(r - title.size().x, top + bar_h / 2.0 - title.size().y / 2.0);
        let logo = Rect::from_min_size(pos2(title_pos.x - 30.0, top - 2.0), vec2(26.0, 26.0));
        painter.galley(title_pos, title, pal::ORANGE);
        let tex = self.logo_texture(ui.ctx());
        painter.image(tex, logo, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        bar_segments(&painter, elbow.right() + gap, logo.left() - 8.0, top, bar_h, gap, &[(0.6, pal::PEACH), (0.4, pal::TAN)], t, true);
        let band = Rect::from_min_max(pos2(l + arm_w + 10.0, top + bar_h + 8.0), pos2(r, elbow.bottom()));
        let mut sweep_right = band.right();
        if let Some(org) = &self.org {
            let g = painter.layout_no_wrap(org.to_uppercase(), theme::display(15.0), pal::LAVENDER);
            sweep_right -= g.size().x + 10.0;
            painter.galley(pos2(r - g.size().x, band.center().y - g.size().y / 2.0), g, pal::LAVENDER);
        }
        if sweep_right - band.left() > 40.0 {
            theme::sweep(&painter, Rect::from_min_max(band.min, pos2(sweep_right, band.bottom())), t, pal::CYAN);
        }

        // Footer: the status bar, and the section buttons above it.
        let (status_text, status_color) = self.status_line();
        let status = painter.layout_no_wrap(status_text, theme::display(15.0), status_color);
        let status_x = r - status.size().x;
        bar_segments(&painter, l, status_x - 10.0, b - bar_h, bar_h, gap, &[(0.3, pal::LAVENDER), (0.4, pal::BLUE), (0.3, pal::GOLD)], t, false);
        painter.galley(pos2(status_x, b - bar_h / 2.0 - status.size().y / 2.0), status, status_color);
        let pad_h = 52.0;
        let pad_y = b - bar_h - gap - pad_h;
        let w = (r - l - gap * (NAV.len() - 1) as f32) / NAV.len() as f32;
        for i in 0..NAV.len() {
            self.nav_button(ui, i, Rect::from_min_size(pos2(l + i as f32 * (w + gap), pad_y), vec2(w, pad_h)), true);
        }

        Rect::from_min_max(pos2(l + 2.0, elbow.bottom() + 12.0), pos2(r, pad_y - 20.0))
    }

    /// The Slipstream mark as a texture (rendered once).
    pub fn logo_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.logo
            .get_or_insert_with(|| {
                let img = theme::render_logo(128, false);
                let color = egui::ColorImage::from_rgba_unmultiplied([128, 128], &img.rgba);
                ctx.load_texture("slipstream-logo", color, egui::TextureOptions::LINEAR)
            })
            .id()
    }

    fn status_line(&self) -> (String, Color32) {
        let incoming = if self.incoming.is_empty() {
            String::new()
        } else {
            format!("  ·  {} INBOUND", self.incoming.len())
        };
        match &self.selected_dir_status() {
            DirStatus::Online { id } => (format!("STATION ONLINE  ·  ID {id}{incoming}"), pal::GREEN),
            DirStatus::Connecting => ("CONTACTING RELAY NETWORK…".into(), pal::GOLD),
            DirStatus::Ready => ("SERVER LINK READY".into(), pal::GREEN),
            DirStatus::NotConfigured => ("NO SERVER CONFIGURED".into(), pal::RED),
            DirStatus::Error(_) => (format!("SERVER CONNECTION FAILED{incoming}"), pal::RED),
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.until > Instant::now());
        if self.toasts.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 60.0;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("toasts")));
        for toast in self.toasts.iter().rev().take(4) {
            let galley = painter.layout(toast.text.clone(), theme::body(16.0), pal::TEXT, 520.0);
            let size = galley.size() + vec2(48.0, 18.0);
            let rect = Rect::from_min_size(pos2(screen.right() - size.x - 24.0, y - size.y), size);
            theme::glow(&painter, rect, 8.0, toast.color, 1.0);
            painter.rect_filled(rect, 8.0, pal::PANEL);
            painter.rect_filled(Rect::from_min_size(rect.min, vec2(10.0, rect.height())), egui::CornerRadius { nw: 8, sw: 8, ..Default::default() }, toast.color);
            painter.galley(rect.min + vec2(26.0, 9.0), galley, pal::TEXT);
            y -= size.y + 10.0;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn draw_tray_hint(&mut self, ctx: &egui::Context) {
        if !self.tray_hint {
            return;
        }
        let (mut hide, mut quit) = (false, false);
        let mut close=false;
        let result=theme::modal("tray-hint",pal::LAVENDER).show(ctx, |ui| {
            ui.set_width(theme::dialog_width(ctx,460.0));
            let chrome=theme::window_header(ui,"Keep running in the tray?",pal::LAVENDER);
            close=chrome.close;
            ui.add_space(8.0);
            ui.label("Slipstream is set to keep running in the system tray when its window is closed, so this computer stays reachable. Use the tray icon to open it again or to quit.");
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                hide = theme::pill(ui, "Hide to tray", pal::LAVENDER).clicked();
                quit = theme::pill(ui, "Quit Slipstream", pal::RED).clicked();
            });
            chrome.footer(ui);
        });
        if close || result.should_close() {self.tray_hint=false;}
        if hide || quit {
            self.tray_hint = false;
            self.client.update_settings(|s| s.app.tray_hint_shown = true);
        }
        if hide {
            self.set_visible(ctx, false);
        }
        if quit {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn draw_attempt_overlay(&mut self, ctx: &egui::Context) {
        let Some(a) = &self.attempt else { return };
        let progress = a.progress.lock().unwrap().clone();
        let id = a.id;
        let t = self.time();
        let mut cancel = false;
        let result=theme::modal("connecting",pal::BLUE).show(ctx, |ui| {
            ui.set_width(theme::dialog_width(ctx,420.0));
            let chrome=theme::window_header(ui,"Establishing link",pal::BLUE);
            cancel=chrome.close;
            ui.label(egui::RichText::new(format!("PARTNER {id}")).font(theme::mono(15.0)).color(pal::BLUE));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                theme::spinner(ui.painter(), rect.center(), 14.0, pal::CYAN, t);
                ui.add(egui::Label::new(egui::RichText::new(progress).font(theme::body(18.0))).wrap());
            });
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::pill(ui, "Abort", pal::RED).clicked() {
                    cancel = true;
                }
            });
            chrome.footer(ui);
        });
        if cancel || result.should_close() {
            self.cancel_attempt();
        }
        ctx.request_repaint_after(self.frame_interval().max(Duration::from_millis(33)));
    }
}

impl App {
    fn maybe_screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, delay)) = &self.screenshot else { return };
        if self.time() < *delay {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        let path = path.clone();
        self.screenshot = None;
        let exit = ctx.clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
            if let Err(e) = save_png(&path, &img) {
                eprintln!("screenshot failed: {e}");
            } else {
                eprintln!("screenshot saved to {}", path.display());
            }
            exit.send_viewport_cmd(egui::ViewportCommand::Close);
        })));
    }
}

#[cfg(windows)]
enum Renderer {
    Full,
    Compact,
    /// Run this copy of the app instead (see `softgl::prepare_beside`).
    Relaunch(std::path::PathBuf),
}

/// Decides how the full window draws: Direct3D 12 or Vulkan if wgpu can use
/// them, else the system's OpenGL (checked in a separate process, so its
/// opengl32.dll never loads into this one), else the built-in software
/// renderer: loaded here, or failing that from a copy of the app beside it.
#[cfg(windows)]
fn pick_renderer(rt: &tokio::runtime::Runtime) -> Renderer {
    softgl::note_build();
    if pick_hardware_or_software(rt) {
        return Renderer::Full;
    }
    // The software renderer didn't draw from here: try the copy beside it
    // (once: the copy itself goes to the compact window).
    if softgl::running_beside() || softgl::problem().is_none() || softgl::failed_before() {
        return Renderer::Compact;
    }
    match softgl::prepare_beside() {
        Ok(exe) => {
            softgl::retry();
            Renderer::Relaunch(exe)
        }
        Err(e) => {
            softgl::set_problem(format!("{e:#}"));
            Renderer::Compact
        }
    }
}

#[cfg(windows)]
fn pick_hardware_or_software(rt: &tokio::runtime::Runtime) -> bool {
    use eframe::egui_wgpu::wgpu::Backends;
    // SLIPSTREAM_SOFTWARE_RENDERER=1 goes straight to it (troubleshooting).
    let forced = std::env::var_os("SLIPSTREAM_SOFTWARE_RENDERER").is_some();
    if !forced && count_adapters(rt, Backends::DX12 | Backends::VULKAN) > 0 {
        return true;
    }
    if !forced && system_gl_works() {
        tracing::info!("using the system's OpenGL");
        return true;
    }
    if let Err(e) = softgl::prepare() {
        softgl::set_problem(format!("{e:#}"));
        return false;
    }
    // SLIPSTREAM_TEST_BESIDE=1: fail here, to test the copy beside Mesa.
    if std::env::var_os("SLIPSTREAM_TEST_BESIDE").is_some() && !softgl::running_beside() {
        softgl::set_problem("test: not from here".into());
        softgl::retry();
        return false;
    }
    if !use_software_gl() {
        softgl::retry();
        return false;
    }
    if count_adapters(rt, Backends::GL) > 0 {
        return true;
    }
    softgl::set_problem(gl_problem());
    softgl::retry(); // couldn't even start: not a crash, try again next time
    false
}

/// Why OpenGL (here: the software renderer) offers wgpu nothing, from the
/// OpenGL backend itself.
#[cfg(windows)]
fn gl_problem() -> String {
    use wgpu_hal::Instance as _;
    let desc = wgpu_hal::InstanceDescriptor {
        name: "slipstream",
        flags: eframe::egui_wgpu::wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        telemetry: None,
        display: None,
    };
    // SAFETY: creating and dropping a throwaway OpenGL instance.
    match unsafe { wgpu_hal::gles::Instance::init(&desc) } {
        Err(e) => format!("the software renderer's OpenGL didn't start ({e})"),
        Ok(instance) => match unsafe { instance.enumerate_adapters(None) }.len() {
            0 => "the software renderer's OpenGL started but offered no usable adapter".into(),
            n => format!("the software renderer has {n} OpenGL adapter(s), but they weren't listed"),
        },
    }
}

/// Runs `slipstream` again with SLIPSTREAM_PROBE_GL set ([`probe_gl`]) and
/// waits up to 10 s for its verdict.
#[cfg(windows)]
fn system_gl_works() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let Ok(mut child) = std::process::Command::new(exe).env("SLIPSTREAM_PROBE_GL", "1").spawn() else { return false };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return false;
            }
        }
    }
}

/// The probe process: exit code 0 when the system's OpenGL is enough for
/// wgpu (a real driver; the GDI one that ships with Windows isn't).
#[cfg(windows)]
pub fn probe_gl() -> i32 {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else { return 1 };
    if count_adapters(&rt, eframe::egui_wgpu::wgpu::Backends::GL) > 0 { 0 } else { 1 }
}

/// Makes Mesa's `opengl32.dll` (unpacked by `softgl::prepare`) the one this
/// process uses, with its CPU renderer.
#[cfg(windows)]
fn use_software_gl() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{LoadLibraryW, SetDllDirectoryW};
    let dir = softgl::dir();
    let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    // SAFETY: NUL-terminated paths. Setting the variable is sound: nothing
    // else reads the environment while the app starts up.
    let loaded = unsafe {
        std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
        // Mesa's opengl32.dll finds libgallium_wgl.dll through this folder.
        SetDllDirectoryW(wide(&dir).as_ptr());
        // Loaded by full path now, it's the opengl32.dll that wgpu's later
        // by-name loads get (opengl32 is delay-loaded, see build.rs).
        !LoadLibraryW(wide(&dir.join("opengl32.dll")).as_ptr()).is_null()
    };
    if !loaded {
        let e = std::io::Error::last_os_error();
        softgl::set_problem(format!("Windows wouldn't load the software renderer ({e})"));
        return false;
    }
    tracing::info!(dir = %dir.display(), "using the built-in software renderer");
    true
}

/// How many adapters wgpu finds on these backends.
#[cfg(windows)]
fn count_adapters(rt: &tokio::runtime::Runtime, backends: eframe::egui_wgpu::wgpu::Backends) -> usize {
    use eframe::egui_wgpu::{WgpuSetupCreateNew, wgpu};
    let instance = wgpu::Instance::new(WgpuSetupCreateNew::without_display_handle().instance_descriptor);
    let adapters = rt.block_on(instance.enumerate_adapters(backends));
    for a in &adapters {
        let info = a.get_info();
        tracing::info!(name = %info.name, backend = ?info.backend, kind = ?info.device_type, "graphics adapter found");
    }
    adapters.len()
}

/// "Slipstream Remote - Acme IT" (matches the station's download names).
fn window_title(org: Option<&str>) -> String {
    match org.map(str::trim).filter(|o| !o.is_empty() && !o.eq_ignore_ascii_case("Slipstream Remote")) {
        Some(o) => format!("Slipstream Remote - {o}"),
        None => "Slipstream Remote".into(),
    }
}

/// Writes the app icon as PNG, or as a multi-size Windows `.ico` if the
/// path ends in `.ico`.
pub fn export_icon(path: &std::path::Path, size: u32) -> Result<()> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ico")) {
        return std::fs::write(path, ico_bytes(&[16, 24, 32, 48, 64, 128, 256])?).map_err(Into::into);
    }
    let icon = theme::icon(size);
    let img = egui::ColorImage::from_rgba_unmultiplied([size as usize, size as usize], &icon.rgba);
    save_png(path, &img)
}

/// An .ico with PNG-compressed images (supported since Windows Vista).
fn ico_bytes(sizes: &[u32]) -> Result<Vec<u8>> {
    let pngs: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| {
            let icon = theme::icon(s);
            let mut buf = Vec::new();
            let mut enc = png::Encoder::new(&mut buf, s, s);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&icon.rgba)?;
            Ok(buf)
        })
        .collect::<Result<_>>()?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, png) in sizes.iter().zip(&pngs) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        out.extend_from_slice(&png);
    }
    Ok(out)
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    w.write_image_data(&bytes)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn bar_segments(
    painter: &egui::Painter,
    left: f32,
    right: f32,
    top: f32,
    h: f32,
    gap: f32,
    segs: &[(f32, Color32)],
    t: f64,
    pulse: bool,
) {
    let width = right - left - gap * (segs.len() as f32 - 1.0);
    if width < 40.0 {
        return;
    }
    let packet = pulse.then(|| {
        let px = left + ((t * 0.18).fract() as f32) * (right - left);
        Rect::from_center_size(pos2(px, top + h / 2.0), vec2(28.0, h))
    });
    let mut x = left;
    for (i, (frac, color)) in segs.iter().enumerate() {
        let w = width * frac;
        let rect = Rect::from_min_size(pos2(x, top), vec2(w, h));
        let last = i == segs.len() - 1;
        let cr = if last { egui::CornerRadius { ne: (h / 2.0) as u8, se: (h / 2.0) as u8, ..Default::default() } } else { egui::CornerRadius::ZERO };
        painter.rect_filled(rect, cr, *color);
        if let Some(packet) = packet {
            // Clip a copy of the actual segment, including its rounded cap.
            // Painting the packet itself leaks square corners past the cap.
            painter.with_clip_rect(packet.intersect(painter.clip_rect()))
                .rect_filled(rect, cr, Color32::WHITE.gamma_multiply(0.25));
        }
        x += w + gap;
    }
}

impl App {
    fn update_tray(&mut self) {
        let Some(tray) = &self.tray else { return };
        let status = match (&self.selected_dir_status(), self.incoming.len()) {
            (_, n) if n > 0 => format!("{n} remote session(s) active"),
            (DirStatus::Online { id }, _) => format!("Online · ID {id}"),
            (DirStatus::Connecting, _) => "Connecting…".into(),
            (DirStatus::Ready, _) => "Server reachable".into(),
            (DirStatus::NotConfigured, _) => "No server configured".into(),
            (DirStatus::Error(_), _) => "Server connection failed".into(),
        };
        if status != self.tray_status {
            tray.set_status(&self.rt, status.clone());
            self.tray_status = status;
        }
    }
}

/// A service install, start or removal under way: (restart afterwards,
/// what to say when it worked, the result).
type ServiceJob = (bool, &'static str, oneshot::Receiver<Result<(), String>>);

/// Resolve focus to one session; tool repaints never steal its input.
fn route_session_input(
    tabs: &[Arc<Mutex<remote::RemoteTab>>], page: Page, viewport: egui::ViewportId, focused: bool,
) -> Option<Arc<Mutex<remote::RemoteTab>>> {
    let target = tabs.iter().find(|tab| {
        let tab = tab.lock().unwrap();
        tab.viewport_id() == viewport && ((tab.detached && tab.window_active) || (!tab.detached && page == Page::Remote(tab.id)))
    }).cloned();
    for tab in tabs {
        let is_target = target.as_ref().is_some_and(|target| Arc::ptr_eq(tab, target));
        if (focused && !is_target) || (!focused && is_target) {
            tab.lock().unwrap().release_keyboard();
        }
    }
    target
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32;4] { [0.0;4] }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "linux")]
        self.screen_access.poll();
        while let Ok((id, action)) = self.session_actions.1.try_recv() { self.session_action(ctx, id, action); }
        let mut restored=Vec::new();
        for shared in &self.tabs {
            let mut tab = shared.lock().unwrap();
            tab.poll(ctx);
            if tab.maintain_connection(ctx,&self.rt,&self.client) {restored.push(tab.id);}
            // Drops and the receipt's Files action also work with the parent minimized.
            if ctx.viewport_id()==egui::ViewportId::ROOT && !ctx.embed_viewports() {
                files::FileManager::show(&tab.files,ctx,&self.rt,tab.id);
                tab.network_window(ctx, &self.rt);
            }
        }
        if self.hidden || ctx.input(|i| i.raw.viewports.get(&egui::ViewportId::ROOT).is_some_and(|v| v.minimized == Some(true))) {
            self.session_tabs.lock().unwrap().hide_target();
        }
        let live:Vec<_>=std::iter::once(egui::ViewportId::ROOT).chain(self.tabs.iter().map(|t|t.lock().unwrap().viewport_id())).collect();
        self.session_tabs.lock().unwrap().retain_targets(&live);
        session_windows::show(&self.tabs, ctx, &self.rt, &self.session_actions.0, &self.session_tabs, self.t0.elapsed().as_secs_f64(), &window_title(self.org.as_deref()));
        // Frame serials restart on a new session; discard cached old textures.
        if !restored.is_empty() {video::retain_streams(&self.render_state,|id|!restored.contains(&id));}
        if self.software.is_none()
            && let Some(rs) = frame.wgpu_render_state()
        {
            let info = rs.adapter.get_info();
            let software = info.device_type == eframe::egui_wgpu::wgpu::DeviceType::Cpu
                || ["llvmpipe", "softpipe", "swiftshader", "microsoft basic render"].iter().any(|n| info.name.to_lowercase().contains(n));
            if software {
                tracing::info!("{} draws on the processor: animations run slowly to spare it", info.name);
            }
            self.software = Some(software);
        }
        // Runs even while the window is hidden in the tray.
        self.handle_events();
        meetings::poll(self, ctx);
        let title = window_title(self.org.as_deref());
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        self.update_tray();
        home::incoming_windows(self, ctx);
        saved::windows(self, ctx);
        quick_connect::window(self, ctx);
        if self.start_hidden {
            self.start_hidden = false;
            self.set_visible(ctx, false);
        }
        let mut show = false;
        while let Some(req) = self.show_requests.as_ref().and_then(|rx| rx.try_recv().ok()) {
            show = true;
            if let single::Request::Link(url) = req {
                if url.starts_with("slipstream://meeting") {
                    self.meetings.join_text = url;
                    self.page = Page::Meetings;
                    continue;
                }
                match StationLink::parse(&url) {
                    Some(link) => self.start_link(ctx, link, true),
                    None => self.toast("That link isn't a Slipstream station link", pal::RED),
                }
            }
        }
        while let Some(cmd) = self.tray.as_ref().and_then(|t| t.poll()) {
            match cmd {
                tray::TrayCmd::Show => show = true,
                tray::TrayCmd::Quit => {
                    self.quitting = true;
                    self.set_visible(ctx, true);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if show {
            self.set_visible(ctx, true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // Closing quits, unless the user chose to keep running in the tray
        // (or the service hosts this computer: the app then sits in the
        // tray, unless they turned that off).
        let app = self.client.settings().app;
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && self.tray.is_some()
            && (app.run_in_tray || (self.via_service && !app.hide_tray_with_service))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.client.settings().app.tray_hint_shown {
                self.set_visible(ctx, false);
            } else {
                self.tray_hint = true;
            }
        }
        // Keep tray/status fresh while hidden.
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        #[cfg(target_os = "android")]
        android::input(_ctx, raw);
        #[cfg(target_os = "linux")]
        desktop_drop::update_pointer(_ctx, raw);
        self.session_tabs.lock().unwrap().update_input(_ctx, raw);
        let root_input = raw.viewport_id == egui::ViewportId::ROOT;
        let focused_now = raw.focused;
        if root_input { self.window_focused = focused_now; }
        let target = route_session_input(&self.tabs, self.page, raw.viewport_id, focused_now);
        let dragging = self.session_tabs.lock().unwrap().dragging();
        let captured = !self.quick_connect.open && !dragging && focused_now && target.as_ref().is_some_and(|tab| tab.lock().unwrap().keyboard_captured());
        if !captured {
            // Ours: the usual clipboard shortcuts for egui's text fields.
            for ev in &mut raw.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = *ev
                    && let Some(shortcut) = clipboard_shortcut(modifiers, key)
                {
                    *ev = match shortcut {
                        Shortcut::Cut => egui::Event::Cut,
                        Shortcut::Copy => egui::Event::Copy,
                        Shortcut::Paste => match slipstream_platform::clipboard::read_local() {
                            Some(slipstream_platform::clipboard::Clip::Text(t)) => {
                                egui::Event::Paste(t.replace("\r\n", "\n"))
                            }
                            _ => continue,
                        },
                    };
                }
            }
            return;
        }
        // Steal keyboard events so egui doesn't act on them (Tab focus
        // cycling, shortcuts…) and forward them to the remote machine.
        let mut pending_keys = Vec::new();
        let mut keep = Vec::with_capacity(raw.events.len());
        for ev in raw.events.drain(..) {
            match ev {
                egui::Event::Key { physical_key, key, pressed, repeat, modifiers } => {
                    // Ours, not theirs: the way out of full screen.
                    if key == egui::Key::Enter && modifiers.ctrl && modifiers.alt {
                        if pressed && !repeat {
                            pending_keys.push(remote::CapturedKey::ToggleFullscreen);
                        }
                        continue;
                    }
                    // A paste sends this computer's clipboard over first:
                    // text through the session, files by upload.
                    if pressed && !repeat && clipboard_shortcut(modifiers, key) == Some(Shortcut::Paste) {
                        use slipstream_platform::clipboard::{Clip, read_local};
                        match read_local() {
                            Some(Clip::Files(paths)) => {
                                pending_keys.push(remote::CapturedKey::PasteFiles(paths));
                                continue;
                            }
                            Some(Clip::Text(text)) => {
                                pending_keys.push(remote::CapturedKey::Shortcut("KeyV", Some(text)));
                                continue;
                            }
                            None => {}
                        }
                    }
                    let k = physical_key.unwrap_or(key);
                    pending_keys.push(remote::CapturedKey::Key { key: k, pressed, repeat });
                }
                egui::Event::Text(_) | egui::Event::Ime(_) => {}
                other => keep.push(other),
            }
        }
        raw.events = keep;
        if let Some(target) = target { target.lock().unwrap().pending_keys.extend(pending_keys); }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        softgl::started();
        let ctx = ui.ctx().clone();
        let workstation = if let Page::Remote(id) = self.page { self.tab(id).map(|t| t.lock().unwrap().info.host_name.clone()) } else { None };
        let body = theme::native_session_title_bar(ui, &window_title(self.org.as_deref()), workstation.as_deref());
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(body));
        // Clear of the status and navigation bars, the camera cutout and
        // the soft keyboard (the root Ui covers the whole screen).
        #[cfg(target_os = "android")]
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(ctx.content_rect()));
        self.poll_attempt(&ctx);
        self.poll_update(&ctx);
        self.poll_link();
        self.poll_service(&ctx);
        saved::poll(self, &ctx);
        for shared in &self.tabs {
            let tab = shared.lock().unwrap();
            // Native tools are registered in logic, including while hidden.
            if ctx.embed_viewports() {
                tab.network_window(&ctx, &self.rt);
                files::FileManager::show(&tab.files,&ctx,&self.rt,tab.id);
            }
        }

        let entries: Vec<_> = self.tabs.iter().filter_map(|tab| {
            let tab = tab.lock().unwrap();
            (!tab.detached).then(|| (tab.id, tab.info.host_name.clone()))
        }).collect();
        let mut content_ui = {
            let active = if let Page::Remote(id) = self.page {Some(id)} else {None};
            let visible = !self.hidden && ctx.input(|i| i.viewport().minimized != Some(true));
            let (body, actions) = session_tabs::show(&self.session_tabs, ui, ui.max_rect(), &entries, active, true, visible);
            for (id, action) in actions { self.session_action(&ctx, id, action); }
            ui.new_child(UiBuilder::new().max_rect(body))
        };
        let ui = &mut content_ui;

        if let Page::Remote(id) = self.page
            && let Some(tab) = self.tab(id)
            && !tab.lock().unwrap().detached
        {
            let full = ui.max_rect();
            let action = tab.lock().unwrap().ui(ui, full, self.t0.elapsed().as_secs_f64(), &self.rt);
            self.session_action(&ctx, id, action);
        } else {
            if matches!(self.page, Page::Remote(_)) {
                self.page = Page::Home;
            }
            // Full screen belongs to a session; the other pages get the window back.
            for tab in &self.tabs {
                let mut tab = tab.lock().unwrap();
                if !tab.detached { tab.leave_fullscreen(&ctx); }
            }
            if ctx.input(|i| i.viewport().fullscreen == Some(true)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            let content = self.draw_frame(ui);
            let mut child = ui.new_child(UiBuilder::new().max_rect(content));
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(&mut child, |ui| {
                    #[cfg(target_os = "android")]
                    android::keep_focus_in_view(ui);
                    match self.page {
                        Page::Home => home::ui(self, ui),
                        Page::Links => home::links_ui(self, ui),
                        Page::Meetings => meetings::ui(self, ui),
                        Page::Settings => settings::ui(self, ui),
                        Page::About => settings::about_ui(self, ui),
                        Page::Remote(_) => {}
                    }
                });
        }

        // Animate at the display's refresh rate whenever the window can be
        // seen (vsync paces it; compositors pause windows that are covered),
        // or a few times a second when the processor draws (input and new
        // video still redraw at once). Hidden or minimized, a slow tick
        // keeps the state fresh.
        if self.hidden || ctx.input(|i| i.viewport().minimized == Some(true)) {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else if self.software == Some(true) {
            ctx.request_repaint_after(if self.window_focused { self.frame_interval() } else { Duration::from_millis(500) });
        } else if self.window_focused {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        self.draw_tray_hint(&ctx);
        self.draw_attempt_overlay(&ctx);
        self.draw_toasts(&ctx);
        #[cfg(target_os = "android")]
        android::hold_keyboard(&ctx);
        self.maybe_screenshot(&ctx);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Cut,
    Copy,
    Paste,
}

/// The platform's clipboard shortcuts (as egui-winit would recognise them).
fn clipboard_shortcut(m: egui::Modifiers, key: egui::Key) -> Option<Shortcut> {
    use egui::Key;
    let win = cfg!(windows);
    match key {
        Key::Cut => Some(Shortcut::Cut),
        Key::Copy => Some(Shortcut::Copy),
        Key::Paste => Some(Shortcut::Paste),
        Key::X if m.command_only() => Some(Shortcut::Cut),
        Key::C if m.command_only() => Some(Shortcut::Copy),
        Key::V if m.command_only() => Some(Shortcut::Paste),
        Key::Delete if win && m.shift => Some(Shortcut::Cut),
        Key::Insert if win && m.ctrl => Some(Shortcut::Copy),
        Key::Insert if win && m.shift => Some(Shortcut::Paste),
        _ => None,
    }
}
