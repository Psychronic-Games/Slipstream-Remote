//! Bridge (home) page: this station's ID/password, connect form, recents.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, vec2};
use slipstream_session::DirStatus;

use super::{App, Page, theme, theme::pal};

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

    if ui.available_width() > 820.0 {
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
    recent_panel(app, ui);
}

fn station_panel(app: &mut App, ui: &mut Ui, t: f64) {
    theme::panel(ui, "This station", pal::LAVENDER, |ui| {
        ui.label(RichText::new("Give your partner this ID and password to let them in.").color(pal::DIM));
        ui.add_space(8.0);

        let (status, color) = match &app.dir_status {
            DirStatus::Online { .. } => ("ONLINE · READY FOR LINK", pal::GREEN),
            DirStatus::Connecting | DirStatus::Ready => ("CONTACTING SERVER…", pal::GOLD),
            DirStatus::NotConfigured => ("NO SERVER — SEE CONFIG", pal::RED),
            DirStatus::Error(_) => ("SERVER CONNECTION FAILED", pal::RED),
        };
        let id_text = match &app.dir_status {
            DirStatus::Online { id } => id.to_string(),
            _ => app.client.settings().host.last_device_id.map(|i| i.to_string()).unwrap_or_else(|| "--- --- ---".into()),
        };

        ui.label(RichText::new("YOUR ID").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.horizontal(|ui| {
            ui.label(RichText::new(&id_text).font(theme::mono(44.0)).color(pal::TAN));
            ui.add_space(8.0);
            if theme::pill_sized(ui, "Copy", pal::LAVENDER, vec2(70.0, 28.0)).clicked() {
                ui.ctx().copy_text(id_text.replace(' ', ""));
                app.toast("ID copied", pal::LAVENDER);
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new("ONE-TIME PASSWORD").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.horizontal(|ui| {
            ui.label(RichText::new(&app.password).font(theme::mono(34.0)).color(pal::GOLD));
            ui.add_space(8.0);
            if theme::pill_sized(ui, "Copy", pal::LAVENDER, vec2(70.0, 28.0)).clicked() {
                ui.ctx().copy_text(app.password.clone());
                app.toast("Password copied", pal::LAVENDER);
            }
            if theme::pill_sized(ui, "New", pal::VIOLET, vec2(64.0, 28.0)).clicked() {
                app.password = app.client.regenerate_password();
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            theme::lamp(ui.painter(), rect.center(), color, t);
            ui.label(RichText::new(status).font(theme::display(17.0)).color(color));
        });
        if let DirStatus::Error(e) = &app.dir_status {
            ui.label(RichText::new(e).font(theme::body(16.0)).color(pal::PEACH));
            if let Ok(store) = slipstream_core::config::Store::open_default() {
                let log = store.dir().join("slipstream.log").display().to_string();
                ui.label(RichText::new("Log file:").small().color(pal::DIM));
                ui.add(egui::Label::new(RichText::new(&log).small().monospace().color(pal::DIM)).truncate())
                    .on_hover_text(&log);
            }
        }
        if app.client.settings().host.permanent_password_secret.is_some() {
            ui.label(RichText::new("Unattended access enabled — your permanent password also works.").small().color(pal::DIM));
        }
    });
}

fn connect_panel(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    theme::panel(ui, "Establish link", pal::ORANGE, |ui| {
        ui.label(RichText::new("Control another device.").color(pal::DIM));
        ui.add_space(8.0);
        ui.label(RichText::new("PARTNER ID").font(theme::display(15.0)).color(pal::ORANGE));
        let id_resp = theme::input_frame()
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.connect_id)
                        .hint_text("482 913 077")
                        .font(theme::mono(30.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner;
        ui.add_space(4.0);
        ui.label(RichText::new("PASSWORD").font(theme::display(15.0)).color(pal::ORANGE));
        let pw_resp = theme::input_frame()
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.connect_pw)
                        .hint_text("partner's password")
                        .password(true)
                        .font(theme::mono(24.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner;
        let enter = (id_resp.lost_focus() || pw_resp.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if id_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && app.connect_pw.is_empty() {
            pw_resp.request_focus();
        }
        ui.add_space(12.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 44.0), Layout::right_to_left(Align::Center), |ui| {
            let busy = app.attempt.is_some();
            let engage = ui.add_enabled_ui(!busy, |ui| theme::pill_sized(ui, "Engage", pal::ORANGE, vec2(150.0, 40.0))).inner;
            if (engage.clicked() || (enter && !app.connect_pw.is_empty())) && !busy {
                app.start_connect(&ctx);
            }
        });
    });
}

fn recent_panel(app: &mut App, ui: &mut Ui) {
    let recent = app.client.settings().recent;
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
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    app.connect_id = r.id.to_string();
                    app.connect_pw.clear();
                    app.toast(format!("Enter the password for {}", r.name), pal::BLUE);
                }
            }
        });
    });
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
                    RichText::new(format!("{} is controlling this station ({}s)", s.peer, s.started.elapsed().as_secs()))
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
            app.client.disconnect_host_session(id);
        }
        if let Some(id) = chat
            && let Some(s) = app.incoming.iter_mut().find(|s| s.id == id)
        {
            s.show_chat = !s.show_chat;
        }
    });
}

/// Floating chat windows for incoming sessions (shown on any page).
pub fn incoming_windows(app: &mut App, ctx: &egui::Context) {
    let client = app.client.clone();
    for s in &mut app.incoming {
        if !s.show_chat {
            continue;
        }
        let mut open = true;
        egui::Window::new(format!("CHAT · {}", s.peer.to_uppercase()))
            .id(egui::Id::unique(("incoming-chat", s.id)))
            .open(&mut open)
            .default_size([360.0, 300.0])
            .show(ctx, |ui| {
                chat_ui(ui, &mut s.chat, &mut s.chat_input, |text| client.host_chat(s.id, text));
            });
        s.show_chat = open;
    }
}

/// Shared chat log + input. `send` is called with each outgoing message.
pub fn chat_ui(ui: &mut Ui, log: &mut Vec<(bool, String)>, input: &mut String, send: impl FnOnce(String)) {
    egui::ScrollArea::vertical().stick_to_bottom(true).max_height(ui.available_height() - 48.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (mine, text) in log.iter() {
            let (who, color) = if *mine { ("YOU", pal::ORANGE) } else { ("THEM", pal::BLUE) };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(who).font(theme::display(14.0)).color(color));
                ui.label(text);
            });
        }
    });
    ui.horizontal(|ui| {
        let resp = theme::input_frame()
            .show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(input).hint_text("message").frame(egui::Frame::NONE).desired_width(ui.available_width() - 80.0))
            })
            .inner;
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (theme::pill(ui, "Send", pal::ORANGE).clicked() || enter) && !input.trim().is_empty() {
            let text = std::mem::take(input);
            log.push((true, text.clone()));
            send(text);
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
        let mut close = None;
        for tab in &app.tabs {
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
                });
            });
            ui.separator();
        }
        if let Some(id) = open {
            app.page = Page::Remote(id);
        }
        if let Some(id) = close {
            app.close_tab(id);
        }
    });
}
