//! A remote-control session tab: the remote screen, input forwarding and
//! the session toolbar.

use std::{collections::HashSet, sync::Arc, time::Instant};

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

use super::{files::FileManager, home, keys, theme, theme::pal, video};

pub enum CapturedKey {
    Key { key: egui::Key, pressed: bool, repeat: bool },
    /// A clipboard shortcut egui turned into an event; replay as Ctrl/Cmd+key.
    Shortcut(&'static str, Option<String>),
}

pub enum TabAction {
    None,
    Bridge,
    Close,
    Reconnect(DeviceId, String),
}

const PRESETS: [(&str, StreamSettings); 4] = [
    ("Balanced", StreamSettings { bitrate_kbps: 0, max_fps: 60, max_in_flight: 2 }),
    ("Speed", StreamSettings { bitrate_kbps: 0, max_fps: 30, max_in_flight: 1 }),
    ("Quality", StreamSettings { bitrate_kbps: 20_000, max_fps: 60, max_in_flight: 3 }),
    ("Low bandwidth", StreamSettings { bitrate_kbps: 1_200, max_fps: 20, max_in_flight: 1 }),
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
    video_rect: Option<Rect>,
    last_pointer: Option<(f32, f32)>,
    held_buttons: HashSet<usize>,
    held_keys: HashSet<&'static str>,
    forward_repeats: bool,
    notice: Option<(String, Instant)>,
    pub files: FileManager,
    /// Playing the remote computer's sound.
    audio: bool,
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
            video_rect: None,
            last_pointer: None,
            held_buttons: HashSet::new(),
            held_keys: HashSet::new(),
            forward_repeats,
            notice: None,
            audio,
            files,
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

    pub fn poll(&mut self, _ctx: &egui::Context) {
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

    pub fn forward_keys(&mut self, keys: Vec<CapturedKey>) {
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

    pub fn ui(&mut self, ui: &mut Ui, full: Rect, t: f64, rt: &tokio::runtime::Runtime) -> TabAction {
        let mut action = TabAction::None;
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(full, 0.0, Color32::BLACK);

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let toolbar_h = 44.0;
        let show_toolbar = !self.fullscreen
            || pointer.is_some_and(|p| p.y < full.top() + toolbar_h + 4.0)
            || self.closed.is_some();
        let chat_w = if self.show_chat { 330.0 } else { 0.0 };
        let video_area = Rect::from_min_max(
            pos2(full.left(), full.top() + if self.fullscreen { 0.0 } else { toolbar_h }),
            pos2(full.right() - chat_w, full.bottom()),
        );

        self.draw_video(ui, video_area, t);
        self.handle_pointer(ui, video_area);

        if self.show_chat {
            let chat_rect = Rect::from_min_max(pos2(full.right() - chat_w, full.top() + toolbar_h), full.max);
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
                let rect = video::fit_rect(area, (frame.width, frame.height));
                self.video_rect = Some(rect);
                painter.add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    video::VideoCallback { stream: self.id, serial, frame },
                ));
                if self.captured && !self.fullscreen {
                    theme::brackets(painter, rect.shrink(4.0), pal::ORANGE.gamma_multiply(0.8), 22.0);
                }
            }
            None => {
                self.video_rect = None;
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
        let on_video = |pos: egui::Pos2| rect.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == my_layer);
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            match ev {
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
        self.stats_readout(&mut right, compact_stats, t);
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

        if theme::pill(ui, if self.fullscreen { "Window" } else { "Full" }, pal::BLUE).clicked() {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }

        let mut combo: Option<&'static [&'static str]> = None;
        ui.menu_button(RichText::new("KEYS").font(theme::display(17.0)).color(pal::LAVENDER), |ui| {
            for (label, keys) in [
                ("Ctrl + Alt + Del", &["ControlLeft", "AltLeft", "Delete"][..]),
                ("Alt + Tab", &["AltLeft", "Tab"][..]),
                ("Super / Windows key", &["MetaLeft"][..]),
                ("Alt + F4", &["AltLeft", "F4"][..]),
                ("Print Screen", &["PrintScreen"][..]),
                ("Ctrl + Esc", &["ControlLeft", "Escape"][..]),
            ] {
                if ui.button(label).clicked() {
                    combo = Some(keys);
                    ui.close();
                }
            }
        });
        if let Some(keys) = combo {
            self.send_combo(keys);
        }

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

        let mut new_preset = None;
        // Only the arrow when there's no room for the preset's name.
        let preset_label = if ui.available_width() < 120.0 { "▾".to_string() } else { format!("{} ▾", PRESETS[self.preset].0.to_uppercase()) };
        ui.menu_button(RichText::new(preset_label).font(theme::display(17.0)).color(pal::BLUE), |ui| {
            for (i, (name, _)) in PRESETS.iter().enumerate() {
                if ui.selectable_label(i == self.preset, *name).clicked() {
                    new_preset = Some(i);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Picture quality");
        if let Some(i) = new_preset {
            self.preset = i;
            self.send(ViewerCmd::Settings(PRESETS[i].1));
        }
        action
    }

    /// Frame rate, bandwidth, round trip and path, right to left; the full
    /// details are in the tooltip when `compact`.
    fn stats_readout(&mut self, ui: &mut Ui, compact: bool, t: f64) {
        let s = &self.stats;
        let (fps, mbps, rtt) = (
            theme::glide(ui, "fps", s.fps),
            theme::glide(ui, "mbps", s.kbps / 1000.0),
            theme::glide(ui, "rtt", s.rtt_ms),
        );
        let path = if s.relayed { ("RELAY", pal::GOLD) } else { ("DIRECT", pal::GREEN) };
        let details = format!(
            "{fps:.0} frames a second · {mbps:.1} Mbit/s · {rtt:.0} ms round trip · {}",
            if s.relayed { "through the station's relay" } else { "direct connection" }
        );
        if !compact {
            ui.label(RichText::new(path.0).font(theme::display(16.0)).color(path.1));
        }
        let text = if compact {
            format!("{fps:>3.0} FPS · {rtt:>3.0} MS")
        } else {
            format!("{fps:>3.0} FPS · {mbps:>4.1} MBPS · {rtt:>3.0} MS")
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
                out = Some(TabAction::Reconnect(self.peer, self.password.clone()));
            }
            if theme::pill(ui, "Close", pal::LAVENDER).clicked() {
                out = Some(TabAction::Close);
            }
        });
        out
    }
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
