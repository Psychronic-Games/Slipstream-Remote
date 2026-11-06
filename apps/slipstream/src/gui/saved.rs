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
    search: String,
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
    group_editor: Option<GroupEditor>,
    prompt: Option<Prompt>,
}

struct GroupEditor {
    account: AccountRef,
    name: String,
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
    /// A shared device seen by a technician who isn't a station admin:
    /// only their own saved password for it can change.
    locked: bool,
    /// Remote management, as saved in this technician's own list.
    manage: ManageForm,
}

/// The editor's Remote management fields.
#[derive(Default)]
struct ManageForm {
    open: bool,
    gateway: Option<DeviceId>,
    wake_mac: String,
    kind: Option<slipstream_core::account::ControllerKind>,
    address: String,
    username: String,
    password: String,
    /// The saved (sealed) controller password, kept unless replaced.
    sealed: Option<slipstream_core::vault::SealedSecret>,
    forget_password: bool,
}

impl ManageForm {
    fn from(m: Option<&slipstream_core::account::Management>) -> Self {
        let Some(m) = m else { return Self::default() };
        let c = m.controller.as_ref();
        Self {
            open: !m.is_empty(),
            gateway: m.gateway,
            wake_mac: m.wake_mac.clone(),
            kind: c.map(|c| c.kind),
            address: c.map(|c| c.address.clone()).unwrap_or_default(),
            username: c.map(|c| c.username.clone()).unwrap_or_default(),
            password: String::new(),
            sealed: c.and_then(|c| c.secret.clone()),
            forget_password: false,
        }
    }

    /// What to save; an empty one clears it. `old` is the ID the saved
    /// controller password was sealed for.
    fn build(&self, client: &slipstream_session::Client, r: &AccountRef, id: DeviceId, old: Option<DeviceId>) -> anyhow::Result<slipstream_core::account::Management> {
        use slipstream_core::account::{Controller, Management, normalize_mac};
        let wake_mac = match self.wake_mac.trim() {
            "" => String::new(),
            mac => normalize_mac(mac).ok_or_else(|| anyhow::anyhow!("Enter the Wake-on-LAN address like 00:11:22:33:44:55"))?,
        };
        let controller = match self.kind {
            None => None,
            Some(kind) => {
                let mut controller = Controller { kind, address: self.address.trim().to_string(), username: self.username.trim().to_string(), secret: None };
                if controller.target().is_none() {
                    anyhow::bail!("Enter the management controller's address, like 10.0.0.20 or https://10.0.0.20:8443");
                }
                controller.secret = if !self.password.is_empty() {
                    Some(client.seal_secret(r, id, &self.password)?)
                } else if self.forget_password {
                    None
                } else {
                    match (&self.sealed, old) {
                        // Sealed for this device's ID: re-seal if the ID changed.
                        (Some(sealed), Some(old)) if old != id => Some(client.seal_secret(r, id, &client.open_secret(r, old, sealed)?)?),
                        (sealed, _) => sealed.clone(),
                    }
                };
                Some(controller)
            }
        };
        Ok(Management { gateway: self.gateway.filter(|g| *g != id), wake_mac, controller })
    }
}

/// Asking for a password before connecting or chatting.
struct Prompt {
    destination: Option<egui::ViewportId>,
    account: AccountRef,
    id: DeviceId,
    name: String,
    password: String,
    remember: bool,
    purpose: Purpose,
    /// Why it's asking (a refused password).
    note: Option<String>,
}

/// What a password prompt leads to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    Connect,
    Chat,
}

pub(super) const REFUSED: &str = "That password was refused. Enter the current one; Remember replaces a saved password.";

/// Widest a device row grows: Connect, Chat, Meeting and Edit stay beside
/// the name in a wide window instead of drifting to the far edge.
const ROW_MAX_WIDTH: f32 = 860.0;

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
    if app.client.with_settings(|s| s.accounts.is_empty()) || app.saved.busy.load(Ordering::Relaxed) {
        return;
    }
    let due = |t: Option<Instant>, every| t.is_none_or(|t: Instant| t.elapsed() >= every);
    let sync = due(app.saved.last_sync, SYNC_EVERY);
    let presence = (app.page == Page::Home || app.quick_connect.open) && due(app.saved.last_presence, PRESENCE_EVERY);
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

pub(super) fn refresh(app: &mut App) {
    app.saved.last_sync = None;
    app.saved.last_presence = None;
}

pub(super) fn presence(app: &App, account: &AccountRef, id: DeviceId) -> Option<Presence> {
    app.saved.presence.lock().unwrap().get(&(account.clone(), id)).cloned()
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
        ui.horizontal(|ui| {
            if !app.saved.search.is_empty() && theme::pill(ui, "Clear", pal::BLUE).clicked() {
                app.saved.search.clear();
            }
            ui.add(theme::text_input(&mut app.saved.search)
                .id_salt("saved-device-search")
                .hint_text("Find a device by name, ID, group or station…")
                .desired_width(ui.available_width()));
        });
        let filter = app.saved.search.to_lowercase();
        let total = accounts.iter().map(|a| a.devices.len() + a.shared.len()).sum::<usize>();
        let found = accounts.iter().map(|a| {
            a.devices.iter().chain(&a.shared)
                .filter(|d| matches(&filter, &d.name, d.id, &d.group, &a.label())).count()
        }).sum::<usize>();
        if !filter.trim().is_empty() {
            ui.label(RichText::new(format!("{found} of {total} devices")).small().color(pal::DIM));
            if found == 0 {
                ui.label(RichText::new("No matching devices").color(pal::DIM));
            }
        }
        for (i, a) in accounts.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.0);
            }
            let r = AccountRef::of(a);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(a.label().to_uppercase()).font(theme::display(18.0)).color(pal::LAVENDER));
                ui.label(RichText::new(format!("· {}", a.user.name)).color(pal::DIM));
                if a.signed_out {
                    ui.label(RichText::new("SIGNED OUT: sign in again under Config → Stations").font(theme::display(14.0)).color(pal::RED));
                }
                    if !a.signed_out && theme::pill(ui, "+ Group", pal::BLUE).clicked() {
                        app.saved.group_editor = Some(GroupEditor {account:r.clone(),name:String::new()});
                    }
                    if !a.signed_out && theme::pill(ui, "+ Device", pal::VIOLET).clicked() {
                        open_editor(app, &r, None, false);
                    }
                    if !a.signed_out {
                        let own = own_device(app, a);
                        let button = ui.add_enabled_ui(own.is_some(), |ui| theme::pill(ui, "This device", pal::BLUE)).inner
                            .on_hover_text("Fill in this computer's ID and name; choose a group before saving")
                            .on_disabled_hover_text("This device needs an ID on this station. Its ID is shown in This device above.");
                        if button.clicked() && let Some(device) = own {
                            let existing = a.devices.iter().any(|d| d.id == device.id);
                            open_editor(app, &r, Some(&device), false);
                            if !existing && let Some(editor) = &mut app.saved.editor { editor.original = None; }
                        }
                    }
            });
            if a.devices.is_empty() && a.shared.is_empty() && a.groups.is_empty() && a.shared_groups.is_empty() {
                ui.label(RichText::new("Nothing saved yet: add a device, or use the star on a recent link.").color(pal::DIM));
            }
            grouped_devices(app, ui, &ctx, &r, a, &a.devices, false, t);
            if !a.shared.is_empty() || !a.shared_groups.is_empty() {
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
    if slipstream_core::config::is_public_station(&account.server) {
        let status = if app.via_service { app.service_public_status.as_ref() } else { Some(&app.public_status) };
        let id = match status {
            Some(slipstream_session::DirStatus::Online { id }) => Some(*id),
            _ if !app.via_service => settings.host.public_device_id,
            _ => None,
        };
        return own_device_entry(account, slipstream_core::config::PUBLIC_STATION_URL, id, &settings.device_name);
    }
    if app.via_service && !app.service_station_matches() { return None; }
    let id = match app.dir_status {
        slipstream_session::DirStatus::Online { id } => Some(id),
        _ => settings.host.last_device_id,
    };
    #[cfg(windows)]
    let id = if app.via_service { crate::service::hosted_id() } else { id };
    own_device_entry(account, app.client.station_settings().url.as_deref()?, id, &settings.device_name)
}

fn own_device_entry(account: &StationAccount, server: &str, id: Option<DeviceId>, name: &str) -> Option<SavedDevice> {
    if !account.is_station(server) { return None; }
    let id = id?;
    let existing = account.devices.iter().find(|d| d.id == id);
    Some(existing.cloned().unwrap_or_else(|| SavedDevice {
        id, name: name.to_string(), notes: String::new(), group: String::new(), secret: None, added_by: None, management: None,
    }))
}

#[allow(clippy::too_many_arguments)]
fn grouped_devices(app: &mut App, ui: &mut Ui, ctx: &egui::Context, r: &AccountRef,
    account: &StationAccount, devices: &[SavedDevice], shared: bool, t: f64) {
    let filter = app.saved.search.to_lowercase();
    let empty = if shared { account.shared_groups.as_slice() } else { account.groups.as_slice() };
    for (group, devices) in grouped(devices, empty, &filter, &account.label()) {
        let title = format!("{}  ·  {}", if group.is_empty() { "UNGROUPED".into() } else { group.to_uppercase() }, devices.len());
        egui::CollapsingHeader::new(RichText::new(title).font(theme::display_bold(17.0)).color(if shared { pal::GOLD } else { pal::LAVENDER }))
            .id_salt(("saved-group", &account.server, &account.user.username, shared, &group))
            .default_open(true).open((!filter.trim().is_empty()).then_some(true)).show(ui, |ui| {
                if !account.signed_out && (!shared || account.user.admin) {
                    ui.horizontal(|ui| {
                        if ui.small_button("+ Add device").clicked() {
                            open_editor(app,r,None,shared);
                            if let Some(editor) = &mut app.saved.editor {editor.group=group.to_owned();}
                        }
                        if devices.is_empty() && !shared && ui.small_button("Remove empty group").clicked() {
                            let (client,r,name)=(app.client.clone(),r.clone(),group.to_owned());
                            spawn_job(app,ctx,"Remove group",async move {
                                client.delete_group(&r,&name).await?; Ok("Empty group removed".into())
                            });
                        }
                    });
                }
                if devices.is_empty() {ui.label(RichText::new("No devices yet").color(pal::DIM));}
                for device in devices { device_row(app, ui, ctx, r, account, device, shared, t); }
            });
    }
}

/// Shared by Saved devices and the New link picker. Each word may match a
/// different field; station-qualified searches retain the original account.
pub(super) fn matches(search: &str, name: &str, id: DeviceId, group: &str, station: &str) -> bool {
    let text = format!("{name} {id} {} {group} {station}", id.0).to_lowercase();
    search.split_whitespace().all(|word| text.contains(word))
}

pub(super) fn grouped<'a>(
    devices: &'a [SavedDevice],
    empty: &'a [String],
    search: &str,
    station: &str,
) -> BTreeMap<&'a str, Vec<&'a SavedDevice>> {
    let mut groups = BTreeMap::<&str, Vec<_>>::new();
    for device in devices {
        if matches(search, &device.name, device.id, &device.group, station) {
            groups.entry(&device.group).or_default().push(device);
        }
    }
    for group in empty {
        let text = format!("{group} {station}").to_lowercase();
        if search.split_whitespace().all(|word| text.contains(word)) {
            groups.entry(group).or_default();
        }
    }
    for devices in groups.values_mut() {
        devices.sort_by_key(|d| d.name.to_lowercase());
    }
    groups
}

#[allow(clippy::too_many_arguments)]
fn device_row(app: &mut App, ui: &mut Ui, ctx: &egui::Context, r: &AccountRef, a: &StationAccount, d: &SavedDevice, shared: bool, t: f64) {
    let presence = app.saved.presence.lock().unwrap().get(&(r.clone(), d.id)).cloned();
    let (color,tip)=match presence {
        Some(Presence::Online)=>(pal::GREEN,"online"),
        Some(Presence::Offline)=>(pal::DIM,"offline"),
        _=>(pal::GOLD,"checking…"),
    };
    let name=if d.name.is_empty() {d.id.to_string()} else {d.name.clone()};
    let chat_live = app.chats.live(r, d.id);
    let unread = app.chats.unread(r, d.id);
    let saved_password = a.devices.iter().any(|own| own.id == d.id && own.secret.is_some());
    let info = |ui: &mut Ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(20.0, 21.0), Sense::hover());
            theme::lamp(ui.painter(), rect.center(), color, t);
            ui.add(egui::Label::new(RichText::new(name.to_uppercase()).font(theme::display(18.0)).color(pal::TAN)).truncate())
                .on_hover_text(format!("{name} · {tip}"));
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(d.id.to_string()).font(theme::mono(13.0)).color(pal::BLUE));
            if saved_password {ui.label(RichText::new("PASSWORD SAVED").font(theme::display(12.0)).color(pal::GOLD));}
            if !d.notes.is_empty() {ui.add(egui::Label::new(RichText::new(&d.notes).small().color(pal::DIM)).truncate()).on_hover_text(&d.notes);}
        });
    };
    let mut action = None;
    let mut manage = None;
    let console = super::manage::console_open(app, r, d.id);
    // The actions, laid out right to left (wide) or left to right (narrow);
    // `action` records which was pressed, `manage` anchors its menu.
    let mut actions = |ui: &mut Ui, right_to_left: bool| {
        let mut buttons: Vec<(&str, egui::Color32, bool, RowAction)> = vec![
            ("Connect", pal::ORANGE, (app.attempt.is_none() || chat_live.is_some()) && !a.signed_out, RowAction::Connect),
            (if unread { "Chat ◆" } else { "Chat" }, pal::BLUE, !a.signed_out, RowAction::Chat),
            ("Meeting", pal::GREEN, !a.signed_out, RowAction::Meeting),
            (if console { "Manage ◆" } else { "Manage" }, pal::PEACH, !a.signed_out, RowAction::Manage),
            // Everyone may edit their own saved password for a shared
            // device; only admins change its details.
            ("Edit", pal::LAVENDER, !a.signed_out, RowAction::Edit),
        ];
        if right_to_left {
            buttons.reverse();
        }
        for (label, color, enabled, which) in buttons {
            let response = ui.add_enabled_ui(enabled, |ui| theme::pill_sized(ui, label, color, vec2(0.0, 28.0))).inner;
            let response = match which {
                RowAction::Chat => response.on_hover_text("Message the person at this computer without showing its screen"),
                RowAction::Manage => response.on_hover_text("Restart, wake, and its management controller (BIOS-level console and power)"),
                _ => response,
            };
            if which == RowAction::Manage {
                manage = Some(response);
            } else if response.clicked() {
                action = Some(which);
            }
        }
    };
    let width = ui.available_width().min(ROW_MAX_WIDTH);
    if width < 470.0 {
        // Phones and narrow windows: the actions go under the name.
        ui.vertical(|ui| {
            ui.set_width(width);
            info(ui);
            ui.horizontal_wrapped(|ui| actions(ui, false));
        });
        ui.add_space(4.0);
    } else {
        // Reserve the actions first; long names and notes truncate in the
        // space left rather than pushing the actions out of the panel.
        ui.allocate_ui_with_layout(vec2(width, 44.0), Layout::right_to_left(Align::Center), |ui| {
            actions(ui, true);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 44.0), Layout::top_down(Align::Min), info);
        });
    }
    if let Some(response) = manage {
        super::manage::menu_for(app, &response, r, d);
    }
    match action {
        // An open chat becomes the session, keeping its messages.
        Some(RowAction::Connect) => match chat_live {
            Some(key) => super::chats::promote(app, ctx, key),
            None => connect(app, ctx, r, d, shared),
        },
        Some(RowAction::Chat) => super::chats::open(app, ctx, r, d.id, &name),
        Some(RowAction::Meeting) => {
            let mut server = app.client.settings().server;
            server.url = Some(a.server.clone());
            server.key = a.key.clone();
            app.meetings.call_target(d.id, server);
            app.page = Page::Meetings;
        }
        Some(RowAction::Edit) => open_editor(app, r, Some(d), shared),
        Some(RowAction::Manage) | None => {}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowAction {
    Connect,
    Chat,
    Meeting,
    Manage,
    Edit,
}

pub(super) fn connect(app: &mut App, ctx: &egui::Context, r: &AccountRef, d: &SavedDevice, _shared: bool) {
    match app.client.saved_password(r, d.id) {
        Ok(Some(pw)) => {
            app.start_connect_to(ctx, d.id, pw, Some(r.clone()));
            return;
        }
        Ok(None) => {}
        Err(e) => app.toast(format!("{e:#}"), pal::RED),
    }
    let name = if d.name.is_empty() { d.id.to_string() } else { d.name.clone() };
    ask_password(app, r, d.id, &name, Purpose::Connect, None);
}

/// Asks for a device's password, then connects or opens a chat. Remember
/// starts ticked when a saved password exists, so typing replaces it.
pub(super) fn ask_password(app: &mut App, r: &AccountRef, id: DeviceId, name: &str, purpose: Purpose, note: Option<String>) {
    // An automatic re-prompt never replaces a prompt someone is typing in.
    if note.is_some() && app.saved.prompt.is_some() {
        return;
    }
    let saved = accounts(app).iter().find(|a| AccountRef::of(a) == *r)
        .is_some_and(|a| a.devices.iter().any(|d| d.id == id && d.secret.is_some()));
    app.saved.prompt = Some(Prompt {
        destination: if purpose == Purpose::Connect { app.connect_destination.take() } else { None },
        account: r.clone(),
        id,
        name: name.to_string(),
        password: String::new(),
        remember: saved,
        purpose,
        note,
    });
}

pub(super) fn prompt_open(app: &App) -> bool {
    app.saved.prompt.is_some()
}

/// Where the open password prompt's link opens (a session window's +).
pub(super) fn set_prompt_destination(app: &mut App, destination: Option<egui::ViewportId>) {
    if let Some(p) = &mut app.saved.prompt {
        p.destination = destination;
    }
}

/// A saved device's name (its ID when it has none or isn't listed).
pub(super) fn device_name(app: &App, r: &AccountRef, id: DeviceId) -> String {
    accounts(app).iter().find(|a| AccountRef::of(a) == *r)
        .and_then(|a| a.devices.iter().chain(&a.shared).find(|d| d.id == id && !d.name.is_empty()))
        .map(|d| d.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// From a recent link: save it in a signed-in station's list.
pub fn save_recent(app: &mut App, recent: &slipstream_core::config::RecentConnection) {
    let (id, name) = (recent.id, recent.name.as_str());
    let station = recent.station.clone().or(app.client.station_settings().url).unwrap_or_default();
    let Some(a) = accounts(app).into_iter().find(|a| !a.signed_out && a.is_station(&station)) else {
        app.toast("Sign in to this device’s station first (Config → Stations) to save it", pal::VIOLET);
        return;
    };
    let draft = SavedDevice { id, name: name.to_string(), notes: String::new(), group: String::new(), secret: None, added_by: None, management: None };
    open_editor(app, &AccountRef::of(&a), Some(&draft), false);
    // It's a new entry, not an edit of an existing one.
    if let Some(e) = &mut app.saved.editor
        && !a.devices.iter().any(|d| d.id == id)
    {
        e.original = None;
    }
}

fn open_editor(app: &mut App, r: &AccountRef, d: Option<&SavedDevice>, shared: bool) {
    let account = accounts(app).into_iter().find(|a| AccountRef::of(a) == *r);
    // A shared device's password lives in this technician's own list.
    let had_password = d.is_some_and(|d| d.secret.is_some())
        || d.is_some_and(|d| account.as_ref().is_some_and(|a| a.devices.iter().any(|own| own.id == d.id && own.secret.is_some())));
    let locked = shared && d.is_some() && !account.as_ref().is_some_and(|a| a.user.admin);
    // Remote management is the technician's own, also for shared devices.
    let manage = ManageForm::from(d.and_then(|d| account.as_ref()?.devices.iter().find(|own| own.id == d.id)?.management.as_ref()));
    app.saved.editor = Some(Editor {
        account: r.clone(),
        id: d.map(|d| d.id.to_string()).unwrap_or_default(),
        name: d.map(|d| d.name.clone()).unwrap_or_default(),
        notes: d.map(|d| d.notes.clone()).unwrap_or_default(),
        group: d.map(|d| d.group.clone()).unwrap_or_default(),
        shared,
        original: d.map(|d| (d.id, shared)),
        password: String::new(),
        had_password,
        forget_password: false,
        locked,
        manage,
    });
}

/// Opens the editor on a listed device (the Manage menu's set-up item).
pub(super) fn edit_device(app: &mut App, r: &AccountRef, d: &SavedDevice) {
    let shared = !accounts(app).iter().find(|a| AccountRef::of(a) == *r).is_some_and(|a| a.devices.iter().any(|own| own.id == d.id));
    open_editor(app, r, Some(d), shared);
    if let Some(e) = &mut app.saved.editor {
        e.manage.open = true;
    }
}

/// The editor's Remote management section: a gateway on the device's
/// network, its Wake-on-LAN address and its management controller.
fn manage_section(ui: &mut Ui, e: &mut Editor, account: &StationAccount) {
    use slipstream_core::account::ControllerKind;
    ui.add_space(8.0);
    let id = "saved-editor-manage";
    egui::CollapsingHeader::new(RichText::new("REMOTE MANAGEMENT").font(theme::display(15.0)).color(pal::PEACH))
        .id_salt(id).open(e.manage.open.then_some(true)).show(ui, |ui| {
        e.manage.open = false;
        ui.label(RichText::new("Reach this computer when its own system can't help: wake it, and reach its management \
            controller (a server's BMC, Intel vPro) for power and a console that works in the BIOS. A gateway is another \
            of your Slipstream computers on the same network that stays on; it needs its password saved, Port forwarding \
            allowed and, for waking, Wake-on-LAN allowed.").small().color(pal::DIM));
        ui.add_space(4.0);
        let own: Option<DeviceId> = e.id.parse().ok();
        let mut candidates: Vec<(DeviceId, String)> = account.devices.iter().chain(&account.shared)
            .filter(|d| Some(d.id) != own)
            .map(|d| (d.id, if d.name.is_empty() { d.id.to_string() } else { d.name.clone() }))
            .collect();
        candidates.sort_by_key(|(_, name)| name.to_lowercase());
        candidates.dedup_by_key(|(id, _)| *id);
        let label = |g: Option<DeviceId>| match g {
            None => "None".to_string(),
            Some(g) => candidates.iter().find(|(id, _)| *id == g).map(|(id, name)| format!("{name} · {id}")).unwrap_or_else(|| g.to_string()),
        };
        egui::Grid::new("saved-editor-manage-grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label(RichText::new("GATEWAY").font(theme::display(15.0)).color(pal::LAVENDER));
            egui::ComboBox::from_id_salt("saved-editor-gateway").width(280.0).selected_text(label(e.manage.gateway)).show_ui(ui, |ui| {
                theme::console_menu_style(ui.style_mut());
                ui.selectable_value(&mut e.manage.gateway, None, "None");
                for (gid, name) in &candidates {
                    ui.selectable_value(&mut e.manage.gateway, Some(*gid), format!("{name} · {gid}"));
                }
            });
            ui.end_row();
            ui.label(RichText::new("WAKE MAC").font(theme::display(15.0)).color(pal::LAVENDER));
            field(ui, &mut e.manage.wake_mac, "00:11:22:33:44:55 (its network adapter)", false, 280.0);
            ui.end_row();
            ui.label(RichText::new("CONTROLLER").font(theme::display(15.0)).color(pal::LAVENDER));
            egui::ComboBox::from_id_salt("saved-editor-controller").width(280.0)
                .selected_text(e.manage.kind.map(ControllerKind::label).unwrap_or("None")).show_ui(ui, |ui| {
                    theme::console_menu_style(ui.style_mut());
                    ui.selectable_value(&mut e.manage.kind, None, "None");
                    for kind in ControllerKind::ALL {
                        ui.selectable_value(&mut e.manage.kind, Some(kind), kind.label());
                    }
                });
            ui.end_row();
            if e.manage.kind.is_some() {
                ui.label(RichText::new("ADDRESS").font(theme::display(15.0)).color(pal::LAVENDER));
                field(ui, &mut e.manage.address, "10.0.0.20, or https://10.0.0.20:8443", false, 280.0);
                ui.end_row();
                ui.label(RichText::new("USERNAME").font(theme::display(15.0)).color(pal::LAVENDER));
                field(ui, &mut e.manage.username, "ADMIN", false, 280.0);
                ui.end_row();
                ui.label(RichText::new("PASSWORD").font(theme::display(15.0)).color(pal::LAVENDER));
                ui.vertical(|ui| {
                    if e.manage.sealed.is_some() && !e.manage.forget_password {
                        ui.label(RichText::new("🔑 Saved. Type a new one to replace it.").color(pal::GOLD));
                    }
                    if !e.manage.forget_password {
                        field(ui, &mut e.manage.password, "the controller's password", true, 280.0);
                    }
                    if e.manage.sealed.is_some() {
                        ui.checkbox(&mut e.manage.forget_password, "Forget the saved controller password");
                    }
                    ui.label(RichText::new("Addresses are as seen from the gateway. Encrypted like device passwords.").small().color(pal::DIM));
                });
                ui.end_row();
            }
        });
    });
}

fn field(ui: &mut Ui, value: &mut String, hint: &str, password: bool, width: f32) -> egui::Response {
    ui.add(theme::text_input(value).hint_text(hint).password(password)
        .desired_width(width.min(ui.available_width())))
}

/// An editable group name with its chooser integrated into the same field.
fn group_field(ui: &mut Ui, value: &mut String, groups: BTreeSet<&str>) {
    let input = ui.add(theme::text_input(value).hint_text("Ungrouped or new group")
        .char_limit(64).desired_width(280.0)
        .frame(theme::input_frame().inner_margin(egui::Margin {left:9,right:36,top:4,bottom:4})));
    let rect = input.rect;
    let cap=egui::Rect::from_min_max(egui::pos2(rect.right()-34.0,rect.top()+2.0),rect.max-vec2(2.0,2.0));
    let response=ui.interact(cap,ui.make_persistent_id("group-choices"),Sense::click()).on_hover_text("Choose a group, or type a new name");
    let color=if response.hovered() {pal::TAN} else {pal::ORANGE};
    ui.painter().rect_filled(cap,egui::CornerRadius {ne:4,se:4,..Default::default()},color);
    let center=cap.center();
    ui.painter().line_segment([center+vec2(-5.0,-2.0),center+vec2(0.0,3.0)],egui::Stroke::new(2.0,pal::BG));
    ui.painter().line_segment([center+vec2(0.0,3.0),center+vec2(5.0,-2.0)],egui::Stroke::new(2.0,pal::BG));
    response.widget_info(||egui::WidgetInfo::labeled(egui::Role::Button,true,"Choose group"));
    theme::console_menu(&response).anchor(rect).width(rect.width()).show(|ui| {
        ui.set_min_width((rect.width()-12.0).max(120.0));
        if ui.add_sized(vec2(ui.available_width(),28.0),egui::Button::selectable(value.is_empty(),"Ungrouped")).clicked() {value.clear();ui.close();}
        for group in groups {
            if ui.add_sized(vec2(ui.available_width(),28.0),egui::Button::selectable(value==group,group).truncate()).on_hover_text(group).clicked() {*value=group.to_string();ui.close();}
        }
    });
}

/// The add/edit and password dialogs (every frame).
pub fn windows(app: &mut App, ctx: &egui::Context) {
    group_window(app,ctx);
    editor_window(app, ctx);
    prompt_window(app, ctx);
}

fn group_window(app: &mut App, ctx: &egui::Context) {
    let Some(mut editor)=app.saved.group_editor.take() else {return};
    let mut open=true;
    let mut save=false;
    theme::window("new-device-group",pal::BLUE).collapsible(false).resizable(false)
        .default_width(330.0).anchor(egui::Align2::CENTER_CENTER,vec2(0.0,0.0)).show(ctx,|ui| {
            let chrome=theme::window_header_with_close(ui,"New group",pal::BLUE,pal::PEACH);
            if chrome.close {open=false;}
            ui.label(RichText::new("Saved on your technician account, available on your other computers.").small().color(pal::DIM));
            let input=field(ui,&mut editor.name,"Group name",false,280.0);
            let valid=!editor.name.trim().is_empty() && app.saved.job.is_none();
            save=ui.add_enabled_ui(valid,|ui|theme::pill(ui,"Create group",pal::GREEN)).inner.clicked()
                || (valid && input.lost_focus() && ui.input(|i|i.key_pressed(egui::Key::Enter)));
            chrome.footer(ui);
        });
    if save {
        let client=app.client.clone();
        spawn_job(app,ctx,"Create group",async move {
            client.save_group(&editor.account,&editor.name).await?; Ok("Group created".into())
        });
    } else if open {app.saved.group_editor=Some(editor);}
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
                        theme::console_menu_style(ui.style_mut());
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
                let editable = !e.locked;
                ui.label(RichText::new("ID").font(theme::display(15.0)).color(pal::LAVENDER));
                ui.add_enabled_ui(editable, |ui| field(ui, &mut e.id, "482 913 077", false, 200.0));
                ui.end_row();
                ui.label(RichText::new("NAME").font(theme::display(15.0)).color(pal::LAVENDER));
                ui.add_enabled_ui(editable, |ui| field(ui, &mut e.name, "Front desk PC", false, 280.0));
                ui.end_row();
                ui.label(RichText::new("GROUP").font(theme::display(15.0)).color(pal::LAVENDER));
                let groups: BTreeSet<_> = account.devices.iter().chain(&account.shared)
                    .map(|d|d.group.as_str()).chain(account.groups.iter().chain(&account.shared_groups).map(String::as_str))
                    .filter(|g|!g.is_empty()).collect();
                ui.add_enabled_ui(editable, |ui| group_field(ui,&mut e.group,groups));
                ui.end_row();
                ui.label(RichText::new("NOTES").font(theme::display(15.0)).color(pal::LAVENDER));
                // Keep the text editor out of the grid layout: a frame that
                // inherits its cached row height cannot shrink after deleting lines.
                ui.vertical(|ui| {
                    ui.add_enabled(editable, theme::text_area(&mut e.notes).hint_text("Optional notes…")
                        .desired_rows(1).char_limit(500).desired_width(280.0));
                });
                ui.end_row();
                ui.label(RichText::new(if e.shared { "YOUR PASSWORD" } else { "PASSWORD" }).font(theme::display(15.0)).color(pal::LAVENDER));
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
                    let note = if e.shared {
                        "Saved in your own list, encrypted: only your sign-in can open it. Other technicians keep their own."
                    } else {
                        "Kept on the station encrypted: only your sign-in can open it."
                    };
                    ui.label(RichText::new(note).small().color(pal::DIM));
                });
                ui.end_row();
                if e.locked {
                    ui.label("");
                    ui.label(RichText::new("A station admin manages this shared device's name, group and notes.").small().color(pal::DIM));
                    ui.end_row();
                }
                if account.user.admin {
                    ui.label("");
                    ui.vertical(|ui| {
                        ui.checkbox(&mut e.shared, "Save in station list (visibility assigned in station admin)");
                        ui.label(
                            RichText::new("Shown in the lists of technicians it's assigned to. Each technician's \
                                           password for it stays in their own list.").small().color(pal::DIM),
                        );
                    });
                    ui.end_row();
                }
            });
            manage_section(ui, &mut e, &account);
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = theme::pill(ui, "Save", pal::GREEN).clicked();
                if e.original.is_some() && !e.locked {
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
        let management = match e.manage.build(&app.client, &e.account, id, e.original.map(|(old, _)| old)) {
            Ok(m) => m,
            Err(err) => {
                app.toast(format!("{err:#}"), pal::RED);
                app.saved.editor = Some(e);
                return;
            }
        };
        let saved_management = accounts(app).iter().find(|a| AccountRef::of(a) == e.account)
            .and_then(|a| a.devices.iter().find(|d| d.id == id)).and_then(|d| d.management.clone()).unwrap_or_default();
        let management_changed = management != saved_management;
        let mut device = SavedDevice { id, name: e.name.trim().to_string(), notes: e.notes.trim().to_string(), group: e.group.trim().to_string(), secret: None, added_by: None, management: None };
        if !e.shared {
            device.management = Some(management.clone());
        }
        let password = if e.forget_password {
            Some(String::new())
        } else if !e.password.is_empty() {
            Some(e.password.clone())
        } else {
            None
        };
        let (client, r, shared, original, locked) = (app.client.clone(), e.account.clone(), e.shared, e.original, e.locked);
        let name = if device.name.is_empty() { id.to_string() } else { device.name.clone() };
        spawn_job(app, ctx, "Saving", async move {
            if !locked {
                // Moved to another ID or list: the old entry goes.
                if let Some((old_id, old_shared)) = original
                    && (old_id != id || old_shared != shared)
                {
                    client.delete_device(&r, old_id, old_shared).await?;
                }
                client.save_device(&r, device, if shared { None } else { password.clone() }, shared).await?;
            }
            // A shared device's password and remote management go in this
            // technician's own list.
            if shared && (password.is_some() || management_changed) {
                let a = client.settings().accounts.into_iter().find(|a| AccountRef::of(a) == r).ok_or_else(|| anyhow::anyhow!("signed out"))?;
                let mut entry = password_entry(&a, id).ok_or_else(|| anyhow::anyhow!("device is no longer in the station list"))?;
                entry.secret = None;
                entry.management = Some(management);
                client.save_device(&r, entry, password, false).await?;
            }
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
    let title = match p.purpose {
        Purpose::Connect => format!("Connect to {}", p.name),
        Purpose::Chat => format!("Chat with {}", p.name),
    };
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
            if let Some(note) = &p.note {
                ui.label(RichText::new(note).color(pal::GOLD));
            }
            ui.label(RichText::new(format!("{} · its one-time or unattended password", p.id)).color(pal::DIM));
            let r = field(ui, &mut p.password, "password", true, 300.0);
            r.request_focus();
            go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.checkbox(&mut p.remember, "Remember unattended password in my list");
            if p.purpose == Purpose::Chat {
                ui.label(RichText::new("Opens a chat only: their screen isn't shared until you press Connect.").small().color(pal::DIM));
            }
            ui.add_space(6.0);
            go |= theme::pill(ui, if p.purpose == Purpose::Chat { "Open chat" } else { "Engage" }, pal::ORANGE).clicked();
            chrome.footer(ui);
        });
    if go && !p.password.is_empty() {
        if p.remember {
            // Save it as well, sealed, alongside connecting.
            let (client, r, id, pw) = (app.client.clone(), p.account.clone(), p.id, p.password.clone());
            spawn_job(app, ctx, "Saving the password", async move {
                let a = client.settings().accounts.into_iter().find(|a| AccountRef::of(a) == r).ok_or_else(|| anyhow::anyhow!("signed out"))?;
                let mut device = password_entry(&a, id).ok_or_else(|| anyhow::anyhow!("device is no longer in the station list"))?;
                device.secret = None;
                client.save_device(&r, device, Some(pw), false).await?;
                Ok(String::new())
            });
        }
        match p.purpose {
            Purpose::Connect => {
                app.connect_destination=p.destination;
                app.start_connect_to(ctx, p.id, p.password, Some(p.account));
            }
            Purpose::Chat => super::chats::start(app, ctx, &p.account, p.id, &p.name, p.password),
        }
        return;
    }
    if open {
        app.saved.prompt = Some(p);
    }
}

/// A shared device's credential belongs in this technician's personal vault.
fn password_entry(account: &StationAccount, id: DeviceId) -> Option<SavedDevice> {
    let mut device = account.devices.iter().find(|d| d.id == id)
        .or_else(|| account.shared.iter().find(|d| d.id == id))?.clone();
    device.added_by = None;
    Some(device)
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
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(a.label().to_uppercase()).font(theme::display(17.0)).color(pal::LAVENDER));
                ui.label(RichText::new(&a.server).font(theme::mono(14.0)).color(pal::BLUE));
                let who = format!("{}{}", a.user.name, if a.user.admin { " · station admin" } else { "" });
                ui.label(RichText::new(who).color(pal::DIM));
                if a.signed_out {
                    ui.label(RichText::new("SIGNED OUT").font(theme::display(14.0)).color(pal::RED));
                }
                    if theme::pill_sized(ui, "Sign out", pal::LAVENDER, vec2(84.0, 26.0)).clicked() {
                        sign_out = Some(AccountRef::of(a));
                    }
                    if a.signed_out && theme::pill_sized(ui, "Sign in again", pal::VIOLET, vec2(110.0, 26.0)).clicked() {
                        app.saved.station = a.server.clone();
                        app.saved.username = a.user.username.clone();
                    }
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new("SIGN IN TO A STATION").font(theme::display(15.0)).color(pal::VIOLET));
        let busy=app.saved.job.is_some();
        let label=if busy {"Signing in…"} else {"Sign in"};
        let own=app.client.station_settings().url.unwrap_or_default();
        let hint=if own.is_empty() {"support.example.com"} else {own.as_str()};
        let mut go=false;
        let width=ui.available_width();
        let pressed_enter=ui.input(|i|i.key_pressed(egui::Key::Enter));
        let enter=|response: &egui::Response|response.lost_focus() && pressed_enter;
        if width<480.0 {
            let a=field(ui,&mut app.saved.station,hint,false,width-28.0);
            let b=field(ui,&mut app.saved.username,"Username",false,width-28.0);
            let c=field(ui,&mut app.saved.password,"Password",true,width-28.0);
            go=enter(&a) || enter(&b) || enter(&c);
            go|=ui.add_enabled_ui(!busy,|ui|theme::pill(ui,label,pal::VIOLET)).inner.clicked();
        } else {
            let a=field(ui,&mut app.saved.station,hint,false,width-28.0);
            go|=enter(&a);
            ui.horizontal(|ui| {ui.with_layout(Layout::right_to_left(Align::Center),|ui| {
                go|=ui.add_enabled_ui(!busy,|ui|theme::pill_sized(ui,label,pal::VIOLET,vec2(0.0,34.0))).inner.clicked();
                let field_width=(ui.available_width()-6.0)/2.0-28.0;
                ui.with_layout(Layout::left_to_right(Align::Center),|ui| {
                    let b=field(ui,&mut app.saved.username,"Username",false,field_width);
                    let c=field(ui,&mut app.saved.password,"Password",true,field_width);
                    go|=(b.lost_focus() || c.lost_focus()) && ui.input(|i|i.key_pressed(egui::Key::Enter));
                });
            });});
        }
        if go {
            // An empty address means this app's own station.
            let station = Some(app.saved.station.trim().to_string())
                .filter(|s| !s.is_empty())
                .or(app.client.station_settings().url)
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
    fn shared_password_entries_use_the_personal_vault_and_preserve_personal_details() {
        let id = DeviceId(482913077);
        let shared = SavedDevice { id, name: "OFFICE-PC".into(), group: String::new(), notes: String::new(), secret: None, added_by: Some("Automatic enrollment".into()), management: None };
        let mut account = StationAccount { shared: vec![shared.clone()], ..Default::default() };
        let copied = password_entry(&account, id).unwrap();
        assert!(copied.added_by.is_none());
        assert!(copied.secret.is_none());
        assert_eq!(account.shared[0], shared);
        let own = SavedDevice { name: "My preferred name".into(), group: "Office".into(), notes: "Keep these notes".into(), ..copied };
        account.devices.push(own.clone());
        assert_eq!(password_entry(&account, id), Some(own));
        assert!(password_entry(&account, DeviceId(482913078)).is_none());
    }

    #[test]
    fn search_combines_device_name_id_group_and_station_without_changing_membership() {
        let device = SavedDevice {
            id: DeviceId(482913077), name: "Front Desk".into(), group: "Office".into(),
            notes: String::new(), secret: None, added_by: None, management: None,
        };
        let devices = vec![device];
        let empty = vec!["Empty lab".to_owned()];
        let results = grouped(&devices, &empty, "desk office alpha 482913", "Alpha station");
        assert_eq!(results.len(), 1);
        assert_eq!(results["Office"][0].id, DeviceId(482913077));
        assert!(grouped(&devices, &empty, "desk beta", "Alpha station").is_empty());
        assert_eq!(grouped(&devices, &empty, "   ", "Alpha station").len(), 2);
        assert_eq!(devices[0].name, "Front Desk");
        assert_eq!(devices[0].group, "Office");
    }

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
