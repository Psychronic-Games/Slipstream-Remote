//! The Manage menu on saved devices: restarting them (normally, into Safe
//! Mode with Networking, into the BIOS setup), waking them through a gateway
//! device on their network, and their management controller (console and
//! power) through that gateway. See `slipstream_session::management`.

use eframe::egui::{self, Response, RichText};
use slipstream_core::{
    account::{ControllerKind, Management, SavedDevice},
    directory::DeviceId,
    proto::PowerAction,
};
use slipstream_session::{
    AccountRef, Presence,
    management::{self, Console, ControllerPower},
};
use tokio::sync::oneshot;

use super::{App, theme, theme::pal};

#[derive(Default)]
pub struct Manage {
    /// One background job at a time: what it is, and its outcome.
    job: Option<(String, oneshot::Receiver<Result<Outcome, String>>)>,
    /// Asked for, waiting for its confirmation.
    confirm: Option<Pending>,
    /// Open management consoles: the gateway link and the forward live as
    /// long as these.
    consoles: Vec<OpenConsole>,
}

struct OpenConsole {
    account: AccountRef,
    device: DeviceId,
    _gateway: slipstream_session::ViewerSession,
    console: Console,
}

enum Outcome {
    Message(String),
    Console(Box<OpenConsole>),
}

#[derive(Clone)]
enum Action {
    Wake,
    Restart(PowerAction),
    Console,
    Power(ControllerPower),
}

struct Pending {
    account: AccountRef,
    device: SavedDevice,
    action: Action,
}

/// Everything needed to act on a device, from its saved details.
struct Target {
    account: AccountRef,
    device: SavedDevice,
    name: String,
    management: Management,
    gateway_name: Option<String>,
}

fn target(app: &App, account: &AccountRef, device: &SavedDevice) -> Target {
    // Remote management lives on the technician's own entry for the device.
    let (management, gateway_name) = app.client.with_settings(|s| {
        let a = s.accounts.iter().find(|a| AccountRef::of(a) == *account);
        let management = a.and_then(|a| a.devices.iter().find(|d| d.id == device.id))
            .and_then(|d| d.management.clone()).unwrap_or_default();
        let gateway_name = management.gateway.map(|g| {
            a.and_then(|a| a.devices.iter().chain(&a.shared).find(|d| d.id == g && !d.name.is_empty()))
                .map(|d| d.name.clone()).unwrap_or_else(|| g.to_string())
        });
        (management, gateway_name)
    });
    let name = if device.name.is_empty() { device.id.to_string() } else { device.name.clone() };
    Target { account: account.clone(), device: device.clone(), name, management, gateway_name }
}

/// A management console is open for this device (its Manage button says so).
pub fn console_open(app: &App, account: &AccountRef, id: DeviceId) -> bool {
    app.manage.consoles.iter().any(|c| c.account == *account && c.device == id)
}

/// The menu of a saved device's Manage button.
pub fn menu_for(app: &mut App, response: &Response, account: &AccountRef, device: &SavedDevice) {
    let t = target(app, account, device);
    let open = console_open(app, account, device.id);
    menu(app, response, t, open);
}

fn menu(app: &mut App, response: &Response, t: Target, console_open: bool) {
    let online = matches!(super::saved::presence(app, &t.account, t.device.id), Some(Presence::Online));
    let busy = app.manage.job.is_some();
    let mut chosen = None;
    let mut close_console = false;
    let mut copy_password = false;
    let mut set_up = false;
    theme::console_menu(response).show(|ui| {
        ui.set_min_width(260.0);
        ui.label(RichText::new(t.name.to_uppercase()).font(theme::display(15.0)).color(pal::PEACH));
        for action in [PowerAction::Restart, PowerAction::RestartSafeMode, PowerAction::RestartToFirmware] {
            if ui.add_enabled(online && !busy, egui::Button::new(format!("{}…", action.label())))
                .on_disabled_hover_text("It has to be online (and its password saved)")
                .clicked()
            {
                chosen = Some(Action::Restart(action));
            }
        }
        ui.separator();
        let gateway = t.gateway_name.as_deref();
        let wake = gateway.is_some() && t.management.mac().is_some();
        if ui.add_enabled(wake && !busy, egui::Button::new("Wake (Wake-on-LAN)"))
            .on_hover_text(gateway.map(|g| format!("Sent from {g} on its network")).unwrap_or_default())
            .on_disabled_hover_text("Set a gateway device and its Wake-on-LAN address under Edit → Remote management")
            .clicked()
        {
            chosen = Some(Action::Wake);
        }
        if let Some(controller) = &t.management.controller {
            ui.separator();
            ui.label(RichText::new(controller.kind.label()).small().color(pal::DIM));
            if console_open {
                if ui.button("Close management console").clicked() {
                    close_console = true;
                }
            } else if ui.add_enabled(gateway.is_some() && !busy, egui::Button::new("Open management console"))
                .on_hover_text("Its web page (with the BIOS-level console and power) in your browser, through the gateway")
                .on_disabled_hover_text("Choose a gateway device under Edit → Remote management")
                .clicked()
            {
                chosen = Some(Action::Console);
            }
            if controller.kind == ControllerKind::Redfish {
                for power in ControllerPower::ALL {
                    if ui.add_enabled(gateway.is_some() && !busy, egui::Button::new(format!("{}…", power.label()))).clicked() {
                        chosen = Some(Action::Power(power));
                    }
                }
            }
            if controller.secret.is_some() && ui.button("Copy controller password").clicked() {
                copy_password = true;
            }
        }
        if t.management.is_empty() {
            ui.separator();
            if ui.button("Set up remote management…").on_hover_text("A gateway, Wake-on-LAN and its management controller").clicked() {
                set_up = true;
            }
        }
    });
    if close_console {
        app.manage.consoles.retain(|c| !(c.account == t.account && c.device == t.device.id));
        app.toast(format!("Closed the management console for {}", t.name), pal::LAVENDER);
    }
    if copy_password && let Some(sealed) = t.management.controller.as_ref().and_then(|c| c.secret.as_ref()) {
        match app.client.open_secret(&t.account, t.device.id, sealed) {
            Ok(password) => {
                response.ctx.copy_text(password);
                app.toast("Controller password copied", pal::GREEN);
            }
            Err(e) => app.toast(format!("{e:#}"), pal::RED),
        }
    }
    if set_up {
        super::saved::edit_device(app, &t.account, &t.device);
    }
    if let Some(action) = chosen {
        // Waking and opening a console change nothing: no question.
        match action {
            Action::Wake | Action::Console => start(app, &response.ctx, &t, action),
            _ => app.manage.confirm = Some(Pending { account: t.account, device: t.device, action }),
        }
    }
}

/// Confirmations and finished jobs (every frame).
pub fn windows(app: &mut App, ctx: &egui::Context) {
    if let Some((what, rx)) = &mut app.manage.job
        && let Ok(result) = rx.try_recv()
    {
        let what = what.clone();
        app.manage.job = None;
        match result {
            Ok(Outcome::Message(text)) => app.toast(text, pal::GREEN),
            Ok(Outcome::Console(console)) => {
                ctx.open_url(egui::OpenUrl::new_tab(console.console.url.clone()));
                app.toast("Management console opened in your browser (its certificate is self-signed: allow it there)", pal::GREEN);
                app.manage.consoles.push(*console);
            }
            Err(e) => app.toast(format!("{what}: {e}"), pal::RED),
        }
    }
    if app.manage.job.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    let Some(pending) = app.manage.confirm.take() else { return };
    let t = target(app, &pending.account, &pending.device);
    let (title, body) = match &pending.action {
        Action::Restart(action) => (format!("{}?", action.label()), format!("{} restarts in a few seconds. Unsaved work there is lost.", t.name)),
        Action::Power(power) => (format!("{}?", power.label()), format!("{} — sent to {}'s management controller.", power.label(), t.name)),
        _ => (String::new(), String::new()),
    };
    let (mut go, mut cancel) = (false, false);
    theme::modal("manage-confirm", pal::RED).show(ctx, |ui| {
        ui.set_width(theme::dialog_width(ctx, 420.0));
        cancel = theme::window_header_with_close(ui, &title, pal::RED, pal::BLUE).close;
        ui.label(body);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            go = theme::pill(ui, title.trim_end_matches('?'), pal::RED).clicked();
            cancel |= theme::pill(ui, "Cancel", pal::BLUE).clicked();
        });
    });
    if go {
        start(app, ctx, &t, pending.action);
    } else if !cancel {
        app.manage.confirm = Some(pending);
    }
}

fn start(app: &mut App, ctx: &egui::Context, t: &Target, action: Action) {
    if app.manage.job.is_some() {
        app.toast("One moment: still working on the last request", pal::GOLD);
        return;
    }
    let gateway = t.management.gateway;
    let controller = t.management.controller.clone();
    // The controller password is opened here, where the vault key is.
    let password = match controller.as_ref().and_then(|c| c.secret.as_ref()) {
        Some(sealed) => match app.client.open_secret(&t.account, t.device.id, sealed) {
            Ok(p) => p,
            Err(e) => return app.toast(format!("{e:#}"), pal::RED),
        },
        None => String::new(),
    };
    let (client, account, device, name, mac) = (app.client.clone(), t.account.clone(), t.device.id, t.name.clone(), t.management.wake_mac.clone());
    let gateway_name = t.gateway_name.clone().unwrap_or_default();
    let what = match &action {
        Action::Wake => format!("Waking {name}"),
        Action::Restart(a) => format!("{} {name}", a.label()),
        Action::Console => format!("Management console for {name}"),
        Action::Power(p) => format!("{} {name}", p.label()),
    };
    app.toast(format!("{what}…"), pal::BLUE);
    let (tx, rx) = oneshot::channel();
    let wake = ctx.clone();
    app.rt.spawn(async move {
        let result = async {
            let quiet = |_| {};
            let gateway_link = || async {
                let id = gateway.ok_or_else(|| anyhow::anyhow!("choose a gateway device under Edit → Remote management"))?;
                client.tool_link(&account, id, quiet).await
                    .map(|(session, _)| session)
                    .map_err(|e| anyhow::anyhow!("couldn't reach the gateway {gateway_name}: {e:#}"))
            };
            Ok::<_, anyhow::Error>(match action {
                Action::Wake => {
                    let link = gateway_link().await?;
                    let adapters = management::wake_via(&link, &mac).await?;
                    Outcome::Message(format!("Wake packets sent to {name} from {gateway_name} ({adapters} network adapter{}); its lamp turns green once it has started", if adapters == 1 { "" } else { "s" }))
                }
                Action::Restart(power) => {
                    let (link, _) = client.tool_link(&account, device, quiet).await?;
                    link.fs().power(power).await?;
                    Outcome::Message(format!("{name} is restarting ({})", power.label().to_lowercase()))
                }
                Action::Console => {
                    let controller = controller.ok_or_else(|| anyhow::anyhow!("no management controller is set up"))?;
                    let link = gateway_link().await?;
                    let console = management::console(&link, &controller).await?;
                    Outcome::Console(Box::new(OpenConsole { account, device, _gateway: link, console }))
                }
                Action::Power(power) => {
                    let controller = controller.ok_or_else(|| anyhow::anyhow!("no management controller is set up"))?;
                    let link = gateway_link().await?;
                    Outcome::Message(format!("{name}: {}", management::controller_power(&link, &controller, &password, power).await?))
                }
            })
        }.await;
        let _ = tx.send(result.map_err(|e| format!("{e:#}")));
        wake.request_repaint();
    });
    app.manage.job = Some((what, rx));
}
