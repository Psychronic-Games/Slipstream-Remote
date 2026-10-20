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
    allow_tunnels: bool,
    allow_network_scan: bool,
    allow_wake_on_lan: bool,
    password_len: usize,
    new_permanent: String,
    has_permanent: bool,
    max_fps: u32,
    sync_clipboard: bool,
    play_audio: bool,
    separate_windows: bool,
    run_in_tray: bool,
    service_tray: bool,
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
            allow_tunnels: s.host.allow_tunnels,
            allow_network_scan: s.host.allow_network_scan,
            allow_wake_on_lan: s.host.allow_wake_on_lan,
            password_len: s.host.one_time_password_len,
            new_permanent: String::new(),
            has_permanent: s.host.permanent_password_secret.is_some(),
            max_fps: s.viewer.max_fps,
            sync_clipboard: s.viewer.sync_clipboard,
            play_audio: s.viewer.play_audio,
            separate_windows: s.viewer.separate_windows,
            run_in_tray: s.app.run_in_tray,
            service_tray: !s.app.hide_tray_with_service,
            autostart: super::autostart::is_enabled(),
            error: None,
        }
    }
}

impl SettingsForm {
    pub fn refresh_recent(&mut self, _s: &Settings) {}
}

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    // On a phone the label goes above what it labels.
    if ui.available_width() < 480.0 {
        ui.add_space(2.0);
        ui.label(RichText::new(label.to_uppercase()).font(theme::display(16.0)).color(pal::LAVENDER));
        ui.vertical(add);
        ui.add_space(4.0);
        return;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui(vec2(210.0, 30.0), |ui| {
            ui.set_width(210.0);
            ui.label(RichText::new(label.to_uppercase()).font(theme::display(16.0)).color(pal::LAVENDER));
        });
        add(ui);
    });
}

fn text_field(ui: &mut Ui, value: &mut String, hint: &str, password: bool) -> egui::Response {
    let width = ui.available_width().min(520.0);
    text_field_sized(ui, value, hint, password, width)
}

fn text_field_sized(ui: &mut Ui, value: &mut String, hint: &str, password: bool, width: f32) -> egui::Response {
    theme::input_frame()
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(value).align(egui::Align2::LEFT_CENTER)
                    .hint_text(hint)
                    .password(password)
                    .frame(egui::Frame::NONE)
                    .desired_width(width),
            )
        })
        .inner
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    #[cfg(target_os = "linux")]
    let mut approve_screen = None;
    let mut link_now = false;
    let linking = app.linking();
    #[cfg(windows)]
    let (via_service, service_busy) = (app.via_service, app.service_busy());
    #[cfg(windows)]
    let mut service_clicked: Option<(bool, bool)> = None;
    let network_help = match &app.org {
        Some(org) => format!(
            "Set up by {org}'s station when you downloaded this app. Change these only if {org} asks you to."
        ),
        None => "Your station gives this device its ID, and relays the link when the two computers can't reach \
                 each other directly. Enter its web address, e.g. support.example.com."
            .to_string(),
    };
    super::saved::stations_panel(app, ui);
    ui.add_space(14.0);
    let f = &mut app.settings_form;
    theme::panel(ui, "Device identity", pal::LAVENDER, |ui| {
        row(ui, "Device name", |ui| {
            text_field(ui, &mut f.device_name, "My Computer", false);
        });
        #[cfg(not(target_os="android"))]
        row(ui, "Cloned-device detection", |ui| {
            let ready=slipstream_core::config::clone_detection_available();
            ui.label(RichText::new(if ready {"Automatic · firmware UUID/serial available"}else{"Unavailable · no reliable firmware UUID/serial readable; existing ID retained"}).small().color(if ready {pal::GREEN}else{pal::GOLD}));
        });
        #[cfg(not(target_os="android"))]
        ui.label(RichText::new("Before capturing a master image, run this updated app and update/start its installed service once. A clone with different firmware identifiers gets a new ID at startup while retaining station/unattended settings. Identical or unavailable firmware identifiers cannot establish a different device.").small().color(pal::DIM));
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
                // Leave room for the button.
                let width = (ui.available_width() - 130.0).min(520.0);
                let r = text_field_sized(ui, &mut app.link_code, "from the station's download page (fills in the key)", false, width);
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

    {
        theme::panel(ui, "Incoming access", pal::ORANGE, |ui| {
            row(ui, "Accept connections", |ui| {
                ui.checkbox(&mut f.accept, "Allow others to connect to this device");
            });
            row(ui, "Permissions", |ui| { ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut f.allow_input, "Control");
                #[cfg(not(target_os="android"))]
                {
                ui.checkbox(&mut f.allow_clipboard, "Clipboard");
                ui.checkbox(&mut f.allow_files, "Files");
                ui.checkbox(&mut f.allow_audio, "Sound");
                ui.checkbox(&mut f.allow_wake_on_lan, "Wake-on-LAN").on_hover_text("Let authenticated controllers wake other devices through a selected LAN adapter; also requires Network scan and Control");
                ui.checkbox(&mut f.allow_network_scan, "Network scan").on_hover_text("Let authenticated viewers discover devices and open TCP ports on this computer's connected IPv4 networks");
                }
                ui.checkbox(&mut f.allow_tunnels, "Port forwarding").on_hover_text(
                    "Let the person connecting reach services on this computer and its network through the link (e.g. Remote Desktop). Off unless you need it.",
                );
            }); });
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
                RichText::new(if cfg!(target_os="android") {"A saved password still requires Start sharing and Android approval on the phone. It does not bypass screen-sharing consent."}else{"A permanent password lets you reach this device when nobody is at it. Use a long one — it is never stored in plain text."})
                    .small()
                    .color(pal::DIM),
            );
            #[cfg(not(target_os="android"))]
            row(ui, "Start with system", |ui| {
                if ui.checkbox(&mut f.autostart, "Launch hidden in the tray when you log in").changed() && f.autostart {
                    f.run_in_tray = true; // autostart only makes sense if it keeps running
                }
            });
            #[cfg(not(target_os="android"))]
            row(ui, "Close button", |ui| {
                ui.checkbox(&mut f.run_in_tray, "Keep running in the tray when the window is closed (stay reachable)");
            });
            #[cfg(target_os = "linux")]
            row(ui, "Screen permission", |ui| { ui.vertical(|ui| {
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    let busy = app.screen_access.busy();
                    ui.add_enabled_ui(!busy && f.accept && app.incoming.is_empty(), |ui| { ui.horizontal_wrapped(|ui| {
                        if theme::pill(ui, "Approve now", pal::GREEN).clicked() {
                            approve_screen = Some(false);
                        }
                        if theme::pill(ui, "Reset / approve again", pal::VIOLET).clicked() {
                            approve_screen = Some(true);
                        }
                    }); });
                    let (text, color) = if busy {
                        ("Waiting for your desktop's screen-sharing approval…", pal::GOLD)
                    } else {
                        match &app.screen_access.result {
                            Some(Ok(true)) => ("Ready · permission saved for future starts", pal::GREEN),
                            Some(Ok(false)) => ("Ready while running · this desktop did not save permission", pal::GREEN),
                            Some(Err(error)) => (error.as_str(), pal::RED),
                            None => ("Approve access before someone connects", pal::DIM),
                        }
                    };
                    ui.label(RichText::new(text).small().color(color));
                } else {
                    ui.label(RichText::new("Available automatically on X11, including while running in the tray").small().color(pal::DIM));
                }
            }); });
            #[cfg(target_os = "linux")]
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                ui.label(RichText::new("Screen access is prepared when Slipstream starts and remains available in the tray. Your desktop decides whether approval survives a restart or reboot.").small().color(pal::DIM));
            }
            #[cfg(windows)]
            row(ui, "Screen access", |ui| {
                let text = if via_service {
                    "The Windows service provides background screen access, including the login screen and administrator prompts. No screen-sharing approval is needed."
                } else {
                    "Available automatically while Slipstream runs, including in the tray. Install the Windows service below for access before sign-in and to administrator prompts."
                };
                ui.label(RichText::new(text).small().color(pal::DIM));
            });
            #[cfg(target_os = "macos")]
            row(ui, "Screen access", |ui| {
                ui.label(RichText::new("Allow Slipstream in System Settings → Privacy & Security → Screen Recording and Accessibility before connecting. Access stays available while Slipstream runs in the tray.").small().color(pal::DIM));
            });
            #[cfg(windows)]
            row(ui, "Windows service", |ui| {
                let tip = "Unattended access including UAC prompts, elevated windows and the login screen. Needs administrator rights and an unattended password.";
                ui.add_enabled_ui(!service_busy, |ui| {
                    if service_busy {
                        theme::pill(ui, "Working…", pal::BLUE);
                    } else if crate::service::is_running() {
                        if theme::pill(ui, "Update", pal::BLUE).on_hover_text("Give the service this page's saved settings").clicked() {
                            service_clicked = Some((true, false));
                        }
                        if theme::pill(ui, "Remove", pal::RED).clicked() {
                            service_clicked = Some((false, false));
                        }
                    } else if crate::service::installed() {
                        // Installed but not running (stopped, or failed to start).
                        if theme::pill(ui, "Start", pal::BLUE).on_hover_text("Start the service (reinstalling it if needed)").clicked() {
                            service_clicked = Some((true, true));
                        }
                        if theme::pill(ui, "Reinstall", pal::VIOLET).on_hover_text("Install this version of the service again and start it").clicked() {
                            service_clicked = Some((true, false));
                        }
                        if theme::pill(ui, "Remove", pal::RED).clicked() {
                            service_clicked = Some((false, false));
                        }
                    } else if theme::pill(ui, "Install", pal::BLUE).on_hover_text(tip).clicked() {
                        service_clicked = Some((true, false));
                    }
                });
                let status = crate::service::status_text();
                ui.label(RichText::new(status).color(if status == "Running" { pal::GREEN } else { pal::DIM }));
                if crate::service::installed() {
                    let log = crate::service::log_to_show();
                    if ui.link(RichText::new("Service log").small()).on_hover_text(log.display().to_string()).clicked() {
                        let _ = std::process::Command::new("notepad").arg(log).spawn();
                    }
                }
            });
            #[cfg(windows)]
            if let Some(why) = crate::service::start_problem() {
                // Windows' own explanation, to read or paste into a ticket.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("WHY THE SERVICE ISN'T RUNNING").font(theme::display(14.0)).color(pal::PEACH));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(why.clone());
                    }
                });
                let mut text = why.as_str();
                ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY).desired_rows(3).font(egui::TextStyle::Small));
            }
            #[cfg(windows)]
            if via_service {
                row(ui, "Tray icon", |ui| {
                    ui.checkbox(&mut f.service_tray, "Show Slipstream in the tray while the service runs (from sign-in)");
                });
                ui.label(
                    RichText::new("The service keeps its own copy of these settings: after saving, press Update to give it the new ones.")
                        .small()
                        .color(pal::DIM),
                );
            }
        });
        ui.add_space(14.0);
    }
    #[cfg(windows)]
    if let Some((install, start_first)) = service_clicked {
        let ctx = ui.ctx().clone();
        app.service_action(&ctx, install, start_first);
    }
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
    #[cfg(target_os = "linux")]
    if let Some(reset) = approve_screen {
        app.screen_access.start(&app.rt, &app.client, ui.ctx(), reset);
    }

    let f = &mut app.settings_form;
    theme::panel(ui, "Viewing", pal::GOLD, |ui| {
        #[cfg(not(target_os = "android"))]
        row(ui, "New links", |ui| {
            egui::ComboBox::from_id_salt("new-link-layout")
                .selected_text(if f.separate_windows {"Separate windows"} else {"Tabs"})
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.separate_windows, false, "Tabs");
                    ui.selectable_value(&mut f.separate_windows, true, "Separate windows");
                });
        });
        #[cfg(not(target_os = "android"))]
        ui.label(RichText::new("Drag tabs to reorder, drag out for a window, or drop a floating tab back onto the main tab strip. You can also use Pop out / Dock.").small().color(pal::DIM));
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
    let service_tray = f.service_tray;
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
    let (allow_audio, play_audio, allow_tunnels) = (f.allow_audio, f.play_audio, f.allow_tunnels);
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
        s.host.allow_tunnels = allow_tunnels;
        s.host.allow_network_scan = f.allow_network_scan;
        s.host.allow_wake_on_lan = f.allow_wake_on_lan;
        s.host.one_time_password_len = len;
        if !new_permanent.is_empty() {
            s.host.set_permanent_password(Some(&new_permanent));
        } else if !has_permanent {
            s.host.set_permanent_password(None);
        }
        s.viewer.max_fps = fps;
        s.viewer.sync_clipboard = sync;
        s.viewer.play_audio = play_audio;
        s.viewer.separate_windows = f.separate_windows;
        s.app.run_in_tray = run_in_tray;
        s.app.hide_tray_with_service = !service_tray;
    });
    app.toast("Configuration saved", pal::GREEN);
}

pub fn about_ui(app: &mut App, ui: &mut Ui) {
    let logo = app.logo_texture(ui.ctx());
    theme::panel(ui, "Slipstream Remote", pal::GOLD, |ui| {
        ui.add(egui::Image::new((logo, vec2(120.0, 120.0))));
        ui.label(RichText::new(format!("VERSION {}", slipstream_core::version_string())).font(theme::display(20.0)).color(pal::TAN));
        ui.horizontal(|ui| {
            let checking = app.checking_update();
            let label = if checking { "Checking…" } else { "Check for updates" };
            if ui.add_enabled_ui(!checking, |ui| theme::pill(ui, label, pal::GREEN)).inner.clicked() {
                app.check_update_now(ui.ctx());
            }
            ui.label(RichText::new("Updates come from your station.").small().color(pal::DIM));
        });
        // Found one: install it from here (it also shows on the Bridge).
        if app.update.is_some() {
            ui.add_space(6.0);
            super::home::update_banner(app, ui);
        }
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
