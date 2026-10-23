//! New-link picker shared by the console and floating session tabs.
use eframe::egui::{self, RichText, vec2};
use slipstream_core::{account::SavedDevice, config::RecentConnection, directory::DeviceId};
use slipstream_session::{AccountRef, Presence, StationRef};

use super::{
    App, Page, saved,
    saved::{grouped, matches},
    theme::{self, pal},
};

#[derive(Default)]
pub(super) struct QuickConnect {
    pub open: bool,
    pub destination: Option<egui::ViewportId>,
    pub focus: bool,
    search: String,
    id: String,
    password: String,
    pub(super) via: StationRef,
    manual_open: bool,
    reveal_manual: bool,
}

enum Choice {
    Recent(RecentConnection),
    Saved(AccountRef, SavedDevice, bool),
    Connect,
    Stations,
}

pub(super) fn window(app: &mut App, ctx: &egui::Context) {
    if !app.quick_connect.open {
        return;
    }
    let mut picker = std::mem::take(&mut app.quick_connect);
    let settings = app.client.settings();
    let mut choice = None;
    let mut refresh = false;
    let result = theme::modal("new-link-picker", pal::GREEN).show(ctx, |ui| {
        ui.set_width(theme::dialog_width(ctx, 550.0));
        let chrome = theme::window_header_with_close(ui, "New link", pal::GREEN, pal::PEACH);
        if chrome.close {
            picker.open = false;
        }
        if picker.destination.is_some() {ui.label("This link will be added to the window where you clicked +.");}
        let search = ui.add(theme::text_input(&mut picker.search)
            .hint_text("Find a system, group or station…").desired_width(ui.available_width()));
        if std::mem::take(&mut picker.focus) {
            search.request_focus();
        }
        ui.add_space(6.0);
        let filter = picker.search.to_lowercase();
        let overhead = if picker.manual_open { 390.0 } else { 270.0 };
        egui::ScrollArea::vertical()
            .id_salt("quick-link-results")
            .auto_shrink([false, true])
            .max_height((ctx.content_rect().height() - overhead).clamp(70.0, 420.0))
            .show(ui, |ui| {
                // Saved groups come first, including empty groups on the account.
                for account in &settings.accounts {
                    let r = AccountRef::of(account);
                    ui.push_id((&account.server, &account.user.username), |ui| {
                        theme::header(ui, &account.label(), pal::LAVENDER);
                        ui.label(RichText::new(&account.user.name).small().color(pal::DIM));
                        if account.signed_out
                            && theme::pill(ui, "Sign in again", pal::ORANGE).clicked()
                        {
                            choice = Some(Choice::Stations);
                        }
                        for (devices, empty, shared) in [
                            (&account.devices, account.groups.as_slice(), false),
                            (&account.shared, &[][..], true),
                        ] {
                            let groups = grouped(devices, empty, &filter, &account.label());
                            if groups.is_empty() {
                                continue;
                            }
                            let color = if shared { pal::GOLD } else { pal::BLUE };
                            ui.label(
                                RichText::new(if shared {
                                    "SHARED SYSTEMS"
                                } else {
                                    "MY SYSTEMS"
                                })
                                .font(theme::display(14.0))
                                .color(color),
                            );
                            for (group, devices) in groups {
                                let name = if group.is_empty() { "Ungrouped" } else { group };
                                egui::CollapsingHeader::new(
                                    RichText::new(format!("{}  ·  {}", name.to_uppercase(), devices.len()))
                                        .font(theme::display_bold(18.0))
                                        .color(color),
                                )
                                .id_salt((shared, group))
                                .default_open(true)
                                .open((!filter.trim().is_empty()).then_some(true))
                                .show(ui, |ui| {
                                    if devices.is_empty() {
                                        ui.label(
                                            RichText::new("No systems in this group yet")
                                                .small()
                                                .color(pal::DIM),
                                        );
                                    }
                                    for device in devices {
                                        let (lamp, status) =
                                            match saved::presence(app, &r, device.id) {
                                                Some(Presence::Online) => (pal::GREEN, "Online"),
                                                Some(Presence::Offline) => (pal::DIM, "Offline"),
                                                _ => (pal::GOLD, "Checking availability"),
                                            };
                                        let detail = if shared {
                                            "Shared system"
                                        } else if device.secret.is_some() {
                                            "Saved password"
                                        } else {
                                            "Password required"
                                        };
                                        let name = if device.name.is_empty() {
                                            device.id.to_string()
                                        } else {
                                            device.name.clone()
                                        };
                                        ui.push_id(device.id, |ui| {
                                            if link_row(
                                                ui,
                                                &name,
                                                device.id,
                                                detail,
                                                lamp,
                                                !account.signed_out && app.attempt.is_none(),
                                            )
                                            .on_hover_text(format!(
                                                "{status} · {name}\n{}",
                                                device.notes
                                            ))
                                            .clicked()
                                            {
                                                choice = Some(Choice::Saved(
                                                    r.clone(),
                                                    device.clone(),
                                                    shared,
                                                ));
                                            }
                                        });
                                    }
                                });
                            }
                        }
                        if account.devices.is_empty()
                            && account.shared.is_empty()
                            && account.groups.is_empty()
                        {
                            ui.label(
                                RichText::new(
                                    "No saved systems. Add devices and groups on Bridge.",
                                )
                                .small()
                                .color(pal::DIM),
                            );
                        }
                        ui.add_space(8.0);
                    });
                }
                egui::CollapsingHeader::new(
                    RichText::new("RECENT LINKS")
                        .font(theme::display_bold(18.0))
                        .color(pal::BLUE),
                )
                .id_salt("quick-link-recents")
                .default_open(settings.accounts.is_empty())
                .show(ui, |ui| {
                    let mut count = 0;
                    for recent in &settings.recent {
                        let station = recent.station.as_deref().unwrap_or("");
                        if !matches(&filter, &recent.name, recent.id, "", station) {
                            continue;
                        }
                        count += 1;
                        ui.push_id((station, recent.id), |ui| {
                            if link_row(
                                ui,
                                &recent.name,
                                recent.id,
                                &recent.os,
                                pal::BLUE,
                                app.attempt.is_none(),
                            )
                            .on_hover_text(station)
                            .clicked()
                            {
                                choice = Some(Choice::Recent(recent.clone()));
                            }
                        });
                    }
                    if count == 0 {
                        ui.label(RichText::new("No matching recent links").color(pal::DIM));
                    }
                });
            });
        ui.add_space(6.0);
        let manual = egui::CollapsingHeader::new(
            RichText::new("LINK BY ID")
                .font(theme::display_bold(17.0))
                .color(pal::ORANGE),
        )
        .id_salt("quick-link-manual")
        .open(std::mem::take(&mut picker.reveal_manual).then_some(true))
        .show(ui, |ui| {
            egui::ComboBox::from_id_salt("quick-link-station")
                .width(ui.available_width())
                .truncate()
                .selected_text(
                    match &picker.via {
                        StationRef::Public => "Slipstream Public".into(),
                        StationRef::Primary => if app.client.has_custom_station() { "My station".into() } else { "Slipstream Public".into() },
                        StationRef::Account(r) => settings.accounts.iter().find(|a| AccountRef::of(a) == *r).map(|a| a.label()).unwrap_or_else(|| "Signed out".into()),
                    },
                )
                .show_ui(ui, |ui| {
                    theme::console_menu_style(ui.style_mut());
                    ui.selectable_value(&mut picker.via, StationRef::Primary, if app.client.has_custom_station() { "My station" } else { "Slipstream Public" });
                    if app.client.has_custom_station() { ui.selectable_value(&mut picker.via, StationRef::Public, "Slipstream Public"); }
                    for account in settings.accounts.iter().filter(|a| !a.signed_out) {
                        ui.selectable_value(
                            &mut picker.via,
                            StationRef::Account(AccountRef::of(account)),
                            format!("{} · {}", account.label(), account.user.name),
                        );
                    }
                });
            ui.horizontal_wrapped(|ui| {
                let width = ((ui.available_width() - 88.0) / 2.0).clamp(90.0, 220.0);
                ui.add(theme::text_input(&mut picker.id).hint_text("Partner ID").desired_width(width));
                let password = ui.add(theme::text_input(&mut picker.password)
                    .password(true).hint_text("Password").desired_width(width));
                let ready = app.attempt.is_none()
                    && picker.id.parse::<DeviceId>().is_ok()
                    && !picker.password.trim().is_empty();
                if ui
                    .add_enabled_ui(ready, |ui| theme::pill(ui, "Link", pal::ORANGE))
                    .inner
                    .clicked()
                    || (ready
                        && password.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    choice = Some(Choice::Connect);
                }
            });
        });
        picker.manual_open = manual.body_response.is_some();
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if theme::pill(ui, "Refresh", pal::BLUE).clicked() {
                refresh = true;
            }
            if theme::pill(ui, "Stations", pal::VIOLET).clicked() {
                choice = Some(Choice::Stations);
            }
        });
        chrome.footer(ui);
    });
    if result.should_close() {
        picker.open = false;
    }
    let previous_destination=app.connect_destination;
    app.connect_destination=picker.destination;
    match choice {
        Some(Choice::Saved(r, device, shared)) => {
            picker.open = false;
            saved::connect(app, ctx, &r, &device, shared);
        }
        Some(Choice::Recent(recent)) => {
            picker.id = recent.id.to_string();
            picker.manual_open = true;
            picker.reveal_manual = true;
            picker.password.clear();
            let Some(route) = recent_station(app, &recent) else {
                app.toast("Sign in to this link's station under Config → Stations", pal::GOLD);
                picker.id.clear();
                app.quick_connect = picker;
                app.connect_destination = previous_destination;
                return;
            };
            picker.via = route;
            if let StationRef::Account(r) = &picker.via
                && let Some(account) = settings.accounts.iter().find(|a| AccountRef::of(a) == *r)
                && let Some(device) = account
                    .devices
                    .iter()
                    .find(|d| d.id == recent.id && d.secret.is_some())
            {
                picker.open = false;
                saved::connect(app, ctx, r, device, false);
            }
        }
        Some(Choice::Connect) => {
            if let Ok(id) = picker.id.parse() {
                app.start_connect_at(
                    ctx,
                    id,
                    std::mem::take(&mut picker.password),
                    picker.via.clone(),
                );
                picker.open = false;
            }
        }
        Some(Choice::Stations) => {
            picker.open = false;
            app.page = Page::Settings;
        }
        None => {}
    }
    app.connect_destination=previous_destination;
    if refresh {
        saved::refresh(app);
    }
    if !picker.open {
        picker.password.clear();
    }
    app.quick_connect = picker;
}

pub(super) fn recent_station(app: &App, recent: &RecentConnection) -> Option<StationRef> {
    let settings = app.client.settings();
    let Some(station) = recent.station.as_deref() else { return Some(StationRef::Primary) };
    if slipstream_core::config::is_public_station(station) { return Some(StationRef::Public); }
    if app.client.station_settings().url.as_deref().is_some_and(|url| same_station(station, url)) {
        return Some(StationRef::Primary);
    }
    settings.accounts.iter().find(|a| !a.signed_out && a.is_station(station)).map(|a| StationRef::Account(AccountRef::of(a)))
}

fn same_station(a: &str, b: &str) -> bool {
    match (
        slipstream_core::directory::normalize_server_url(a),
        slipstream_core::directory::normalize_server_url(b),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn link_row(
    ui: &mut egui::Ui,
    name: &str,
    id: DeviceId,
    detail: &str,
    color: egui::Color32,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), 48.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let p = ui.painter();
    let fill = if response.hovered() && enabled {
        egui::Color32::from_rgb(26, 36, 54)
    } else {
        egui::Color32::from_rgb(14, 19, 31)
    };
    p.rect_filled(rect, 7.0, fill);
    p.rect_filled(
        rect.with_max_x(rect.left() + 6.0),
        egui::CornerRadius {
            nw: 7,
            sw: 7,
            ..Default::default()
        },
        color,
    );
    p.circle_filled(rect.left_center() + vec2(19.0, 0.0), 4.0, color);
    let text = rect.shrink2(vec2(34.0, 3.0));
    for (label, font, ink, y) in [
        (
            name.to_uppercase(),
            theme::display(18.0),
            if enabled { pal::TAN } else { pal::DIM },
            text.top(),
        ),
        (
            format!("{id} · {detail}"),
            theme::body(13.0),
            pal::DIM,
            text.top() + 23.0,
        ),
    ] {
        let mut job = egui::text::LayoutJob::simple(label, font, ink, text.width().max(1.0));
        job.wrap.max_rows = 1;
        p.with_clip_rect(text)
            .galley(egui::pos2(text.left(), y), p.layout_job(job), ink);
    }
    theme::delta(
        p,
        egui::Rect::from_center_size(rect.right_center() - vec2(17.0, 0.0), vec2(18.0, 18.0)),
        if enabled { pal::ORANGE } else { pal::DIM },
        fill,
        true,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled, format!("Link to {name}, {id}"))
    });
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_search_keeps_empty_groups_and_device_membership() {
        let device = |id, name: &str, group: &str| SavedDevice {
            id: DeviceId(id),
            name: name.into(),
            group: group.into(),
            notes: String::new(),
            secret: None,
            added_by: None,
        };
        let devices = vec![
            device(123456789, "Engineering", "Lab"),
            device(123456790, "Console", "Lab"),
            device(123456791, "Gateway", "Servers"),
        ];
        let empty = vec!["Empty lab".into()];
        let all = grouped(&devices, &empty, "", "Station");
        assert_eq!(all.len(), 3);
        assert_eq!(all["Lab"].len(), 2);
        assert!(all["Empty lab"].is_empty());
        let lab = grouped(&devices, &empty, "lab", "Station");
        assert_eq!(lab.len(), 2);
        assert!(!lab.contains_key("Servers"));
        let engineering = grouped(&devices, &empty, "engineering", "Station");
        assert_eq!(engineering.len(), 1);
        assert_eq!(engineering["Lab"][0].id, DeviceId(123456789));
        assert!(grouped(&devices, &empty, "missing", "Station").is_empty());
    }

    #[test]
    fn lcars_modal_header_and_footer_stay_inside_the_frame() {
        for size in [vec2(720.0, 480.0), vec2(360.0, 640.0)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            for frame in 0..4 {
                let mut bounds = egui::Rect::NOTHING;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |_| {
                        bounds = theme::modal("layout-regression", pal::GREEN)
                            .show(&ctx, |ui| {
                                ui.set_width(theme::dialog_width(&ctx, 550.0));
                                let chrome = theme::window_header_with_close(
                                    ui,
                                    "New link",
                                    pal::GREEN,
                                    pal::PEACH,
                                );
                                ui.label("Grouped systems");
                                chrome.footer(ui);
                            })
                            .response
                            .rect;
                    },
                );
                output.textures_delta.clear();
                if frame < 3 {
                    continue;
                }
                assert!(screen.contains_rect(bounds), "modal overflow at {size:?}");
                let mut bands = 0;
                for shape in output.shapes {
                    if let egui::Shape::Rect(rect) = shape.shape
                        && [pal::GREEN, pal::PEACH].contains(&rect.fill)
                    {
                        bands += 1;
                        assert!(
                            bounds.expand(0.5).contains_rect(rect.rect),
                            "LCARS chrome escaped the modal frame"
                        );
                    }
                }
                assert!(bands >= 3, "missing header, close segment or footer");
            }
        }
    }
}
