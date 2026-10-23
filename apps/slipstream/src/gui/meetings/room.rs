//! The meeting room: participants on a stage in the chosen layout (speaker
//! with a strip of the others, or a gallery grid; double-click spotlights
//! one person), with Subspace chat, people or devices in a column beside it.
use super::{Active, Meetings, STREAM_BASE, devices, participants, toolbar};
use crate::gui::{App, theme, theme::pal, video};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, Vec2, vec2};
use slipstream_core::meeting::{Source, State};
use slipstream_session::meetings::Command;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

/// Loudness (RMS) above which someone counts as speaking.
pub(super) const SPEAKING: f32 = 0.02;
/// Below this width the side column takes the whole room instead.
const NARROW: f32 = 760.0;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Layout {
    /// One large picture (the spotlight, pin or current speaker) and the
    /// others small along one edge.
    #[default]
    Speaker,
    /// Everyone the same size in a grid.
    Gallery,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Strip {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Side {
    #[default]
    None,
    Chat,
    People,
    Devices,
}

/// Room view state, kept with the meeting.
#[derive(Default)]
pub(super) struct View {
    pub layout: Layout,
    pub strip: Strip,
    /// Filling the stage with this one participant (double-click).
    pub focus: Option<u64>,
    /// Shown large in the speaker layout instead of whoever speaks.
    pub pin: Option<u64>,
    /// Who is speaking, and since when (held briefly to avoid flicker).
    pub speaker: Option<(u64, Instant)>,
    pub levels: std::collections::HashMap<u64, f32>,
    pub side: Side,
    pub unread: usize,
    /// Reactions to chat messages: message → reaction → who.
    pub reactions: std::collections::HashMap<u64, BTreeMap<u8, BTreeSet<u64>>>,
}

impl View {
    /// Follows the loudest remote voice, switching only once the current
    /// speaker has had the floor for a moment or has gone quiet.
    pub fn update_speaker(&mut self, state: &State) {
        let present = |id: u64| state.participants.iter().any(|p| p.id == id && p.id != state.local);
        if self.speaker.is_some_and(|(id, _)| !present(id)) {
            self.speaker = None;
        }
        let loudest = self
            .levels
            .iter()
            .filter(|(id, level)| **level > SPEAKING && present(**id))
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(id, _)| *id);
        let Some(loudest) = loudest else { return };
        match self.speaker {
            Some((current, since)) if current != loudest => {
                let quiet = self.levels.get(&current).is_none_or(|l| *l < SPEAKING);
                if since.elapsed() > Duration::from_millis(1200) && (quiet || since.elapsed() > Duration::from_secs(4)) {
                    self.speaker = Some((loudest, Instant::now()));
                }
            }
            Some(_) => {}
            None => self.speaker = Some((loudest, Instant::now())),
        }
    }

    /// Who should get one of the few video subscriptions, best first.
    pub fn video_priority(&self, state: &State) -> Vec<u64> {
        let mut order = Vec::new();
        let sharing = |id: u64| state.participants.iter().any(|p| p.id == id && p.id != state.local && p.source != Source::None);
        for id in [self.focus, self.pin, self.speaker.map(|s| s.0)].into_iter().flatten() {
            if sharing(id) && !order.contains(&id) {
                order.push(id);
            }
        }
        for source in [Source::Screen, Source::Camera] {
            for p in &state.participants {
                if p.id != state.local && p.source == source && !order.contains(&p.id) {
                    order.push(p.id);
                }
            }
        }
        order
    }

    pub fn react(&mut self, chat: u64, reaction: u8, participant: u64, on: bool) {
        let who = self.reactions.entry(chat).or_default().entry(reaction).or_default();
        if on {
            who.insert(participant);
        } else {
            who.remove(&participant);
        }
    }
}

/// One participant on the stage.
struct Tile {
    id: u64,
    name: String,
    me: bool,
    microphone: bool,
    source: Source,
    video: Option<(u64, u64, Arc<slipstream_codec::YuvFrame>)>,
}

fn tiles(active: &Active, state: &State) -> Vec<Tile> {
    let frames = active.video.frames.lock().unwrap().clone();
    let preview = active.sender.as_ref().and_then(|s| s.preview.lock().unwrap().clone());
    state
        .participants
        .iter()
        .map(|p| {
            let me = p.id == state.local;
            let video = if me {
                preview.clone().map(|(serial, frame)| (STREAM_BASE, serial, frame))
            } else if p.source != Source::None && active.subscriptions.contains(&p.id) {
                frames.get(&p.id).map(|(serial, frame)| (STREAM_BASE + p.id, *serial, frame.clone()))
            } else {
                None
            };
            Tile { id: p.id, name: p.name.clone(), me, microphone: p.microphone, source: p.source, video }
        })
        .collect()
}

/// The whole room below the meeting's title: controls, then stage and side.
pub(super) fn ui(meetings: &mut Meetings, active: &mut Active, state: &State, app: &mut App, ui: &mut Ui, leave: &mut bool) {
    let narrow = ui.available_width() < NARROW;
    header(meetings, active, state, app, ui, leave);
    ui.add_space(6.0);
    let area = ui.available_rect_before_wrap();
    let area = area.with_max_y(area.max.y.max(area.min.y + 160.0));
    ui.allocate_rect(area, Sense::hover());
    let side = active.room.side;
    let (stage_rect, side_rect) = if side == Side::None {
        (Some(area), None)
    } else if narrow {
        (None, Some(area))
    } else {
        let width = (area.width() * 0.3).clamp(280.0, 400.0);
        (Some(area.with_max_x(area.max.x - width - 8.0)), Some(area.with_min_x(area.max.x - width)))
    };
    if let Some(rect) = stage_rect {
        stage(active, state, ui, rect);
    }
    if let Some(rect) = side_rect {
        let mut column = ui.new_child(UiBuilder::new().max_rect(rect));
        column.painter().rect_filled(rect, 8.0, pal::PANEL);
        // Panels draw corner brackets outside their frame: leave room for them.
        let mut inner = column.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(16.0, 10.0))));
        inner.set_clip_rect(rect);
        let width = rect.width() - 36.0;
        if narrow && theme::pill(&mut inner, "◀ Back to the meeting", pal::BLUE).clicked() {
            active.room.side = Side::None;
        }
        match side {
            Side::Chat => {
                active.room.unread = 0;
                chat(meetings, active, state, &mut inner);
            }
            Side::People => {
                egui::ScrollArea::vertical().id_salt("meeting-people").auto_shrink([false, false]).show(&mut inner, |ui| {
                    ui.set_max_width(width);
                    participants(meetings, active, state, app, ui);
                });
            }
            Side::Devices => {
                egui::ScrollArea::vertical().id_salt("meeting-devices").auto_shrink([false, false]).show(&mut inner, |ui| {
                    ui.set_max_width(width);
                    devices::ui(meetings, Some(active), app, ui);
                });
            }
            Side::None => {}
        }
    }
}

fn header(meetings: &mut Meetings, active: &mut Active, state: &State, app: &mut App, ui: &mut Ui, leave: &mut bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(state.title.to_uppercase()).font(theme::display_bold(22.0)).color(pal::GREEN));
        ui.label(
            RichText::new(format!(
                "{} / {} participants{}",
                state.participants.len(),
                state.capacity,
                if state.host { " · you are the organizer" } else { "" }
            ))
            .color(pal::DIM),
        );
        if state.locked {
            ui.label(RichText::new("Joining closed").color(pal::GOLD));
        }
        ui.add_space(12.0);
        let room = &mut active.room;
        let chat = if room.unread > 0 { format!("Chat {}", room.unread) } else { "Chat".into() };
        let waiting = state.host && !state.lobby.is_empty();
        let people = if waiting { format!("People · {} waiting", state.lobby.len()) } else { format!("People {}", state.participants.len()) };
        for (side, label, icon, attention) in [
            (Side::Chat, chat.as_str(), theme::ConsoleIcon::Chat, room.unread > 0),
            (Side::People, people.as_str(), theme::ConsoleIcon::People, waiting),
            (Side::Devices, "Devices", theme::ConsoleIcon::Camera, false),
        ] {
            let color = if room.side == side { pal::GREEN } else if attention { pal::GOLD } else { pal::BLUE };
            if theme::console_button(ui, label, color, icon, false).clicked() {
                room.side = if room.side == side { Side::None } else { side };
            }
        }
        layout_menu(&mut active.room, ui);
        #[cfg(not(target_os = "android"))]
        super::recording::button(active, app, ui);
    });
    toolbar(meetings, active, state, app, ui, leave);
}

impl View {
    /// The icon for the current arrangement.
    fn icon(&self) -> theme::ConsoleIcon {
        match (self.layout, self.strip) {
            (Layout::Gallery, _) => theme::ConsoleIcon::Gallery,
            (Layout::Speaker, Strip::Bottom) => theme::ConsoleIcon::Speaker,
            (Layout::Speaker, Strip::Top) => theme::ConsoleIcon::StripTop,
            (Layout::Speaker, Strip::Left) => theme::ConsoleIcon::StripLeft,
            (Layout::Speaker, Strip::Right) => theme::ConsoleIcon::StripRight,
        }
    }
}

/// One choice in the layout menu: its icon, its name, and whether it's in use.
fn icon_row(ui: &mut Ui, icon: theme::ConsoleIcon, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(250.0, 32.0), Sense::click());
    let fill = if selected { pal::VIOLET } else if response.hovered() { pal::GRID } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, 6.0, fill);
    let ink = if selected { pal::BG } else { pal::TAN };
    let symbol = Rect::from_center_size(Pos2::new(rect.left() + 20.0, rect.center().y), vec2(20.0, 20.0));
    theme::console_icon(ui.painter(), symbol, icon, ink, fill);
    ui.painter().text(Pos2::new(rect.left() + 40.0, rect.center().y), Align2::LEFT_CENTER, label, theme::body(16.0), ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn layout_menu(room: &mut View, ui: &mut Ui) {
    let button = theme::console_button(ui, "Layout", pal::LAVENDER, room.icon(), true).on_hover_text("Arrange the participants");
    egui::Popup::menu(&button).show(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let mut pick = |ui: &mut Ui, icon, label: &str, layout, strip: Option<Strip>| {
            let selected = room.layout == layout && strip.is_none_or(|s| s == room.strip);
            if icon_row(ui, icon, label, selected) {
                room.layout = layout;
                if let Some(strip) = strip {
                    room.strip = strip;
                }
                room.focus = None;
                ui.close();
            }
        };
        pick(ui, theme::ConsoleIcon::Speaker, "Speaker · others below", Layout::Speaker, Some(Strip::Bottom));
        pick(ui, theme::ConsoleIcon::StripTop, "Speaker · others above", Layout::Speaker, Some(Strip::Top));
        pick(ui, theme::ConsoleIcon::StripLeft, "Speaker · others on the left", Layout::Speaker, Some(Strip::Left));
        pick(ui, theme::ConsoleIcon::StripRight, "Speaker · others on the right", Layout::Speaker, Some(Strip::Right));
        pick(ui, theme::ConsoleIcon::Gallery, "Gallery · everyone the same size", Layout::Gallery, None);
        if room.pin.is_some() {
            ui.separator();
            if ui.button("Unpin · follow whoever speaks").clicked() {
                room.pin = None;
                ui.close();
            }
        }
        ui.separator();
        ui.label(RichText::new("Double-click anyone to fill the stage with them.").small().color(pal::DIM));
    });
}

/// Who gets the large picture: the spotlight, pin or speaker, else a shared
/// screen, someone with video, or anyone else.
fn main_id(room: &View, tiles: &[Tile]) -> Option<u64> {
    room.focus
        .or(room.pin)
        .or(room.speaker.map(|s| s.0))
        .filter(|id| tiles.iter().any(|t| t.id == *id))
        .or_else(|| tiles.iter().find(|t| !t.me && t.source == Source::Screen).map(|t| t.id))
        .or_else(|| tiles.iter().find(|t| !t.me && t.video.is_some()).map(|t| t.id))
        .or_else(|| tiles.iter().find(|t| !t.me).map(|t| t.id))
        .or_else(|| tiles.first().map(|t| t.id))
}

#[cfg(not(target_os = "android"))]
/// The large picture's current frame, for a recording. When the large
/// picture has no video (camera off), whoever has some: others before us.
pub(super) fn main_picture(active: &Active, state: &State) -> Option<Arc<slipstream_codec::YuvFrame>> {
    let tiles = tiles(active, state);
    let id = main_id(&active.room, &tiles)?;
    let main = tiles.iter().find(|t| t.id == id && t.video.is_some());
    let shown = main.or_else(|| tiles.iter().find(|t| !t.me && t.video.is_some())).or_else(|| tiles.iter().find(|t| t.video.is_some()))?;
    shown.video.as_ref().map(|v| v.2.clone())
}

fn stage(active: &mut Active, state: &State, ui: &mut Ui, rect: Rect) {
    let tiles = tiles(active, state);
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        active.room.focus = None;
    }
    let room = &mut active.room;
    if room.focus.is_some_and(|id| !tiles.iter().any(|t| t.id == id)) {
        room.focus = None;
    }
    if room.pin.is_some_and(|id| !tiles.iter().any(|t| t.id == id)) {
        room.pin = None;
    }
    if tiles.len() == 1 {
        draw(ui, room, &tiles[0], rect.shrink(4.0), false);
        let hint = rect.center_top() + vec2(0.0, 24.0);
        ui.painter().text(hint, Align2::CENTER_CENTER, "Waiting for others to join · voice and Subspace chat work with cameras off", theme::body(16.0), pal::DIM);
        return;
    }
    if let Some(id) = room.focus
        && let Some(tile) = tiles.iter().find(|t| t.id == id)
    {
        draw(ui, room, tile, rect, false);
        ui.painter().text(rect.right_top() + vec2(-12.0, 14.0), Align2::RIGHT_TOP, "Double-click or Esc to go back", theme::body(15.0), pal::TAN);
        return;
    }
    match room.layout {
        Layout::Gallery => {
            let (cols, size) = grid(tiles.len(), rect.size());
            let rows = tiles.len().div_ceil(cols);
            let used = vec2(cols as f32 * size.x, rows as f32 * size.y);
            let origin = rect.center() - used / 2.0;
            for (i, tile) in tiles.iter().enumerate() {
                let (c, r) = (i % cols, i / cols);
                // Centre a short last row.
                let in_row = if r == rows - 1 { tiles.len() - r * cols } else { cols };
                let shift = (cols - in_row) as f32 * size.x / 2.0;
                let min = origin + vec2(c as f32 * size.x + shift, r as f32 * size.y);
                draw(ui, room, tile, Rect::from_min_size(min, size).shrink(4.0), false);
            }
        }
        Layout::Speaker => {
            let main_id = main_id(room, &tiles).unwrap_or(tiles[0].id);
            let horizontal = matches!(room.strip, Strip::Bottom | Strip::Top);
            let thickness = if horizontal { (rect.height() * 0.2).clamp(90.0, 150.0) } else { (rect.width() * 0.2).clamp(150.0, 240.0) };
            let (main, strip) = match room.strip {
                Strip::Bottom => (rect.with_max_y(rect.max.y - thickness), rect.with_min_y(rect.max.y - thickness)),
                Strip::Top => (rect.with_min_y(rect.min.y + thickness), rect.with_max_y(rect.min.y + thickness)),
                Strip::Left => (rect.with_min_x(rect.min.x + thickness), rect.with_max_x(rect.min.x + thickness)),
                Strip::Right => (rect.with_max_x(rect.max.x - thickness), rect.with_min_x(rect.max.x - thickness)),
            };
            if let Some(tile) = tiles.iter().find(|t| t.id == main_id) {
                draw(ui, room, tile, main.shrink(4.0), false);
            }
            let others: Vec<&Tile> = tiles.iter().filter(|t| t.id != main_id).collect();
            let size = if horizontal { vec2(thickness * 16.0 / 9.0, thickness) } else { vec2(thickness, thickness * 9.0 / 16.0) };
            let mut child = ui.new_child(UiBuilder::new().max_rect(strip));
            let scroll = if horizontal { egui::ScrollArea::horizontal() } else { egui::ScrollArea::vertical() };
            scroll.id_salt("meeting-strip").auto_shrink([false, false]).show(&mut child, |ui| {
                let layout = if horizontal { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::top_down(egui::Align::Center) };
                ui.with_layout(layout, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for tile in others {
                        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                        draw(ui, room, tile, r.shrink(4.0), true);
                    }
                });
            });
        }
    }
}

/// Columns and tile size for `n` 16:9 tiles in `area`, as large as they fit.
fn grid(n: usize, area: Vec2) -> (usize, Vec2) {
    let mut best = (1, Vec2::ZERO);
    for cols in 1..=n.max(1) {
        let rows = n.div_ceil(cols);
        let cell = vec2(area.x / cols as f32, area.y / rows as f32);
        let w = cell.x.min(cell.y * 16.0 / 9.0);
        if w > best.1.x {
            best = (cols, vec2(w, w * 9.0 / 16.0));
        }
    }
    best
}

fn draw(ui: &mut Ui, room: &mut View, tile: &Tile, rect: Rect, small: bool) {
    let response = ui.interact(rect, ui.scope_id().with(("meeting-tile", tile.id, small)), Sense::click());
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 8.0, Color32::from_rgb(4, 6, 12));
    if let Some((stream, serial, frame)) = &tile.video {
        let fit = video::fit_rect(rect, (frame.width, frame.height));
        painter.add(eframe::egui_wgpu::Callback::new_paint_callback(
            fit,
            video::VideoCallback { stream: *stream, serial: *serial, frame: frame.clone() },
        ));
    } else {
        let radius = (rect.width().min(rect.height()) * 0.22).clamp(14.0, 90.0);
        let colors = [pal::VIOLET, pal::BLUE, pal::PEACH, pal::LAVENDER, pal::GOLD, pal::CYAN];
        painter.circle_filled(rect.center() - vec2(0.0, radius * 0.25), radius, colors[(tile.id % colors.len() as u64) as usize]);
        painter.text(rect.center() - vec2(0.0, radius * 0.25), Align2::CENTER_CENTER, initials(&tile.name), theme::display_bold(radius * 0.9), pal::BG);
        if !small && tile.source != Source::None && !tile.me {
            painter.text(rect.center_bottom() - vec2(0.0, 44.0), Align2::CENTER_CENTER, "Video for the first few people only: double-click to watch", theme::body(13.0), pal::DIM);
        }
    }
    let level = room.levels.get(&tile.id).copied().unwrap_or(0.0);
    let speaking = !tile.me && tile.microphone && level > SPEAKING;
    let (width, color) = if speaking {
        (3.0, pal::GREEN)
    } else if room.pin == Some(tile.id) {
        (2.0, pal::LAVENDER)
    } else if response.hovered() {
        (1.5, pal::BLUE)
    } else {
        (1.0, pal::GRID)
    };
    painter.rect_stroke(rect, 8.0, Stroke::new(width, color), egui::StrokeKind::Inside);
    let label = format!(
        "{} {}{}{}",
        if tile.microphone { "🎤" } else { "🔇" },
        tile.name,
        if tile.me { " (you)" } else { "" },
        if tile.source == Source::Screen { " · screen" } else { "" }
    );
    let font = theme::body(if small { 13.0 } else { 16.0 });
    let galley = painter.layout_no_wrap(label, font, pal::TEXT);
    let pad = vec2(8.0, 4.0);
    let at = Pos2::new(rect.min.x + 6.0, rect.max.y - galley.size().y - pad.y * 2.0 - 6.0);
    painter.rect_filled(Rect::from_min_size(at, galley.size() + pad * 2.0), 6.0, Color32::from_black_alpha(170));
    painter.galley(at + pad, galley, pal::TEXT);
    if response.double_clicked() {
        room.focus = if room.focus == Some(tile.id) { None } else { Some(tile.id) };
    } else if response.clicked() && small {
        room.pin = Some(tile.id);
    }
    response.context_menu(|ui| {
        if ui.button(if room.focus == Some(tile.id) { "Back to the layout" } else { "Fill the stage" }).clicked() {
            room.focus = if room.focus == Some(tile.id) { None } else { Some(tile.id) };
            ui.close();
        }
        if room.pin == Some(tile.id) {
            if ui.button("Unpin · follow whoever speaks").clicked() {
                room.pin = None;
                ui.close();
            }
        } else if ui.button("Pin as the large picture").clicked() {
            room.pin = Some(tile.id);
            room.layout = Layout::Speaker;
            ui.close();
        }
    });
    response.on_hover_text(if small { "Click to make large · double-click to fill the stage" } else { "Double-click to fill the stage" });
}

fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let pick = |w: &str| w.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [first, .., last] => pick(first) + &pick(last),
    }
}

/// Subspace chat with reactions, filling its column.
fn chat(meetings: &mut Meetings, active: &mut Active, state: &State, ui: &mut Ui) {
    ui.label(RichText::new("SUBSPACE CHAT").font(theme::display_bold(18.0)).color(pal::VIOLET));
    ui.label(RichText::new("Shared with everyone in this meeting").small().color(pal::DIM));
    let input_height = 96.0;
    let height = (ui.available_height() - input_height).max(80.0);
    egui::ScrollArea::vertical().id_salt("meeting-chat").max_height(height).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        if active.chat.is_empty() {
            ui.label(RichText::new("No messages yet. Say hello to the meeting.").color(pal::DIM));
        }
        for line in &active.chat {
            ui.push_id(line.id, |ui| {
                let top = ui.cursor().min;
                ui.label(RichText::new(&line.name).color(if line.sender == state.local { pal::GREEN } else { pal::LAVENDER }));
                ui.label(&line.text);
                let reactions = active.room.reactions.get(&line.id).filter(|r| r.values().any(|who| !who.is_empty()));
                if let Some(reactions) = reactions {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (reaction, who) in reactions {
                            if who.is_empty() {
                                continue;
                            }
                            let mine = who.contains(&state.local);
                            let names: Vec<String> = who
                                .iter()
                                .filter_map(|id| state.participants.iter().find(|p| p.id == *id).map(|p| p.name.clone()))
                                .collect();
                            let chip = egui::Frame::new()
                                .fill(if mine { pal::VIOLET.gamma_multiply(0.45) } else { pal::GRID })
                                .stroke(Stroke::new(1.0, if mine { pal::VIOLET } else { pal::GRID }))
                                .corner_radius(10.0)
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 3.0;
                                        super::reactions::image(ui, *reaction, 18.0);
                                        ui.label(RichText::new(who.len().to_string()).color(pal::TEXT));
                                    });
                                });
                            let chip = ui.interact(chip.response.rect, ui.scope_id().with(("chip", *reaction)), Sense::click());
                            let hover = format!("{} · {}", super::reactions::NAMES[*reaction as usize], names.join(", "));
                            if chip.on_hover_text(hover).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                active.meeting.command(Command::React { chat: line.id, reaction: *reaction });
                            }
                        }
                    });
                }
                // React: on hover, at the message's top right.
                let rect = Rect::from_min_max(top, Pos2::new(ui.max_rect().right(), ui.cursor().min.y));
                let popup = egui::Id::unique(("meeting-react", line.id));
                if ui.rect_contains_pointer(rect) || egui::Popup::is_id_open(ui.ctx(), popup) {
                    let spot = Rect::from_min_size(Pos2::new(rect.right() - 64.0, rect.top()), vec2(62.0, 22.0));
                    let button = ui.put(spot, egui::Button::new(RichText::new("+ React").size(13.0).color(pal::TAN)).fill(pal::PANEL).corner_radius(8.0))
                        .on_hover_text("React to this message");
                    egui::Popup::menu(&button).id(popup).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        let mut more = ui.data(|d| d.get_temp::<bool>(popup.with("more")).unwrap_or(false));
                        if let Some(reaction) = super::reactions::picker(ui, &mut more) {
                            active.meeting.command(Command::React { chat: line.id, reaction });
                            more = false;
                            ui.close();
                        }
                        ui.data_mut(|d| d.insert_temp(popup.with("more"), more));
                    });
                }
                ui.add_space(6.0);
            });
        }
    });
    ui.add_space(6.0);
    let response = ui.add(
        theme::text_area(&mut active.message)
            .desired_rows(2)
            .desired_width(ui.available_width())
            .hint_text("Message everyone · Enter to send")
            .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter)),
    );
    let enter = response.has_focus() && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let can_send = !active.message.trim().is_empty();
    let mut send = false;
    ui.add_enabled_ui(can_send, |ui| {
        send = theme::pill(ui, "Send", pal::BLUE).clicked();
    });
    if can_send && (send || enter) {
        if active.message.len() <= 4000 {
            active.meeting.command(Command::Chat(std::mem::take(&mut active.message)));
        } else {
            meetings.error = Some("Keep messages under 4000 bytes".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slipstream_core::meeting::Participant;

    fn state(ids: &[(u64, Source)]) -> State {
        State {
            local: 1,
            participants: ids.iter().map(|(id, source)| Participant { id: *id, name: format!("P{id}"), microphone: true, source: *source }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn speaker_switches_after_a_hold_and_video_follows_focus_pin_speaker() {
        let state = state(&[(1, Source::Camera), (2, Source::Camera), (3, Source::Screen), (4, Source::None)]);
        let mut view = View::default();
        view.levels.insert(2, 0.1);
        view.update_speaker(&state);
        assert_eq!(view.speaker.map(|s| s.0), Some(2));
        // 3 talks over 2 straight away: held until 2 has had the floor a moment.
        view.levels.insert(3, 0.2);
        view.update_speaker(&state);
        assert_eq!(view.speaker.map(|s| s.0), Some(2));
        view.speaker = Some((2, Instant::now() - Duration::from_secs(5)));
        view.update_speaker(&state);
        assert_eq!(view.speaker.map(|s| s.0), Some(3));
        // We never subscribe to ourselves or to someone without video.
        assert_eq!(view.video_priority(&state), vec![3, 2]);
        view.focus = Some(2);
        assert_eq!(view.video_priority(&state)[0], 2);
        view.focus = Some(4);
        assert!(!view.video_priority(&state).contains(&4));
    }

    #[test]
    fn gallery_grid_fills_the_area() {
        assert_eq!(grid(1, vec2(1600.0, 900.0)).0, 1);
        assert_eq!(grid(4, vec2(1600.0, 900.0)).0, 2);
        let (cols, size) = grid(9, vec2(1600.0, 900.0));
        assert_eq!(cols, 3);
        assert!(size.x * 3.0 <= 1600.0 + 0.1 && size.y * 3.0 <= 900.0 + 0.1);
        assert_eq!(initials("Jean-Luc Picard"), "JP");
        assert_eq!(initials("data"), "DA");
    }
}
