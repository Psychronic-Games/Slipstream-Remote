//! A remote-control session tab: the remote screen, input forwarding and
//! the session toolbar.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Instant,
};

use eframe::{
    egui::{self, Align, Align2, Color32, Layout, PointerButton, Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2},
    egui_wgpu,
};
use slipstream_core::{
    directory::DeviceId,
    proto::{InputEvent, MouseButton, SessionInfo, StreamSettings},
};
use slipstream_session::{FrameShared, ViewerCmd, ViewerEvent, ViewerSession, ViewerStats};
use tokio::sync::mpsc;

use super::{
    files::FileManager,
    home, keys, session_tabs, theme,
    theme::{ConsoleIcon as Icon, pal},
    touch, video,
};

pub enum CapturedKey {
    Key {
        key: egui::Key,
        pressed: bool,
        repeat: bool,
    },
    /// A paste with text on this computer's clipboard: send the text, then
    /// replay the key (Ctrl/Cmd is already held over there).
    Shortcut(&'static str, Option<String>),
    /// A paste with files on this computer's clipboard.
    PasteFiles(Vec<std::path::PathBuf>),
    /// Ctrl+Alt+Enter: full screen on/off, never sent to the remote side.
    ToggleFullscreen,
    /// Text from a phone's soft keyboard.
    Text(String),
}

/// The System information window's answer: None while it's being fetched.
type SysInfo = Option<Result<Vec<slipstream_core::proto::InfoSection>, String>>;

/// Files on their way to the remote clipboard for a paste.
#[derive(Default)]
struct Pasting {
    sent: u64,
    total: u64,
    done: Option<Result<usize, String>>,
}

struct Reconnecting {
    progress: Arc<Mutex<String>>,
    rx: tokio::sync::oneshot::Receiver<super::ConnectResult>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Reconnecting {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub enum TabAction {
    None,
    Bridge,
    /// Show the meeting we're in (its tab in the strip).
    Meeting,
    NewLink(egui::ViewportId),
    Close,
    Detach,
    Dock,
    Activate,
    PlaceTab(egui::ViewportId, Option<u64>),
    FloatAt(egui::Pos2),
    /// Close this screen's tab (the link ends with its last one).
    CloseScreen,
    /// Show this link's tab for that display.
    Screen(u32),
}

/// Picture quality presets. Balanced and Quality run at the viewer's own
/// Config → Max frame rate (see `RemoteTab::preset_settings`); the others
/// cap it at the rate listed here.
/// Phones and tablets: the trackpad, key bar and touch buttons.
/// SLIPSTREAM_TEST_TOUCH=1 shows them on a desktop (screenshots).
pub fn touch_ui() -> bool {
    cfg!(target_os = "android") || std::env::var_os("SLIPSTREAM_TEST_TOUCH").is_some()
}

const TRACKPAD_HINT: &str = "Trackpad: slide to move the pointer · tap to click · tap then slide to drag · two-finger tap to right-click · two fingers scroll · pinch to zoom";

/// The phone's bar of special keys.
const KEY_BAR_HEIGHT: f32 = 50.0;
const F_KEYS: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

const PRESETS: [(&str, StreamSettings); 4] = [
    ("Balanced", StreamSettings { bitrate_kbps: 0, max_fps: 60, max_in_flight: 2, adaptive_resolution: false }),
    ("Speed", StreamSettings { bitrate_kbps: 0, max_fps: 30, max_in_flight: 1, adaptive_resolution: true }),
    ("Quality", StreamSettings { bitrate_kbps: 20_000, max_fps: 60, max_in_flight: 3, adaptive_resolution: false }),
    ("Low bandwidth", StreamSettings { bitrate_kbps: 1_200, max_fps: 20, max_in_flight: 1, adaptive_resolution: true }),
];

/// Session tab and screen ids, from one sequence (they share the tab strip,
/// `Page::Remote` and the video textures).
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A session tab: one of the remote computer's screens, wherever its tab
/// is (the console or a window of its own). The link itself (control,
/// chat, files, tools) is shared by all of that computer's screen tabs.
pub struct RemoteTab {
    pub id: u64,
    pub detached: bool,
    pub window_group: Option<egui::ViewportId>,
    pub window_active: bool,
    pub window_position: Option<egui::Pos2>,
    pub pending_keys: Vec<CapturedKey>,
    pub link: Arc<Mutex<Link>>,
    screen: Screen,
}

/// What one screen tab shows of its display.
struct Screen {
    display: u32,
    /// The tab is in a window of its own (copied from its tab each frame).
    detached: bool,
    /// The link's generation this screen's pictures come from (a restored
    /// link has new ones).
    generation: u64,
    frames: Arc<FrameShared>,
    fullscreen: bool,
    /// Full screen: when the pointer last brought the toolbar up.
    toolbar_seen: Option<Instant>,
    video_rect: Option<Rect>,
    /// The whole picture fitted into the video area (the view zooms from it).
    video_fit: Option<Rect>,
    view: touch::View,
    touch: touch::Touch,
    /// Phones: the soft keyboard and the bar of special keys are up.
    keyboard: bool,
    /// Ctrl, Alt, Shift and Win latched on the key bar for the next key.
    latched: Vec<&'static str>,
}

/// One link to a remote computer: what all its screen tabs share.
pub struct Link {
    pub id: u64,
    session: ViewerSession,
    cmds: mpsc::UnboundedSender<ViewerCmd>,
    events: mpsc::UnboundedReceiver<ViewerEvent>,
    pub info: SessionInfo,
    pub peer: DeviceId,
    password: String,
    /// The signed-in station this device was found on (`None`: this app's own).
    pub via: slipstream_session::StationRef,
    pub stats: ViewerStats,
    pub closed: Option<String>,
    reconnect_requested: bool,
    reconnect: Option<Reconnecting>,
    /// Bumped when the link is restored after a reconnect.
    generation: u64,
    /// The display streaming, for hosts that stream one at a time.
    streaming: u32,
    /// Displays whose tabs were closed (unticked under Screens).
    hidden: std::collections::BTreeSet<u32>,
    /// What the host was last asked to stream, and the screens drawn
    /// lately (screens out of sight for a while are paused).
    shown_sent: Option<Vec<u32>>,
    drawn: std::collections::HashMap<u32, Instant>,
    /// Phones: a trackpad-style pointer rather than direct touch.
    trackpad: bool,
    preset: usize,
    chat: Vec<(bool, String)>,
    chat_input: String,
    show_chat: bool,
    /// This computer's Slipstream name, for its own chat messages.
    pub local_name: String,
    unread: bool,
    captured: bool,
    last_pointer: Option<(u32, f32, f32)>,
    held_buttons: HashSet<usize>,
    held_keys: HashSet<&'static str>,
    forward_repeats: bool,
    notice: Option<(String, Instant)>,
    pub files: Arc<Mutex<FileManager>>,
    network: Arc<Mutex<super::network::NetworkScanner>>,
    /// Playing the remote computer's sound.
    audio: bool,
    /// The remote computer's own screen is blacked out.
    privacy: bool,
    show_tunnels: bool,
    tunnels: Vec<slipstream_session::tunnel::Tunnel>,
    /// The "add a tunnel" form: local port, remote host, remote port.
    tunnel_form: (String, String, String),
    tunnel_error: Option<String>,
    pasting: Option<Arc<Mutex<Pasting>>>,
    /// A restart asked for in Tools, waiting for Restart in its dialog.
    power_confirm: Option<slipstream_core::proto::PowerAction>,
    power_reply: Option<tokio::sync::oneshot::Receiver<Result<(), String>>>,
    /// This viewer restarted the computer: reconnect for longer.
    restart_expected: Option<Instant>,
    /// The System information window: open, and its latest answer (None
    /// while it's being fetched).
    show_sysinfo: bool,
    /// Fetch it on the next frame (asked for from the toolbar).
    sysinfo_fetch: bool,
    /// Changes with every fetch, so a fresh answer starts scrolled to the top.
    sysinfo_serial: u64,
    /// A fetch is under way, and when the figures last came in (they're
    /// refreshed every few seconds while the window is open).
    sysinfo_busy: Arc<std::sync::atomic::AtomicBool>,
    sysinfo_at: Option<Instant>,
    sysinfo: Arc<Mutex<SysInfo>>,
    public_ip: Option<Result<std::net::IpAddr, String>>,
    public_ip_rx: Option<tokio::sync::oneshot::Receiver<Result<std::net::IpAddr, String>>>,
    public_ip_at: Option<Instant>,
    report: super::support_report::Report,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.reconnect = None;
        let mut files = self.files.lock().unwrap();
        files.disconnect();
        files.open = false;
        files.progress_open = false;
        self.network.lock().unwrap().close();
    }
}

impl RemoteTab {
    pub fn window_id(id: u64) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("remote-session", id))
    }

    pub fn viewport_id(&self) -> egui::ViewportId {
        if self.detached {
            self.window_group.unwrap_or_else(|| Self::window_id(self.id))
        } else {
            egui::ViewportId::ROOT
        }
    }

    /// A tab for `display` of the link.
    pub fn for_screen(link: &Arc<Mutex<Link>>, display: u32, ctx: &egui::Context) -> Self {
        let l = link.lock().unwrap();
        let tab = Self {
            id: next_id(),
            detached: false,
            window_group: None,
            window_active: true,
            window_position: None,
            pending_keys: Vec::new(),
            link: link.clone(),
            screen: Screen {
                display,
                detached: false,
                generation: l.generation,
                frames: l.session.frames_for(display),
                fullscreen: false,
                toolbar_seen: None,
                video_rect: None,
                video_fit: None,
                view: touch::View::default(),
                touch: touch::Touch::default(),
                keyboard: false,
                latched: Vec::new(),
            },
        };
        drop(l);
        tab.update_window_repaint(ctx);
        tab
    }

    /// Which of the remote computer's displays this tab shows.
    pub fn display(&self) -> u32 {
        self.screen.display
    }

    pub fn host_name(&self) -> String {
        self.link.lock().unwrap().info.host_name.clone()
    }

    /// Its tab in the strip: id, the computer's name, and the screen's
    /// number when the computer has several screen tabs.
    pub(super) fn strip_entry(&self) -> session_tabs::Entry {
        let link = self.link.lock().unwrap();
        session_tabs::Entry {
            id: self.id,
            name: link.info.host_name.clone(),
            screen: link.several_screens().then_some(self.screen.display + 1),
        }
    }

    /// The phone's soft keyboard is up for this screen: its text goes over.
    pub fn wants_text(&self) -> bool {
        self.screen.keyboard
    }

    /// Whether this tab and `other` show screens of the same link.
    pub fn same_link(&self, other: &Arc<Mutex<Link>>) -> bool {
        Arc::ptr_eq(&self.link, other)
    }

    pub fn update_window_repaint(&self, ctx: &egui::Context) {
        let (wake, viewport) = (ctx.clone(), self.viewport_id());
        self.screen.frames.set_repaint(move || {
            wake.request_repaint_of(egui::ViewportId::ROOT);
            theme::repaint_existing_viewport(&wake, viewport);
        });
    }

    pub fn keyboard_captured(&self) -> bool {
        self.link.lock().unwrap().keyboard_captured()
    }

    pub fn release_keyboard(&mut self) {
        self.pending_keys.clear();
        self.link.lock().unwrap().release_keyboard();
    }

    /// Leaving the session's page: don't strand the window in full screen.
    pub fn leave_fullscreen(&mut self, ctx: &egui::Context) {
        if self.screen.fullscreen {
            toggle_fullscreen(&mut self.screen, ctx);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, full: Rect, t: f64, rt: &tokio::runtime::Runtime) -> TabAction {
        let keys = std::mem::take(&mut self.pending_keys);
        let link = self.link.clone();
        let mut link = link.lock().unwrap();
        if self.screen.generation != link.generation {
            // Restored after a reconnect: its pictures come from the new session.
            self.screen.generation = link.generation;
            self.screen.frames = link.session.frames_for(self.screen.display);
            self.screen.touch = touch::Touch::default();
            self.update_window_repaint(ui.ctx());
        }
        self.screen.detached = self.detached;
        link.forward_keys(keys, &mut self.screen, rt, ui.ctx());
        link.ui(ui, &mut self.screen, self.id, full, t, rt)
    }
}

fn toggle_fullscreen(sc: &mut Screen, ctx: &egui::Context) -> Option<String> {
    sc.fullscreen = !sc.fullscreen;
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(sc.fullscreen));
    sc.fullscreen
        .then(|| "Full screen: point at the top edge for the bar, or press Ctrl+Alt+Enter to leave".to_string())
}

impl Link {
    pub fn end_link(&mut self) {
        self.release_keyboard();
        self.reconnect = None;
        self.send(ViewerCmd::Disconnect);
    }

    /// Messages from the chat this session grew out of.
    pub fn adopt_chat(&mut self, chat: Vec<(bool, String)>) {
        if !chat.is_empty() {
            self.chat = chat;
            self.show_chat = true;
        }
    }

    pub fn new(
        session: ViewerSession,
        events: mpsc::UnboundedReceiver<ViewerEvent>,
        password: String,
        play_audio: bool,
        trackpad: bool,
    ) -> Self {
        let info = session.info().clone();
        let files = Arc::new(Mutex::new(FileManager::new(session.fs(), info.host_name.clone())));
        let network = Arc::new(Mutex::new(super::network::NetworkScanner::new(
            session.fs(),
            info.capabilities.input,
            info.capabilities.tunnels,
        )));
        // Wayland apps repeat held keys themselves; Windows/macOS need repeats sent.
        let forward_repeats = info.os != "Linux";
        let streaming =
            info.displays.iter().find(|d| d.primary).or(info.displays.first()).map(|d| d.index).unwrap_or(0);
        let audio = play_audio && info.capabilities.audio;
        let notice = trackpad.then(|| (TRACKPAD_HINT.to_string(), Instant::now()));
        Self {
            id: next_id(),
            cmds: session.command_sender(),
            peer: session.peer_id(),
            info,
            session,
            events,
            password,
            via: Default::default(),
            stats: ViewerStats::default(),
            closed: None,
            reconnect_requested: false,
            reconnect: None,
            generation: 0,
            streaming,
            hidden: Default::default(),
            shown_sent: None,
            drawn: Default::default(),
            trackpad,
            preset: 0,
            chat: Vec::new(),
            chat_input: String::new(),
            show_chat: false,
            local_name: String::new(),
            unread: false,
            captured: false,
            last_pointer: None,
            held_buttons: HashSet::new(),
            held_keys: HashSet::new(),
            forward_repeats,
            notice,
            power_confirm: None,
            power_reply: None,
            restart_expected: None,
            audio,
            privacy: false,
            show_tunnels: false,
            tunnels: Vec::new(),
            tunnel_form: (String::new(), "localhost".into(), String::new()),
            tunnel_error: None,
            pasting: None,
            // SLIPSTREAM_TEST_SYSINFO=1 opens it straight away (screenshots).
            show_sysinfo: std::env::var_os("SLIPSTREAM_TEST_SYSINFO").is_some(),
            sysinfo_fetch: std::env::var_os("SLIPSTREAM_TEST_SYSINFO").is_some(),
            sysinfo_serial: 0,
            sysinfo_busy: Arc::default(),
            sysinfo_at: None,
            sysinfo: Arc::default(),
            public_ip: None,
            public_ip_rx: None,
            public_ip_at: None,
            report: Default::default(),
            files,
            network,
        }
    }

    /// The displays that get a tab: all of them but those closed.
    pub fn wanted_displays(&self) -> Vec<u32> {
        let mut displays: Vec<u32> =
            self.info.displays.iter().map(|d| d.index).filter(|d| !self.hidden.contains(d)).collect();
        if displays.is_empty() {
            // A chat-only link before its screen opened, or every one closed.
            displays.push(
                self.info.displays.iter().map(|d| d.index).find(|d| !self.hidden.contains(d)).unwrap_or(self.streaming),
            );
        }
        displays
    }

    /// Whether the link has more than one screen tab.
    pub fn several_screens(&self) -> bool {
        self.wanted_displays().len() > 1
    }

    /// Closes `display`'s tab (the link carries on with the others).
    pub fn hide_display(&mut self, display: u32) {
        self.hidden.insert(display);
        self.drawn.remove(&display);
    }

    /// Asks the host for the screens drawn in the last moments: hidden
    /// tabs pause after a couple of seconds out of sight.
    pub fn stream_visible(&mut self) {
        if self.closed.is_some() {
            return;
        }
        // Several at once: pause a screen once out of sight for a couple of
        // seconds. One at a time: switch as soon as another is the one on view.
        let multi = self.session.multi_display();
        let recent = std::time::Duration::from_millis(if multi { 2000 } else { 400 });
        let mut shown: Vec<(Instant, u32)> = self
            .drawn
            .iter()
            .filter(|(d, at)| at.elapsed() < recent && !self.hidden.contains(d))
            .map(|(&d, &at)| (at, d))
            .collect();
        // The one looked at most recently first (a host streaming one
        // display at a time switches to it).
        shown.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut shown: Vec<u32> = shown.into_iter().map(|(_, d)| d).collect();
        let mut sorted = shown.clone();
        sorted.sort_unstable();
        if self.shown_sent.as_ref() == Some(&sorted) {
            return;
        }
        // Nothing on view yet (a new link): leave the host's first picture.
        if shown.is_empty() && self.shown_sent.is_none() {
            return;
        }
        if !multi {
            // One display at a time: keep the one streaming if it's on view.
            if shown.contains(&self.streaming) {
                shown.retain(|&d| d == self.streaming);
            }
            shown.truncate(1);
        }
        self.shown_sent = Some(sorted);
        self.send(ViewerCmd::ShowDisplays(shown));
    }

    /// The host shows one display at a time and this one isn't it.
    fn other_display_streaming(&self, display: u32) -> bool {
        !self.session.multi_display() && self.streaming != display
    }

    fn send(&self, cmd: ViewerCmd) {
        let _ = self.cmds.send(cmd);
    }

    fn input(&self, ev: InputEvent) {
        if self.closed.is_none() && self.info.capabilities.input {
            self.session.input(ev);
        }
    }

    /// Pointer input on one display.
    fn input_on(&self, display: u32, ev: InputEvent) {
        if self.closed.is_none() && self.info.capabilities.input {
            self.session.input_on(display, ev);
        }
    }

    /// Typed text: as characters where the host takes them, otherwise as
    /// the keys of a US keyboard.
    fn type_text(&self, text: &str) {
        if self.session.text_input() {
            self.input(InputEvent::Text(text.into()));
        } else {
            for ev in slipstream_core::proto::us_keys_for_text(text) {
                self.input(ev);
            }
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        self.poll_paste(ctx);
        self.stream_visible();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                ViewerEvent::Stats(s) => self.stats = s,
                ViewerEvent::StreamStarted { display, .. } => self.streaming = display,
                ViewerEvent::Displays(d) => self.info.displays = d,
                ViewerEvent::Capabilities(c) => {
                    self.network.lock().unwrap().set_permissions(c.input, c.tunnels);
                    self.info.capabilities = c;
                }
                ViewerEvent::Chat(text) => {
                    self.chat.push((false, text));
                    self.unread = !self.show_chat;
                    self.show_chat = true;
                }
                ViewerEvent::Notice(n) => self.notice = Some((n, Instant::now())),
                ViewerEvent::Disconnected { reason, retryable } => {
                    tracing::info!(peer=%self.peer,retryable,"viewer link ended: {reason}");
                    self.release_keyboard();
                    self.files.lock().unwrap().disconnect();
                    self.network.lock().unwrap().disconnect();
                    self.report.cancel_requests();
                    self.tunnels.clear();
                    self.closed = Some(reason);
                    self.stats = ViewerStats::default();
                    self.reconnect_requested = retryable && self.peer.0 != 0;
                    self.repaint_tools(ctx);
                }
            }
        }
    }

    pub fn keyboard_captured(&self) -> bool {
        self.captured && self.closed.is_none()
    }

    /// Keep recovery attached to this tab: ending the link drops the job.
    pub fn maintain_connection(
        &mut self,
        ctx: &egui::Context,
        rt: &tokio::runtime::Runtime,
        client: &slipstream_session::Client,
    ) -> bool {
        let mut restored = false;
        if std::mem::take(&mut self.reconnect_requested) && self.reconnect.is_none() {
            tracing::info!(peer=%self.peer,"reconnecting lost link");
            let progress = Arc::new(Mutex::new("Waiting for the remote host…".into()));
            let (tx, rx) = tokio::sync::oneshot::channel();
            let (client, peer, password, via, wake, status) =
                (client.clone(), self.peer, self.password.clone(), self.via.clone(), ctx.clone(), progress.clone());
            // After a restart this viewer asked for, wait out a slow boot.
            let window = if self.restart_expected.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(15 * 60))
            {
                std::time::Duration::from_secs(10 * 60)
            } else {
                std::time::Duration::from_secs(120)
            };
            let task = rt.spawn(async move {
                let w = wake.clone();
                let result = client
                    .reconnect_within(peer, &password, &via, window, move |text| {
                        *status.lock().unwrap() = text;
                        w.request_repaint_of(egui::ViewportId::ROOT);
                    })
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
                wake.request_repaint_of(egui::ViewportId::ROOT);
            });
            self.reconnect = Some(Reconnecting { progress, rx, task });
        }
        let result = self.reconnect.as_mut().and_then(|retry| match retry.rx.try_recv() {
            Ok(result) => Some(result),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => None,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                Some(Err("Reconnect ended without an answer".into()))
            }
        });
        if let Some(result) = result {
            self.reconnect = None;
            match result {
                Ok((session, events)) => {
                    self.restart_expected = None;
                    self.restore(session, events, ctx, client.settings().viewer.play_audio);
                    restored = true;
                }
                Err(error) => {
                    tracing::warn!(peer=%self.peer,"automatic reconnect stopped: {error}");
                    self.closed = Some(error);
                }
            }
        }
        if self.reconnect.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        restored
    }

    fn restore(
        &mut self,
        session: ViewerSession,
        events: mpsc::UnboundedReceiver<ViewerEvent>,
        ctx: &egui::Context,
        default_audio: bool,
    ) {
        tracing::info!(peer=%self.peer,"viewer link restored");
        self.info = session.info().clone();
        self.cmds = session.command_sender();
        self.events = events;
        self.session = session;
        self.forward_repeats = self.info.os != "Linux";
        self.streaming =
            self.info.displays.iter().find(|d| d.primary).or(self.info.displays.first()).map(|d| d.index).unwrap_or(0);
        // The screen tabs pick up the new pictures, and the host is asked
        // for the ones on view again.
        self.generation += 1;
        self.shown_sent = None;
        self.closed = None;
        self.captured = false;
        self.last_pointer = None;
        self.held_keys.clear();
        self.held_buttons.clear();
        self.privacy = false;
        self.audio &= self.info.capabilities.audio;
        self.send(ViewerCmd::Settings(self.preset_settings(self.preset)));
        if self.audio != default_audio {
            self.send(ViewerCmd::Audio(self.audio));
        }
        self.files.lock().unwrap().reconnect(self.session.fs(), self.info.host_name.clone());
        self.network.lock().unwrap().reconnect(
            self.session.fs(),
            self.info.capabilities.input,
            self.info.capabilities.tunnels,
        );
        self.repaint_tools(ctx);
        self.pasting = None;
        self.sysinfo = Arc::default();
        self.sysinfo_busy = Arc::default();
        self.sysinfo_at = None;
        self.sysinfo_fetch = self.show_sysinfo;
        self.public_ip = None;
        self.public_ip_rx = None;
        self.public_ip_at = None;
        self.notice = Some(("Link restored · click the picture to take control".into(), Instant::now()));
    }

    fn repaint_tools(&self, ctx: &egui::Context) {
        if self.files.lock().unwrap().open {
            ctx.request_repaint_of(egui::ViewportId::from_hash_of(("file-transfer", self.id)));
        }
        if self.network.lock().unwrap().open {
            ctx.request_repaint_of(egui::ViewportId::from_hash_of(("network-scan", self.id)));
        }
    }

    pub fn release_keyboard(&mut self) {
        if self.captured || !self.held_keys.is_empty() || !self.held_buttons.is_empty() {
            self.input(InputEvent::ReleaseAll);
        }
        self.held_keys.clear();
        self.held_buttons.clear();
        self.captured = false;
    }

    /// Ctrl+V with files copied here: upload them onto the remote
    /// clipboard, then paste there.
    fn paste_files(&mut self, paths: Vec<std::path::PathBuf>, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        let caps = &self.info.capabilities;
        if !(caps.clipboard && caps.file_transfer) {
            self.notice = Some((
                "That computer doesn't accept files — drop them on the screen or use Files".into(),
                Instant::now(),
            ));
            return;
        }
        if self.pasting.is_some() {
            return; // one at a time
        }
        let state = Arc::new(Mutex::new(Pasting::default()));
        let (s, wake) = (state.clone(), ctx.clone());
        let progress = move |sent, total| {
            let mut p = s.lock().unwrap();
            (p.sent, p.total) = (sent, total);
            wake.request_repaint();
        };
        let job = self.session.paste_files(paths, progress);
        let (s, wake) = (state.clone(), ctx.clone());
        rt.spawn(async move {
            let r = job.await.map_err(|e| format!("{e:#}"));
            s.lock().unwrap().done = Some(r);
            wake.request_repaint();
        });
        self.pasting = Some(state);
    }

    fn poll_paste(&mut self, ctx: &egui::Context) {
        let Some(state) = &self.pasting else { return };
        let mut p = state.lock().unwrap();
        let Some(done) = p.done.take() else {
            let pct = (p.sent * 100).checked_div(p.total).unwrap_or(0);
            self.notice = Some((format!("Copying files to the remote clipboard… {pct}%"), Instant::now()));
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        };
        drop(p);
        self.pasting = None;
        match done {
            Ok(n) => {
                // Ctrl (Cmd on a Mac) may have been let go meanwhile.
                let held =
                    ["ControlLeft", "ControlRight", "MetaLeft", "MetaRight"].iter().any(|k| self.held_keys.contains(k));
                if held {
                    self.send_combo(&["KeyV"]);
                } else if self.info.os == "macOS" {
                    self.send_combo(&["MetaLeft", "KeyV"]);
                } else {
                    self.send_combo(&["ControlLeft", "KeyV"]);
                }
                self.notice = Some((format!("Pasted {n} file{}", if n == 1 { "" } else { "s" }), Instant::now()));
            }
            Err(e) => self.notice = Some((format!("Couldn't paste the files: {e}"), Instant::now())),
        }
    }

    fn toggle_fullscreen(&mut self, sc: &mut Screen, ctx: &egui::Context) {
        if let Some(notice) = toggle_fullscreen(sc, ctx) {
            self.notice = Some((notice, Instant::now()));
        }
    }

    fn forward_keys(
        &mut self,
        keys: Vec<CapturedKey>,
        sc: &mut Screen,
        rt: &tokio::runtime::Runtime,
        ctx: &egui::Context,
    ) {
        for k in keys {
            match k {
                CapturedKey::Key { key, pressed, repeat } => {
                    let Some(code) = keys::w3c_code(key) else { continue };
                    if repeat && !self.forward_repeats {
                        continue;
                    }
                    if pressed {
                        self.held_keys.insert(code);
                    } else if !self.held_keys.remove(code) {
                        continue; // never sent the press (e.g. focus moved)
                    }
                    self.input(InputEvent::Key { code: code.into(), down: pressed });
                }
                CapturedKey::Shortcut(code, paste) => {
                    if let Some(text) = paste {
                        self.send(ViewerCmd::Clipboard(text));
                    }
                    self.input(InputEvent::Key { code: code.into(), down: true });
                    self.input(InputEvent::Key { code: code.into(), down: false });
                }
                CapturedKey::PasteFiles(paths) => self.paste_files(paths, rt, ctx),
                CapturedKey::ToggleFullscreen => {
                    // Ctrl and Alt went over already: let them go there.
                    self.release_keyboard();
                    self.toggle_fullscreen(sc, ctx);
                }
                CapturedKey::Text(text) => self.type_latched(sc, &text),
            }
        }
    }

    fn send_combo(&self, codes: &[&str]) {
        for c in codes {
            self.input(InputEvent::Key { code: (*c).into(), down: true });
        }
        for c in codes.iter().rev() {
            self.input(InputEvent::Key { code: (*c).into(), down: false });
        }
    }

    pub fn network_window(&self, ctx: &egui::Context, rt: &Arc<tokio::runtime::Runtime>) {
        super::network::NetworkScanner::show(&self.network, ctx, rt, self.id, &self.info.host_name);
    }

    fn ui(
        &mut self,
        ui: &mut Ui,
        sc: &mut Screen,
        tab: u64,
        full: Rect,
        t: f64,
        rt: &tokio::runtime::Runtime,
    ) -> TabAction {
        let mut action = TabAction::None;
        let ctx = ui.ctx().clone();
        self.power_ui(&ctx, rt);
        ui.painter().rect_filled(full, 0.0, Color32::BLACK);

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let toolbar_h = 36.0;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::ALT, egui::Key::Enter)) {
            self.toggle_fullscreen(sc, &ctx);
        }
        // Full screen: the bar comes down at the top edge and stays while one
        // of its menus is open (the pointer is then below it) or briefly after.
        if pointer.is_some_and(|p| p.y < full.top() + toolbar_h + 4.0) || egui::Popup::is_any_open(&ctx) {
            sc.toolbar_seen = Some(Instant::now());
        }
        let lingering = sc.toolbar_seen.is_some_and(|t| t.elapsed().as_millis() < 700);
        if sc.fullscreen && lingering {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let show_toolbar = !sc.fullscreen || lingering || self.closed.is_some();
        let framed = !sc.fullscreen && !touch_ui();
        let body = if framed {
            theme::session_body(full, toolbar_h)
        } else {
            full.with_min_y(full.top() + if sc.fullscreen { 0.0 } else { toolbar_h })
        };
        // Chat goes beside the picture, or under it on a phone; the key bar
        // under the picture, just above the soft keyboard.
        let narrow = full.width() < super::NARROW;
        let chat_w = if self.show_chat && !narrow { 330.0 } else { 0.0 };
        let chat_h = if self.show_chat && narrow { (body.height() * 0.42).max(220.0) } else { 0.0 };
        let keybar_h = if sc.keyboard { KEY_BAR_HEIGHT } else { 0.0 };
        let video_area = Rect::from_min_max(body.min, pos2(body.right() - chat_w, body.bottom() - chat_h - keybar_h));

        self.drawn.insert(sc.display, Instant::now());
        self.draw_video(ui, sc, tab, video_area, t);
        let rail_width = framed.then(|| theme::session_rails(ui.painter(), full, body));
        self.handle_pointer(ui, sc, video_area);
        if sc.keyboard {
            let bar = Rect::from_min_max(
                pos2(video_area.left(), video_area.bottom()),
                pos2(video_area.right(), video_area.bottom() + keybar_h),
            );
            self.key_bar(ui, sc, bar);
        }

        if self.show_chat {
            let chat_rect = if narrow {
                Rect::from_min_max(pos2(body.left(), body.bottom() - chat_h), body.max)
            } else {
                Rect::from_min_max(pos2(body.right() - chat_w, body.top()), body.max)
            };
            ui.painter().rect_filled(chat_rect, 0.0, pal::PANEL);
            let mut child = ui.new_child(UiBuilder::new().max_rect(chat_rect.shrink(12.0)));
            theme::header(&mut child, "Subspace chat", pal::LAVENDER);
            child.add_space(6.0);
            let cmds = self.cmds.clone();
            let names = home::ChatNames { me: &self.local_name, them: &self.info.host_name };
            home::chat_ui(&mut child, &mut self.chat, &mut self.chat_input, names, |text| {
                let _ = cmds.send(ViewerCmd::Chat(text));
            });
            self.unread = false;
        }

        if show_toolbar {
            let bar = Rect::from_min_size(full.min, vec2(full.width(), toolbar_h));
            action = self.toolbar(ui, sc, bar, t, rail_width);
        }

        self.tunnels_window(&ctx, rt);
        self.sysinfo_window(&ctx, rt);
        if self.report.open {
            let scanner = self.network.lock().unwrap();
            let network = format!("{}\n{}", scanner.export(&self.info.host_name), scanner.diagnostic_text());
            drop(scanner);
            self.report.show(
                &ctx,
                rt,
                super::support_report::Source {
                    id: self.id,
                    host: &self.info.host_name,
                    connected: self.closed.is_none(),
                    can_control: self.info.capabilities.input,
                    fs: self.session.fs(),
                    stats: &self.stats,
                    network,
                    frame: sc.frames.latest().1,
                },
            );
        }
        self.handle_drops(ui, sc, &ctx, video_area, rt);

        if let Some(reason) = self.closed.clone() {
            action = self.closed_overlay(ui, video_area, &reason).unwrap_or(action);
        }
        if let Some((n, at)) = &self.notice {
            if at.elapsed().as_secs() < 6 {
                let p = ui.painter();
                let pos = pos2(video_area.center().x, video_area.bottom() - 40.0);
                let g = p.layout(n.clone(), theme::body(17.0), pal::TEXT, (video_area.width() - 56.0).max(120.0));
                let r = Rect::from_center_size(
                    pos - vec2(0.0, (g.size().y - 20.0).max(0.0) / 2.0),
                    g.size() + vec2(32.0, 14.0),
                );
                p.rect_filled(r, 14.0, pal::PANEL.gamma_multiply(0.95));
                p.rect_stroke(r, 14.0, egui::Stroke::new(1.0, pal::GOLD), egui::StrokeKind::Inside);
                p.galley(r.min + vec2(16.0, 7.0), g, pal::TEXT);
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                self.notice = None;
            }
        }
        self.stream_visible();
        action
    }

    /// Files dragged in from the desktop: onto the file window's remote pane
    /// → that folder; onto the remote screen → the remote folder under the
    /// drop point (or its Desktop).
    fn handle_drops(
        &mut self,
        ui: &mut Ui,
        sc: &Screen,
        ctx: &egui::Context,
        video_area: Rect,
        rt: &tokio::runtime::Runtime,
    ) {
        if self.closed.is_some() {
            return;
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !self.info.capabilities.file_transfer {
            if !dropped.is_empty() {
                self.notice = Some(("File transfer is turned off on the remote computer".into(), Instant::now()));
            }
            return;
        }
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let pane_target = pos.and_then(|p| self.files.lock().unwrap().remote_drop_target(p, ctx));
        let screen_pos = pos.filter(|p| {
            video_area.contains(*p)
                && sc.video_rect.is_some_and(|r| r.contains(*p))
                && ctx.layer_id_at(*p).is_none_or(|layer| layer == ui.layer_id())
        });
        if hovering && pane_target.is_none() && screen_pos.is_some() {
            let p = ui.painter();
            p.rect_filled(video_area, 0.0, Color32::BLACK.gamma_multiply(0.45));
            p.text(video_area.center(), Align2::CENTER_CENTER, "DROP TO SEND", theme::display_bold(42.0), pal::ORANGE);
            p.text(
                video_area.center() + vec2(0.0, 40.0),
                Align2::CENTER_CENTER,
                "Open remote folder or Desktop · files and folders",
                theme::body(17.0),
                pal::TAN,
            );
        }
        if dropped.is_empty() {
            return;
        }
        let n = dropped.len();
        if let Some(target) = pane_target {
            self.files.lock().unwrap().upload(dropped, target, rt, ctx);
        } else if let (Some(p), Some(r), Some(display)) =
            (screen_pos, sc.video_rect, self.info.displays.iter().find(|d| d.index == sc.display))
        {
            // Resolve a physical point on the host, independent of keyboard
            // capture or the asynchronously injected remote mouse position.
            let x = display.x
                + (((p.x - r.left()) / r.width()).clamp(0.0, 1.0) * display.width.saturating_sub(1) as f32).round()
                    as i32;
            let y = display.y
                + (((p.y - r.top()) / r.height()).clamp(0.0, 1.0) * display.height.saturating_sub(1) as f32).round()
                    as i32;
            self.files.lock().unwrap().upload_to_screen(dropped, (x, y), rt, ctx);
        } else {
            self.notice =
                Some(("Drop onto the remote picture or the Files window's remote pane".into(), Instant::now()));
            return;
        }
        self.notice = Some((
            format!("Sending {n} item{} — transfer panel opened", if n == 1 { "" } else { "s" }),
            Instant::now(),
        ));
    }

    fn draw_video(&mut self, ui: &mut Ui, sc: &mut Screen, tab: u64, area: Rect, t: f64) {
        let painter = ui.painter();
        let (serial, frame) = sc.frames.latest();
        // A host that streams one display at a time is showing another.
        let elsewhere = self.other_display_streaming(sc.display);
        match frame.filter(|_| !elsewhere) {
            Some(frame) => {
                let fit = video::fit_rect(area, (frame.width, frame.height));
                let rect = sc.view.rect(fit, area);
                sc.video_fit = Some(fit);
                sc.video_rect = Some(rect);
                // Zoomed in, the picture runs past the area.
                painter.with_clip_rect(area).add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    video::VideoCallback { stream: tab, serial, frame },
                ));
                if self.captured && !sc.fullscreen && !touch_ui() {
                    theme::brackets(painter, rect.intersect(area).shrink(4.0), pal::ORANGE.gamma_multiply(0.8), 22.0);
                }
            }
            None => {
                sc.video_rect = None;
                sc.video_fit = None;
                theme::backdrop(painter, area, t);
                if elsewhere {
                    painter.text(
                        area.center() - vec2(0.0, 18.0),
                        Align2::CENTER_CENTER,
                        format!("SCREEN {}", sc.display + 1),
                        theme::display_bold(30.0),
                        pal::ORANGE,
                    );
                    painter.text(area.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER,
                        "That computer shows one screen at a time · click to show this one, or update Slipstream there to see them together",
                        theme::body(15.0), pal::TAN);
                    let clicked = ui
                        .ctx()
                        .input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| area.contains(p)));
                    if clicked && self.closed.is_none() {
                        self.streaming = sc.display;
                        self.shown_sent = Some(vec![sc.display]);
                        self.send(ViewerCmd::SelectDisplay(sc.display));
                    }
                    // Keep counting as on view, so the host switches to it.
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    return;
                }
                theme::spinner(painter, area.center() - vec2(0.0, 40.0), 26.0, pal::CYAN, t);
                painter.text(
                    area.center() + vec2(0.0, 10.0),
                    Align2::CENTER_CENTER,
                    "AWAITING SIGNAL",
                    theme::display_bold(30.0),
                    pal::ORANGE,
                );
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
        }
    }

    fn handle_pointer(&mut self, ui: &mut Ui, sc: &mut Screen, area: Rect) {
        let ctx = ui.ctx().clone();
        // Claim the area so egui widgets underneath don't react.
        let resp = ui.interact(area, ui.scope_id().with(("video", sc.display)), Sense::click_and_drag());
        let Some(rect) = sc.video_rect else { return };
        if self.closed.is_some() || !ctx.input(|i| i.focused) {
            return;
        }
        let display = sc.display;
        let hovered = resp.hovered()
            && ctx
                .pointer_hover_pos()
                .is_some_and(|p| rect.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l == ui.layer_id()));
        // Hide our pointer only when the host's video shows its own.
        if hovered && self.captured && self.info.capabilities.cursor_in_video {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        if hovered && !self.captured && !touch_ui() {
            let p = ui.painter();
            let label = "CLICK TO TAKE CONTROL";
            let pos = pos2(rect.center().x, rect.top() + 28.0);
            let g = p.layout_no_wrap(label.into(), theme::display(16.0), Color32::BLACK);
            let r = Rect::from_center_size(pos, g.size() + vec2(28.0, 10.0));
            p.rect_filled(r, r.height() / 2.0, pal::ORANGE.gamma_multiply(0.9));
            p.galley(r.min + vec2(14.0, 5.0), g, Color32::BLACK);
        }

        let normalize = |pos: egui::Pos2| {
            (
                ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
                ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
            )
        };
        // Only the part of the screen not covered by our own windows (file
        // transfer, chat…) belongs to the remote computer.
        let my_layer = ui.layer_id();
        let ours = |pos: egui::Pos2| area.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == my_layer);
        let on_video = |pos: egui::Pos2| rect.contains(pos) && ours(pos);
        let events = ctx.input(|i| i.events.clone());

        // Fingers: see touch.rs. While they're down, the pointer events egui
        // makes from the first one are ignored.
        let fit = sc.video_fit.unwrap_or(rect);
        let gesture = ctx.input(|i| i.multi_touch()).map(|m| touch::Gesture::from_egui(&m));
        let view_before = sc.view;
        let mode = if self.trackpad { touch::Mode::Trackpad } else { touch::Mode::Direct };
        let (actions, touching) =
            sc.touch.handle(&events, gesture, Instant::now(), mode, ours, &mut sc.view, (fit, area));
        if sc.view != view_before {
            ctx.request_repaint();
        }
        if sc.touch.waiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        // The picture may have moved under the fingers (zoom, or the view
        // following the trackpad pointer).
        let rect = sc.view.rect(fit, area);
        let normalize_now = |pos: egui::Pos2| {
            (
                ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
                ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
            )
        };
        for action in actions {
            self.captured = true;
            match action {
                touch::Action::Move(pos) => {
                    let n = normalize_now(pos);
                    self.last_pointer = Some((display, n.0, n.1));
                    self.input_on(display, InputEvent::MouseMove { x: n.0, y: n.1 });
                }
                touch::Action::Button(button, down) => self.input_on(display, InputEvent::MouseButton { button, down }),
                touch::Action::Scroll(points) => {
                    self.input_on(display, InputEvent::Wheel { dx: points.x / 40.0, dy: points.y / 40.0 })
                }
            }
        }
        self.draw_touch_feedback(ui, sc, area, rect);

        for ev in events {
            match ev {
                egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } if touching => {}
                egui::Event::PointerMoved(pos) => {
                    if (on_video(pos) || !self.held_buttons.is_empty()) && self.captured {
                        let n = normalize(pos);
                        if self.last_pointer != Some((display, n.0, n.1)) {
                            self.last_pointer = Some((display, n.0, n.1));
                            self.input_on(display, InputEvent::MouseMove { x: n.0, y: n.1 });
                        }
                    }
                }
                egui::Event::PointerButton { pos, button, pressed, .. } => {
                    let inside = on_video(pos);
                    if pressed && !inside {
                        if self.captured && (area.contains(pos) || sc.keyboard) {
                            // Letterbox click, or the phone's key bar: keep control.
                            continue;
                        }
                        if self.captured {
                            self.release_keyboard();
                        }
                        continue;
                    }
                    let Some(b) = map_button(button) else { continue };
                    if pressed {
                        if !self.captured {
                            // First click only takes control.
                            self.captured = true;
                            let n = normalize(pos);
                            self.last_pointer = Some((display, n.0, n.1));
                            self.input_on(display, InputEvent::MouseMove { x: n.0, y: n.1 });
                            continue;
                        }
                        let n = normalize(pos);
                        if self.last_pointer != Some((display, n.0, n.1)) {
                            self.last_pointer = Some((display, n.0, n.1));
                            self.input_on(display, InputEvent::MouseMove { x: n.0, y: n.1 });
                        }
                        self.held_buttons.insert(button as usize);
                        self.input_on(display, InputEvent::MouseButton { button: b, down: true });
                    } else if self.held_buttons.remove(&(button as usize)) {
                        self.input_on(display, InputEvent::MouseButton { button: b, down: false });
                    }
                }
                egui::Event::MouseWheel { unit, delta, .. } if hovered && self.captured => {
                    let lines = match unit {
                        egui::MouseWheelUnit::Line => delta,
                        egui::MouseWheelUnit::Point => delta / 40.0,
                        egui::MouseWheelUnit::Page => delta * 8.0,
                    };
                    self.input_on(display, InputEvent::Wheel { dx: lines.x, dy: lines.y });
                }
                _ => {}
            }
        }
    }

    /// Touch feedback on the picture: the trackpad pointer, a ring filling
    /// up towards a long press, and Fit when zoomed in.
    fn draw_touch_feedback(&mut self, ui: &mut Ui, sc: &mut Screen, area: Rect, rect: Rect) {
        let p = ui.painter().with_clip_rect(area);
        if self.trackpad && sc.touch.used() {
            let at = sc.touch.pointer_pos(rect);
            // An arrow, outlined so it shows on light and dark screens.
            let pts = [
                at,
                at + vec2(0.0, 17.0),
                at + vec2(4.5, 13.0),
                at + vec2(8.0, 20.0),
                at + vec2(11.0, 18.5),
                at + vec2(7.5, 11.5),
                at + vec2(13.0, 11.5),
            ];
            p.add(egui::Shape::convex_polygon(
                vec![pts[0], pts[1], pts[2], pts[5], pts[6]],
                Color32::WHITE,
                egui::Stroke::new(1.5, Color32::BLACK),
            ));
            p.add(egui::Shape::convex_polygon(
                vec![pts[2], pts[3], pts[4], pts[5]],
                Color32::WHITE,
                egui::Stroke::new(1.5, Color32::BLACK),
            ));
        }
        if let Some((at, progress)) = sc.touch.pressing(Instant::now()) {
            let r = 26.0;
            p.circle_stroke(at, r, egui::Stroke::new(3.0, pal::DIM.gamma_multiply(0.6)));
            let steps = 32;
            let end = (progress.clamp(0.0, 1.0) * steps as f32) as usize;
            let arc: Vec<_> = (0..=end)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 / steps as f32 * std::f32::consts::TAU;
                    at + vec2(a.cos(), a.sin()) * r
                })
                .collect();
            if arc.len() > 1 {
                p.add(egui::Shape::line(arc, egui::Stroke::new(4.0, pal::GOLD)));
            }
        }
        if touch_ui() && sc.view.zoomed() {
            let chip = Rect::from_min_size(pos2(area.right() - 76.0, area.top() + 10.0), vec2(64.0, 30.0));
            let resp = ui.interact(chip, ui.scope_id().with(("fit", sc.display)), Sense::click());
            p.rect_filled(chip, 15.0, pal::PANEL.gamma_multiply(0.9));
            p.rect_stroke(chip, 15.0, egui::Stroke::new(1.0, pal::BLUE), egui::StrokeKind::Inside);
            p.text(chip.center(), Align2::CENTER_CENTER, "FIT", theme::display(16.0), pal::BLUE);
            if resp.clicked() {
                sc.view = touch::View::default();
            }
        }
    }

    /// Phones: special keys above the soft keyboard. Ctrl, Alt, Shift and
    /// Win latch for the next key or typed letter.
    fn key_bar(&mut self, ui: &mut Ui, sc: &mut Screen, bar: Rect) {
        ui.painter().rect_filled(bar, 0.0, pal::PANEL);
        let mut child = ui.new_child(
            UiBuilder::new().max_rect(bar.shrink2(vec2(6.0, 4.0))).layout(Layout::left_to_right(Align::Center)),
        );
        // A field to keep the soft keyboard up; what's typed goes straight
        // to the remote computer (see `raw_input_hook`), so it stays empty.
        let mut sink = String::new();
        let field = child.add_sized(vec2(1.0, 30.0), egui::TextEdit::singleline(&mut sink).frame(egui::Frame::NONE));
        if !field.has_focus() {
            field.request_focus();
        }
        if !sink.is_empty() {
            // Typed while the link wasn't taking keys.
            self.type_latched(sc, &sink);
        }
        // A thin scroll rail that floats over the keys rather than below them.
        child.spacing_mut().scroll = egui::style::ScrollStyle::floating();
        egui::ScrollArea::horizontal().id_salt(("key-bar", sc.display)).show(&mut child, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let modifiers =
                    [("Ctrl", "ControlLeft"), ("Alt", "AltLeft"), ("Shift", "ShiftLeft"), ("Win", "MetaLeft")];
                for (label, code) in modifiers {
                    let on = sc.latched.contains(&code);
                    if theme::pill_sized(ui, label, if on { pal::GOLD } else { pal::VIOLET }, vec2(54.0, 32.0))
                        .clicked()
                    {
                        if on {
                            sc.latched.retain(|c| *c != code);
                        } else {
                            sc.latched.push(code);
                        }
                        self.captured = true;
                    }
                }
                for (label, code) in [
                    ("Esc", "Escape"),
                    ("Tab", "Tab"),
                    ("←", "ArrowLeft"),
                    ("↑", "ArrowUp"),
                    ("↓", "ArrowDown"),
                    ("→", "ArrowRight"),
                    ("Del", "Delete"),
                    ("⌫", "Backspace"),
                    ("Enter", "Enter"),
                    ("Home", "Home"),
                    ("End", "End"),
                    ("PgUp", "PageUp"),
                    ("PgDn", "PageDown"),
                ] {
                    if theme::pill_sized(
                        ui,
                        label,
                        pal::BLUE,
                        vec2(if label.chars().count() > 2 { 58.0 } else { 40.0 }, 32.0),
                    )
                    .clicked()
                    {
                        self.press_latched(sc, code);
                    }
                }
                for n in 1..=12 {
                    if theme::pill_sized(ui, &format!("F{n}"), pal::LAVENDER, vec2(44.0, 32.0)).clicked() {
                        self.press_latched(sc, F_KEYS[n - 1]);
                    }
                }
            });
        });
    }

    /// Presses `code` with the latched modifiers, then lets them go.
    fn press_latched(&mut self, sc: &mut Screen, code: &'static str) {
        self.captured = true;
        let mut combo: Vec<&str> = std::mem::take(&mut sc.latched);
        combo.push(code);
        self.send_combo(&combo);
    }

    /// Text from the soft keyboard: with a modifier latched, each character
    /// is a shortcut (Ctrl+C); otherwise it's typed as it is.
    fn type_latched(&mut self, sc: &mut Screen, text: &str) {
        if sc.latched.is_empty() {
            self.type_text(text);
            return;
        }
        let latched = std::mem::take(&mut sc.latched);
        for c in text.chars() {
            let Some((code, shift)) = slipstream_core::proto::us_key_for_char(c.to_ascii_lowercase()) else { continue };
            let mut combo = latched.clone();
            if shift && !combo.contains(&"ShiftLeft") {
                combo.push("ShiftLeft");
            }
            combo.push(code);
            self.send_combo(&combo);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, sc: &mut Screen, bar: Rect, t: f64, rail_width: Option<f32>) -> TabAction {
        let mut action = TabAction::None;
        let painter = ui.painter().clone();
        painter.rect_filled(bar, 0.0, pal::BG);
        let rail = rail_width.unwrap_or(0.0);
        if rail_width.is_some() {
            let turn = Rect::from_min_max(bar.min, pos2(bar.left() + rail.max(36.0) + 32.0, bar.bottom()));
            painter.rect_filled(turn, egui::CornerRadius { nw: 32, ..Default::default() }, pal::ORANGE);
            painter.text(
                pos2(turn.right() - 17.0, turn.center().y),
                Align2::CENTER_CENTER,
                "LINK",
                theme::display_bold(14.0),
                pal::BG,
            );
        } else {
            painter.rect_filled(
                Rect::from_min_size(bar.min + vec2(8.0, 6.0), vec2(36.0, bar.height() - 12.0)),
                egui::CornerRadius { nw: 10, sw: 10, ..Default::default() },
                pal::ORANGE,
            );
        }
        let ctx = ui.ctx().clone();
        let start = rail.max(36.0) + 36.0;
        let inner = bar.with_min_x(bar.left() + start);
        let w = inner.width();
        if w < 780.0 {
            return self.toolbar_narrow(ui, sc, inner, &ctx, t);
        }
        let compact_stats = w < 1440.0;
        let tight = w < 1050.0;

        // Right side first, so the buttons know how much room is left.
        let mut right = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 4.0;
        if theme::console_button(&mut right, if tight { "" } else { "End link" }, pal::RED, Icon::End, false)
            .on_hover_text("End this link")
            .clicked()
        {
            self.send(ViewerCmd::Disconnect);
            action = TabAction::Close;
        }
        theme::session_stats_frame().show(&mut right, |ui| {
            ui.set_min_height(34.0);
            self.stats_readout(ui, if compact_stats { Detail::Compact } else { Detail::Full }, t);
        });
        let quality = theme::console_button(
            &mut right,
            if tight { "" } else { PRESETS[self.preset].0 },
            pal::BLUE,
            Icon::Performance,
            true,
        )
        .on_hover_text(format!("Picture quality · {}", PRESETS[self.preset].0));
        theme::console_menu(&quality).show(|ui| self.quality_menu(ui));
        let right_left = right.min_rect().left();

        let left_rect = inner.with_max_x(right_left - 12.0);
        let mut child = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
        child.set_clip_rect(left_rect.expand2(vec2(0.0, 8.0)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 4.0;
        // Measure the actual labels after reserving quality/stats/End. Long
        // presets, several screens and active transfers must not clip tools.
        let mut nav = vec![("Bridge", false), (if sc.fullscreen { "Window" } else { "Full" }, false)];
        if !ctx.embed_viewports() {
            nav.push((if sc.detached { "Dock" } else { "Pop out" }, false));
        }
        if self.info.displays.len() > 1 {
            nav.push(("Screens", true));
        }
        let mut tools = vec![
            ("Keys", true),
            (if self.unread { "Chat ◆" } else { "Chat" }, false),
            (if self.audio { "Sound" } else { "Muted" }, false),
            ("Tools", true),
        ];
        if self.info.capabilities.file_transfer {
            tools.push(("Files", false));
        }
        let count = nav.len() + tools.len();
        let icons = nav.iter().chain(&tools).map(|(_, menu)| theme::console_button_width(ui, "", *menu)).sum::<f32>()
            + (count - 1) as f32 * 4.0;
        let extra = |buttons: &[(&str, bool)]| {
            buttons
                .iter()
                .map(|(label, menu)| {
                    theme::console_button_width(ui, label, *menu) - theme::console_button_width(ui, "", *menu)
                })
                .sum::<f32>()
        };
        let tool_labels = left_rect.width() >= icons + extra(&tools) + 4.0;
        let tight = left_rect.width() < icons + extra(&tools) + extra(&nav) + 4.0;
        let icon_only_back = tight;

        if tight {
            ui.spacing_mut().button_padding.x = 8.0;
        }

        let back_tip = format!("Back to the Bridge (the link with {} keeps running)", self.info.host_name);
        if theme::console_button(ui, if icon_only_back { "" } else { "Bridge" }, pal::PEACH, Icon::Back, false)
            .on_hover_text(back_tip)
            .clicked()
        {
            self.release_keyboard();
            action = TabAction::Bridge;
        }

        if !ctx.embed_viewports()
            && theme::console_button(
                ui,
                if tight {
                    ""
                } else if sc.detached {
                    "Dock"
                } else {
                    "Pop out"
                },
                pal::BLUE,
                Icon::Window,
                false,
            )
            .on_hover_text(if sc.detached {
                "Move this screen into the main console"
            } else {
                "Open this screen in its own window"
            })
            .clicked()
        {
            self.release_keyboard();
            action = if sc.detached { TabAction::Dock } else { TabAction::Detach };
        }

        if self.info.displays.len() > 1 {
            let screens =
                theme::console_button(ui, if tight { "" } else { "Screens" }, pal::VIOLET, Icon::Screens, true);
            theme::console_menu(&screens).show(|ui| self.screens_menu(ui));
            screens.on_hover_text("The remote computer's screens: each ticked one has a tab of its own");
        }

        if theme::console_button(
            ui,
            if tight {
                ""
            } else if sc.fullscreen {
                "Window"
            } else {
                "Full"
            },
            pal::BLUE,
            if sc.fullscreen { Icon::Window } else { Icon::Fullscreen },
            false,
        )
        .on_hover_text("Full screen on/off (Ctrl+Alt+Enter)")
        .clicked()
        {
            self.toggle_fullscreen(sc, &ctx);
        }

        let keys = theme::console_button(ui, if tool_labels { "Keys" } else { "" }, pal::LAVENDER, Icon::Keys, true);
        theme::console_menu(&keys).show(|ui| self.keys_menu(ui));

        let chat_label = if self.unread { "Chat ◆" } else { "Chat" };
        if theme::console_button(ui, if tool_labels { chat_label } else { "" }, pal::LAVENDER, Icon::Chat, false)
            .on_hover_text(chat_label)
            .clicked()
        {
            self.show_chat = !self.show_chat;
        }
        if self.info.capabilities.file_transfer
            && theme::console_button(ui, if tool_labels { "Files" } else { "" }, pal::GOLD, Icon::Files, false)
                .on_hover_text("Browse and transfer files")
                .clicked()
        {
            {
                let mut files = self.files.lock().unwrap();
                files.open = !files.open;
            }
            theme::repaint_console(&ctx);
            self.release_keyboard();
        }

        // Sound: gold while playing.
        let can_hear = self.info.capabilities.audio;
        let (label, color, tip) = match (can_hear, self.audio) {
            (false, _) => (
                "Sound",
                pal::DIM,
                "That computer doesn't share its sound (not supported there yet, or switched off in its Config)",
            ),
            (true, true) => ("Sound", pal::GOLD, "Playing the remote computer's sound. Click to mute"),
            (true, false) => ("Muted", pal::LAVENDER.gamma_multiply(0.6), "Click to hear the remote computer's sound"),
        };
        let sound = ui
            .add_enabled(can_hear, |ui: &mut egui::Ui| {
                theme::console_button(
                    ui,
                    if tool_labels { label } else { "" },
                    color,
                    if self.audio { Icon::Sound } else { Icon::Muted },
                    false,
                )
            })
            .on_hover_text(tip)
            .on_disabled_hover_text(tip);
        if sound.clicked() {
            self.audio = !self.audio;
            self.send(ViewerCmd::Audio(self.audio));
        }

        // Tools: privacy mode, port forwarding. Gold while privacy is on.
        let tools_color = if self.privacy { pal::GOLD } else { pal::LAVENDER };
        let tools = theme::console_button(ui, if tool_labels { "Tools" } else { "" }, tools_color, Icon::Tools, true);
        theme::console_menu(&tools).show(|ui| self.tools_menu(ui));

        if sc.fullscreen || touch_ui() {
            let gap =
                Rect::from_min_max(pos2(ui.min_rect().right() + 4.0, bar.top()), pos2(right_left - 4.0, bar.bottom()));
            theme::session_rail_label(&painter, gap, &self.info.host_name.to_uppercase());
        }
        action
    }

    /// The bar on a phone: back, a Console menu with everything else, the
    /// link's figures and End.
    fn toolbar_narrow(&mut self, ui: &mut Ui, sc: &mut Screen, inner: Rect, ctx: &egui::Context, t: f64) -> TabAction {
        let mut action = TabAction::None;
        let mut right = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 4.0;
        if theme::console_button(&mut right, "", pal::RED, Icon::End, false).on_hover_text("End this link").clicked() {
            self.send(ViewerCmd::Disconnect);
            action = TabAction::Close;
        }
        theme::session_stats_frame().show(&mut right, |ui| {
            ui.set_min_height(34.0);
            self.stats_readout(ui, if touch_ui() { Detail::Fps } else { Detail::Compact }, t)
        });
        if !touch_ui() {
            let quality = theme::console_button(&mut right, "", pal::BLUE, Icon::Performance, true)
                .on_hover_text(format!("Picture quality · {}", PRESETS[self.preset].0));
            theme::console_menu(&quality).show(|ui| self.quality_menu(ui));
        }
        let left_rect = inner.with_max_x(right.min_rect().left() - 8.0);
        let mut left = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
        left.set_clip_rect(left_rect.expand2(vec2(0.0, 8.0)));
        left.spacing_mut().item_spacing.x = 4.0;
        if theme::console_button(&mut left, "", pal::PEACH, Icon::Back, false).clicked() {
            self.release_keyboard();
            action = TabAction::Bridge;
        }
        let console = theme::console_button(
            &mut left,
            if left_rect.width() < 300.0 { "" } else { "Console" },
            if self.unread { pal::GOLD } else { pal::LAVENDER },
            Icon::Console,
            true,
        );
        theme::console_menu(&console).show(|ui| {
            ui.set_min_width(230.0);
            if sc.fullscreen || touch_ui() {
                ui.label(RichText::new(self.info.host_name.to_uppercase()).font(theme::display_bold(18.0)).color(pal::TAN));
                ui.separator();
            }
            if !ctx.embed_viewports() && ui.button(if sc.detached {"Dock in main console"} else {"Open in separate window"}).clicked() {
                self.release_keyboard();
                action = if sc.detached {TabAction::Dock} else {TabAction::Detach};
                ui.close();
            }
            if self.info.displays.len() > 1 {
                ui.label(RichText::new("SCREEN").font(theme::display(15.0)).color(pal::LAVENDER));
                ui.horizontal_wrapped(|ui| {
                    for d in self.wanted_displays() {
                        let color = if d == sc.display { pal::GOLD } else { pal::VIOLET };
                        if theme::pill(ui, &format!("{}", d + 1), color).clicked() {
                            action = TabAction::Screen(d);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Screens to show", |ui| self.screens_menu(ui));
                ui.separator();
            }
            if sc.view.zoomed() && ui.button("Fit to screen").clicked() {
                sc.view = touch::View::default();
                ui.close();
            }
            if ui.button(if self.trackpad { "Control: trackpad · switch to touch" } else { "Control: touch · switch to trackpad" })
                .on_hover_text("Trackpad: slide a finger to move the pointer, tap to click. Touch: tap exactly where you want to click.")
                .clicked()
            {
                self.toggle_trackpad();
                ui.close();
            }
            if ui.button(if sc.fullscreen { "Leave full screen" } else { "Full screen" }).clicked() {
                self.toggle_fullscreen(sc, ctx);
                ui.close();
            }
            ui.menu_button("Keys", |ui| self.keys_menu(ui));
            if ui.button(if self.show_chat { "Hide chat" } else if self.unread { "Chat ◆" } else { "Chat" }).clicked() {
                self.show_chat = !self.show_chat;
                ui.close();
            }
            if self.info.capabilities.file_transfer && ui.button("Files").clicked() {
                { let mut files=self.files.lock().unwrap();files.open = !files.open; }
                theme::repaint_console(ctx);
                self.release_keyboard();
                ui.close();
            }
            if self.info.capabilities.audio && ui.button(if self.audio { "Mute sound" } else { "Play sound" }).clicked() {
                self.audio = !self.audio;
                self.send(ViewerCmd::Audio(self.audio));
                ui.close();
            }
            ui.menu_button("Tools", |ui| self.tools_menu(ui));
            ui.menu_button(format!("Quality: {}", PRESETS[self.preset].0), |ui| self.quality_menu(ui));
        });
        // Touch screens: the keyboard and the way fingers control the pointer, a tap away.
        if touch_ui() || ctx.input(|i| i.any_touches()) {
            if theme::console_button(
                &mut left,
                "",
                if sc.keyboard { pal::GOLD } else { pal::LAVENDER },
                Icon::Keys,
                false,
            )
            .on_hover_text("Keyboard: type on the remote computer, with Ctrl, Alt, arrows and F-keys")
            .clicked()
            {
                sc.keyboard = !sc.keyboard;
                sc.latched.clear();
                self.captured |= sc.keyboard;
            }
            let (icon, tip) = if self.trackpad {
                (
                    Icon::Trackpad,
                    "Trackpad: slide to move the pointer, tap to click, two-finger tap to right-click. Tap for direct touch.",
                )
            } else {
                (Icon::Touch, "Touch: tap where you want to click, hold to right-click. Tap for trackpad.")
            };
            if theme::console_button(&mut left, "", pal::BLUE, icon, false).on_hover_text(tip).clicked() {
                self.toggle_trackpad();
            }
        }
        if sc.fullscreen || touch_ui() {
            let gap = Rect::from_min_max(
                pos2(left.min_rect().right() + 4.0, inner.top()),
                pos2(right.min_rect().left() - 4.0, inner.bottom()),
            );
            theme::session_rail_label(ui.painter(), gap, &self.info.host_name.to_uppercase());
        }
        action
    }

    /// Switches between trackpad and direct touch, and says what it means.
    fn toggle_trackpad(&mut self) {
        self.trackpad = !self.trackpad;
        self.notice = Some((
            if self.trackpad {
                TRACKPAD_HINT.into()
            } else {
                "Touch: tap to click · hold to right-click · drag to drag · two fingers scroll · pinch to zoom".into()
            },
            Instant::now(),
        ));
    }

    /// The remote computer's screens, ticked when they have a tab. The
    /// last one ticked can't be unticked (End link ends it all).
    fn screens_menu(&mut self, ui: &mut Ui) {
        let displays = self.info.displays.clone();
        let ticked = displays.iter().filter(|d| !self.hidden.contains(&d.index)).count();
        for d in &displays {
            let mut on = !self.hidden.contains(&d.index);
            let label = format!(
                "Screen {} · {} · {}×{}{}",
                d.index + 1,
                d.name,
                d.width,
                d.height,
                if d.primary { " (main)" } else { "" }
            );
            if ui.add_enabled(!(on && ticked == 1), egui::Checkbox::new(&mut on, label)).changed() {
                if on {
                    self.hidden.remove(&d.index);
                } else {
                    self.hide_display(d.index);
                }
            }
        }
        ui.label(
            RichText::new("Each ticked screen has a tab: drag one out to see it in a window of its own")
                .small()
                .color(pal::DIM),
        );
    }

    /// Key combinations to send.
    fn keys_menu(&mut self, ui: &mut Ui) {
        if self.info.os.eq_ignore_ascii_case("android") {
            for (label, code) in [
                ("Android · Back", "AndroidBack"),
                ("Android · Home", "AndroidHome"),
                ("Android · Recents", "AndroidRecents"),
            ] {
                if ui.add_enabled(self.info.capabilities.input, egui::Button::new(label)).clicked() {
                    self.send_combo(&[code]);
                    ui.close();
                }
            }
            return;
        }
        for (label, keys) in [
            ("Ctrl + Alt + Del", &["ControlLeft", "AltLeft", "Delete"][..]),
            ("Alt + Tab", &["AltLeft", "Tab"][..]),
            ("Super / Windows key", &["MetaLeft"][..]),
            ("Alt + F4", &["AltLeft", "F4"][..]),
            ("Print Screen", &["PrintScreen"][..]),
            ("Ctrl + Esc", &["ControlLeft", "Escape"][..]),
        ] {
            if ui.button(label).clicked() {
                self.send_combo(keys);
                ui.close();
            }
        }
    }

    /// Privacy mode, port forwarding, system information.
    fn tools_menu(&mut self, ui: &mut Ui) {
        if ui
            .button("System information")
            .on_hover_text(
                "Its processor, memory, graphics, drives, temperatures, network and Windows/macOS/Linux version",
            )
            .clicked()
        {
            self.show_sysinfo = true;
            self.sysinfo_fetch = true;
            ui.close();
        }
        if ui.button("Support report").clicked() {
            self.report.open = true;
            self.release_keyboard();
            ui.close();
        }
        if ui
            .add_enabled(self.info.capabilities.network_scan, egui::Button::new("Network scan"))
            .on_hover_text("Discover devices and open TCP ports on the remote computer's network")
            .on_disabled_hover_text("Not allowed on that computer (Config → Permissions → Network scan)")
            .clicked()
        {
            self.release_keyboard();
            self.network.lock().expect("network window").open = true;
            theme::repaint_console(ui.ctx());
            ui.close();
        }
        let (privacy_ok, tunnels_ok) = (self.info.capabilities.privacy, self.info.capabilities.tunnels);
        let privacy_label = if self.privacy { "Blank remote screen · ON" } else { "Blank remote screen" };
        let r = ui
            .add_enabled(privacy_ok, egui::Button::selectable(self.privacy, privacy_label))
            .on_hover_text("Privacy mode: cover the remote computer's physical displays while you continue to see its desktop. Useful when people nearby should not see your work. With the Slipstream service, local keyboard and mouse input is also blocked. Turning it off or ending the session removes the cover.")
            .on_disabled_hover_text("Blank remote screen (privacy mode) is unavailable on this computer. It needs Windows capture-exclusion support (Windows 10 version 2004 or later); older systems such as Windows Server 2016 cannot hide the cover from your remote picture.");
        if r.clicked() {
            self.privacy = !self.privacy;
            self.send(ViewerCmd::Privacy(self.privacy));
            ui.close();
        }
        let r = ui
            .add_enabled(tunnels_ok, egui::Button::new("Port forwarding"))
            .on_hover_text("Reach a service on the remote computer or its network, e.g. its Remote Desktop")
            .on_disabled_hover_text("Not allowed on that computer (its Config → Permissions → Port forwarding)");
        if r.clicked() {
            self.show_tunnels = true;
            ui.close();
        }
        ui.separator();
        use slipstream_core::proto::PowerAction;
        let control = self.info.capabilities.input;
        for (action, hover) in [
            (PowerAction::Restart, "Restart the remote computer and reconnect when it's back"),
            (
                PowerAction::RestartSafeMode,
                "Windows with the Slipstream service: start once in Safe Mode with Networking (the next restart is normal), then reconnect",
            ),
            (
                PowerAction::RestartToFirmware,
                "UEFI computers: restart into the BIOS/firmware setup. You'll only see it through a management controller (Manage on the Bridge)",
            ),
        ] {
            if ui
                .add_enabled(control && self.power_reply.is_none(), egui::Button::new(action.label()))
                .on_hover_text(hover)
                .on_disabled_hover_text("Needs Control permission on that computer")
                .clicked()
            {
                self.power_confirm = Some(action);
                self.release_keyboard();
                ui.close();
            }
        }
    }

    /// The restart confirmation, and its answer.
    fn power_ui(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        if let Some(rx) = &mut self.power_reply
            && let Ok(result) = rx.try_recv()
        {
            self.power_reply = None;
            match result {
                Ok(()) => {
                    self.restart_expected = Some(Instant::now());
                    self.notice =
                        Some(("Restarting · this tab reconnects when the computer is back".into(), Instant::now()));
                }
                Err(e) => self.notice = Some((format!("Couldn't restart: {e}"), Instant::now())),
            }
        }
        let Some(action) = self.power_confirm else { return };
        let (mut go, mut cancel) = (false, false);
        theme::modal("restart-remote", pal::RED).show(ctx, |ui| {
            ui.set_width(theme::dialog_width(ctx, 420.0));
            cancel = theme::window_header_with_close(ui, &format!("{}?", action.label()), pal::RED, pal::BLUE).close;
            ui.label(format!("{} restarts in a few seconds. Unsaved work there is lost.", self.info.host_name));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                go = theme::pill(ui, action.label(), pal::RED).clicked();
                cancel |= theme::pill(ui, "Cancel", pal::BLUE).clicked();
            });
        });
        if go {
            self.power_confirm = None;
            let (tx, rx) = tokio::sync::oneshot::channel();
            let (fs, wake) = (self.session.fs(), ctx.clone());
            rt.spawn(async move {
                let _ = tx.send(fs.power(action).await.map_err(|e| format!("{e:#}")));
                wake.request_repaint();
            });
            self.power_reply = Some(rx);
        } else if cancel {
            self.power_confirm = None;
        }
    }

    /// A preset at this viewer's frame rate: the full rate for Balanced and
    /// Quality, at most the preset's own for Speed and Low bandwidth.
    fn preset_settings(&self, preset: usize) -> StreamSettings {
        let mut settings = PRESETS[preset].1;
        let wanted = self.session.max_fps().clamp(1, 144);
        settings.max_fps = if settings.adaptive_resolution { settings.max_fps.min(wanted) } else { wanted };
        settings
    }

    /// The picture quality presets.
    fn quality_menu(&mut self, ui: &mut Ui) {
        for (i, (name, _)) in PRESETS.iter().enumerate() {
            if ui.selectable_label(i == self.preset, *name)
                .on_hover_text(if PRESETS[i].1.adaptive_resolution {
                    "May reduce picture size on a slow computer to keep motion responsive; sharpens when still"
                } else {
                    "Keeps full picture resolution while moving; frame rate follows the remote computer's encoding speed"
                }).clicked() {
                self.preset = i;
                self.send(ViewerCmd::Settings(self.preset_settings(i)));
                ui.close();
            }
        }
    }

    /// The port forwarding window: open tunnels and a form to add one.
    /// `fresh`: opening the window (a spinner and the top of the list).
    /// Otherwise the figures on show are replaced in place when the new
    /// ones arrive, and kept if the refresh fails.
    fn fetch_sysinfo(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, fresh: bool) {
        use std::sync::atomic::Ordering;
        self.sysinfo_fetch = false;
        self.sysinfo_at = Some(Instant::now());
        if self.sysinfo_busy.swap(true, Ordering::AcqRel) {
            return;
        }
        if fresh {
            // Unique across runs too: egui keeps scroll positions between them.
            self.sysinfo_serial = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            *self.sysinfo.lock().unwrap() = None;
        }
        let (slot, busy, wake, job) =
            (self.sysinfo.clone(), self.sysinfo_busy.clone(), ctx.clone(), self.session.system_info());
        rt.spawn(async move {
            let r = job.await.map_err(|e| format!("{e:#}"));
            {
                let mut slot = slot.lock().unwrap();
                if r.is_ok() || !matches!(*slot, Some(Ok(_))) {
                    *slot = Some(r);
                }
            }
            busy.store(false, Ordering::Release);
            wake.request_repaint();
        });
    }

    fn fetch_public_ip(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, refresh: bool) {
        if self.public_ip_rx.is_some() || self.closed.is_some() {
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (fs, wake) = (self.session.fs(), ctx.clone());
        self.public_ip_rx = Some(rx);
        self.public_ip_at = Some(Instant::now());
        rt.spawn(async move {
            let result = fs.public_ip(refresh).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            wake.request_repaint_of(egui::ViewportId::ROOT);
        });
    }

    fn sysinfo_window(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        if !self.show_sysinfo {
            return;
        }
        if let Some(rx) = &mut self.public_ip_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.public_ip = Some(result);
                    self.public_ip_rx = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.public_ip = Some(Err("Public-IP request ended without an answer".into()));
                    self.public_ip_rx = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if self.public_ip_at.is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(60)) {
            self.fetch_public_ip(ctx, rt, false);
        }
        if self.sysinfo_fetch {
            self.fetch_sysinfo(ctx, rt, true);
        } else if self.sysinfo_at.is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(3))
            && matches!(*self.sysinfo.lock().unwrap(), Some(Ok(_)))
        {
            self.fetch_sysinfo(ctx, rt, false);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let mut open = true;
        let mut refresh = false;
        let state = self.sysinfo.lock().unwrap().clone();
        let title = format!("System · {}", self.info.host_name);
        theme::window("sysinfo", pal::GOLD)
            .id(egui::Id::unique(("sysinfo", self.id)))
            .collapsible(false)
            .default_pos(ctx.content_rect().min + vec2(24.0, 60.0))
            .default_width(680.0)
            .default_height((ctx.content_rect().height() - 150.0).clamp(300.0, 700.0))
            .show(ctx, |ui| {
                let chrome = theme::window_header(ui, &title, pal::GOLD);
                if chrome.close {
                    open = false;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("PUBLIC IP").font(theme::display(16.0)).color(pal::LAVENDER))
                        .on_hover_text("The remote computer's outbound internet address, observed by ipify over HTTPS. VPN, proxy or NAT routing may affect this address. Cached for up to one minute.");
                    match &self.public_ip {
                        Some(Ok(address)) => {
                            ui.label(RichText::new(address.to_string()).monospace().color(pal::BLUE));
                            if theme::pill_sized(ui,"Copy IP",pal::BLUE,vec2(74.0,26.0)).clicked() { ui.ctx().copy_text(address.to_string()); }
                        },
                        Some(Err(error)) => { ui.label(RichText::new("Unavailable").color(pal::DIM)).on_hover_text(error); },
                        None => { ui.label(RichText::new("Looking up from the remote computer…").color(pal::DIM)); },
                    }
                    if self.public_ip_rx.is_some() { ui.spinner(); }
                });
                ui.add_space(6.0);
                match &state {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Asking the remote computer…").color(pal::DIM));
                        });
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(pal::RED));
                    }
                    Some(Ok(sections)) => {
                        ui.horizontal(|ui| {
                            if theme::pill(ui, "Refresh", pal::BLUE).clicked() {
                                refresh = true;
                            }
                            if theme::pill(ui, "Copy all", pal::LAVENDER).on_hover_text("As text, e.g. for a ticket").clicked() {
                                let public = match &self.public_ip { Some(Ok(ip)) => ip.to_string(), Some(Err(_)) => "Unavailable".into(), None => "Looking up…".into() };
                                ui.ctx().copy_text(format!("{}\nPublic IP: {public}\n",sysinfo_text(&self.info.host_name, sections)));
                                self.notice = Some(("System information copied".into(), Instant::now()));
                            }
                            ui.label(RichText::new("Live: use, memory and temperatures update every 3 s").small().color(pal::DIM));
                        });
                        ui.add_space(6.0);
                        // A new scroll area per answer, so it starts at the top
                        // (egui remembers scroll positions by id, across runs too).
                        egui::ScrollArea::vertical()
                            .id_salt(("sysinfo", self.id, self.sysinfo_serial))
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - theme::FOOTER_SPACE)
                            .show(
                            ui,
                            |ui| {
                                // The label column fits the longest label, up to two
                                // fifths of the width; longer labels wrap, and the values
                                // start right beside it with the rest of the width.
                                let width = ui.available_width();
                                let gap = 16.0;
                                let longest = sections
                                    .iter()
                                    .flat_map(|s| &s.rows)
                                    .map(|(k, _)| {
                                        egui::WidgetText::from(k.as_str())
                                            .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body)
                                            .size()
                                            .x
                                    })
                                    .fold(0.0f32, f32::max);
                                let label_w = (longest + 2.0).min(width * 0.4).max(60.0);
                                ui.spacing_mut().item_spacing.y = 5.0;
                                for s in sections {
                                    ui.label(RichText::new(s.title.to_uppercase()).font(theme::display(16.0)).color(pal::LAVENDER));
                                    for (k, v) in &s.rows {
                                        ui.horizontal_top(|ui| {
                                            ui.spacing_mut().item_spacing.x = gap;
                                            ui.allocate_ui_with_layout(vec2(label_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                                                ui.set_width(label_w);
                                                ui.add(egui::Label::new(RichText::new(k).color(pal::DIM)).wrap());
                                            });
                                            ui.allocate_ui_with_layout(vec2((width - label_w - gap).max(60.0), 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                                                ui.add(egui::Label::new(RichText::new(v).color(pal::TEXT)).wrap());
                                            });
                                        });
                                    }
                                }
                            },
                        );
                    }
                }
                chrome.footer(ui);
            });
        if refresh {
            self.fetch_sysinfo(ctx, rt, false);
            self.fetch_public_ip(ctx, rt, true);
        }
        if !open {
            self.show_sysinfo = false;
            theme::repaint_console(ctx);
            // The Close click was drawn with the panel still present. Redraw
            // this pass without it instead of waiting for focus/input to return.
            ctx.request_discard("System information closed");
        }
    }

    fn tunnels_window(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        if !self.show_tunnels {
            return;
        }
        let mut open = true;
        let mut close: Option<usize> = None;
        let mut add = false;
        theme::window("port-forwarding", pal::GOLD)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                let chrome = theme::window_header(ui, "Port forwarding", pal::GOLD);
                if chrome.close {
                    open = false;
                }
                ui.label(
                    RichText::new(format!(
                        "A port on this computer that reaches a service as {} sees it: its own (localhost) or another machine on its network.",
                        self.info.host_name
                    ))
                    .color(pal::DIM),
                );
                ui.add_space(6.0);
                for (i, t) in self.tunnels.iter().enumerate() {
                    let st = t.status();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("127.0.0.1:{} → {}:{}", t.local_port, t.host, t.port)).font(theme::mono(16.0)).color(pal::TAN));
                        ui.label(RichText::new(format!("{} open · {} so far", st.active, st.total)).color(pal::DIM));
                        if ui.small_button("Copy").on_hover_text("Copy the local address").clicked() {
                            ui.ctx().copy_text(format!("127.0.0.1:{}", t.local_port));
                        }
                        if ui.small_button("Close").clicked() {
                            close = Some(i);
                        }
                    });
                    if let Some(e) = st.last_error {
                        ui.label(RichText::new(e).small().color(pal::RED));
                    }
                }
                if !self.tunnels.is_empty() {
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    let (local, host, port) = &mut self.tunnel_form;
                    ui.label("Local port");
                    ui.add(theme::text_input(local).align(egui::Align2::LEFT_CENTER).hint_text("auto").desired_width(60.0));
                    ui.label("→");
                    ui.add(theme::text_input(host).align(egui::Align2::LEFT_CENTER).hint_text("localhost").desired_width(130.0));
                    ui.label(":");
                    ui.add(theme::text_input(port).align(egui::Align2::LEFT_CENTER).hint_text("3389").desired_width(60.0));
                    add = theme::pill(ui, "Forward", pal::GOLD).clicked();
                });
                if let Some(e) = &self.tunnel_error {
                    ui.label(RichText::new(e).color(pal::RED));
                }
                ui.label(
                    RichText::new("Example: local port 13389 → localhost : 3389, then open Remote Desktop to 127.0.0.1:13389.")
                        .small()
                        .color(pal::DIM),
                );
                chrome.footer(ui);
            });
        if let Some(i) = close {
            self.tunnels.remove(i);
        }
        if add {
            let (local, host, port) = &self.tunnel_form;
            let local: Result<u16, _> = if local.trim().is_empty() { Ok(0) } else { local.trim().parse() };
            let host = if host.trim().is_empty() { "localhost".to_string() } else { host.trim().to_string() };
            match (local, port.trim().parse::<u16>()) {
                (Ok(local), Ok(port)) => match rt.block_on(self.session.tunnel(local, host, port)) {
                    Ok(t) => {
                        self.tunnels.push(t);
                        self.tunnel_error = None;
                        self.tunnel_form.0.clear();
                        self.tunnel_form.2.clear();
                    }
                    Err(e) => self.tunnel_error = Some(format!("{e:#}")),
                },
                _ => self.tunnel_error = Some("Enter port numbers (1–65535)".into()),
            }
        }
        if !open {
            self.show_tunnels = false;
        }
        if !self.tunnels.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    /// Frame rate, bandwidth, round trip and path, right to left; the full
    /// details are in the tooltip when `compact`.
    fn stats_readout(&mut self, ui: &mut Ui, detail: Detail, t: f64) {
        let compact = detail != Detail::Full;
        let s = &self.stats;
        let (fps, mbps, rtt) = (
            theme::glide(ui, "fps", s.fps),
            theme::glide(ui, "mbps", s.kbps / 1000.0),
            theme::glide(ui, "rtt", s.rtt_ms),
        );
        let path = if s.relayed { ("RELAY", pal::GOLD) } else { ("DIRECT", pal::GREEN) };
        // A still screen streams at a few dozen kbit/s: "0.0 MBPS" told
        // nothing, so below 1 Mbit/s it's kbit/s.
        let (rate, rate_unit) =
            if mbps < 1.0 { (format!("{:.0}", mbps * 1000.0), "KBPS") } else { (format!("{mbps:.1}"), "MBPS") };
        let mut details = format!(
            "{fps:.0} frames a second · {rate} {} · {rtt:.0} ms round trip · {} · decoding {:.1} ms a frame",
            if rate_unit == "KBPS" { "kbit/s" } else { "Mbit/s" },
            if s.relayed { "through a relay" } else { "direct connection" },
            s.decode_ms
        );
        if s.width > 0 && s.height > 0 {
            details.push_str(&format!(
                "\nEncoded picture: {}×{} · {}",
                s.width,
                s.height,
                if PRESETS[self.preset].1.adaptive_resolution { "adaptive resolution" } else { "full resolution" }
            ));
        }
        if !s.host.is_empty() {
            details.push_str("\n\nOn the remote computer:");
            for st in &s.host {
                if st.fps == 0.0 && st.ms == 0.0 {
                    details.push_str(&format!("\n  {}", st.name));
                } else {
                    details.push_str(&format!("\n  {}: {:.0} a second, {:.1} ms each", st.name, st.fps, st.ms));
                }
            }
        }
        if !compact {
            ui.label(RichText::new(path.0).font(theme::display(16.0)).color(path.1));
        }
        let text = match detail {
            Detail::Fps => format!("{fps:.0} FPS"),
            Detail::Compact => format!("{fps:>3.0} FPS · {rtt:>3.0} MS"),
            Detail::Full => format!("{fps:>3.0} FPS · {rate:>4} {rate_unit} · {rtt:>3.0} MS"),
        };
        ui.label(RichText::new(text).font(theme::mono(15.0)).color(pal::BLUE)).on_hover_text(&details);
        let activity = self.files.lock().unwrap().activity();
        if let Some((n, frac)) = activity {
            let r = ui
                .add(
                    egui::Button::new(
                        RichText::new(if detail == Detail::Full {
                            format!("⇅ {n} · {:.0}%", frac * 100.0)
                        } else {
                            "⇅".into()
                        })
                        .font(theme::mono(14.0))
                        .color(pal::GOLD),
                    )
                    .frame(false),
                )
                .on_hover_text(format!("Show {n} transfers · {:.0}%", frac * 100.0));
            if r.clicked() {
                self.files.lock().unwrap().open = true;
            }
        }
        // The lamp shows the path when the label is left out.
        let lamp = if self.closed.is_some() {
            pal::RED
        } else if compact {
            path.1
        } else {
            pal::GREEN
        };
        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        theme::lamp(ui.painter(), r.center(), lamp, t);
        resp.on_hover_text(details);
    }

    fn closed_overlay(&mut self, ui: &mut Ui, area: Rect, reason: &str) -> Option<TabAction> {
        let progress = self.reconnect.as_ref().map(|r| r.progress.lock().unwrap().clone());
        let recovering = progress.is_some();
        let color = if recovering { pal::BLUE } else { pal::RED };
        let painter = ui.painter();
        painter.rect_filled(area, 0.0, Color32::BLACK.gamma_multiply(0.75));
        let panel = Rect::from_center_size(area.center(), vec2((area.width() - 24.0).clamp(260.0, 560.0), 240.0));
        theme::glow(painter, panel, 16.0, color, 1.0);
        painter.rect_filled(panel, 16.0, pal::PANEL);
        let mut child = ui.new_child(UiBuilder::new().max_rect(panel.shrink(20.0)));
        let ui = &mut child;
        theme::header(ui, if recovering { "Re-establishing link" } else { "Link terminated" }, color);
        ui.add_space(8.0);
        ui.label(RichText::new(reason).font(theme::body(17.0)));
        if let Some(progress) = progress {
            ui.add_space(8.0);
            ui.label(RichText::new(progress).color(pal::BLUE));
            ui.label(
                RichText::new("Retrying for up to two minutes. Transfers are not resumed automatically.")
                    .small()
                    .color(pal::DIM),
            );
        }
        ui.add_space(16.0);
        let mut out = None;
        ui.horizontal_wrapped(|ui| {
            if recovering {
                if theme::pill(ui, "Cancel reconnect", pal::ORANGE).clicked() {
                    tracing::info!(peer=%self.peer,"automatic reconnect canceled");
                    self.reconnect = None;
                }
            } else if theme::pill(ui, "Reconnect", pal::ORANGE).clicked() {
                self.reconnect_requested = true;
            }
            if theme::pill(ui, "End link", pal::RED).clicked() {
                out = Some(TabAction::Close);
            }
        });
        out
    }
}

/// How much of the link's figures the session bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    Full,
    /// Frame rate and round trip; the lamp shows the path.
    Compact,
    /// Frame rate only (a phone).
    Fps,
}

fn map_button(b: PointerButton) -> Option<MouseButton> {
    Some(match b {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::Back,
        PointerButton::Extra2 => MouseButton::Forward,
    })
}

/// The System information window as plain text.
fn sysinfo_text(host: &str, sections: &[slipstream_core::proto::InfoSection]) -> String {
    let mut out = format!("{host}\n");
    for s in sections {
        out.push_str(&format!("\n{}\n", s.title));
        for (k, v) in &s.rows {
            out.push_str(&format!("  {k}: {v}\n"));
        }
    }
    out
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use slipstream_core::config::Store;
    use slipstream_session::{AudioOut, Client, ClientOptions};

    #[test]
    fn video_repaint_for_a_docked_or_closed_window_creates_no_native_viewport() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let floating = RemoteTab::window_id(7);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let sink = requests.clone();
        ctx.set_repaint_observer(move |request| sink.lock().unwrap().push(request.viewport_id));
        // Video callbacks run between UI passes, too. Requesting an unknown
        // viewport then sends the backend an event for a nonexistent window.
        theme::repaint_existing_viewport(&ctx, floating);
        theme::repaint_existing_after(&ctx, floating, std::time::Duration::from_millis(100));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            theme::repaint_existing_viewport(ui.ctx(), floating);
            theme::repaint_existing_after(ui.ctx(), floating, std::time::Duration::from_millis(100));
        });
        output.textures_delta.clear();
        assert!(!output.viewport_output.contains_key(&floating));
        assert!(!requests.lock().unwrap().contains(&floating));
        // A live floating viewport still receives a repaint request.
        let mut input = egui::RawInput::default();
        input.viewports.insert(floating, egui::ViewportInfo::default());
        let mut output = ctx.run_ui(input, |ui| {
            ui.ctx().show_viewport_deferred(
                floating,
                egui::ViewportBuilder::default().with_title("Workstation"),
                |_, _| {},
            );
            theme::repaint_existing_viewport(ui.ctx(), floating);
        });
        output.textures_delta.clear();
        assert_eq!(output.viewport_output[&floating].repaint_delay, std::time::Duration::ZERO);
        requests.lock().unwrap().clear();
        theme::repaint_existing_viewport(&ctx, floating);
        assert!(requests.lock().unwrap().contains(&floating));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn window_focus_routes_to_one_live_session_and_background_tools_keep_control() {
        let work = tempfile::tempdir().unwrap();
        let mut clients = Vec::new();
        for (name, host) in [("first", true), ("second", true), ("viewer", false)] {
            let store = Store::open(work.path().join(name)).unwrap();
            let mut settings = store.load_settings();
            settings.server.use_public_relays = false;
            settings.host.set_permanent_password(Some("window-test-password"));
            store.save_settings(&settings).unwrap();
            let (client, _) = Client::start(
                store,
                ClientOptions {
                    test_pattern: host,
                    accept_sessions: host,
                    clipboard: slipstream_platform::clipboard::ClipboardSource::Memory(Default::default()),
                    audio_out: AudioOut::Memory(Default::default()),
                    public_station: None,
                    system_screen: false,
                },
            )
            .await
            .unwrap();
            clients.push(client);
        }
        let ctx = egui::Context::default();
        let mut tabs = Vec::new();
        let mut links = Vec::new();
        for host in &clients[..2] {
            let (session, events) =
                clients[2].connect_direct(host.endpoint_addr(), "window-test-password", |_| {}).await.unwrap();
            let link = Arc::new(Mutex::new(Link::new(session, events, "window-test-password".into(), false, false)));
            let mut tab = RemoteTab::for_screen(&link, 0, &ctx);
            tab.detached = true;
            {
                let mut link = link.lock().unwrap();
                link.captured = true;
                link.held_keys.insert("ControlLeft");
            }
            tab.pending_keys.push(CapturedKey::ToggleFullscreen);
            tabs.push(Arc::new(Mutex::new(tab)));
            links.push(link);
        }
        let ids: Vec<u64> = tabs.iter().map(|t| t.lock().unwrap().id).collect();
        let route =
            |viewport, focused| super::super::route_session_input(&tabs, super::super::Page::Links, viewport, focused);
        assert!(Arc::ptr_eq(&route(RemoteTab::window_id(ids[0]), true).unwrap(), &tabs[0]));
        assert!(tabs[0].lock().unwrap().keyboard_captured());
        assert!(!tabs[1].lock().unwrap().keyboard_captured());
        assert!(tabs[1].lock().unwrap().pending_keys.is_empty());
        assert!(links[1].lock().unwrap().held_keys.is_empty());
        let tool = egui::ViewportId::from_hash_of("background-tool");
        assert!(route(tool, false).is_none());
        assert!(tabs[0].lock().unwrap().keyboard_captured());
        assert!(route(tool, true).is_none());
        assert!(!tabs[0].lock().unwrap().keyboard_captured());
        {
            links[1].lock().unwrap().captured = true;
            tabs[1].lock().unwrap().pending_keys.push(CapturedKey::ToggleFullscreen);
        }
        route(RemoteTab::window_id(ids[1]), false);
        assert!(!tabs[1].lock().unwrap().keyboard_captured());
        assert!(tabs[1].lock().unwrap().pending_keys.is_empty());
        // Only the selected member of a floating group receives input.
        let group = RemoteTab::window_id(ids[1]);
        {
            let mut first = tabs[0].lock().unwrap();
            first.window_group = Some(group);
            first.window_active = false;
        }
        assert!(Arc::ptr_eq(&route(group, true).unwrap(), &tabs[1]));
        {
            tabs[0].lock().unwrap().window_active = true;
            tabs[1].lock().unwrap().window_active = false;
        }
        assert!(Arc::ptr_eq(&route(group, true).unwrap(), &tabs[0]));
        tabs[0].lock().unwrap().detached = false;
        assert!(route(egui::ViewportId::ROOT, true).is_none());
        let docked =
            super::super::route_session_input(&tabs, super::super::Page::Remote(ids[0]), egui::ViewportId::ROOT, true)
                .unwrap();
        assert!(Arc::ptr_eq(&docked, &tabs[0]));
        drop(docked);
        // A second screen of the same link, in a window of its own: typing
        // there keeps the link's keyboard (the first screen isn't released).
        let mut second = RemoteTab::for_screen(&links[0], 1, &ctx);
        second.detached = true;
        let second_id = second.id;
        tabs.push(Arc::new(Mutex::new(second)));
        links[0].lock().unwrap().captured = true;
        tabs[0].lock().unwrap().pending_keys.push(CapturedKey::ToggleFullscreen);
        let routed =
            super::super::route_session_input(&tabs, super::super::Page::Links, RemoteTab::window_id(second_id), true)
                .unwrap();
        assert!(Arc::ptr_eq(&routed, &tabs[2]));
        assert!(links[0].lock().unwrap().keyboard_captured());
        assert!(!tabs[0].lock().unwrap().pending_keys.is_empty(), "a screen of the same link keeps its keys");
        drop(routed);
        for link in &links {
            link.lock().unwrap().end_link();
        }
        drop(tabs);
        drop(links);
        for client in &clients {
            client.shutdown().await;
        }
    }
}
