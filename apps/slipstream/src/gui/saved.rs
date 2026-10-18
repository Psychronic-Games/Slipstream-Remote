//! Stations a technician signs in to and the devices saved on them: the
//! Saved devices panel on the Bridge, the Stations panel in Config, and the
//! dialogs to add, edit and connect.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, vec2};
use slipstream_core::{account::SavedDevice, config::StationAccount, directory::DeviceId};
use slipstream_session::{AccountRef, Presence};
use tokio::sync::oneshot;

use super::{App, Page, theme, theme::pal};

/// How often the Bridge re-checks which saved devices are online, and the
/// lists are fetched again.
const PRESENCE_EVERY: Duration = Duration::from_secs(30);
const SYNC_EVERY: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct Saved {
    // The sign-in form (Config → Stations).
    station: String,
    username: String,
    password: String,
    /// A station call in progress: what for, and its outcome (a message
    /// for a toast).
    job: Option<(&'static str, oneshot::Receiver<Result<String, String>>)>,
    presence: Arc<Mutex<HashMap<(AccountRef, DeviceId), Presence>>>,
    busy: Arc<AtomicBool>,
    last_presence: Option<Instant>,
    last_sync: Option<Instant>,
    editor: Option<Editor>,
    prompt: Option<Prompt>,
}

/// Adding or editing a saved device.
struct Editor {
    account: AccountRef,
    id: String,
    name: String,
    notes: String,
    group: String,
    shared: bool,
    /// Editing this entry (its ID and list); `None` when adding.
    original: Option<(DeviceId, bool)>,
    password: String,
    had_password: bool,
    forget_password: bool,
}

/// Asking for a password before connecting.
struct Prompt {
    account: AccountRef,
    id: DeviceId,
    name: String,
    password: String,
    remember: bool,
    /// Own list (passwords can be remembered there, not in the shared one).
    own: bool,
}

fn accounts(app: &App) -> Vec<StationAccount> {
    app.client.settings().accounts
}

/// Runs a station call in the background; its message (or error) becomes a
/// toast. One at a time.
fn spawn_job<F>(app: &mut App, ctx: &egui::Context, what: &'static str, job: F)
where
    F: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
{
    if app.saved.job.is_some() {
        app.toast("One moment: still talking to the station", pal::GOLD);
        return;
    }
    let (tx, rx) = oneshot::channel();
    let wake = ctx.clone();
    app.rt.spawn(async move {
        let _ = tx.send(job.await.map_err(|e| format!("{e:#}")));
        wake.request_repaint();
    });
    app.saved.job = Some((what, rx));
}

/// Every frame: finished jobs, and the periodic refresh of lists and
/// online lamps.
pub fn poll(app: &mut App, ctx: &egui::Context) {
    // SLIPSTREAM_TEST_SAVED=edit opens the editor on the first saved device
    // (screenshots).
    static TESTED: AtomicBool = AtomicBool::new(false);
    if std::env::var("SLIPSTREAM_TEST_SAVED").as_deref() == Ok("edit") && !TESTED.swap(true, Ordering::Relaxed)
        && let Some(a) = accounts(app).first()
        && let Some(d) = a.devices.first()
    {
        open_editor(app, &AccountRef::of(a), Some(d), false);
    }
    if let Some((what, rx)) = &mut app.saved.job
        && let Ok(result) = rx.try_recv()
    {
        let what = *what;
        app.saved.job = None;
        match result {
            Ok(msg) if !msg.is_empty() => app.toast(msg, pal::GREEN),
            Ok(_) => {}
            Err(e) => app.toast(format!("{what}: {e}"), pal::RED),
        }
        // Lists changed: look at who's online again soon.
        app.saved.last_presence = None;
    }
    let accounts = accounts(app);
    if accounts.is_empty() || app.saved.busy.load(Ordering::Relaxed) {
        return;
    }
    let due = |t: Option<Instant>, every| t.is_none_or(|t: Instant| t.elapsed() >= every);
    let sync = due(app.saved.last_sync, SYNC_EVERY);
    let presence = app.page == Page::Home && due(app.saved.last_presence, PRESENCE_EVERY);
    if !sync && !presence {
        ctx.request_repaint_after(Duration::from_secs(5));
        return;
    }
    if sync {
        app.saved.last_sync = Some(Instant::now());
    }
    app.saved.last_presence = Some(Instant::now());
    app.saved.busy.store(true, Ordering::Relaxed);
    let (client, map, busy, wake) = (app.client.clone(), app.saved.presence.clone(), app.saved.busy.clone(), ctx.clone());
    app.rt.spawn(async move {
        if sync {
            client.sync_accounts().await;
        }
        for a in client.settings().accounts.iter().filter(|a| !a.signed_out) {
            let r = AccountRef::of(a);
            let ids: Vec<DeviceId> = a.devices.iter().chain(&a.shared).map(|d| d.id).collect();
            let found = client.presence(&r, &ids).await;
            let mut map = map.lock().unwrap();
            for (id, p) in found {
                map.insert((r.clone(), id), p);
            }
        }
        busy.store(false, Ordering::Relaxed);
        wake.request_repaint();
    });
}

/// The Bridge's list of saved devices, per station.
pub fn panel(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let t = app.time();
    let accounts = accounts(app);
    theme::panel(ui, "Saved devices", pal::VIOLET, |ui| {
        if accounts.is_empty() {
            ui.label(
                RichText::new(
                    "Sign in to a station to keep a list of devices there, with their passwords if you like. \
                     It follows you to any computer, and works with other organisations' stations too.",
                )
                .color(pal::DIM),
            );
            ui.add_space(6.0);
            if theme::pill(ui, "Sign in to a station", pal::VIOLET).clicked() {
                app.page = Page::Settings;
            }
            return;
        }
        for (i, a) in accounts.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.0);
            }
            let r = AccountRef::of(a);
            ui.horizontal(|ui| {
                ui.label(RichText::new(a.label().to_uppercase()).font(theme::display(18.0)).color(pal::LAVENDER));
                ui.label(RichText::new(format!("· {}", a.user.name)).color(pal::DIM));
                if a.signed_out {
                    ui.label(RichText::new("SIGNED OUT: sign in again under Config → Stations").font(theme::display(14.0)).color(pal::RED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !a.signed_out && theme::pill(ui, "+ Add device", pal::VIOLET).clicked() {
                        open_editor(app, &r, None, false);
                    }
                    if !a.signed_out {
                        let own = own_device(app, a);
                        let button = ui.add_enabled_ui(own.is_some(), |ui| theme::pill(ui, "Add this device", pal::BLUE)).inner
                            .on_hover_text("Fill in this computer's ID and name; choose a group before saving")
                            .on_disabled_hover_text("This device needs an ID on this station. Its ID is shown in This device above.");
                        if button.clicked() && let Some(device) = own {
                            let existing = a.devices.iter().any(|d| d.id == device.id);
                            open_editor(app, &r, Some(&device), false);
                            if !existing && let Some(editor) = &mut app.saved.editor { editor.original = None; }
                        }
                    }
                });
            });
            if a.devices.is_empty() && a.shared.is_empty() {
                ui.label(RichText::new("Nothing saved yet: add a device, or press ★ on a recent link.").color(pal::DIM));
            }
            grouped_devices(app, ui, &ctx, &r, a, &a.devices, false, t);
            if !a.shared.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("SHARED BY {}", a.label().to_uppercase())).font(theme::display(14.0)).color(pal::GOLD));
                grouped_devices(app, ui, &ctx, &r, a, &a.shared, true, t);
            }
        }
    });
}

/// Only use this computer's ID on the station it belongs to. The Windows
/// service's published ID takes precedence over the viewer-only GUI identity.
fn own_device(app: &App, account: &StationAccount) -> Option<SavedDevice> {
    if cfg!(target_os = "android") { return None; }
    let settings = app.client.settings();
    let id = match app.dir_status {
        slipstream_session::DirStatus::Online { id } => Some(id),
        _ => settings.host.last_device_id,
    };
    #[cfg(windows)]
    let id = if app.via_service { crate::service::hosted_id() } else { id };
    own_device_entry(account, settings.server.url.as_deref()?, id, &settings.device_name)
}

fn own_device_entry(account: &StationAccount, server: &str, id: Option<DeviceId>, name: &str) -> Option<SavedDevice> {
    if !account.is_station(server) { return None; }
    let id = id?;
    let existing = account.devices.iter().find(|d| d.id == id);
    Some(existing.cloned().unwrap_or_else(|| SavedDevice {
        id, name: name.to_string(), notes: String::new(), group: String::new(), secret: None, added_by: None,
    }))
}

#[allow(clippy::too_many_arguments)]
fn grouped_devices(app: &mut App, ui: &mut Ui, ctx: &egui::Context, r: &AccountRef,
    account: &StationAccount, devices: &[SavedDevice], shared: bool, t: f64) {
    let mut groups = BTreeMap::<&str, Vec<&SavedDevice>>::new();
    for device in devices { groups.entry(&device.group).or_default().push(device); }
    for (group, mut devices) in groups {
        devices.sort_by_key(|d| d.name.to_lowercase());
        let title = format!("{}  ·  {}", if group.is_empty() { "UNGROUPED".into() } else { group.to_uppercase() }, devices.len());
        egui::CollapsingHeader::new(RichText::new(title).font(theme::display_bold(17.0)).color(if shared { pal::GOLD } else { pal::LAVENDER }))
            .id_salt(("saved-group", &account.server, &account.user.username, shared, group))
            .default_open(true).show(ui, |ui| {
                for device in devices { device_row(app, ui, ctx, r, account, device, shared, t); }
            });
    }
}

#[allow(clippy::too_many_arguments)]
fn device_row(app: &mut App, ui: &mut Ui, ctx: &egui::Context, r: &AccountRef, a: &StationAccount, d: &SavedDevice, shared: bool, t: f64) {
    let presence = app.saved.presence.lock().unwrap().get(&(r.clone(), d.id)).cloned();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::hover());
        let (color, tip) = match presence {
            Some(Presence::Online) => (pal::GREEN, "online"),
            Some(Presence::Offline) => (pal::DIM, "offline"),
            _ => (pal::GOLD, "checking…"),
        };
        theme::lamp(ui.painter(), rect.center(), color, t);
        let name = if d.name.is_empty() { d.id.to_string() } else { d.name.clone() };
        ui.label(RichText::new(name.to_uppercase()).font(theme::display(18.0)).color(pal::TAN)).on_hover_text(tip);
        ui.label(RichText::new(d.id.to_string()).font(theme::mono(15.0)).color(pal::BLUE));
        if d.secret.is_some() {
            ui.label(RichText::new("🔑").color(pal::GOLD)).on_hover_text("Password saved: connects in one click");
        }
        if !d.notes.is_empty() {
            ui.add(egui::Label::new(RichText::new(&d.notes).color(pal::DIM)).truncate()).on_hover_text(&d.notes);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_edit = !a.signed_out && (!shared || a.user.admin);
            if can_edit && theme::pill_sized(ui, "Edit", pal::LAVENDER, vec2(58.0, 26.0)).clicked() {
                open_editor(app, r, Some(d), shared);
            }
            let busy = app.attempt.is_some();
            if ui.add_enabled_ui(!busy && !a.signed_out, |ui| theme::pill(ui, "Connect", pal::ORANGE)).inner.clicked() {
                connect(app, ctx, r, d, shared);
            }
        });
    });
}

fn connect(app: &mut App, ctx: &egui::Context, r: &AccountRef, d: &SavedDevice, shared: bool) {
    if !shared && d.secret.is_some() {
        match app.client.saved_password(r, d.id) {
            Ok(Some(pw)) => {
                app.start_connect_to(ctx, d.id, pw, Some(r.clone()));
                return;
            }
            Ok(None) => {}
            Err(e) => app.toast(format!("{e:#}"), pal::RED),
        }
    }
    app.saved.prompt = Some(Prompt {
        account: r.clone(),
        id: d.id,
        name: if d.name.is_empty() { d.id.to_string() } else { d.name.clone() },
        password: String::new(),
        remember: false,
        own: !shared,
    });
}

/// From a recent link: save it in a signed-in station's list.
pub fn save_recent(app: &mut App, id: DeviceId, name: &str) {
    let Some(a) = accounts(app).into_iter().find(|a| !a.signed_out) else {
        app.toast("Sign in to a station first (Config → Stations) to keep a list of devices", pal::VIOLET);
        return;
    };
    let draft = SavedDevice { id, name: name.to_string(), notes: String::new(), group: String::new(), secret: None, added_by: None };
    open_editor(app, &AccountRef::of(&a), Some(&draft), false);
    // It's a new entry, not an edit of an existing one.
    if let Some(e) = &mut app.saved.editor
        && !a.devices.iter().any(|d| d.id == id)
    {
        e.original = None;
    }
}

fn open_editor(app: &mut App, r: &AccountRef, d: Option<&SavedDevice>, shared: bool) {
    app.saved.editor = Some(Editor {
        account: r.clone(),
        id: d.map(|d| d.id.to_string()).unwrap_or_default(),
        name: d.map(|d| d.name.clone()).unwrap_or_default(),
        notes: d.map(|d| d.notes.clone()).unwrap_or_default(),
        group: d.map(|d| d.group.clone()).unwrap_or_default(),
        shared,
        original: d.map(|d| (d.id, shared)),
        password: String::new(),
        had_password: d.is_some_and(|d| d.secret.is_some()),
        forget_password: false,
    });
}

fn field(ui: &mut Ui, value: &mut String, hint: &str, password: bool, width: f32) -> egui::Response {
    theme::input_frame()
        .show(ui, |ui| ui.add(egui::TextEdit::singleline(value).align(egui::Align2::LEFT_CENTER).hint_text(hint).password(password).frame(egui::Frame::NONE).desired_width(width)))
        .inner
}

/// The add/edit and password dialogs (every frame).
pub fn windows(app: &mut App, ctx: &egui::Context) {
    editor_window(app, ctx);
    prompt_window(app, ctx);
}

fn editor_window(app: &mut App, ctx: &egui::Context) {
    let Some(mut e) = app.saved.editor.take() else { return };
    let all = accounts(app);
    let Some(account) = all.iter().find(|a| AccountRef::of(a) == e.account).cloned() else { return };
    let mut open = true;
    let (mut save, mut delete) = (false, false);
    let title = if e.original.is_some() { "Edit device" } else { "Save a device" };
    theme::window("saved-editor", pal::VIOLET)
        .id(egui::Id::unique("saved-editor"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let chrome = theme::window_header_with_close(ui, title, pal::VIOLET, pal::PEACH);
            if chrome.close {
                open = false;
            }
            egui::ScrollArea::vertical().id_salt("saved-editor-form")
                .max_height((ctx.content_rect().height() - 190.0).max(120.0))
                .show(ui, |ui| {
            egui::Grid::new("saved-editor-grid").num_columns(2).spacing([12.0, 14.0]).show(ui, |ui| {
                ui.label(RichText::new("STATION").font(theme::display(15.0)).color(pal::LAVENDER));
                if e.original.is_none() && all.iter().filter(|a| !a.signed_out).count() > 1 {
                    egui::ComboBox::from_id_salt("saved-editor-station").selected_text(account.label()).show_ui(ui, |ui| {
                        for a in all.iter().filter(|a| !a.signed_out) {
                            let r = AccountRef::of(a);
                            if ui.selectable_label(r == e.account, a.label()).clicked() {
                                e.account = r;
                            }
                        }
                    });
                } else {
                    ui.label(account.label());
                }
                ui.end_row();
                ui.label(RichText::new("ID").font(theme::display(15.0)).color(pal::LAVENDER));
                field(ui, &mut e.id, "482 913 077", false, 200.0);
                ui.end_row();
                ui.label(RichText::new("NAME").font(theme::display(15.0)).color(pal::LAVENDER));
                field(ui, &mut e.name, "Front desk PC", false, 280.0);
                ui.end_row();
                ui.label(RichText::new("GROUP").font(theme::display(15.0)).color(pal::LAVENDER));
                ui.horizontal_top(|ui| {
                    theme::input_frame().show(ui, |ui| {
                        ui.add(egui::TextEdit::singleline(&mut e.group).hint_text("Ungrouped / new group")
                            .char_limit(64).frame(egui::Frame::NONE).desired_width(175.0));
                    });
                    let groups: BTreeSet<_> = account.devices.iter().chain(&account.shared)
                        .map(|d| d.group.as_str()).filter(|g| !g.is_empty()).collect();
                    egui::ComboBox::from_id_salt("saved-editor-group").width(90.0).selected_text("Choose…").show_ui(ui, |ui| {
                        theme::console_menu_style(ui.style_mut());
                        if ui.selectable_label(e.group.is_empty(), "Ungrouped").clicked() { e.group.clear(); }
                        for group in groups {
                            if ui.selectable_label(e.group == group, group).clicked() { e.group = group.to_string(); }
                        }
                    });
                });
                ui.end_row();
                ui.label(RichText::new("NOTES").font(theme::display(15.0)).color(pal::LAVENDER));
                // Keep the text editor out of the grid layout: a frame that
                // inherits its cached row height cannot shrink after deleting lines.
                ui.vertical(|ui| {
                    theme::input_frame().show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut e.notes).hint_text("Optional notes…")
                            .desired_rows(1).char_limit(500).frame(egui::Frame::NONE).desired_width(280.0));
                    });
                });
                ui.end_row();
                if !e.shared {
                    ui.label(RichText::new("PASSWORD").font(theme::display(15.0)).color(pal::LAVENDER));
                    ui.vertical(|ui| {
                        if e.had_password && !e.forget_password {
                            ui.label(RichText::new("🔑 Saved. Type a new one to replace it.").color(pal::GOLD));
                        }
                        if !e.forget_password {
                            field(ui, &mut e.password, "its unattended password (optional)", true, 280.0);
                        }
                        if e.had_password {
                            ui.checkbox(&mut e.forget_password, "Forget the saved password");
                        }
                        ui.label(
                            RichText::new("Kept on the station encrypted: only your sign-in can open it.").small().color(pal::DIM),
                        );
                    });
                    ui.end_row();
                }
                if account.user.admin {
                    ui.label("");
                    ui.checkbox(&mut e.shared, "Shared with every technician at this station (no password)");
                    ui.end_row();
                }
            });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = theme::pill(ui, "Save", pal::GREEN).clicked();
                if e.original.is_some() {
                    delete = theme::pill(ui, "Remove", pal::RED).clicked();
                }
            });
            chrome.footer(ui);
        });
    if save {
        let id: DeviceId = match e.id.parse() {
            Ok(id) => id,
            Err(err) => {
                app.toast(format!("{err}"), pal::RED);
                app.saved.editor = Some(e);
                return;
            }
        };
        let device = SavedDevice { id, name: e.name.trim().to_string(), notes: e.notes.trim().to_string(), group: e.group.trim().to_string(), secret: None, added_by: None };
        let password = if e.shared {
            None
        } else if e.forget_password {
            Some(String::new())
        } else if !e.password.is_empty() {
            Some(e.password.clone())
        } else {
            None
        };
        let (client, r, shared, original) = (app.client.clone(), e.account.clone(), e.shared, e.original);
        let name = if device.name.is_empty() { id.to_string() } else { device.name.clone() };
        spawn_job(app, ctx, "Saving", async move {
            // Moved to another ID or list: the old entry goes.
            if let Some((old_id, old_shared)) = original
                && (old_id != id || old_shared != shared)
            {
                client.delete_device(&r, old_id, old_shared).await?;
            }
            client.save_device(&r, device, password, shared).await?;
            Ok(format!("Saved {name}"))
        });
        return;
    }
    if delete && let Some((id, shared)) = e.original {
        let (client, r) = (app.client.clone(), e.account.clone());
        spawn_job(app, ctx, "Removing", async move {
            client.delete_device(&r, id, shared).await?;
            Ok("Removed from the list".into())
        });
        return;
    }
    if open {
        app.saved.editor = Some(e);
    }
}

fn prompt_window(app: &mut App, ctx: &egui::Context) {
    let Some(mut p) = app.saved.prompt.take() else { return };
    let mut open = true;
    let mut go = false;
    let title = format!("Connect to {}", p.name);
    theme::window("saved-prompt", pal::ORANGE)
        .id(egui::Id::unique("saved-prompt"))
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let chrome = theme::window_header(ui, &title, pal::ORANGE);
            if chrome.close {
                open = false;
            }
            ui.label(RichText::new(format!("{} · its one-time or unattended password", p.id)).color(pal::DIM));
            let r = field(ui, &mut p.password, "password", true, 300.0);
            r.request_focus();
            go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if p.own {
                ui.checkbox(&mut p.remember, "Remember it (for unattended passwords)");
            }
            ui.add_space(6.0);
            go |= theme::pill(ui, "Engage", pal::ORANGE).clicked();
            chrome.footer(ui);
        });
    if go && !p.password.is_empty() {
        if p.remember {
            // Save it as well, sealed, alongside connecting.
            let (client, r, id, pw) = (app.client.clone(), p.account.clone(), p.id, p.password.clone());
            spawn_job(app, ctx, "Saving the password", async move {
                let a = client.settings().accounts.into_iter().find(|a| AccountRef::of(a) == r).ok_or_else(|| anyhow::anyhow!("signed out"))?;
                let mut device = a.devices.into_iter().find(|d| d.id == id).ok_or_else(|| anyhow::anyhow!("not in your list"))?;
                device.secret = None;
                client.save_device(&r, device, Some(pw), false).await?;
                Ok(String::new())
            });
        }
        app.start_connect_to(ctx, p.id, p.password, Some(p.account));
        return;
    }
    if open {
        app.saved.prompt = Some(p);
    }
}

/// Config → Stations: sign in and out.
pub fn stations_panel(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let accounts = accounts(app);
    let mut sign_out = None;
    theme::panel(ui, "Stations", pal::VIOLET, |ui| {
        ui.label(
            RichText::new(
                "Sign in as a technician to reach a station's devices and keep your list of them there: this app's \
                 own station or any other (your station admin sets up your username). You can be signed in to \
                 several at once.",
            )
            .color(pal::DIM),
        );
        ui.add_space(6.0);
        for a in &accounts {
            ui.horizontal(|ui| {
                ui.label(RichText::new(a.label().to_uppercase()).font(theme::display(17.0)).color(pal::LAVENDER));
                ui.label(RichText::new(&a.server).font(theme::mono(14.0)).color(pal::BLUE));
                let who = format!("{}{}", a.user.name, if a.user.admin { " · station admin" } else { "" });
                ui.label(RichText::new(who).color(pal::DIM));
                if a.signed_out {
                    ui.label(RichText::new("SIGNED OUT").font(theme::display(14.0)).color(pal::RED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::pill_sized(ui, "Sign out", pal::LAVENDER, vec2(84.0, 26.0)).clicked() {
                        sign_out = Some(AccountRef::of(a));
                    }
                    if a.signed_out && theme::pill_sized(ui, "Sign in again", pal::VIOLET, vec2(110.0, 26.0)).clicked() {
                        app.saved.station = a.server.clone();
                        app.saved.username = a.user.username.clone();
                    }
                });
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new("SIGN IN TO A STATION").font(theme::display(15.0)).color(pal::VIOLET));
        let mut go = false;
        // On a phone, one field under another.
        let narrow = ui.available_width() < 600.0;
        let mut form = |ui: &mut Ui| {
            let full = ui.available_width() - 24.0;
            let w = |wide: f32| if narrow { full } else { wide };
            let own = app.client.settings().server.url.unwrap_or_default();
            let a = field(ui, &mut app.saved.station, if own.is_empty() { "support.example.com" } else { own.as_str() }, false, w(210.0));
            let b = field(ui, &mut app.saved.username, "username", false, w(140.0));
            let c = field(ui, &mut app.saved.password, "password", true, w(160.0));
            let mut go = (a.lost_focus() || b.lost_focus() || c.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let busy = app.saved.job.is_some();
            let label = if busy { "Signing in…" } else { "Sign in" };
            go |= ui.add_enabled_ui(!busy, |ui| theme::pill(ui, label, pal::VIOLET)).inner.clicked();
            go
        };
        if narrow {
            go = ui.vertical(form).inner;
        } else {
            ui.horizontal_wrapped(|ui| go = form(ui));
        }
        if go {
            // An empty address means this app's own station.
            let station = Some(app.saved.station.trim().to_string())
                .filter(|s| !s.is_empty())
                .or(app.client.settings().server.url)
                .unwrap_or_default();
            let (user, pw) = (app.saved.username.trim().to_string(), std::mem::take(&mut app.saved.password));
            if station.is_empty() || user.is_empty() || pw.is_empty() {
                app.toast("Enter the station address, your username and password", pal::RED);
            } else {
                let client = app.client.clone();
                app.saved.last_presence = None;
                spawn_job(app, &ctx, "Sign-in", async move {
                    client.sign_in(&station, &user, &pw).await?;
                    let a = client.settings().accounts.into_iter().find(|a| a.is_station(&station) && a.user.username == user.to_lowercase());
                    Ok(match a {
                        Some(a) => format!("Signed in to {} as {}", a.label(), a.user.name),
                        None => "Signed in".into(),
                    })
                });
            }
        }
    });
    if let Some(r) = sign_out {
        let client = app.client.clone();
        spawn_job(app, &ctx, "Sign-out", async move {
            client.sign_out(&r).await;
            Ok(format!("Signed out of {}", r.server))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_device_uses_matching_station_and_preserves_saved_details() {
        let mut account = StationAccount { server: "relay.example".into(), ..Default::default() };
        let id = DeviceId(482913077);
        assert!(own_device_entry(&account, "https://other.example", Some(id), "This PC").is_none());
        assert!(own_device_entry(&account, "https://relay.example", None, "This PC").is_none());
        let mut device = own_device_entry(&account, "https://relay.example", Some(id), "This PC").unwrap();
        assert_eq!(device.name, "This PC");
        assert!(device.secret.is_none());
        device.group = "Servers".into();
        device.notes = "Keep this note\nAnd this one".into();
        account.devices.push(device.clone());
        assert_eq!(own_device_entry(&account, "https://relay.example", Some(id), "A new name"), Some(device));
    }
}
