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

use super::{files::FileManager, home, keys, theme, theme::{pal,ConsoleIcon as Icon}, touch, video};

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
    progress:Arc<Mutex<String>>,
    rx:tokio::sync::oneshot::Receiver<super::ConnectResult>,
    task:tokio::task::JoinHandle<()>,
}
impl Drop for Reconnecting {fn drop(&mut self) {self.task.abort();}}

pub enum TabAction {
    None,
    Bridge,
    Close,
    Detach,
    Dock,
}

const PRESETS: [(&str, StreamSettings); 4] = [
    ("Balanced", StreamSettings { bitrate_kbps: 0, max_fps: 60, max_in_flight: 2, adaptive_resolution: false }),
    ("Speed", StreamSettings { bitrate_kbps: 0, max_fps: 30, max_in_flight: 1, adaptive_resolution: true }),
    ("Quality", StreamSettings { bitrate_kbps: 20_000, max_fps: 60, max_in_flight: 3, adaptive_resolution: false }),
    ("Low bandwidth", StreamSettings { bitrate_kbps: 1_200, max_fps: 20, max_in_flight: 1, adaptive_resolution: true }),
];

pub struct RemoteTab {
    pub id: u64,
    pub detached: bool,
    pub pending_keys: Vec<CapturedKey>,
    session: ViewerSession,
    cmds: mpsc::UnboundedSender<ViewerCmd>,
    events: mpsc::UnboundedReceiver<ViewerEvent>,
    frames: Arc<FrameShared>,
    pub info: SessionInfo,
    pub peer: DeviceId,
    password: String,
    /// The signed-in station this device was found on (`None`: this app's own).
    pub via: Option<slipstream_session::AccountRef>,
    pub stats: ViewerStats,
    pub closed: Option<String>,
    reconnect_requested:bool,
    reconnect:Option<Reconnecting>,
    display: u32,
    preset: usize,
    chat: Vec<(bool, String)>,
    chat_input: String,
    show_chat: bool,
    unread: bool,
    captured: bool,
    fullscreen: bool,
    /// Full screen: when the pointer last brought the toolbar up.
    toolbar_seen: Option<Instant>,
    video_rect: Option<Rect>,
    /// The whole picture fitted into the video area (the view zooms from it).
    video_fit: Option<Rect>,
    view: touch::View,
    touch: touch::Touch,
    last_pointer: Option<(f32, f32)>,
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
}

impl Drop for RemoteTab {
    fn drop(&mut self) {
        self.reconnect=None;
        let mut files=self.files.lock().unwrap();files.disconnect();files.open=false;files.progress_open=false;
        self.network.lock().unwrap().close();
    }
}

impl RemoteTab {
    pub fn window_id(id: u64) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("remote-session", id))
    }

    pub fn viewport_id(&self) -> egui::ViewportId {
        if self.detached { Self::window_id(self.id) } else { egui::ViewportId::ROOT }
    }

    pub fn end_link(&mut self) {
        self.release_keyboard();
        self.reconnect = None;
        self.detached = false;
        self.send(ViewerCmd::Disconnect);
    }

    pub fn new(
        id: u64,
        session: ViewerSession,
        events: mpsc::UnboundedReceiver<ViewerEvent>,
        password: String,
        ctx: &egui::Context,
        play_audio: bool,
    ) -> Self {
        let frames = session.frames();
        let wake = ctx.clone();
        frames.set_repaint(move || {
            wake.request_repaint_of(egui::ViewportId::ROOT);
            wake.request_repaint_of(Self::window_id(id));
        });
        let info = session.info().clone();
        let files = Arc::new(Mutex::new(FileManager::new(session.fs(), info.host_name.clone())));
        let network = Arc::new(Mutex::new(super::network::NetworkScanner::new(session.fs(), info.capabilities.input)));
        // Wayland apps repeat held keys themselves; Windows/macOS need repeats sent.
        let forward_repeats = info.os != "Linux";
        let display = info.displays.iter().find(|d| d.primary).map(|d| d.index).unwrap_or(0);
        let audio = play_audio && info.capabilities.audio;
        Self {
            id,
            detached: false,
            pending_keys: Vec::new(),
            cmds: session.command_sender(),
            peer: session.peer_id(),
            frames,
            info,
            session,
            events,
            password,
            via: None,
            stats: ViewerStats::default(),
            closed: None,
            reconnect_requested:false,reconnect:None,
            display,
            preset: 0,
            chat: Vec::new(),
            chat_input: String::new(),
            show_chat: false,
            unread: false,
            captured: false,
            fullscreen: false,
            toolbar_seen: None,
            video_rect: None,
            video_fit: None,
            view: touch::View::default(),
            touch: touch::Touch::default(),
            last_pointer: None,
            held_buttons: HashSet::new(),
            held_keys: HashSet::new(),
            forward_repeats,
            notice: None,
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
            public_ip: None, public_ip_rx: None, public_ip_at: None,
            files,
            network,
        }
    }

    fn send(&self, cmd: ViewerCmd) {
        let _ = self.cmds.send(cmd);
    }

    fn input(&self, ev: InputEvent) {
        if self.closed.is_none() && self.info.capabilities.input {
            self.session.input(ev);
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        self.poll_paste(ctx);
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                ViewerEvent::Stats(s) => self.stats = s,
                ViewerEvent::StreamStarted { display, .. } => self.display = display,
                ViewerEvent::Displays(d) => self.info.displays = d,
                ViewerEvent::Chat(text) => {
                    self.chat.push((false, text));
                    self.unread = !self.show_chat;
                    self.show_chat = true;
                }
                ViewerEvent::Notice(n) => self.notice = Some((n, Instant::now())),
                ViewerEvent::Disconnected { reason,retryable } => {
                    tracing::info!(peer=%self.peer,retryable,"viewer link ended: {reason}");
                    self.release_keyboard();
                    self.files.lock().unwrap().disconnect();
                    self.tunnels.clear();
                    self.closed = Some(reason);
                    self.stats=ViewerStats::default();
                    self.reconnect_requested=retryable && self.peer.0!=0;
                    self.repaint_tools(ctx);
                }
            }
        }
    }

    pub fn keyboard_captured(&self) -> bool {
        self.captured && self.closed.is_none()
    }

    /// Keep recovery attached to this tab: ending the link drops the job.
    pub fn maintain_connection(&mut self,ctx:&egui::Context,rt:&tokio::runtime::Runtime,client:&slipstream_session::Client)->bool {
        let mut restored=false;
        if std::mem::take(&mut self.reconnect_requested) && self.reconnect.is_none() {
            tracing::info!(peer=%self.peer,"reconnecting lost link");
            let progress=Arc::new(Mutex::new("Waiting for the remote host…".into()));
            let (tx,rx)=tokio::sync::oneshot::channel();
            let (client,peer,password,via,wake,status)=(client.clone(),self.peer,self.password.clone(),self.via.clone(),ctx.clone(),progress.clone());
            let task=rt.spawn(async move {
                let w=wake.clone();
                let result=client.reconnect(peer,&password,via.as_ref(),move |text|{*status.lock().unwrap()=text;w.request_repaint_of(egui::ViewportId::ROOT);}).await.map_err(|e|format!("{e:#}"));
                let _=tx.send(result);wake.request_repaint_of(egui::ViewportId::ROOT);
            });
            self.reconnect=Some(Reconnecting{progress,rx,task});
        }
        let result=self.reconnect.as_mut().and_then(|retry|match retry.rx.try_recv() {
            Ok(result)=>Some(result),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)=>None,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)=>Some(Err("Reconnect ended without an answer".into())),
        });
        if let Some(result)=result {
            self.reconnect=None;
            match result {
                Ok((session,events))=>{self.restore(session,events,ctx,client.settings().viewer.play_audio);restored=true;},
                Err(error)=>{tracing::warn!(peer=%self.peer,"automatic reconnect stopped: {error}");self.closed=Some(error);},
            }
        }
        if self.reconnect.is_some() {ctx.request_repaint_after(std::time::Duration::from_millis(100));}
        restored
    }

    fn restore(&mut self,session:ViewerSession,events:mpsc::UnboundedReceiver<ViewerEvent>,ctx:&egui::Context,default_audio:bool) {
        tracing::info!(peer=%self.peer,"viewer link restored");
        self.info=session.info().clone();
        self.cmds=session.command_sender();self.frames=session.frames();
        let (wake, viewport) = (ctx.clone(), Self::window_id(self.id));
        self.frames.set_repaint(move || {
            wake.request_repaint_of(egui::ViewportId::ROOT);
            wake.request_repaint_of(viewport);
        });
        self.events=events;self.session=session;
        self.forward_repeats=self.info.os!="Linux";
        if !self.info.displays.iter().any(|d|d.index==self.display) {
            self.display=self.info.displays.iter().find(|d|d.primary).or(self.info.displays.first()).map(|d|d.index).unwrap_or(0);
        }
        self.closed=None;self.captured=false;self.last_pointer=None;
        self.held_keys.clear();self.held_buttons.clear();self.touch=touch::Touch::default();
        self.privacy=false;self.audio &= self.info.capabilities.audio;
        self.send(ViewerCmd::SelectDisplay(self.display));
        self.send(ViewerCmd::Settings(PRESETS[self.preset].1));
        if self.audio!=default_audio {self.send(ViewerCmd::Audio(self.audio));}
        self.files.lock().unwrap().reconnect(self.session.fs(),self.info.host_name.clone());
        self.network.lock().unwrap().reconnect(self.session.fs(),self.info.capabilities.input);
        self.repaint_tools(ctx);
        self.pasting=None;self.sysinfo=Arc::default();self.sysinfo_busy=Arc::default();self.sysinfo_at=None;
        self.sysinfo_fetch=self.show_sysinfo;
        self.public_ip=None;self.public_ip_rx=None;self.public_ip_at=None;
        self.notice=Some(("Link restored · click the picture to take control".into(),Instant::now()));
    }

    fn repaint_tools(&self,ctx:&egui::Context) {
        if self.files.lock().unwrap().open {ctx.request_repaint_of(egui::ViewportId::from_hash_of(("file-transfer",self.id)));}
        if self.network.lock().unwrap().open {ctx.request_repaint_of(egui::ViewportId::from_hash_of(("network-scan",self.id)));}
    }

    pub fn release_keyboard(&mut self) {
        self.pending_keys.clear();
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

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        if self.fullscreen {
            self.notice = Some(("Full screen: point at the top edge for the bar, or press Ctrl+Alt+Enter to leave".into(), Instant::now()));
        }
    }

    /// Leaving the session's page: don't strand the window in full screen.
    pub fn leave_fullscreen(&mut self, ctx: &egui::Context) {
        if self.fullscreen {
            self.toggle_fullscreen(ctx);
        }
    }

    pub fn forward_keys(&mut self, keys: Vec<CapturedKey>, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
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
                    self.toggle_fullscreen(ctx);
                }
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

    pub fn ui(&mut self, ui: &mut Ui, full: Rect, t: f64, rt: &tokio::runtime::Runtime) -> TabAction {
        let keys = std::mem::take(&mut self.pending_keys);
        self.forward_keys(keys, rt, ui.ctx());
        let mut action = TabAction::None;
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(full, 0.0, Color32::BLACK);

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let toolbar_h = 36.0;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::ALT, egui::Key::Enter)) {
            self.toggle_fullscreen(&ctx);
        }
        // Full screen: the bar comes down at the top edge and stays while one
        // of its menus is open (the pointer is then below it) or briefly after.
        if pointer.is_some_and(|p| p.y < full.top() + toolbar_h + 4.0) || egui::Popup::is_any_open(&ctx) {
            self.toolbar_seen = Some(Instant::now());
        }
        let lingering = self.toolbar_seen.is_some_and(|t| t.elapsed().as_millis() < 700);
        if self.fullscreen && lingering {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let show_toolbar = !self.fullscreen || lingering || self.closed.is_some();
        let framed = !self.fullscreen && !cfg!(target_os = "android");
        let body = if framed {
            theme::session_body(full, toolbar_h)
        } else {
            full.with_min_y(full.top() + if self.fullscreen { 0.0 } else { toolbar_h })
        };
        // Chat goes beside the picture, or under it on a phone.
        let narrow = full.width() < super::NARROW;
        let chat_w = if self.show_chat && !narrow { 330.0 } else { 0.0 };
        let chat_h = if self.show_chat && narrow { (body.height() * 0.42).max(220.0) } else { 0.0 };
        let video_area = Rect::from_min_max(
            body.min,
            pos2(body.right() - chat_w, body.bottom() - chat_h),
        );

        self.draw_video(ui, video_area, t);
        let rail_width = framed.then(|| theme::session_rails(ui.painter(), full, body));
        self.handle_pointer(ui, video_area);

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
            home::chat_ui(&mut child, &mut self.chat, &mut self.chat_input, |text| {
                let _ = cmds.send(ViewerCmd::Chat(text));
            });
            self.unread = false;
        }

        if show_toolbar {
            let bar = Rect::from_min_size(full.min, vec2(full.width(), toolbar_h));
            action = self.toolbar(ui, bar, t, rail_width);
        }

        self.tunnels_window(&ctx, rt);
        self.sysinfo_window(&ctx, rt);
        self.handle_drops(ui, &ctx, video_area, rt);

        if let Some(reason) = self.closed.clone() {
            action = self.closed_overlay(ui, video_area, &reason).unwrap_or(action);
        }
        if let Some((n, at)) = &self.notice {
            if at.elapsed().as_secs() < 6 {
                let p = ui.painter();
                let pos = pos2(video_area.center().x, video_area.bottom() - 40.0);
                let g = p.layout_no_wrap(n.clone(), theme::body(17.0), pal::TEXT);
                let r = Rect::from_center_size(pos, g.size() + vec2(32.0, 14.0));
                p.rect_filled(r, 14.0, pal::PANEL.gamma_multiply(0.95));
                p.rect_stroke(r, 14.0, egui::Stroke::new(1.0, pal::GOLD), egui::StrokeKind::Inside);
                p.galley(r.min + vec2(16.0, 7.0), g, pal::TEXT);
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                self.notice = None;
            }
        }
        action
    }

    /// Files dragged in from the desktop: onto the file window's remote pane
    /// → that folder; onto the remote screen → the remote folder under the
    /// drop point (or its Desktop).
    fn handle_drops(&mut self, ui: &mut Ui, ctx: &egui::Context, video_area: Rect, rt: &tokio::runtime::Runtime) {
        if self.closed.is_some() { return; }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !self.info.capabilities.file_transfer {
            if !dropped.is_empty() {
                self.notice = Some(("File transfer is turned off on the remote computer".into(), Instant::now()));
            }
            return;
        }
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let pane_target = pos.and_then(|p| self.files.lock().unwrap().remote_drop_target(p, ctx));
        let screen_pos = pos.filter(|p| video_area.contains(*p)
            && self.video_rect.is_some_and(|r| r.contains(*p))
            && ctx.layer_id_at(*p).is_none_or(|layer| layer == ui.layer_id()));
        if hovering && pane_target.is_none() && screen_pos.is_some() {
            let p = ui.painter();
            p.rect_filled(video_area, 0.0, Color32::BLACK.gamma_multiply(0.45));
            p.text(video_area.center(), Align2::CENTER_CENTER, "DROP TO SEND", theme::display_bold(42.0), pal::ORANGE);
            p.text(video_area.center() + vec2(0.0, 40.0), Align2::CENTER_CENTER,
                "Open remote folder or Desktop · files and folders", theme::body(17.0), pal::TAN);
        }
        if dropped.is_empty() { return; }
        let n = dropped.len();
        if let Some(target) = pane_target {
            self.files.lock().unwrap().upload(dropped, target, rt, ctx);
        } else if let (Some(p), Some(r), Some(display)) = (screen_pos, self.video_rect,
            self.info.displays.iter().find(|d| d.index == self.display)) {
            // Resolve a physical point on the host, independent of keyboard
            // capture or the asynchronously injected remote mouse position.
            let x = display.x + (((p.x-r.left())/r.width()).clamp(0.0,1.0) * display.width.saturating_sub(1) as f32).round() as i32;
            let y = display.y + (((p.y-r.top())/r.height()).clamp(0.0,1.0) * display.height.saturating_sub(1) as f32).round() as i32;
            self.files.lock().unwrap().upload_to_screen(dropped, (x,y), rt, ctx);
        } else {
            self.notice = Some(("Drop onto the remote picture or the Files window's remote pane".into(), Instant::now()));
            return;
        }
        self.notice = Some((format!("Sending {n} item{} — transfer panel opened", if n == 1 { "" } else { "s" }), Instant::now()));
    }

    fn draw_video(&mut self, ui: &mut Ui, area: Rect, t: f64) {
        let painter = ui.painter();
        let (serial, frame) = self.frames.latest();
        match frame {
            Some(frame) => {
                let fit = video::fit_rect(area, (frame.width, frame.height));
                let rect = self.view.rect(fit, area);
                self.video_fit = Some(fit);
                self.video_rect = Some(rect);
                // Zoomed in, the picture runs past the area.
                painter.with_clip_rect(area).add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    video::VideoCallback { stream: self.id, serial, frame },
                ));
                if self.captured && !self.fullscreen {
                    theme::brackets(painter, rect.intersect(area).shrink(4.0), pal::ORANGE.gamma_multiply(0.8), 22.0);
                }
            }
            None => {
                self.video_rect = None;
                self.video_fit = None;
                theme::backdrop(painter, area, t);
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

    fn handle_pointer(&mut self, ui: &mut Ui, area: Rect) {
        let ctx = ui.ctx().clone();
        // Claim the area so egui widgets underneath don't react.
        let resp = ui.interact(area, ui.scope_id().with(("video", self.id)), Sense::click_and_drag());
        let Some(rect) = self.video_rect else { return };
        if self.closed.is_some() || !ctx.input(|i|i.focused) {
            return;
        }
        let hovered = resp.hovered() && ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l == ui.layer_id()));
        // Hide our pointer only when the host's video shows its own.
        if hovered && self.captured && self.info.capabilities.cursor_in_video {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        if hovered && !self.captured {
            let p = ui.painter();
            let label = "CLICK TO TAKE CONTROL";
            let pos = pos2(rect.center().x, rect.top() + 28.0);
            let g = p.layout_no_wrap(label.into(), theme::display(16.0), Color32::BLACK);
            let r = Rect::from_center_size(pos, g.size() + vec2(28.0, 10.0));
            p.rect_filled(r, r.height() / 2.0, pal::ORANGE.gamma_multiply(0.9));
            p.galley(r.min + vec2(14.0, 5.0), g, Color32::BLACK);
        }

        let normalize = |pos: egui::Pos2| {
            (((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0), ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0))
        };
        // Only the part of the screen not covered by our own windows (file
        // transfer, chat…) belongs to the remote computer.
        let my_layer = ui.layer_id();
        let on_video = |pos: egui::Pos2| rect.contains(pos) && area.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == my_layer);
        let events = ctx.input(|i| i.events.clone());

        // Fingers: see touch.rs. While they're down, the pointer events egui
        // makes from the first one are ignored.
        let fit = self.video_fit.unwrap_or(rect);
        let gesture = ctx.input(|i| i.multi_touch()).map(|m| touch::Gesture::from_egui(&m));
        let view_before = self.view;
        let (actions, touching) = self.touch.handle(&events, gesture, Instant::now(), on_video, &mut self.view, (fit, area));
        if self.view != view_before {
            ctx.request_repaint();
        }
        if self.touch.waiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        for action in actions {
            self.captured = true;
            match action {
                touch::Action::Move(pos) => {
                    let n = normalize(pos);
                    self.last_pointer = Some(n);
                    self.input(InputEvent::MouseMove { x: n.0, y: n.1 });
                }
                touch::Action::Button(button, down) => self.input(InputEvent::MouseButton { button, down }),
                touch::Action::Scroll(points) => self.input(InputEvent::Wheel { dx: points.x / 40.0, dy: points.y / 40.0 }),
            }
        }

        for ev in events {
            match ev {
                egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } if touching => {}
                egui::Event::PointerMoved(pos) => {
                    if (on_video(pos) || !self.held_buttons.is_empty()) && self.captured {
                        let n = normalize(pos);
                        if self.last_pointer != Some(n) {
                            self.last_pointer = Some(n);
                            self.input(InputEvent::MouseMove { x: n.0, y: n.1 });
                        }
                    }
                }
                egui::Event::PointerButton { pos, button, pressed, .. } => {
                    let inside = on_video(pos);
                    if pressed && !inside {
                        if self.captured && area.contains(pos) {
                            // Letterbox click: keep control, ignore.
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
                            self.last_pointer = Some(n);
                            self.input(InputEvent::MouseMove { x: n.0, y: n.1 });
                            continue;
                        }
                        let n = normalize(pos);
                        if self.last_pointer != Some(n) {
                            self.last_pointer = Some(n);
                            self.input(InputEvent::MouseMove { x: n.0, y: n.1 });
                        }
                        self.held_buttons.insert(button as usize);
                        self.input(InputEvent::MouseButton { button: b, down: true });
                    } else if self.held_buttons.remove(&(button as usize)) {
                        self.input(InputEvent::MouseButton { button: b, down: false });
                    }
                }
                egui::Event::MouseWheel { unit, delta, .. } if hovered && self.captured => {
                    let lines = match unit {
                        egui::MouseWheelUnit::Line => delta,
                        egui::MouseWheelUnit::Point => delta / 40.0,
                        egui::MouseWheelUnit::Page => delta * 8.0,
                    };
                    self.input(InputEvent::Wheel { dx: lines.x, dy: lines.y });
                }
                _ => {}
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, bar: Rect, t: f64, rail_width:Option<f32>) -> TabAction {
        let mut action = TabAction::None;
        let painter = ui.painter().clone();
        painter.rect_filled(bar, 0.0, pal::BG);
        let rail=rail_width.unwrap_or(0.0);
        if rail_width.is_some() {
            let turn=Rect::from_min_max(bar.min,pos2(bar.left()+rail.max(36.0)+32.0,bar.bottom()));
            painter.rect_filled(turn,egui::CornerRadius{nw:32,..Default::default()},pal::ORANGE);
            painter.text(pos2(turn.right()-17.0,turn.center().y),Align2::CENTER_CENTER,"LINK",theme::display_bold(14.0),pal::BG);
        } else {
            painter.rect_filled(Rect::from_min_size(bar.min+vec2(8.0,6.0),vec2(36.0,bar.height()-12.0)),egui::CornerRadius{nw:10,sw:10,..Default::default()},pal::ORANGE);
        }
        let ctx = ui.ctx().clone();
        let start=rail.max(36.0)+36.0;
        let inner = bar.with_min_x(bar.left()+start);
        let w = inner.width();
        if w < 780.0 { return self.toolbar_narrow(ui,inner,&ctx,t); }
        let compact_stats = w < 1440.0;
        let tight = w < 1050.0;


        // Right side first, so the buttons know how much room is left.
        let mut right = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 4.0;
        if theme::console_button(&mut right, if tight {""} else {"End link"}, pal::RED,Icon::End,false).on_hover_text("End this link").clicked() {
            self.send(ViewerCmd::Disconnect);
            action = TabAction::Close;
        }
        theme::session_stats_frame().show(&mut right,|ui| {ui.set_min_height(34.0);self.stats_readout(ui,if compact_stats {Detail::Compact}else{Detail::Full},t);});
        let quality = theme::console_button(&mut right, if tight {""} else {PRESETS[self.preset].0}, pal::BLUE, Icon::Performance, true)
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
        let mut nav = vec![("Bridge", false), (if self.fullscreen {"Window"} else {"Full"}, false)];
        if !ctx.embed_viewports() { nav.push((if self.detached {"Dock"} else {"Pop out"}, false)); }
        if self.info.displays.len() > 1 { nav.push(("Screens", true)); }
        let mut tools = vec![("Keys", true), (if self.unread {"Chat ◆"} else {"Chat"}, false), (if self.audio {"Sound"} else {"Muted"}, false), ("Tools", true)];
        if self.info.capabilities.file_transfer { tools.push(("Files", false)); }
        let count = nav.len() + tools.len();
        let icons = nav.iter().chain(&tools).map(|(_, menu)| theme::console_button_width(ui, "", *menu)).sum::<f32>() + (count - 1) as f32 * 4.0;
        let extra = |buttons: &[(&str, bool)]| buttons.iter().map(|(label, menu)| theme::console_button_width(ui, label, *menu) - theme::console_button_width(ui, "", *menu)).sum::<f32>();
        let tool_labels = left_rect.width() >= icons + extra(&tools) + 4.0;
        let tight = left_rect.width() < icons + extra(&tools) + extra(&nav) + 4.0;
        let icon_only_back = tight;

        if tight {
            ui.spacing_mut().button_padding.x = 8.0;
        }

        let back_tip = format!("Back to the Bridge (the link with {} keeps running)", self.info.host_name);
        if theme::console_button(ui, if icon_only_back { "" } else { "Bridge" }, pal::PEACH,Icon::Back,false).on_hover_text(back_tip).clicked() {
            self.release_keyboard();
            action = TabAction::Bridge;
        }

        if !ctx.embed_viewports() && theme::console_button(ui, if tight {""} else if self.detached {"Dock"} else {"Pop out"}, pal::BLUE, Icon::Window, false)
            .on_hover_text(if self.detached {"Move this link into the main console"} else {"Open this workstation in its own window"}).clicked() {
            self.release_keyboard();
            action = if self.detached {TabAction::Dock} else {TabAction::Detach};
        }

        if self.info.displays.len() > 1 {
            let tip = |d: &slipstream_core::proto::DisplayInfo| {
                format!("Screen {}: {} · {}×{}{}", d.index + 1, d.name, d.width, d.height, if d.primary { " (main)" } else { "" })
            };
            let mut pick = None;
            let screens=theme::console_button(ui,if tight {""} else {"Screens"},pal::VIOLET,Icon::Window,true);
            theme::console_menu(&screens).show(|ui| {
                for d in &self.info.displays {
                    if ui.selectable_label(d.index == self.display, tip(d)).clicked() {
                        pick = Some(d.index);
                        ui.close();
                    }
                }
            });
            screens.on_hover_text("The remote computer's screens: pick one to view");
            if let Some(i) = pick.filter(|&i| i != self.display) {
                self.display = i;
                self.send(ViewerCmd::SelectDisplay(i));
            }
        }

        if theme::console_button(ui, if tight {""} else if self.fullscreen { "Window" } else { "Full" }, pal::BLUE,if self.fullscreen{Icon::Window}else{Icon::Fullscreen},false)
            .on_hover_text("Full screen on/off (Ctrl+Alt+Enter)")
            .clicked()
        {
            self.toggle_fullscreen(&ctx);
        }

        let keys=theme::console_button(ui,if tool_labels {"Keys"} else {""},pal::LAVENDER,Icon::Keys,true);
        theme::console_menu(&keys).show(|ui|self.keys_menu(ui));

        let chat_label = if self.unread { "Chat ◆" } else { "Chat" };
        if theme::console_button(ui,if tool_labels {chat_label} else {""},pal::LAVENDER,Icon::Chat,false).on_hover_text(chat_label).clicked() {
            self.show_chat = !self.show_chat;
        }
        if self.info.capabilities.file_transfer && theme::console_button(ui,if tool_labels {"Files"} else {""},pal::GOLD,Icon::Files,false).on_hover_text("Browse and transfer files").clicked() {
            { let mut files=self.files.lock().unwrap();files.open = !files.open; }
            theme::repaint_console(&ctx);
            self.release_keyboard();
        }

        // Sound: gold while playing.
        let can_hear = self.info.capabilities.audio;
        let (label, color, tip) = match (can_hear, self.audio) {
            (false, _) => ("Sound", pal::DIM, "That computer doesn't share its sound (not supported there yet, or switched off in its Config)"),
            (true, true) => ("Sound", pal::GOLD, "Playing the remote computer's sound. Click to mute"),
            (true, false) => ("Muted", pal::LAVENDER.gamma_multiply(0.6), "Click to hear the remote computer's sound"),
        };
        let sound = ui.add_enabled_ui(can_hear, |ui| theme::console_button(ui,if tool_labels {label} else {""},color,if self.audio{Icon::Sound}else{Icon::Muted},false)).inner.on_hover_text(tip).on_disabled_hover_text(tip);
        if sound.clicked() {
            self.audio = !self.audio;
            self.send(ViewerCmd::Audio(self.audio));
        }

        // Tools: privacy mode, port forwarding. Gold while privacy is on.
        let tools_color = if self.privacy { pal::GOLD } else { pal::LAVENDER };
        let tools=theme::console_button(ui,if tool_labels {"Tools"} else {""},tools_color,Icon::Tools,true);
        theme::console_menu(&tools).show(|ui|self.tools_menu(ui));

        if self.fullscreen || cfg!(target_os="android") {
            let gap = Rect::from_min_max(pos2(ui.min_rect().right()+4.0,bar.top()),pos2(right_left-4.0,bar.bottom()));
            theme::session_rail_label(&painter, gap, &self.info.host_name.to_uppercase());
        }
        action
    }

    /// The bar on a phone: back, a Console menu with everything else, the
    /// link's figures and End.
    fn toolbar_narrow(&mut self, ui: &mut Ui, inner: Rect, ctx: &egui::Context, t: f64) -> TabAction {
        let mut action = TabAction::None;
        let mut right = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 4.0;
        if theme::console_button(&mut right,"",pal::RED,Icon::End,false).on_hover_text("End this link").clicked() {
            self.send(ViewerCmd::Disconnect);
            action = TabAction::Close;
        }
        theme::session_stats_frame()
            .show(&mut right,|ui|{ui.set_min_height(34.0);self.stats_readout(ui,if cfg!(target_os="android") {Detail::Fps} else {Detail::Compact},t)});
        if !cfg!(target_os="android") {
            let quality = theme::console_button(&mut right, "", pal::BLUE, Icon::Performance, true)
                .on_hover_text(format!("Picture quality · {}", PRESETS[self.preset].0));
            theme::console_menu(&quality).show(|ui| self.quality_menu(ui));
        }
        let left_rect = inner.with_max_x(right.min_rect().left() - 8.0);
        let mut left = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
        left.set_clip_rect(left_rect.expand2(vec2(0.0, 8.0)));
        left.spacing_mut().item_spacing.x = 4.0;
        if theme::console_button(&mut left,"",pal::PEACH,Icon::Back,false).clicked() {
            self.release_keyboard();
            action = TabAction::Bridge;
        }
        let console = theme::console_button(&mut left,if left_rect.width()<180.0 {""} else {"Console"},if self.unread {pal::GOLD}else{pal::LAVENDER},Icon::Console,true);
        theme::console_menu(&console).show(|ui| {
            ui.set_min_width(230.0);
            if self.fullscreen || cfg!(target_os="android") {
                ui.label(RichText::new(self.info.host_name.to_uppercase()).font(theme::display_bold(18.0)).color(pal::TAN));
                ui.separator();
            }
            if !ctx.embed_viewports() && ui.button(if self.detached {"Dock in main console"} else {"Open in separate window"}).clicked() {
                self.release_keyboard();
                action = if self.detached {TabAction::Dock} else {TabAction::Detach};
                ui.close();
            }
            if self.info.displays.len() > 1 {
                ui.label(RichText::new("SCREEN").font(theme::display(15.0)).color(pal::LAVENDER));
                let mut pick = None;
                ui.horizontal(|ui| {
                    for d in &self.info.displays {
                        let color = if d.index == self.display { pal::GOLD } else { pal::VIOLET };
                        if theme::pill(ui, &format!("{}", d.index + 1), color).clicked() {
                            pick = Some(d.index);
                        }
                    }
                });
                if let Some(i) = pick.filter(|&i| i != self.display) {
                    self.display = i;
                    self.send(ViewerCmd::SelectDisplay(i));
                }
                ui.separator();
            }
            if self.view.zoomed() && ui.button("Fit to screen").clicked() {
                self.view = touch::View::default();
                ui.close();
            }
            if ui.button(if self.fullscreen { "Leave full screen" } else { "Full screen" }).clicked() {
                self.toggle_fullscreen(ctx);
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
        if self.fullscreen || cfg!(target_os="android") {
            let gap = Rect::from_min_max(pos2(left.min_rect().right()+4.0,inner.top()),pos2(right.min_rect().left()-4.0,inner.bottom()));
            theme::session_rail_label(ui.painter(), gap, &self.info.host_name.to_uppercase());
        }
        action
    }

    /// Key combinations to send.
    fn keys_menu(&mut self, ui: &mut Ui) {
        if self.info.os.eq_ignore_ascii_case("android") {
            for (label,code) in [("Android · Back","AndroidBack"),("Android · Home","AndroidHome"),("Android · Recents","AndroidRecents")] {
                if ui.add_enabled(self.info.capabilities.input,egui::Button::new(label)).clicked() {
                    self.send_combo(&[code]);ui.close();
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
        if ui.add_enabled(self.info.capabilities.network_scan, egui::Button::new("Network scan…"))
            .on_hover_text("Discover devices and open TCP ports on the remote computer's network")
            .on_disabled_hover_text("Not allowed on that computer (Config → Permissions → Network scan)")
            .clicked() {
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
            .add_enabled(tunnels_ok, egui::Button::new("Port forwarding…"))
            .on_hover_text("Reach a service on the remote computer or its network, e.g. its Remote Desktop")
            .on_disabled_hover_text("Not allowed on that computer (its Config → Permissions → Port forwarding)");
        if r.clicked() {
            self.show_tunnels = true;
            ui.close();
        }
        if ui
            .button("System information…")
            .on_hover_text("Its processor, memory, graphics, drives, temperatures, network and Windows/macOS/Linux version")
            .clicked()
        {
            self.show_sysinfo = true;
            self.sysinfo_fetch = true;
            ui.close();
        }
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
                self.send(ViewerCmd::Settings(PRESETS[i].1));
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
            self.sysinfo_serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
            *self.sysinfo.lock().unwrap() = None;
        }
        let (slot, busy, wake, job) = (self.sysinfo.clone(), self.sysinfo_busy.clone(), ctx.clone(), self.session.system_info());
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
        if self.public_ip_rx.is_some() || self.closed.is_some() { return; }
        let (tx,rx) = tokio::sync::oneshot::channel();
        let (fs,wake) = (self.session.fs(),ctx.clone());
        self.public_ip_rx = Some(rx);
        self.public_ip_at = Some(Instant::now());
        rt.spawn(async move {
            let result = fs.public_ip(refresh).await.map_err(|e|format!("{e:#}"));
            let _=tx.send(result);wake.request_repaint_of(egui::ViewportId::ROOT);
        });
    }

    fn sysinfo_window(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        if !self.show_sysinfo {
            return;
        }
        if let Some(rx) = &mut self.public_ip_rx {
            match rx.try_recv() {
                Ok(result) => { self.public_ip = Some(result); self.public_ip_rx = None; },
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.public_ip = Some(Err("Public-IP request ended without an answer".into()));self.public_ip_rx = None;
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {},
            }
        }
        if self.public_ip_at.is_none_or(|at|at.elapsed() >= std::time::Duration::from_secs(60)) {
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
                let chrome = theme::window_header_with_close(ui, &title, pal::GOLD, pal::RED);
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
                                // One grid for every section: the label column fits the
                                // longest label anywhere, and nothing is cut short.
                                egui::Grid::new(("sysinfo-grid", self.id)).num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                                    for s in sections {
                                        ui.label(RichText::new(s.title.to_uppercase()).font(theme::display(16.0)).color(pal::LAVENDER));
                                        ui.end_row();
                                        for (k, v) in &s.rows {
                                            ui.add(egui::Label::new(RichText::new(k).color(pal::DIM)).extend());
                                            ui.add(egui::Label::new(RichText::new(v).color(pal::TEXT)).wrap());
                                            ui.end_row();
                                        }
                                    }
                                });
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
                    ui.add(egui::TextEdit::singleline(local).align(egui::Align2::LEFT_CENTER).hint_text("auto").desired_width(60.0));
                    ui.label("→");
                    ui.add(egui::TextEdit::singleline(host).align(egui::Align2::LEFT_CENTER).hint_text("localhost").desired_width(130.0));
                    ui.label(":");
                    ui.add(egui::TextEdit::singleline(port).align(egui::Align2::LEFT_CENTER).hint_text("3389").desired_width(60.0));
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
        let (rate, rate_unit) = if mbps < 1.0 { (format!("{:.0}", mbps * 1000.0), "KBPS") } else { (format!("{mbps:.1}"), "MBPS") };
        let mut details = format!(
            "{fps:.0} frames a second · {rate} {} · {rtt:.0} ms round trip · {} · decoding {:.1} ms a frame",
            if rate_unit == "KBPS" { "kbit/s" } else { "Mbit/s" },
            if s.relayed { "through a relay" } else { "direct connection" },
            s.decode_ms
        );
        if s.width > 0 && s.height > 0 {
            details.push_str(&format!("\nEncoded picture: {}×{} · {}", s.width, s.height,
                if PRESETS[self.preset].1.adaptive_resolution { "adaptive resolution" } else { "full resolution" }));
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
        let activity=self.files.lock().unwrap().activity();
        if let Some((n, frac)) = activity {
            let r = ui
                .add(egui::Button::new(RichText::new(if detail == Detail::Full {format!("⇅ {n} · {:.0}%", frac * 100.0)} else {"⇅".into()}).font(theme::mono(14.0)).color(pal::GOLD)).frame(false))
                .on_hover_text(format!("Show {n} transfers · {:.0}%", frac * 100.0));
            if r.clicked() {
                self.files.lock().unwrap().open = true;
            }
        }
        // The lamp shows the path when the label is left out.
        let lamp = if self.closed.is_some() { pal::RED } else if compact { path.1 } else { pal::GREEN };
        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        theme::lamp(ui.painter(), r.center(), lamp, t);
        resp.on_hover_text(details);
    }

    fn closed_overlay(&mut self,ui:&mut Ui,area:Rect,reason:&str)->Option<TabAction> {
        let progress=self.reconnect.as_ref().map(|r|r.progress.lock().unwrap().clone());
        let recovering=progress.is_some();
        let color=if recovering {pal::BLUE}else{pal::RED};
        let painter=ui.painter();
        painter.rect_filled(area,0.0,Color32::BLACK.gamma_multiply(0.75));
        let panel=Rect::from_center_size(area.center(),vec2((area.width()-24.0).clamp(260.0,560.0),240.0));
        theme::glow(painter,panel,16.0,color,1.0);
        painter.rect_filled(panel,16.0,pal::PANEL);
        let mut child=ui.new_child(UiBuilder::new().max_rect(panel.shrink(20.0)));
        let ui=&mut child;
        theme::header(ui,if recovering {"Re-establishing link"}else{"Link terminated"},color);
        ui.add_space(8.0);
        ui.label(RichText::new(reason).font(theme::body(17.0)));
        if let Some(progress)=progress {
            ui.add_space(8.0);ui.label(RichText::new(progress).color(pal::BLUE));
            ui.label(RichText::new("Retrying for up to two minutes. Transfers are not resumed automatically.").small().color(pal::DIM));
        }
        ui.add_space(16.0);
        let mut out=None;
        ui.horizontal_wrapped(|ui| {
            if recovering {
                if theme::pill(ui,"Cancel reconnect",pal::ORANGE).clicked() {tracing::info!(peer=%self.peer,"automatic reconnect canceled");self.reconnect=None;}
            } else if theme::pill(ui,"Reconnect",pal::ORANGE).clicked() {
                self.reconnect_requested = true;
            }
            if theme::pill(ui,"End link",pal::RED).clicked() {out=Some(TabAction::Close);}
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
    use slipstream_session::{Client, ClientOptions, AudioOut};

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
            let (client, _) = Client::start(store, ClientOptions {
                test_pattern: host, accept_sessions: host,
                clipboard: slipstream_platform::clipboard::ClipboardSource::Memory(Default::default()),
                audio_out: AudioOut::Memory(Default::default()),
            }).await.unwrap();
            clients.push(client);
        }
        let ctx = egui::Context::default();
        let mut tabs = Vec::new();
        for (index, host) in clients[..2].iter().enumerate() {
            let (session, events) = clients[2].connect_direct(host.endpoint_addr(), "window-test-password", |_| {}).await.unwrap();
            let mut tab = RemoteTab::new(index as u64 + 1, session, events, "window-test-password".into(), &ctx, false);
            tab.detached = true;
            tab.captured = true;
            tab.held_keys.insert("ControlLeft");
            tab.pending_keys.push(CapturedKey::ToggleFullscreen);
            tabs.push(Arc::new(Mutex::new(tab)));
        }
        let route = |viewport, focused| super::super::route_session_input(&tabs, super::super::Page::Links, viewport, focused);
        assert!(Arc::ptr_eq(&route(RemoteTab::window_id(1), true).unwrap(), &tabs[0]));
        assert!(tabs[0].lock().unwrap().keyboard_captured());
        assert!(!tabs[1].lock().unwrap().keyboard_captured());
        assert!(tabs[1].lock().unwrap().pending_keys.is_empty());
        assert!(tabs[1].lock().unwrap().held_keys.is_empty());
        let tool = egui::ViewportId::from_hash_of("background-tool");
        assert!(route(tool, false).is_none());
        assert!(tabs[0].lock().unwrap().keyboard_captured());
        assert!(route(tool, true).is_none());
        assert!(!tabs[0].lock().unwrap().keyboard_captured());
        {
            let mut second = tabs[1].lock().unwrap();
            second.captured = true;
            second.pending_keys.push(CapturedKey::ToggleFullscreen);
        }
        route(RemoteTab::window_id(2), false);
        assert!(!tabs[1].lock().unwrap().keyboard_captured());
        assert!(tabs[1].lock().unwrap().pending_keys.is_empty());
        tabs[0].lock().unwrap().detached = false;
        assert!(route(egui::ViewportId::ROOT, true).is_none());
        let docked = super::super::route_session_input(&tabs, super::super::Page::Remote(1), egui::ViewportId::ROOT, true).unwrap();
        assert!(Arc::ptr_eq(&docked, &tabs[0]));
        drop(docked);
        for tab in &tabs { tab.lock().unwrap().end_link(); }
        drop(tabs);
        for client in &clients { client.shutdown().await; }
    }
}
