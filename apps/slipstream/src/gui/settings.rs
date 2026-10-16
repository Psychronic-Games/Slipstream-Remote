//! Config and About pages.

use eframe::egui::{self, RichText, Ui, vec2};
use slipstream_core::{config::Settings, directory::normalize_server_url};

use super::{App, theme, theme::pal};

/// Editable copy of the settings; applied on "Save".
pub struct SettingsForm {
    device_name: String,
    server: String,
    public_relays: bool,
    accept: bool,
    allow_input: bool,
    allow_clipboard: bool,
    allow_files: bool,
    password_len: usize,
    new_permanent: String,
    has_permanent: bool,
    max_fps: u32,
    sync_clipboard: bool,
    close_to_tray: bool,
    autostart: bool,
    error: Option<String>,
}

impl From<&Settings> for SettingsForm {
    fn from(s: &Settings) -> Self {
        Self {
            device_name: s.device_name.clone(),
            server: s.server.url.clone().unwrap_or_default(),
            public_relays: s.server.use_public_relays,
            accept: s.host.enabled,
            allow_input: s.host.allow_input,
            allow_clipboard: s.host.allow_clipboard,
            allow_files: s.host.allow_file_transfer,
            password_len: s.host.one_time_password_len,
            new_permanent: String::new(),
            has_permanent: s.host.permanent_password_secret.is_some(),
            max_fps: s.viewer.max_fps,
            sync_clipboard: s.viewer.sync_clipboard,
            close_to_tray: s.app.close_to_tray,
            autostart: super::autostart::is_enabled(),
            error: None,
        }
    }
}

impl SettingsForm {
    pub fn refresh_recent(&mut self, _s: &Settings) {}
}

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui(vec2(210.0, 30.0), |ui| {
            ui.set_width(210.0);
            ui.label(RichText::new(label.to_uppercase()).font(theme::display(16.0)).color(pal::LAVENDER));
        });
        add(ui);
    });
}

fn text_field(ui: &mut Ui, value: &mut String, hint: &str, password: bool) -> egui::Response {
    theme::input_frame()
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .password(password)
                    .frame(egui::Frame::NONE)
                    .desired_width(ui.available_width().min(520.0)),
            )
        })
        .inner
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    let mut reset_permission = false;
    let f = &mut app.settings_form;
    theme::panel(ui, "Station identity", pal::LAVENDER, |ui| {
        row(ui, "Device name", |ui| {
            text_field(ui, &mut f.device_name, "My Computer", false);
        });
    });
    ui.add_space(14.0);

    theme::panel(ui, "Network", pal::BLUE, |ui| {
        ui.label(
            RichText::new(
                "The server hands out IDs and relays traffic when a direct link is impossible. \
                 Enter its web address — e.g. relay.example.com. See docs/SERVER.md to run your own.",
            )
            .color(pal::DIM),
        );
        ui.add_space(6.0);
        row(ui, "Server address", |ui| {
            text_field(ui, &mut f.server, "relay.example.com", false);
        });
        row(ui, "Public fallback relays", |ui| {
            ui.checkbox(&mut f.public_relays, "Also use the free n0 relays (applies after restart)");
        });
    });
    ui.add_space(14.0);

    theme::panel(ui, "Incoming access", pal::ORANGE, |ui| {
        row(ui, "Accept connections", |ui| {
            ui.checkbox(&mut f.accept, "Allow others to connect to this device");
        });
        row(ui, "Permissions", |ui| {
            ui.checkbox(&mut f.allow_input, "Control");
            ui.checkbox(&mut f.allow_clipboard, "Clipboard");
            ui.checkbox(&mut f.allow_files, "Files");
        });
        row(ui, "One-time password length", |ui| {
            ui.add(egui::Slider::new(&mut f.password_len, 4..=12));
        });
        row(ui, "Unattended password", |ui| {
            text_field(ui, &mut f.new_permanent, if f.has_permanent { "•••••••• (set — type to replace)" } else { "not set" }, true);
            if f.has_permanent && theme::pill(ui, "Remove", pal::RED).clicked() {
                f.has_permanent = false;
                f.new_permanent.clear();
            }
        });
        ui.label(
            RichText::new("A permanent password lets you reach this device when nobody is at it. Use a long one — it is never stored in plain text.")
                .small()
                .color(pal::DIM),
        );
        row(ui, "Start with system", |ui| {
            ui.checkbox(&mut f.autostart, "Launch hidden in the tray when you log in");
        });
        row(ui, "Close button", |ui| {
            ui.checkbox(&mut f.close_to_tray, "Keep running in the tray (stay reachable)");
        });
        row(ui, "Screen permission", |ui| {
            if theme::pill(ui, "Reset", pal::VIOLET).clicked() {
                reset_permission = true;
            }
        });
    });
    ui.add_space(14.0);
    if reset_permission {
        app.client.update_settings(|s| s.host.portal_restore_token = None);
        app.toast("Screen-sharing permission will be asked again next session", pal::VIOLET);
    }

    let f = &mut app.settings_form;
    theme::panel(ui, "Viewing", pal::GOLD, |ui| {
        row(ui, "Max frame rate", |ui| {
            ui.add(egui::Slider::new(&mut f.max_fps, 10..=144).suffix(" fps"));
        });
        row(ui, "Clipboard sync", |ui| {
            ui.checkbox(&mut f.sync_clipboard, "Share clipboard text with remote devices");
        });
    });
    ui.add_space(16.0);

    if let Some(e) = &f.error {
        ui.label(RichText::new(e).color(pal::RED));
    }
    ui.horizontal(|ui| {
        if theme::pill_sized(ui, "Save configuration", pal::ORANGE, vec2(220.0, 40.0)).clicked() {
            save(app);
        }
        if theme::pill_sized(ui, "Revert", pal::LAVENDER, vec2(120.0, 40.0)).clicked() {
            app.settings_form = SettingsForm::from(&app.client.settings());
        }
    });
}

fn save(app: &mut App) {
    let f = &mut app.settings_form;
    let server: Option<String> = if f.server.trim().is_empty() {
        None
    } else {
        match normalize_server_url(&f.server) {
            Ok(_) => Some(f.server.trim().to_string()),
            Err(e) => {
                f.error = Some(format!("Server address: {e}"));
                return;
            }
        }
    };
    if !f.new_permanent.is_empty() && f.new_permanent.chars().count() < 8 {
        f.error = Some("The unattended password must be at least 8 characters".into());
        return;
    }
    f.error = None;
    let new_permanent = std::mem::take(&mut f.new_permanent);
    let has_permanent = f.has_permanent || !new_permanent.is_empty();
    f.has_permanent = has_permanent;
    let f = &app.settings_form;
    let autostart = f.autostart;
    if autostart != super::autostart::is_enabled()
        && let Err(e) = super::autostart::set_enabled(autostart)
    {
        app.settings_form.error = Some(format!("Autostart: {e:#}"));
        return;
    }
    let f = &app.settings_form;
    let close_to_tray = f.close_to_tray;
    let (name, public, accept, input, clip, files, len, fps, sync) = (
        f.device_name.trim().to_string(),
        f.public_relays,
        f.accept,
        f.allow_input,
        f.allow_clipboard,
        f.allow_files,
        f.password_len,
        f.max_fps,
        f.sync_clipboard,
    );
    app.client.update_settings(|s| {
        if !name.is_empty() {
            s.device_name = name;
        }
        s.server.url = server;
        s.server.use_public_relays = public;
        s.host.enabled = accept;
        s.host.allow_input = input;
        s.host.allow_clipboard = clip;
        s.host.allow_file_transfer = files;
        s.host.one_time_password_len = len;
        if !new_permanent.is_empty() {
            s.host.set_permanent_password(Some(&new_permanent));
        } else if !has_permanent {
            s.host.set_permanent_password(None);
        }
        s.viewer.max_fps = fps;
        s.viewer.sync_clipboard = sync;
        s.app.close_to_tray = close_to_tray;
    });
    app.toast("Configuration saved", pal::GREEN);
}

pub fn about_ui(app: &mut App, ui: &mut Ui) {
    theme::panel(ui, "Slipstream Remote", pal::GOLD, |ui| {
        ui.label(RichText::new(format!("VERSION {}", slipstream_core::APP_VERSION)).font(theme::display(20.0)).color(pal::TAN));
        ui.add_space(6.0);
        ui.label("Fast, free and open-source remote desktop. No accounts, no subscriptions.");
        ui.label("Every session is end-to-end encrypted; servers only ever see ciphertext.");
        ui.add_space(10.0);
        ui.label(RichText::new("STATION KEY").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.label(RichText::new(app.client.endpoint_id().to_string()).font(theme::mono(14.0)).color(pal::BLUE));
        ui.add_space(10.0);
        ui.label(RichText::new("LICENSE").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.label("MIT License. Fonts: Antonio, Barlow Semi Condensed and Share Tech Mono under the SIL Open Font License 1.1.");
        ui.label("Built with iroh, egui, OpenH264 (BSD) and many other open-source crates.");
    });
}
