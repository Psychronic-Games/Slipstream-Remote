//! Config and About pages.

use eframe::egui::{self, RichText, Ui, vec2};
use slipstream_core::{config::Settings, directory::normalize_server_url};

use super::{App, theme, theme::pal};

/// Editable copy of the settings; applied on "Save".
pub struct SettingsForm {
    device_name: String,
    server: String,
    key: String,
    public_relays: bool,
    accept: bool,
    allow_input: bool,
    allow_clipboard: bool,
    allow_files: bool,
    allow_audio: bool,
    password_len: usize,
    new_permanent: String,
    has_permanent: bool,
    max_fps: u32,
    sync_clipboard: bool,
    play_audio: bool,
    run_in_tray: bool,
    autostart: bool,
    error: Option<String>,
}

impl From<&Settings> for SettingsForm {
    fn from(s: &Settings) -> Self {
        Self {
            device_name: s.device_name.clone(),
            server: s.server.url.clone().unwrap_or_default(),
            key: s.server.key.clone().unwrap_or_default(),
            public_relays: s.server.use_public_relays,
            accept: s.host.enabled,
            allow_input: s.host.allow_input,
            allow_clipboard: s.host.allow_clipboard,
            allow_files: s.host.allow_file_transfer,
            allow_audio: s.host.allow_audio,
            password_len: s.host.one_time_password_len,
            new_permanent: String::new(),
            has_permanent: s.host.permanent_password_secret.is_some(),
            max_fps: s.viewer.max_fps,
            sync_clipboard: s.viewer.sync_clipboard,
            play_audio: s.viewer.play_audio,
            run_in_tray: s.app.run_in_tray,
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
    let mut link_now = false;
    let linking = app.linking();
    let network_help = match &app.org {
        Some(org) => format!(
            "Set up by {org}'s station when you downloaded this app. Change these only if {org} asks you to."
        ),
        None => "Your station gives this device its ID, and relays the link when the two computers can't reach \
                 each other directly. Enter its web address, e.g. support.example.com."
            .to_string(),
    };
    let f = &mut app.settings_form;
    theme::panel(ui, "Device identity", pal::LAVENDER, |ui| {
        row(ui, "Device name", |ui| {
            text_field(ui, &mut f.device_name, "My Computer", false);
        });
    });
    ui.add_space(14.0);

    theme::panel(ui, "Network", pal::BLUE, |ui| {
        ui.label(RichText::new(network_help).color(pal::DIM));
        ui.add_space(6.0);
        row(ui, "Server address", |ui| {
            text_field(ui, &mut f.server, "support.example.com", false);
        });
        row(ui, "Access key", |ui| {
            text_field(ui, &mut f.key, "only for private stations", true);
        });
        row(ui, "Setup code", |ui| {
            ui.horizontal(|ui| {
                let r = text_field(ui, &mut app.link_code, "from the station's download page (fills in the key)", false);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let label = if linking { "Connecting…" } else { "Connect" };
                link_now = ui.add_enabled_ui(!linking, |ui| theme::pill(ui, label, pal::BLUE)).inner.clicked() || enter;
            });
        });
        row(ui, "Public fallback relays", |ui| {
            ui.checkbox(&mut f.public_relays, "Also use the free public relays (applies after restart)");
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
            ui.checkbox(&mut f.allow_audio, "Sound");
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
            if ui.checkbox(&mut f.autostart, "Launch hidden in the tray when you log in").changed() && f.autostart {
                f.run_in_tray = true; // autostart only makes sense if it keeps running
            }
        });
        row(ui, "Close button", |ui| {
            ui.checkbox(&mut f.run_in_tray, "Keep running in the tray when the window is closed (stay reachable)");
        });
        row(ui, "Screen permission", |ui| {
            if theme::pill(ui, "Reset", pal::VIOLET).clicked() {
                reset_permission = true;
            }
        });
    });
    ui.add_space(14.0);
    if link_now {
        let code = slipstream_core::provision::normalize_code(&app.link_code);
        let station = app.settings_form.server.trim().to_string();
        if station.is_empty() || code.len() != slipstream_core::provision::CODE_LEN {
            app.toast("Enter the server address above and the 8-character setup code", pal::RED);
        } else {
            let ctx = ui.ctx().clone();
            app.start_link(&ctx, slipstream_core::provision::StationLink { station, code }, true);
        }
    }
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
        row(ui, "Remote sound", |ui| {
            ui.checkbox(&mut f.play_audio, "Play the other computer's sound when a link starts")
                .on_hover_text("You can still switch it on or off during a link with the SOUND button");
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
    let run_in_tray = f.run_in_tray;
    let key = Some(f.key.trim().to_string()).filter(|k| !k.is_empty());
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
    let (allow_audio, play_audio) = (f.allow_audio, f.play_audio);
    app.client.update_settings(|s| {
        if !name.is_empty() {
            s.device_name = name;
        }
        if s.server.url != server || s.server.key != key {
            s.server.provisioned = false;
        }
        s.server.url = server;
        s.server.key = key;
        s.server.use_public_relays = public;
        s.host.enabled = accept;
        s.host.allow_input = input;
        s.host.allow_clipboard = clip;
        s.host.allow_file_transfer = files;
        s.host.allow_audio = allow_audio;
        s.host.one_time_password_len = len;
        if !new_permanent.is_empty() {
            s.host.set_permanent_password(Some(&new_permanent));
        } else if !has_permanent {
            s.host.set_permanent_password(None);
        }
        s.viewer.max_fps = fps;
        s.viewer.sync_clipboard = sync;
        s.viewer.play_audio = play_audio;
        s.app.run_in_tray = run_in_tray;
    });
    app.toast("Configuration saved", pal::GREEN);
}

pub fn about_ui(app: &mut App, ui: &mut Ui) {
    let logo = app.logo_texture(ui.ctx());
    theme::panel(ui, "Slipstream Remote", pal::GOLD, |ui| {
        ui.add(egui::Image::new((logo, vec2(120.0, 120.0))));
        ui.label(RichText::new(format!("VERSION {}", slipstream_core::version_string())).font(theme::display(20.0)).color(pal::TAN));
        ui.horizontal(|ui| {
            if theme::pill(ui, "Check for updates", pal::GREEN).clicked() {
                app.check_update_now(ui.ctx());
            }
            ui.label(RichText::new("Updates come from your station.").small().color(pal::DIM));
        });
        ui.add_space(6.0);
        ui.label("Fast, free and open-source remote desktop. No accounts, no subscriptions.");
        ui.label("Every session is end-to-end encrypted; servers only ever see ciphertext.");
        ui.add_space(10.0);
        ui.label(RichText::new("DEVICE KEY").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.label(RichText::new(app.client.endpoint_id().to_string()).font(theme::mono(14.0)).color(pal::BLUE));
        ui.add_space(10.0);
        ui.label(RichText::new("LICENSE").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.label("MIT License. Fonts: Antonio, Barlow Semi Condensed and Share Tech Mono under the SIL Open Font License 1.1.");
        ui.label("Built with iroh, egui, OpenH264 (BSD) and many other open-source crates.");
    });
}
