//! Chat-only links to saved devices: the Subspace chat panel on the Bridge,
//! one tab per workstation. The host captures nothing for these links until
//! the technician presses Connect, which turns the chat into a full session
//! without reconnecting.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, vec2};
use slipstream_core::directory::DeviceId;
use slipstream_session::{AccountRef, LinkMode, StationRef, ViewerCmd, ViewerEvent, ViewerSession};
use tokio::sync::{mpsc, oneshot};

use super::{App, Page, home, theme, theme::pal};

type Linked = Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>), String>;

/// Most chat tabs open at once; the oldest closed one makes way.
const MAX_CHATS: usize = 12;

#[derive(Default)]
pub struct Chats {
    links: Vec<ChatLink>,
    active: Option<u64>,
    next: u64,
    /// Height of the device list beside the panel, from the last frame.
    pub beside: f32,
}

struct ChatLink {
    key: u64,
    account: AccountRef,
    device: DeviceId,
    name: String,
    password: String,
    state: State,
    log: Vec<(bool, String)>,
    input: String,
    notice: Option<String>,
    unread: bool,
}

enum State {
    Connecting { progress: Arc<Mutex<String>>, rx: oneshot::Receiver<Linked>, task: tokio::task::JoinHandle<()> },
    Live { session: Box<ViewerSession>, events: mpsc::UnboundedReceiver<ViewerEvent> },
    Closed(String),
}

impl Drop for ChatLink {
    fn drop(&mut self) {
        if let State::Connecting { task, .. } = &self.state {
            task.abort();
        }
    }
}

impl Chats {
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// The device's chat has messages nobody has looked at yet.
    pub fn unread(&self, account: &AccountRef, device: DeviceId) -> bool {
        self.links.iter().any(|l| l.unread && l.device == device && &l.account == account)
    }

    /// A chat that is connected and can become a full session.
    pub fn live(&self, account: &AccountRef, device: DeviceId) -> Option<u64> {
        self.links.iter()
            .find(|l| l.device == device && &l.account == account && matches!(l.state, State::Live { .. }))
            .map(|l| l.key)
    }

    fn find(&self, account: &AccountRef, device: DeviceId) -> Option<usize> {
        self.links.iter().position(|l| l.device == device && &l.account == account)
    }
}

/// The Chat button beside a saved device: focus its chat, or open one with
/// its saved password (asking for one when there is none).
pub fn open(app: &mut App, ctx: &egui::Context, account: &AccountRef, device: DeviceId, name: &str) {
    if let Some(i) = app.chats.find(account, device) {
        let link = &mut app.chats.links[i];
        app.chats.active = Some(link.key);
        link.unread = false;
        if !matches!(link.state, State::Closed(_)) {
            return;
        }
    }
    match app.client.saved_password(account, device) {
        Ok(Some(password)) => start(app, ctx, account, device, name, password),
        Ok(None) => super::saved::ask_password(app, account, device, name, super::saved::Purpose::Chat, None),
        Err(e) => {
            app.toast(format!("{e:#}"), pal::RED);
            super::saved::ask_password(app, account, device, name, super::saved::Purpose::Chat, None);
        }
    }
}

/// Opens (or reopens) the chat link with `password`.
pub fn start(app: &mut App, ctx: &egui::Context, account: &AccountRef, device: DeviceId, name: &str, password: String) {
    let progress = Arc::new(Mutex::new(format!("Locating {device}…")));
    let (tx, rx) = oneshot::channel();
    let (client, station, wake, p) = (app.client.clone(), StationRef::Account(account.clone()), ctx.clone(), progress.clone());
    let pw = password.clone();
    let task = app.rt.spawn(async move {
        let w = wake.clone();
        let progress = move |msg| {
            *p.lock().unwrap() = msg;
            w.request_repaint();
        };
        let result = client.connect_on_with(&station, device, &pw, LinkMode::ChatOnly, progress).await
            .map_err(|e| format!("{e:#}"))
            .map(|(session, events)| (session, wake_on_events(events, wake.clone())));
        let _ = tx.send(result);
        wake.request_repaint();
    });
    let state = State::Connecting { progress, rx, task };
    let chats = &mut app.chats;
    let key = match chats.find(account, device) {
        Some(i) => {
            let link = &mut chats.links[i];
            // A connection still in progress is superseded: stop it, or it
            // would finish a link of its own and drop it again.
            if let State::Connecting { task, .. } = &link.state {
                task.abort();
            }
            link.state = state;
            link.password = password;
            link.notice = None;
            link.key
        }
        None => {
            if chats.links.len() >= MAX_CHATS {
                match chats.links.iter().position(|l| matches!(l.state, State::Closed(_))) {
                    Some(i) => {
                        chats.links.remove(i);
                    }
                    None => {
                        if let State::Connecting { task, .. } = &state {
                            task.abort();
                        }
                        app.toast(format!("{MAX_CHATS} chats are open: close one first"), pal::GOLD);
                        return;
                    }
                }
            }
            chats.next += 1;
            chats.links.push(ChatLink {
                key: chats.next,
                account: account.clone(),
                device,
                name: name.to_string(),
                password,
                state,
                log: Vec::new(),
                input: String::new(),
                notice: None,
                unread: false,
            });
            chats.next
        }
    };
    chats.active = Some(key);
}

/// Session events arrive on a channel the UI only reads when it draws:
/// forward them, waking the UI for everything but the once-a-second stats.
fn wake_on_events(mut events: mpsc::UnboundedReceiver<ViewerEvent>, ctx: egui::Context) -> mpsc::UnboundedReceiver<ViewerEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let wake = !matches!(event, ViewerEvent::Stats(_));
            if tx.send(event).is_err() {
                break;
            }
            if wake {
                ctx.request_repaint();
            }
        }
    });
    rx
}

/// A refused password, as opposed to a lockout or a network problem.
pub fn wrong_password(error: &str) -> bool {
    error.contains("wrong password") && !error.contains("too many")
}

/// Every frame: finished connections and new messages.
pub fn poll(app: &mut App, ctx: &egui::Context) {
    // SLIPSTREAM_TEST_CHATS=1 opens chats with the first two saved devices,
    // the first with sample messages (screenshots).
    static TESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if std::env::var("SLIPSTREAM_TEST_CHATS").as_deref() == Ok("1") && !TESTED.swap(true, std::sync::atomic::Ordering::Relaxed)
        && let Some(a) = app.client.settings().accounts.first()
    {
        let r = AccountRef::of(a);
        for d in a.devices.iter().chain(&a.shared).take(2) {
            start(app, ctx, &r, d.id, &d.name, "test".into());
        }
        if let Some(first) = app.chats.links.first_mut() {
            app.chats.active = Some(first.key);
            first.log = vec![
                (false, "Hi, the label printer is jammed again.".into()),
                (true, "On it. Can you leave the PC on for a few minutes?".into()),
            ];
        }
    }
    // Read only when someone can see it: the Bridge, in front, not minimized.
    let minimized = ctx.input(|i| i.raw.viewports.get(&egui::ViewportId::ROOT).is_some_and(|v| v.minimized == Some(true)));
    let looking = app.page == Page::Home && app.window_focused && !app.hidden && !minimized;
    let mut toasts = Vec::new();
    let mut ask = None;
    for link in &mut app.chats.links {
        let showing = looking && app.chats.active == Some(link.key);
        if let State::Connecting { rx, .. } = &mut link.state {
            match rx.try_recv() {
                Ok(Ok((session, events))) => link.state = State::Live { session: Box::new(session), events },
                Ok(Err(e)) => {
                    if wrong_password(&e) {
                        ask = Some((link.account.clone(), link.device, link.name.clone()));
                    }
                    link.state = State::Closed(e);
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => link.state = State::Closed("the link attempt stopped".into()),
            }
        }
        let mut ended = None;
        if let State::Live { events, .. } = &mut link.state {
            while let Ok(event) = events.try_recv() {
                match event {
                    ViewerEvent::Chat(text) => {
                        if !showing {
                            link.unread = true;
                            toasts.push(format!("Message from {}", link.name));
                        }
                        link.log.push((false, text));
                    }
                    ViewerEvent::Notice(n) => link.notice = Some(n),
                    ViewerEvent::Disconnected { reason, .. } => ended = Some(reason),
                    _ => {}
                }
            }
        }
        if let Some(reason) = ended {
            link.state = State::Closed(reason);
        }
        if showing {
            link.unread = false;
        }
    }
    if !toasts.is_empty() && !looking {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    }
    for text in toasts {
        app.toast(text, pal::BLUE);
    }
    if let Some((account, device, name)) = ask {
        super::saved::ask_password(app, &account, device, &name, super::saved::Purpose::Chat, Some(super::saved::REFUSED.into()));
    }
}

/// Turns a connected chat into a full remote session (picture, control,
/// clipboard and sound as Config says), keeping its messages.
pub fn promote(app: &mut App, ctx: &egui::Context, key: u64) {
    let Some(i) = app.chats.links.iter().position(|l| l.key == key && matches!(l.state, State::Live { .. })) else { return };
    let mut link = app.chats.links.remove(i);
    let State::Live { session, events } = std::mem::replace(&mut link.state, State::Closed(String::new())) else { return };
    if app.chats.active == Some(key) {
        app.chats.active = app.chats.links.last().map(|l| l.key);
    }
    session.send(ViewerCmd::StartViewing);
    let log = std::mem::take(&mut link.log);
    let password = std::mem::take(&mut link.password);
    app.open_link_tab(ctx, *session, events, password, StationRef::Account(link.account.clone()), None, log);
}

fn close(app: &mut App, key: u64) {
    app.chats.links.retain(|l| l.key != key);
    if app.chats.active == Some(key) {
        app.chats.active = app.chats.links.last().map(|l| l.key);
    }
}

/// The Subspace chat panel: a tab per workstation, and the chosen chat.
pub fn panel(app: &mut App, ui: &mut Ui, height: f32) {
    let ctx = ui.ctx().clone();
    let t = app.time();
    let me = app.client.with_settings(|s| s.device_name.clone());
    if app.chats.active.is_none_or(|k| !app.chats.links.iter().any(|l| l.key == k)) {
        app.chats.active = app.chats.links.last().map(|l| l.key);
    }
    theme::panel(ui, "Subspace chat", pal::BLUE, |ui| {
        ui.set_min_height(height.max(260.0) - 50.0);
        // Shown only while a chat is open (see home::devices_and_chats).
        if app.chats.links.is_empty() {
            return;
        }
        let (mut choose, mut close_key) = (None, None);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for link in &app.chats.links {
                let active = app.chats.active == Some(link.key);
                if let Some(action) = tab(ui, link, active, t) {
                    match action {
                        TabAction::Choose => choose = Some(link.key),
                        TabAction::Close => close_key = Some(link.key),
                    }
                }
            }
        });
        if let Some(key) = choose {
            app.chats.active = Some(key);
        }
        if let Some(key) = close_key {
            close(app, key);
        }
        ui.add_space(6.0);
        let Some(key) = app.chats.active else { return };
        let Some(i) = app.chats.links.iter().position(|l| l.key == key) else { return };
        let (mut connect, mut retry, mut end) = (false, false, false);
        {
            let link = &mut app.chats.links[i];
            link.unread = false;
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    end = theme::pill(ui, "End", pal::RED).on_hover_text("Close this chat and its link").clicked();
                    match &link.state {
                        State::Live { .. } => {
                            connect = theme::pill(ui, "Connect", pal::ORANGE)
                                .on_hover_text("Show and control this computer, keeping the chat")
                                .clicked();
                        }
                        State::Closed(_) => retry = theme::pill(ui, "Reconnect", pal::GREEN).clicked(),
                        State::Connecting { .. } => {}
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(link.name.to_uppercase()).font(theme::display(18.0)).color(pal::TAN)).truncate());
                        ui.label(RichText::new(link.device.to_string()).font(theme::mono(13.0)).color(pal::BLUE));
                    });
                });
            });
            let (status, color) = match &link.state {
                State::Connecting { progress, .. } => (progress.lock().unwrap().clone(), pal::GOLD),
                State::Live { session, .. } if !session.info().capabilities.chat =>
                    ("Linked, but that computer doesn't accept chat (Config → Permissions there)".into(), pal::GOLD),
                State::Live { session, .. } => (format!("Chat link open with {} · screen not shared", session.info().host_name), pal::GREEN),
                State::Closed(reason) => (format!("Link closed: {reason}"), pal::RED),
            };
            ui.label(RichText::new(status).small().color(color));
            if let Some(notice) = &link.notice {
                ui.label(RichText::new(notice).small().color(pal::GOLD));
            }
            ui.add_space(4.0);
            let rest = (height - 150.0).max(180.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), rest), Sense::hover());
            let mut chat = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(("subspace-chat", key)));
            match &link.state {
                State::Live { session, .. } if session.info().capabilities.chat => {
                    let cmds = session.command_sender();
                    let names = home::ChatNames { me: &me, them: &session.info().host_name };
                    home::chat_ui(&mut chat, &mut link.log, &mut link.input, names, |text| {
                        let _ = cmds.send(ViewerCmd::Chat(text));
                    });
                }
                _ => {
                    // Keep the history readable; messages can't go anywhere.
                    let mut idle = String::new();
                    let names = home::ChatNames { me: &me, them: &link.name };
                    chat.add_enabled_ui(false, |ui| home::chat_ui(ui, &mut link.log, &mut idle, names, |_| {}));
                }
            }
        }
        if connect {
            promote(app, &ctx, key);
        } else if retry {
            let link = &app.chats.links[i];
            let (account, device, name, password) = (link.account.clone(), link.device, link.name.clone(), link.password.clone());
            start(app, &ctx, &account, device, &name, password);
        } else if end {
            close(app, key);
        }
    });
    if app.chats.links.iter().any(|l| matches!(l.state, State::Connecting { .. })) {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

enum TabAction {
    Choose,
    Close,
}

/// One chat's tab: a status lamp, the name (◆ for unread) and a close cross.
fn tab(ui: &mut Ui, link: &ChatLink, active: bool, t: f64) -> Option<TabAction> {
    let lamp = match link.state {
        State::Live { .. } => pal::GREEN,
        State::Connecting { .. } => pal::GOLD,
        State::Closed(_) => pal::DIM,
    };
    let color = if active { pal::BLUE } else { pal::PERIWINKLE.gamma_multiply(0.55) };
    let label = format!("{}{}", if link.unread { "◆ " } else { "" }, link.name.to_uppercase());
    let font = theme::display(15.0);
    let galley = ui.painter().layout_no_wrap(label, font, pal::BG);
    let text_w = galley.size().x.min(150.0);
    let size = vec2(26.0 + text_w + 30.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect);
    let radius = egui::CornerRadius { nw: 12, ne: 12, sw: 4, se: 4 };
    painter.rect_filled(rect, radius, if response.hovered() && !active { pal::BLUE.gamma_multiply(0.8) } else { color });
    painter.circle_filled(rect.left_center() + vec2(14.0, 0.0), 6.0, pal::BG);
    theme::lamp(&painter, rect.left_center() + vec2(14.0, 0.0), lamp, t);
    let text_rect = egui::Rect::from_min_size(rect.min + vec2(26.0, (rect.height() - galley.size().y) / 2.0), vec2(text_w, galley.size().y));
    painter.with_clip_rect(text_rect).galley(text_rect.min, galley, pal::BG);
    let cross = egui::Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), vec2(18.0, 18.0));
    let close = ui.interact(cross, ui.make_persistent_id(("chat-tab-close", link.key)), Sense::click());
    let ink = if close.hovered() { pal::RED } else { pal::BG };
    let c = cross.center();
    painter.line_segment([c - vec2(4.0, 4.0), c + vec2(4.0, 4.0)], egui::Stroke::new(2.0, ink));
    painter.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], egui::Stroke::new(2.0, ink));
    let state = match &link.state {
        State::Live { .. } => "connected".to_string(),
        State::Connecting { .. } => "connecting…".to_string(),
        State::Closed(reason) => format!("closed: {reason}"),
    };
    let (closed, chosen) = (close.clicked(), response.clicked());
    close.on_hover_text("Close this chat");
    response.on_hover_text(format!("{} · {} · {state}", link.name, link.device));
    if closed {
        Some(TabAction::Close)
    } else if chosen {
        Some(TabAction::Choose)
    } else {
        None
    }
}
