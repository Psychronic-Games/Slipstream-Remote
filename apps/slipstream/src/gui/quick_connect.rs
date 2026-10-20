//! New-link picker shared by the console and floating session tabs.
use eframe::egui::{self, RichText};
use slipstream_core::{account::SavedDevice, config::RecentConnection, directory::DeviceId};
use slipstream_session::{AccountRef, Presence};

use super::{App, Page, saved, theme::{self, pal}};

#[derive(Default)]
pub(super) struct QuickConnect {
    pub open: bool,
    pub focus: bool,
    search: String,
    id: String,
    password: String,
    via: Option<AccountRef>,
}

enum Choice {
    Recent(RecentConnection),
    Saved(AccountRef, SavedDevice, bool),
    Connect,
    Stations,
}

fn matches(search: &str, name: &str, id: DeviceId, group: &str, station: &str) -> bool {
    search.split_whitespace().all(|word| format!("{name} {id} {} {group} {station}", id.0).to_lowercase().contains(word))
}

pub(super) fn window(app: &mut App, ctx: &egui::Context) {
    if !app.quick_connect.open { return; }
    let mut picker = std::mem::take(&mut app.quick_connect);
    let settings = app.client.settings();
    let mut choice = None;
    let mut refresh = false;
    let result = egui::Modal::new(egui::Id::unique("new-link-picker")).show(ctx, |ui| {
        ui.set_width((ctx.content_rect().width() - 55.0).clamp(240.0, 550.0));
        if theme::window_header_with_close(ui, "New link", pal::GREEN, pal::PEACH).close { picker.open = false; }
        let search = ui.add(egui::TextEdit::singleline(&mut picker.search)
            .hint_text("Find a name, ID, group or station…").desired_width(f32::INFINITY));
        if std::mem::take(&mut picker.focus) { search.request_focus(); }
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("CONNECT BY ID").color(pal::LAVENDER));
            egui::ComboBox::from_id_salt("quick-link-station").selected_text(
                picker.via.as_ref().and_then(|r| settings.accounts.iter().find(|a| AccountRef::of(a) == *r)).map(|a| a.label())
                    .unwrap_or_else(|| "This station".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut picker.via, None, "This station");
                    for account in settings.accounts.iter().filter(|a| !a.signed_out) {
                        ui.selectable_value(&mut picker.via, Some(AccountRef::of(account)), format!("{} · {}", account.label(), account.user.name));
                    }
                });
        });
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut picker.id).hint_text("Partner ID").desired_width(145.0));
            let password = ui.add(egui::TextEdit::singleline(&mut picker.password).password(true).hint_text("Password").desired_width(160.0));
            let ready = app.attempt.is_none() && picker.id.parse::<DeviceId>().is_ok() && !picker.password.trim().is_empty();
            if ui.add_enabled_ui(ready, |ui| theme::pill(ui, "Link", pal::GREEN)).inner.clicked()
                || (ready && password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                choice = Some(Choice::Connect);
            }
        });
        ui.separator();
        let filter = picker.search.to_lowercase();
        egui::ScrollArea::vertical().id_salt("quick-link-results")
            .max_height((ctx.content_rect().height() - 270.0).clamp(90.0, 420.0)).show(ui, |ui| {
                ui.label(RichText::new("RECENT LINKS").color(pal::BLUE));
                let mut count = 0;
                for recent in &settings.recent {
                    let station = recent.station.as_deref().unwrap_or("");
                    if !matches(&filter, &recent.name, recent.id, "", station) { continue; }
                    count += 1;
                    if ui.selectable_label(false, format!("{}   ·   {}", recent.name, recent.id))
                        .on_hover_text(format!("{}\n{}", recent.os, station)).clicked() {
                        choice = Some(Choice::Recent(recent.clone()));
                    }
                }
                if count == 0 { ui.label(RichText::new("No matching recent links").color(pal::DIM)); }
                ui.add_space(10.0);
                for account in &settings.accounts {
                    let r = AccountRef::of(account);
                    ui.push_id((&account.server, &account.user.username), |ui| {
                        ui.label(RichText::new(format!("{} · {}", account.label(), account.user.name)).color(pal::LAVENDER));
                        if account.signed_out && ui.button("Sign in again to connect to saved devices").clicked() { choice = Some(Choice::Stations); }
                        for (devices, shared) in [(&account.devices, false), (&account.shared, true)] {
                            for device in devices {
                                if !matches(&filter, &device.name, device.id, &device.group, &account.label()) { continue; }
                                let color = match saved::presence(app, &r, device.id) {
                                    Some(Presence::Online) => pal::GREEN,
                                    Some(Presence::Offline) => pal::DIM,
                                    _ => pal::GOLD,
                                };
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("●").color(color));
                                    let name = if device.name.is_empty() {device.id.to_string()} else {device.name.clone()};
                                    let label = format!("{name}   ·   {}{}{}", device.id,
                                        if shared {" · shared"} else {""}, if device.secret.is_some() && !shared {" · saved password"} else {""});
                                    if ui.add_enabled(!account.signed_out && app.attempt.is_none(), egui::Button::selectable(false, label))
                                        .on_hover_text(format!("{}\n{}", device.group, device.notes)).clicked() {
                                        choice = Some(Choice::Saved(r.clone(), device.clone(), shared));
                                    }
                                });
                            }
                        }
                        ui.add_space(8.0);
                    });
                }
                if settings.accounts.is_empty() { ui.label(RichText::new("Sign in to a station to see your saved and shared devices here.").color(pal::DIM)); }
            });
        ui.separator();
        ui.horizontal(|ui| {
            if theme::pill(ui, "Refresh devices", pal::BLUE).clicked() { refresh = true; }
            if theme::pill(ui, "Stations…", pal::VIOLET).clicked() { choice = Some(Choice::Stations); }
        });
    });
    if result.should_close() { picker.open = false; }
    match choice {
        Some(Choice::Saved(r, device, shared)) => {
            picker.open = false;
            saved::connect(app, ctx, &r, &device, shared);
        },
        Some(Choice::Recent(recent)) => {
            picker.id = recent.id.to_string();
            picker.password.clear();
            picker.via = recent.station.as_deref().and_then(|station| settings.accounts.iter()
                .find(|a| !a.signed_out && a.is_station(station))).map(AccountRef::of);
            let is_default = recent.station.as_deref().is_none_or(|station| settings.server.url.as_deref()
                .is_some_and(|default| same_station(station, default)));
            if picker.via.is_none() && !is_default {
                app.toast("Sign in to this link's station under Config → Stations", pal::GOLD);
                picker.id.clear();
            } else if let Some(r) = &picker.via && let Some(account) = settings.accounts.iter().find(|a| AccountRef::of(a) == *r)
                && let Some(device) = account.devices.iter().find(|d| d.id == recent.id && d.secret.is_some()) {
                picker.open = false;
                saved::connect(app, ctx, r, device, false);
            }
        },
        Some(Choice::Connect) => {
            if let Ok(id) = picker.id.parse() {
                app.start_connect_to(ctx, id, std::mem::take(&mut picker.password), picker.via.clone());
                picker.open = false;
            }
        },
        Some(Choice::Stations) => { picker.open = false; app.page = Page::Settings; },
        None => {},
    }
    if refresh { saved::refresh(app); }
    if !picker.open { picker.password.clear(); }
    app.quick_connect = picker;
}

fn same_station(a: &str, b: &str) -> bool {
    match (slipstream_core::directory::normalize_server_url(a), slipstream_core::directory::normalize_server_url(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
