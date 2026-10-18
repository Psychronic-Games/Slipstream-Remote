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
    proto::{InputEvent, MouseButton, SessionInfo, StreamSettings, UploadTarget},
};
use slipstream_session::{FrameShared, ViewerCmd, ViewerEvent, ViewerSession, ViewerStats};
use tokio::sync::mpsc;

use super::{files::FileManager, home, keys, theme, theme::pal, touch, video};

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

pub enum TabAction {
    None,
    Bridge,
    Close,
    /// Again, the same way: password, and the station it was found on
    /// (`None` = this app's own).
    Reconnect(DeviceId, String, Option<slipstream_session::AccountRef>),
}

const PRESETS: [(&str, StreamSettings); 4] = [
    ("Balanced", StreamSettings { bitrate_kbps: 0, max_fps: 60, max_in_flight: 2, adaptive_resolution: false }),
    ("Speed", StreamSettings { bitrate_kbps: 0, max_fps: 30, max_in_flight: 1, adaptive_resolution: true }),
    ("Quality", StreamSettings { bitrate_kbps: 20_000, max_fps: 60, max_in_flight: 3, adaptive_resolution: false }),
    ("Low bandwidth", StreamSettings { bitrate_kbps: 1_200, max_fps: 20, max_in_flight: 1, adaptive_resolution: true }),
];

pub struct RemoteTab {
    pub id: u64,
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
    pub files: FileManager,
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
}

impl RemoteTab {
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
        frames.set_repaint(move || wake.request_repaint());
        let info = session.info().clone();
        let files = FileManager::new(session.fs(), info.host_name.clone());
        let network = Arc::new(Mutex::new(super::network::NetworkScanner::new(session.fs())));
        // Wayland apps repeat held keys themselves; Windows/macOS need repeats sent.
        let forward_repeats = info.os != "Linux";
        let display = info.displays.iter().find(|d| d.primary).map(|d| d.index).unwrap_or(0);
        let audio = play_audio && info.capabilities.audio;
        Self {
            id,
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
                ViewerEvent::Disconnected { reason } => {
                    self.closed = Some(reason);
                    self.captured = false;
                }
            }
        }
    }

    pub fn keyboard_captured(&self) -> bool {
        self.captured && self.closed.is_none()
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
        super::network::NetworkScanner::show(&self.network, ctx, rt, self.id, &self.info.host_name, self.closed.is_none());
    }

    pub fn ui(&mut self, ui: &mut Ui, full: Rect, t: f64, rt: &tokio::runtime::Runtime) -> TabAction {
        let mut action = TabAction::None;
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(full, 0.0, Color32::BLACK);

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let toolbar_h = 44.0;
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
        // Chat goes beside the picture, or under it on a phone.
        let narrow = full.width() < super::NARROW;
        let chat_w = if self.show_chat && !narrow { 330.0 } else { 0.0 };
        let chat_h = if self.show_chat && narrow { (full.height() * 0.42).max(220.0) } else { 0.0 };
        let video_area = Rect::from_min_max(
            pos2(full.left(), full.top() + if self.fullscreen { 0.0 } else { toolbar_h }),
            pos2(full.right() - chat_w, full.bottom() - chat_h),
        );

        self.draw_video(ui, video_area, t);
        self.handle_pointer(ui, video_area);

        if self.show_chat {
            let chat_rect = if narrow {
                Rect::from_min_max(pos2(full.left(), full.bottom() - chat_h), full.max)
            } else {
                Rect::from_min_max(pos2(full.right() - chat_w, full.top() + toolbar_h), full.max)
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
            action = self.toolbar(ui, bar, t);
        }

        self.files.ui(&ctx, rt);
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
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !self.info.capabilities.file_transfer {
            if !dropped.is_empty() {
                self.notice = Some(("File transfer is turned off on the remote computer".into(), Instant::now()));
            }
            return;
        }
        let pos = ctx.input(|i| i.pointer.latest_pos());
        if hovering {
            let over_pane = pos.and_then(|p| self.files.remote_drop_target(p)).is_some();
            if !over_pane {
                let p = ui.painter();
                p.rect_filled(video_area, 0.0, Color32::BLACK.gamma_multiply(0.6));
                p.text(video_area.center(), Align2::CENTER_CENTER, "DROP TO SEND", theme::display_bold(42.0), pal::ORANGE);
                p.text(
                    video_area.center() + vec2(0.0, 40.0),
                    Align2::CENTER_CENTER,
                    "Files go to the remote folder under the pointer, or its Desktop",
                    theme::body(17.0),
                    pal::TAN,
                );
            }
        }
        if dropped.is_empty() {
            return;
        }
        let target = match pos.and_then(|p| self.files.remote_drop_target(p)) {
            Some(t) => t,
            None => {
                // Put the remote pointer where the files were dropped, so the
                // host can find the folder under it.
                if let (Some(p), Some(r)) = (pos, self.video_rect)
                    && r.contains(p)
                {
                    let n = (((p.x - r.left()) / r.width()).clamp(0.0, 1.0), ((p.y - r.top()) / r.height()).clamp(0.0, 1.0));
                    self.input(InputEvent::MouseMove { x: n.0, y: n.1 });
                }
                UploadTarget::UnderCursor
            }
        };
        let n = dropped.len();
        self.files.upload(dropped, target, rt, ctx);
        self.notice = Some((format!("Sending {n} item{} — progress in Files", if n == 1 { "" } else { "s" }), Instant::now()));
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
        if self.closed.is_some() {
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

    fn toolbar(&mut self, ui: &mut Ui, bar: Rect, t: f64) -> TabAction {
        let mut action = TabAction::None;
        let painter = ui.painter();
        painter.rect_filled(bar, 0.0, pal::BG);
        // Left cap + bottom rule: a slim console strip.
        painter.rect_filled(
            Rect::from_min_size(bar.min + vec2(8.0, 6.0), vec2(36.0, bar.height() - 12.0)),
            egui::CornerRadius { nw: 16, sw: 16, ..Default::default() },
            pal::ORANGE,
        );
        painter.line_segment(
            [pos2(bar.left(), bar.bottom() - 1.0), pos2(bar.right(), bar.bottom() - 1.0)],
            egui::Stroke::new(1.0, pal::ORANGE.gamma_multiply(0.4)),
        );
        let ctx = ui.ctx().clone();
        let inner = bar.shrink2(vec2(8.0, 6.0)).with_min_x(bar.left() + 50.0);
        // Narrower windows get a tighter bar rather than overlapping buttons.
        let w = bar.width();
        if w < super::NARROW {
            return self.toolbar_narrow(ui, inner, &ctx, t);
        }
        let compact_stats = w < 1460.0;
        let tight = w < 1320.0;
        let show_host_name = w >= 1240.0;
        let screens_as_menu = w < 1100.0;
        let icon_only_back = w < 920.0;

        // Right side first, so the buttons know how much room is left.
        let mut right = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 6.0;
        if theme::pill(&mut right, "End link", pal::RED).clicked() {
            self.send(ViewerCmd::Disconnect);
            action = TabAction::Close;
        }
        self.stats_readout(&mut right, if compact_stats { Detail::Compact } else { Detail::Full }, t);
        let right_left = right.min_rect().left();

        let left_rect = inner.with_max_x(right_left - 12.0);
        let mut child = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
        child.set_clip_rect(left_rect.expand2(vec2(0.0, 8.0)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = if tight { 5.0 } else { 6.0 };
        if tight {
            ui.spacing_mut().button_padding.x = 8.0;
        }

        let back_tip = format!("Back to the Bridge (the link with {} keeps running)", self.info.host_name);
        if theme::back_pill(ui, if icon_only_back { "" } else { "Bridge" }, pal::PEACH).on_hover_text(back_tip).clicked() {
            self.release_keyboard();
            action = TabAction::Bridge;
        }
        if show_host_name {
            ui.label(RichText::new(self.info.host_name.to_uppercase()).font(theme::display_bold(20.0)).color(pal::TAN));
        }

        if self.info.displays.len() > 1 {
            let tip = |d: &slipstream_core::proto::DisplayInfo| {
                format!("Screen {}: {} · {}×{}{}", d.index + 1, d.name, d.width, d.height, if d.primary { " (main)" } else { "" })
            };
            let mut pick = None;
            if screens_as_menu {
                ui.menu_button(RichText::new(format!("SCREEN {} ▾", self.display + 1)).font(theme::display(17.0)).color(pal::VIOLET), |ui| {
                    for d in &self.info.displays {
                        if ui.selectable_label(d.index == self.display, tip(d)).clicked() {
                            pick = Some(d.index);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("The remote computer's screens: pick one to view");
            } else {
                ui.add_space(4.0);
                ui.label(RichText::new("SCREEN").font(theme::display(15.0)).color(pal::LAVENDER))
                    .on_hover_text("The remote computer's screens: pick one to view");
                for d in &self.info.displays {
                    let color = if d.index == self.display { pal::GOLD } else { pal::VIOLET };
                    if theme::pill(ui, &format!("{}", d.index + 1), color).on_hover_text(tip(d)).clicked() {
                        pick = Some(d.index);
                    }
                }
            }
            if let Some(i) = pick.filter(|&i| i != self.display) {
                self.display = i;
                self.send(ViewerCmd::SelectDisplay(i));
            }
        }

        if theme::pill(ui, if self.fullscreen { "Window" } else { "Full" }, pal::BLUE)
            .on_hover_text("Full screen on/off (Ctrl+Alt+Enter)")
            .clicked()
        {
            self.toggle_fullscreen(&ctx);
        }

        ui.menu_button(RichText::new("KEYS").font(theme::display(17.0)).color(pal::LAVENDER), |ui| self.keys_menu(ui));

        let chat_label = if self.unread { "Chat ◆" } else { "Chat" };
        if theme::pill(ui, chat_label, pal::LAVENDER).clicked() {
            self.show_chat = !self.show_chat;
        }
        if self.info.capabilities.file_transfer && theme::pill(ui, "Files", pal::GOLD).on_hover_text("Browse and transfer files").clicked() {
            self.files.open = !self.files.open;
            self.release_keyboard();
        }

        // Sound: gold while playing.
        let can_hear = self.info.capabilities.audio;
        let (label, color, tip) = match (can_hear, self.audio) {
            (false, _) => ("Sound", pal::DIM, "That computer doesn't share its sound (not supported there yet, or switched off in its Config)"),
            (true, true) => ("Sound", pal::GOLD, "Playing the remote computer's sound. Click to mute"),
            (true, false) => ("Muted", pal::LAVENDER.gamma_multiply(0.6), "Click to hear the remote computer's sound"),
        };
        let sound = ui.add_enabled_ui(can_hear, |ui| theme::pill(ui, label, color)).inner.on_hover_text(tip).on_disabled_hover_text(tip);
        if sound.clicked() {
            self.audio = !self.audio;
            self.send(ViewerCmd::Audio(self.audio));
        }

        // Tools: privacy mode, port forwarding. Gold while privacy is on.
        let tools_color = if self.privacy { pal::GOLD } else { pal::LAVENDER };
        ui.menu_button(RichText::new("TOOLS ▾").font(theme::display(17.0)).color(tools_color), |ui| self.tools_menu(ui));

        // Only the arrow when there's no room for the preset's name.
        let preset_label = if ui.available_width() < 120.0 { "▾".to_string() } else { format!("{} ▾", PRESETS[self.preset].0.to_uppercase()) };
        ui.menu_button(RichText::new(preset_label).font(theme::display(17.0)).color(pal::BLUE), |ui| self.quality_menu(ui))
            .response
            .on_hover_text("Picture quality");
        action
    }

    /// The bar on a phone: back, a Console menu with everything else, the
    /// link's figures and End.
    fn toolbar_narrow(&mut self, ui: &mut Ui, inner: Rect, ctx: &egui::Context, t: f64) -> TabAction {
        let mut action = TabAction::None;
        let mut right = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 6.0;
        if theme::pill(&mut right, "End", pal::RED).clicked() {
            self.send(ViewerCmd::Disconnect);
            action = TabAction::Close;
        }
        self.stats_readout(&mut right, Detail::Fps, t);
        let left_rect = inner.with_max_x(right.min_rect().left() - 8.0);
        let mut left = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
        left.set_clip_rect(left_rect.expand2(vec2(0.0, 8.0)));
        left.spacing_mut().item_spacing.x = 6.0;
        if theme::back_pill(&mut left, "", pal::PEACH).clicked() {
            self.release_keyboard();
            action = TabAction::Bridge;
        }
        let console = theme::pill(&mut left, "Console", if self.unread { pal::GOLD } else { pal::LAVENDER });
        egui::Popup::menu(&console).show(|ui| {
            ui.set_min_width(230.0);
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
                self.files.open = !self.files.open;
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
        action
    }

    /// Key combinations to send.
    fn keys_menu(&mut self, ui: &mut Ui) {
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
            ui.close();
        }
        let (privacy_ok, tunnels_ok) = (self.info.capabilities.privacy, self.info.capabilities.tunnels);
        let mut privacy = self.privacy;
        let r = ui
            .add_enabled(privacy_ok, egui::Checkbox::new(&mut privacy, "Privacy mode"))
            .on_hover_text("Black out the remote computer's own screen while you work: people there can't watch (or, with the Slipstream service, type)")
            .on_disabled_hover_text("Needs a Windows 10 (2004 or later) or Windows 11 computer on the other end");
        if r.changed() {
            self.privacy = privacy;
            self.send(ViewerCmd::Privacy(privacy));
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

    fn sysinfo_window(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        if !self.show_sysinfo {
            return;
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
                                ui.ctx().copy_text(sysinfo_text(&self.info.host_name, sections));
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
        }
        if !open {
            self.show_sysinfo = false;
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
        if let Some((n, frac)) = self.files.activity() {
            let r = ui
                .add(egui::Button::new(RichText::new(format!("⇅ {n} · {:.0}%", frac * 100.0)).font(theme::mono(14.0)).color(pal::GOLD)).frame(false))
                .on_hover_text("Show transfers");
            if r.clicked() {
                self.files.open = true;
            }
        }
        // The lamp shows the path when the label is left out.
        let lamp = if self.closed.is_some() { pal::RED } else if compact { path.1 } else { pal::GREEN };
        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        theme::lamp(ui.painter(), r.center(), lamp, t);
        resp.on_hover_text(details);
    }

    fn closed_overlay(&mut self, ui: &mut Ui, area: Rect, reason: &str) -> Option<TabAction> {
        let painter = ui.painter();
        painter.rect_filled(area, 0.0, Color32::BLACK.gamma_multiply(0.75));
        let panel = Rect::from_center_size(area.center(), vec2(520.0, 200.0));
        theme::glow(painter, panel, 16.0, pal::RED, 1.0);
        painter.rect_filled(panel, 16.0, pal::PANEL);
        let mut child = ui.new_child(UiBuilder::new().max_rect(panel.shrink(20.0)));
        let ui = &mut child;
        theme::header(ui, "Link terminated", pal::RED);
        ui.add_space(8.0);
        ui.label(RichText::new(reason).font(theme::body(17.0)));
        ui.add_space(16.0);
        let mut out = None;
        ui.horizontal(|ui| {
            if theme::pill(ui, "Reconnect", pal::ORANGE).clicked() {
                out = Some(TabAction::Reconnect(self.peer, self.password.clone(), self.via.clone()));
            }
            if theme::pill(ui, "Close", pal::LAVENDER).clicked() {
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
