//! Bridge (home) page: this device's ID/password, connect form, recents.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, vec2};
use slipstream_session::DirStatus;

use super::{App, Page, files, theme, theme::pal};

/// Whether the Windows service is running, and its state in words.
fn service_state() -> (bool, &'static str) {
    #[cfg(windows)]
    {
        let s = crate::service::status_text();
        (s == "Running", s)
    }
    #[cfg(not(windows))]
    (true, "Running")
}

/// The ID the Windows service hosts this computer as, while it does.
fn service_id(app: &App) -> Option<slipstream_core::directory::DeviceId> {
    #[cfg(windows)]
    if app.via_service {
        return app.service_station_matches().then(crate::service::hosted_id).flatten();
    }
    let _ = app;
    None
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    let t = app.time();
    if let Some(motd) = app.motd.clone() {
        ui.label(RichText::new(motd).color(pal::BLUE).font(theme::body(15.0)));
        ui.add_space(4.0);
    }
    if !app.incoming.is_empty() {
        incoming_banner(app, ui, t);
        ui.add_space(10.0);
    }
    if app.update.is_some() {
        update_banner(app, ui);
        ui.add_space(10.0);
    }
    #[cfg(target_os="linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && app.client.settings().host.enabled {
        if let Some(Err(error)) = app.screen_access.result.clone() {
            ui.label(RichText::new(error).color(pal::GOLD));
            ui.horizontal(|ui| {
                if theme::pill(ui, "Retry screen access", pal::GREEN).clicked() {
                    app.screen_access.start(&app.rt, &app.client, ui.ctx(), false);
                }
                if theme::pill(ui, "Screen permission…", pal::BLUE).clicked() { app.page = Page::Settings; }
            });
            ui.add_space(6.0);
        } else if app.screen_access.busy() {
            ui.label(RichText::new("Preparing screen access · approve the desktop dialog if shown").color(pal::GOLD));
        }
    }

    station_tabs(app, ui);
    if ui.available_width() > 660.0 {
        ui.columns(2, |cols| {
            station_panel(app, &mut cols[0], t);
            connect_panel(app, &mut cols[1]);
        });
    } else {
        station_panel(app, ui, t);
        ui.add_space(16.0);
        connect_panel(app, ui);
    }
    ui.add_space(20.0);
    super::saved::panel(app, ui);
    ui.add_space(20.0);
    recent_panel(app, ui);
}

fn station_tabs(app: &mut App, ui: &mut Ui) {
    let custom = app.client.has_custom_station();
    let station = if custom { app.client.settings().server.url } else { None };
    if app.primary_station != station {
        if app.selected_public { app.station_draft = Default::default(); }
        else { app.connect_id.clear(); app.connect_pw.clear(); }
        app.primary_station = station;
        app.select_public(!custom);
    }
    if !custom {
        if app.client.public_dir_status() != DirStatus::NotConfigured { app.select_public(true); }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("SLIPSTREAM PUBLIC").font(theme::display_bold(18.0)).color(pal::LAVENDER));
            if !app.link_open
                && theme::pill(ui, "Link to my organisation", pal::BLUE)
                    .on_hover_text("Use your support provider's own station: its address and setup code are on its download page")
                    .clicked()
            {
                app.link_open = true;
            }
        });
        if app.link_open {
            ui.add_space(4.0);
            link_fields(app, ui);
            if theme::pill(ui, "Cancel", pal::BLUE).clicked() {
                app.link_open = false;
            }
        }
    } else {
        app.link_open = false;
        let mut public = app.selected_public;
        ui.horizontal_wrapped(|ui| {
            let label = app.org.as_deref().unwrap_or("My station");
            station_tab(ui, &mut public, false, label, pal::BLUE);
            station_tab(ui, &mut public, true, "Slipstream Public", pal::VIOLET);
        });
        app.select_public(public);
    }
    ui.add_space(10.0);
}

fn station_tab(ui: &mut Ui, selected: &mut bool, value: bool, label: &str, color: Color32) {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.selection.bg_fill = color;
        visuals.selection.stroke = egui::Stroke::new(1.5, color);
        visuals.widgets.hovered.bg_fill = color.gamma_multiply(0.2);
        visuals.widgets.hovered.weak_bg_fill = color.gamma_multiply(0.2);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.5, color);
        visuals.widgets.active.bg_fill = color.gamma_multiply(0.35);
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.5, color);
        let ink = if *selected == value { pal::BG } else { color };
        ui.selectable_value(selected, value, RichText::new(label).font(theme::display_bold(18.0)).color(ink));
    });
}

fn station_panel(app: &mut App, ui: &mut Ui, t: f64) {
    theme::panel(ui, "This device", pal::LAVENDER, |ui| {
        ui.label(RichText::new("Give your partner this ID and password to let them in.").color(pal::DIM));
        ui.label(
            RichText::new("Only share them with someone you know. Never with a caller who contacted you unexpectedly.")
                .small()
                .color(pal::PEACH),
        );
        ui.add_space(8.0);

        let custom = app.client.has_custom_station();
        let public = app.selected_public || !custom;
        let directory = app.selected_dir_status();
        ui.label(RichText::new(if public { "public.slipstreamremote.com".to_string() } else {
            app.client.settings().server.url.unwrap_or_default()
        }).small().color(pal::DIM));
        let (status, color) = match &directory {
            DirStatus::Online { .. } => ("ONLINE · READY FOR LINK", pal::GREEN),
            DirStatus::Connecting | DirStatus::Ready => ("CONTACTING SERVER…", pal::GOLD),
            DirStatus::NotConfigured => ("NO SERVER — SEE CONFIG", pal::RED),
            DirStatus::Error(_) => ("SERVER CONNECTION FAILED", pal::RED),
        };
        let own_id = match &directory {
            DirStatus::Online { id } => Some(*id),
            _ if app.via_service && (public || !app.service_station_matches()) => None,
            _ if public => app.client.settings().host.public_device_id,
            _ => app.client.settings().host.last_device_id,
        };
        // With the service hosting, partners need the ID it registered as.
        let hosted = if public { None } else { service_id(app) };
        let id_text = hosted.or(own_id).map(|i| i.to_string()).unwrap_or_else(|| "--- --- ---".into());

        ui.label(RichText::new("YOUR ID").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.horizontal(|ui| {
            ui.label(RichText::new(&id_text).font(theme::mono(if ui.available_width()<420.0 {32.0}else{44.0})).color(pal::TAN));
            ui.add_space(8.0);
            if theme::pill_sized(ui, "Copy", pal::LAVENDER, vec2(70.0, 28.0)).clicked() {
                ui.ctx().copy_text(id_text.replace(' ', ""));
                app.toast("ID copied", pal::LAVENDER);
            }
        });
        ui.add_space(4.0);
        if app.via_service {
            if let DirStatus::Error(error) = &directory { ui.label(RichText::new(error).color(pal::GOLD)); }
            // The service owns both its one-time code and permanent password.
            one_time_password(app, ui);
            ui.label(RichText::new("REACHABLE THROUGH THE SLIPSTREAM SERVICE").font(theme::display(15.0)).color(pal::LAVENDER));
            ui.label(
                RichText::new("Share the one-time code above, or use this computer's unattended password. Both work with the same ID. The service also reaches the login screen and administrator prompts.")
                    .color(pal::DIM),
            );
            if let (Some(h), Some(own)) = (hosted, own_id)
                && h != own
            {
                ui.label(
                    RichText::new(format!(
                        "The service has its own ID, shown above; this app's is {own}. Service updates and reinstalls preserve the service's ID."
                    ))
                    .color(pal::PEACH),
                );
            }
            ui.add_space(10.0);
            let (running, state) = service_state();
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                let color = if running { pal::GREEN } else { pal::RED };
                theme::lamp(ui.painter(), rect.center(), color, t);
                let text = if running { "SERVICE RUNNING · READY FOR LINK".to_string() } else { format!("SERVICE {} · NOT REACHABLE", state.to_uppercase()) };
                ui.label(RichText::new(text).font(theme::display(17.0)).color(color));
            });
            #[cfg(windows)]
            if !running {
                ui.add_space(6.0);
                if let Some(why) = crate::service::start_problem() {
                    ui.label(RichText::new(why).color(pal::PEACH));
                }
                ui.add_enabled_ui(!app.service_busy(), |ui| {
                    if theme::pill(ui, if app.service_busy() { "Starting…" } else { "Start the service" }, pal::GREEN).clicked() {
                        let ctx = ui.ctx().clone();
                        app.service_action(&ctx, true, true);
                    }
                });
            }
            return;
        }
        one_time_password(app, ui);
        ui.add_space(10.0);
        if directory == DirStatus::NotConfigured {
            link_form(app, ui, t);
            return;
        }
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            theme::lamp(ui.painter(), rect.center(), color, t);
            ui.label(RichText::new(status).font(theme::display(17.0)).color(color));
        });
        if let DirStatus::Error(e) = &directory {
            ui.label(RichText::new(e).font(theme::body(16.0)).color(pal::PEACH));
            if let Ok(store) = slipstream_core::config::Store::open_default() {
                let log = store.dir().join("slipstream.log").display().to_string();
                ui.label(RichText::new("Log file:").small().color(pal::DIM));
                ui.add(egui::Label::new(RichText::new(&log).small().monospace().color(pal::DIM)).truncate())
                    .on_hover_text(&log);
            }
        }
        #[cfg(target_os="android")]
        super::android::hosting_panel(ui);
        if app.client.settings().host.permanent_password_secret.is_some() {
            ui.label(RichText::new(if cfg!(target_os="android") {"Your saved password also works while screen sharing is active."}else{"Unattended access enabled — your permanent password also works."}).small().color(pal::DIM));
        }
    });
}

/// Show the code belonging to the actual host, including the Windows service.
fn one_time_password(app: &mut App, ui: &mut Ui) {
    let password = if app.via_service { app.service_password.clone() } else { Some(app.password.clone()) };
    ui.label(RichText::new("ONE-TIME CODE").font(theme::display(15.0)).color(pal::LAVENDER));
    if let Some(password) = password {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&password).font(theme::mono(if ui.available_width()<420.0 {26.0}else{34.0})).color(pal::GOLD));
            ui.add_space(8.0);
            if theme::pill_sized(ui, "Copy", pal::LAVENDER, vec2(70.0, 28.0)).clicked() {
                ui.ctx().copy_text(password.clone());
                app.toast("One-time code copied", pal::LAVENDER);
            }
            if theme::pill_sized(ui, "New", pal::VIOLET, vec2(64.0, 28.0)).clicked() {
                app.regenerate_password();
            }
        });
        let help = if app.via_service || app.client.settings().host.permanent_password_secret.is_some() {
            "Either this code or the unattended password works. The code changes after a session ends."
        } else { "The code changes after a session ends." };
        ui.label(RichText::new(help).small().color(pal::DIM));
    } else {
        ui.label(RichText::new("Waiting for the service's code…").color(pal::GOLD))
            .on_hover_text("Restart Slipstream to bring an older service up to date. Your unattended password continues to work.");
    }
    ui.add_space(6.0);
}

/// For an app that isn't linked to a station yet: its address and a setup
/// code, both shown on the station's download page.
fn link_form(app: &mut App, ui: &mut Ui, t: f64) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        theme::lamp(ui.painter(), rect.center(), pal::RED, t);
        ui.label(RichText::new("NOT CONNECTED TO A STATION").font(theme::display(17.0)).color(pal::RED));
    });
    link_fields(app, ui);
}

/// A station's address and setup code (both on its download page), and Connect.
fn link_fields(app: &mut App, ui: &mut Ui) {
    ui.label(
        RichText::new(
            "Press “Connect this app” on your support provider's download page, or enter the station address \
             and setup code shown there.",
        )
        .color(pal::DIM),
    );
    ui.add_space(4.0);
    let field = |ui: &mut Ui, value: &mut String, hint: &str, width: f32| {
        ui.add(theme::text_input(value).hint_text(hint).font(theme::mono(18.0)).desired_width(width))
    };
    let mut go = false;
    // On a phone the address gets a line of its own.
    let narrow = ui.available_width() < 440.0;
    let mut station = None;
    if narrow {
        station = Some(field(ui, &mut app.link_station, "support.example.com", ui.available_width() - 24.0));
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        let a = station.take().unwrap_or_else(|| field(ui, &mut app.link_station, "support.example.com", 190.0));
        let c = field(ui, &mut app.link_code, "SETUP CODE", 110.0);
        go = (a.lost_focus() || c.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let label = if app.linking() { "Connecting…" } else { "Connect" };
        go |= ui.add_enabled_ui(!app.linking(), |ui| theme::pill(ui, label, pal::ORANGE)).inner.clicked();
    });
    if go && !app.linking() {
        let code = slipstream_core::provision::normalize_code(&app.link_code);
        if app.link_station.trim().is_empty() || code.len() != slipstream_core::provision::CODE_LEN {
            app.toast("Enter the station address and the 8-character setup code", pal::RED);
        } else {
            let link = slipstream_core::provision::StationLink { station: app.link_station.trim().to_string(), code };
            app.start_link(&ui.ctx().clone(), link, true);
        }
    }
}

fn connect_panel(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    theme::panel(ui, "Establish link", pal::ORANGE, |ui| {
        ui.label(RichText::new("Control another device.").color(pal::DIM));
        ui.label(RichText::new(if app.selected_public || !app.client.has_custom_station() {
            "Looking up IDs on Slipstream Public"
        } else { "Looking up IDs on your self-hosted station" }).small().color(pal::LAVENDER));
        ui.add_space(8.0);
        ui.label(RichText::new("PARTNER ID").font(theme::display(15.0)).color(pal::ORANGE));
        let field_font = theme::mono(22.0);
        let id_resp = ui.add(
            theme::text_input(&mut app.connect_id)
                .id_salt(("partner-id", app.selected_public, &app.primary_station))
                .hint_text(RichText::new("482 913 077").font(field_font.clone()))
                .font(field_font.clone())
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.label(RichText::new("PASSWORD").font(theme::display(15.0)).color(pal::ORANGE));
        let pw_resp = ui.add(
            theme::text_input(&mut app.connect_pw)
                .id_salt(("partner-password", app.selected_public, &app.primary_station))
                .hint_text(RichText::new("partner's password").font(field_font.clone()))
                .password(true)
                .font(field_font)
                .desired_width(f32::INFINITY),
        );
        let enter = (id_resp.lost_focus() || pw_resp.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if id_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && app.connect_pw.is_empty() {
            pw_resp.request_focus();
        }
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), if cfg!(target_os="android") {42.0} else {36.0}), Layout::right_to_left(Align::Center), |ui| {
            let busy = app.attempt.is_some();
            let engage = ui.add_enabled_ui(!busy, |ui| theme::pill_sized(ui, "Engage", pal::ORANGE, vec2(120.0, 34.0))).inner;
            if (engage.clicked() || (enter && !app.connect_pw.is_empty())) && !busy {
                app.start_connect(&ctx);
            }
        });
    });
}

fn recent_panel(app: &mut App, ui: &mut Ui) {
    let recent = app.client.settings().recent.into_iter().filter(|r| {
        match super::quick_connect::recent_station(app, r) {
            Some(slipstream_session::StationRef::Public) => app.selected_public,
            Some(slipstream_session::StationRef::Primary) => !app.selected_public || !app.client.has_custom_station(),
            _ => false,
        }
    }).collect::<Vec<_>>();
    theme::panel(ui, "Recent links", pal::BLUE, |ui| {
        if recent.is_empty() {
            ui.label(RichText::new("Devices you connect to will appear here.").color(pal::DIM));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for r in recent.iter().take(12) {
                let (rect, resp) = ui.allocate_exact_size(vec2(220.0, 62.0), Sense::click());
                let painter = ui.painter();
                let hovered = resp.hovered();
                if hovered {
                    theme::glow(painter, rect, 10.0, pal::BLUE, 0.8);
                }
                painter.rect_filled(rect, 10.0, if hovered { Color32::from_rgb(20, 28, 48) } else { pal::PANEL });
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.min, vec2(8.0, rect.height())),
                    egui::CornerRadius { nw: 10, sw: 10, ..Default::default() },
                    pal::BLUE,
                );
                painter.text(
                    rect.min + vec2(20.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    r.name.to_uppercase(),
                    theme::display(18.0),
                    pal::TAN,
                );
                painter.text(
                    rect.min + vec2(20.0, 34.0),
                    egui::Align2::LEFT_TOP,
                    format!("{}  ·  {}", r.id, r.os),
                    theme::mono(14.0),
                    pal::BLUE,
                );
                // ★: keep it in a signed-in station's list.
                let star = egui::Rect::from_center_size(rect.right_top() + vec2(-18.0, 18.0), vec2(26.0, 26.0));
                let star_resp = ui.interact(star, resp.id.with("save"), Sense::click()).on_hover_text("Save in my devices");
                theme::star(painter, star.shrink(5.0), if star_resp.hovered() { pal::GOLD } else { pal::DIM });
                star_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Save in my devices"));
                if star_resp.clicked() {
                    super::saved::save_recent(app, r);
                } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    let route = super::quick_connect::recent_station(app, r);
                    match route {
                        Some(slipstream_session::StationRef::Primary) => app.select_public(false),
                        Some(slipstream_session::StationRef::Public) => app.select_public(true),
                        _ => { app.toast("Open + / New link to connect through this saved station", pal::GOLD); continue; }
                    }
                    app.connect_id = r.id.to_string();
                    app.connect_pw.clear();
                    app.recent_prompt = Some(RecentPrompt {
                        id: r.id,
                        name: if r.name.is_empty() { r.id.to_string() } else { r.name.clone() },
                        via: route.unwrap_or_default(),
                        password: String::new(),
                    });
                }
            }
        });
    });
}

/// Asking for the password of a recent link, then connecting (Recent links).
pub struct RecentPrompt {
    id: slipstream_core::directory::DeviceId,
    name: String,
    via: slipstream_session::StationRef,
    password: String,
}

pub(super) fn recent_prompt(app: &mut App, ctx: &egui::Context) {
    let Some(mut p) = app.recent_prompt.take() else { return };
    let (mut open, mut go) = (true, false);
    let title = format!("Connect to {}", p.name);
    theme::window("recent-prompt", pal::ORANGE)
        .id(egui::Id::unique("recent-prompt"))
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let chrome = theme::window_header(ui, &title, pal::ORANGE);
            open = !chrome.close;
            ui.label(RichText::new(format!("{} · its one-time or unattended password", p.id)).color(pal::DIM));
            let field = ui.add(theme::text_input(&mut p.password).hint_text("password").password(true).desired_width(300.0f32.min(ui.available_width())));
            field.request_focus();
            go = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                go |= theme::pill(ui, "Engage", pal::ORANGE).clicked();
                if theme::pill(ui, "Cancel", pal::BLUE).clicked() {
                    open = false;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
            chrome.footer(ui);
        });
    if go && !p.password.trim().is_empty() {
        app.start_connect_at(ctx, p.id, std::mem::take(&mut p.password), p.via);
    } else if open {
        app.recent_prompt = Some(p);
    }
}

pub(super) fn update_banner(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let Some(u) = &app.update else { return };
    let (label, installing, error, service_only) = (u.info.label(), u.installing, u.error.clone(), u.service_only);
    let (done, total) = *u.progress.lock().unwrap();
    let mut install = false;
    let mut dismiss = false;
    egui::Frame::new()
        .fill(pal::GREEN.gamma_multiply(0.08))
        .stroke(egui::Stroke::new(1.5, pal::GREEN.gamma_multiply(0.8)))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(if service_only { "◆ WINDOWS SERVICE UPDATE" } else { "◆ UPDATE AVAILABLE" }).font(theme::display_bold(18.0)).color(pal::GREEN));
                ui.label(
                    RichText::new(if service_only { format!("Bring the service up to Slipstream {label}") } else { format!("Slipstream {label} — you have {}", slipstream_core::version_string()) })
                        .font(theme::body(16.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if installing {
                        let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                        let text = if service_only || total > 0 && done >= total {
                            "INSTALLING…".to_string()
                        } else if total == 0 {
                            "DOWNLOADING…".to_string()
                        } else {
                            format!("DOWNLOADING {:.0}% · {} / {}", frac * 100.0, files::human_size(done), files::human_size(total))
                        };
                        theme::progress(ui, frac, 260.0, pal::GREEN, &text);
                    } else {
                        install = theme::pill(ui, if service_only { "Update service" } else { "Install & restart" }, pal::GREEN).clicked();
                        dismiss = ui.small_button("Later").clicked();
                    }
                });
            });
            if let Some(e) = error {
                ui.label(RichText::new(e).color(pal::RED));
            }
            ui.label(RichText::new("Your settings, ID and passwords are kept.").small().color(pal::DIM));
            #[cfg(windows)]
            if app.via_service {
                ui.label(RichText::new(if service_only { "Uses the app already installed. This window stays open; connections pause during the service restart. Windows may ask for administrator approval." } else { "Updates the app and Windows service together. Connections pause during the service restart; Windows may ask for administrator approval." }).small().color(pal::DIM));
            }
        });
    if install {
        app.start_update(&ctx);
    }
    if dismiss {
        app.update = None;
    }
}

fn incoming_banner(app: &mut App, ui: &mut Ui, t: f64) {
    let pulse = 0.55 + 0.45 * ((t * 2.5).sin() as f32 * 0.5 + 0.5);
    let frame = egui::Frame::new()
        .fill(pal::RED.gamma_multiply(0.18 * pulse + 0.05))
        .stroke(egui::Stroke::new(2.0, pal::RED.gamma_multiply(pulse)))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(16, 10));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut end = None;
        let mut chat = None;
        for s in &app.incoming {
            ui.horizontal(|ui| {
                ui.label(RichText::new("◆ INBOUND LINK").font(theme::display_bold(18.0)).color(pal::RED));
                ui.label(
                    RichText::new({
                        let secs = s.started.elapsed().as_secs();
                        format!("{} is controlling this device · {:02}:{:02}", s.peer, secs / 60, secs % 60)
                    })
                        .font(theme::body(17.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::pill(ui, "Terminate", pal::RED).clicked() {
                        end = Some(s.id);
                    }
                    if theme::pill(ui, "Chat", pal::LAVENDER).clicked() {
                        chat = Some(s.id);
                    }
                });
            });
        }
        if let Some(id) = end {
            #[cfg(windows)]
            if let Some(bridge)=&app.service_chat {bridge.sender().disconnect(id);} else {app.client.disconnect_host_session(id);}
            #[cfg(not(windows))]
            app.client.disconnect_host_session(id);
        }
        if let Some(id) = chat
            && let Some(s) = app.incoming.iter_mut().find(|s| s.id == id)
        {
            let mut chat = s.chat.lock().expect("chat lock");
            chat.open = !chat.open;
        }
    });
}

/// Chat with whoever is controlling this computer, in its own window that
/// can be moved anywhere.
pub fn incoming_windows(app: &mut App, ctx: &egui::Context) {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    for s in &app.incoming {
        if s.chat.lock().unwrap().status_open {
            let (chat, client, session, peer) = (s.chat.clone(), app.client.clone(), s.id, s.peer.clone());
            #[cfg(windows)]
            let service_sender = app.service_chat.as_ref().map(|g| g.sender());
            let builder = egui::ViewportBuilder::default().with_title(format!("Support status · {}", s.peer))
                .with_decorations(false).with_transparent(theme::transparent_window())
                .with_window_level(egui::WindowLevel::AlwaysOnTop).with_active(false)
                .with_icon(ICON.get_or_init(|| std::sync::Arc::new(theme::icon(64))).clone())
                .with_inner_size([430.0, 240.0]).with_min_inner_size([330.0, 210.0]);
            ctx.show_viewport_deferred(egui::ViewportId::from_hash_of(("support-status", session)), builder, move |ui, class| {
                let ctx = ui.ctx().clone();
                let mut chat = chat.lock().unwrap();
                if !chat.status_open {theme::dismiss_tool_viewport(&ctx, class == egui::ViewportClass::Deferred);return;}
                let (body, close) = theme::tool_frame(ui, "Support in progress", class == egui::ViewportClass::Deferred, pal::CYAN);
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(body));
                egui::ScrollArea::vertical().max_height((body.height() - 60.0).max(40.0)).show(&mut content, |ui| {
                    ui.label(RichText::new(&peer).color(pal::LAVENDER));
                    ui.label(RichText::new(&chat.status).size(19.0).color(pal::TAN));
                });
                content.add_space(8.0);
                content.horizontal_wrapped(|ui| {
                    if theme::pill(ui, "Stop support", pal::RED).clicked() {
                        #[cfg(windows)]
                        if let Some(sender) = &service_sender {sender.disconnect(session);} else {client.disconnect_host_session(session);}
                        #[cfg(not(windows))]
                        client.disconnect_host_session(session);
                        chat.status_open = false;
                    }
                    if theme::pill(ui, "Dismiss", pal::BLUE).clicked() {chat.status_open = false;}
                });
                if close || ctx.input(|i| i.viewport().close_requested()) {chat.status_open = false;}
                if !chat.status_open {theme::dismiss_tool_viewport(&ctx, class == egui::ViewportClass::Deferred);}
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            });
        }
        let id = egui::ViewportId::from_hash_of(("incoming-chat", s.id));
        {
            let chat = s.chat.lock().expect("chat lock");
            if !chat.open {
                continue;
            }
            if chat.ping {
                ctx.request_repaint_of(id);
            }
        }
        let builder = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(theme::transparent_window())
            .with_title(format!("Chat - {} - Slipstream Remote", s.peer))
            .with_app_id("slipstream-remote")
            .with_icon(ICON.get_or_init(|| std::sync::Arc::new(theme::icon(64))).clone())
            .with_inner_size([460.0, 520.0])
            .with_min_inner_size([340.0, 300.0]);
        let (chat, client, session, peer) = (s.chat.clone(), app.client.clone(), s.id, s.peer.clone());
        #[cfg(windows)]
        let service_sender=app.service_chat.as_ref().map(|g|g.sender());
        ctx.show_viewport_deferred(id, builder, move |ui, _class| {
            let body=theme::native_title_bar_with_rail(ui,&format!("Chat · {peer}"),true);
            let body=theme::chat_body(ui,body);
            let ui=&mut ui.new_child(egui::UiBuilder::new().max_rect(body));
            let mut guard = chat.lock().expect("chat lock");
            let chat = &mut *guard;
            if std::mem::take(&mut chat.ping) {
                ui.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            }
            ui.painter().rect_filled(ui.max_rect(), 0.0, pal::BG);
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(ui.max_rect()));
            #[cfg(windows)]
            if let Some(sender)=&service_sender {
                if !sender.connected() {inner.label(RichText::new("Reconnecting to host service…").color(pal::GOLD));}
                chat_ui_with_echo(&mut inner,&mut chat.log,&mut chat.input,false,|text|sender.chat(session,text));
            } else {chat_ui(&mut inner,&mut chat.log,&mut chat.input,|text|client.host_chat(session,text));}
            #[cfg(not(windows))]
            chat_ui(&mut inner, &mut chat.log, &mut chat.input, |text| client.host_chat(session, text));
            if ui.input(|i| i.viewport().close_requested()) {
                chat.open = false;
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }
}

/// Shared chat log + input. `send` is called with each outgoing message.
pub fn chat_ui(ui: &mut Ui, log: &mut Vec<(bool, String)>, input: &mut String, send: impl FnOnce(String)) {
    chat_ui_with_echo(ui,log,input,true,|text|{send(text);true});
}

fn chat_ui_with_echo(ui: &mut Ui, log: &mut Vec<(bool, String)>, input: &mut String, echo:bool, send: impl FnOnce(String)->bool) {
    let log_height = (ui.available_height() - 52.0).max(0.0);
    egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false])
        .max_height(log_height).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (mine, text) in log.iter() {
            let (who, color) = if *mine { ("YOU", pal::ORANGE) } else { ("REMOTE", pal::BLUE) };
            egui::Frame::new().fill(pal::PANEL).corner_radius(8.0)
                .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
                .inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(who).font(theme::display_bold(14.0)).color(color));
                    ui.label(text);
                });
        }
    });
    ui.horizontal(|ui| {
        let input_width = (ui.available_width() - 108.0).max(40.0);
        let resp = ui.add(theme::text_input(input)
            .hint_text("Transmit a message…").desired_width(input_width));
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (theme::console_button(ui, "Send", pal::ORANGE, theme::ConsoleIcon::Chat, false).clicked() || enter) && !input.trim().is_empty() {
            let text = std::mem::take(input);
            if send(text.clone()) {if echo {log.push((true,text));}} else {*input=text;}
            resp.request_focus();
        }
    });
}

/// "Links" page: all open outgoing sessions.
pub fn links_ui(app: &mut App, ui: &mut Ui) {
    theme::panel(ui, "Active links", pal::BLUE, |ui| {
        if app.tabs.is_empty() {
            ui.label(RichText::new("No active links. Use the Bridge to connect to a device.").color(pal::DIM));
            return;
        }
        let mut open = None;
        let mut window = None;
        let mut close = None;
        for shared in &app.tabs {
            let tab = shared.lock().unwrap();
            ui.horizontal(|ui| {
                let status = match &tab.closed {
                    Some(r) => (format!("TERMINATED — {r}"), pal::RED),
                    None => (format!("{:.0} FPS · {:.1} MBPS", tab.stats.fps, tab.stats.kbps / 1000.0), pal::GREEN),
                };
                ui.label(RichText::new(tab.info.host_name.to_uppercase()).font(theme::display(22.0)).color(pal::TAN));
                ui.label(RichText::new(tab.peer.to_string()).font(theme::mono(16.0)).color(pal::BLUE));
                ui.label(RichText::new(status.0).font(theme::display(16.0)).color(status.1));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::pill(ui, "End", pal::RED).clicked() {
                        close = Some(tab.id);
                    }
                    if theme::pill(ui, "View", pal::BLUE).clicked() {
                        open = Some(tab.id);
                    }
                    if !ui.ctx().embed_viewports() && theme::pill(ui, if tab.detached {"Focus window"} else {"Open window"}, pal::VIOLET).clicked() {
                        window = Some(tab.id);
                    }
                });
            });
            ui.separator();
        }
        if let Some(id) = open {
            if app.tab(id).is_some_and(|t| t.lock().unwrap().detached) { app.open_session_window(ui.ctx(), id); }
            else { app.page = Page::Remote(id); }
        }
        if let Some(id) = window { app.open_session_window(ui.ctx(), id); }
        if let Some(id) = close {
            app.close_tab(id);
        }
    });
}
