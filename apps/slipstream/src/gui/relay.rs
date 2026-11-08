//! The Subspace Relay page: on a phone, share its mobile data over a
//! Wi-Fi Direct network; on a computer, send all of its traffic through such
//! a phone (see `docs/issues/19-subspace-relay.md`).

use std::sync::{Arc, Mutex};

#[cfg(not(target_os = "android"))]
use eframe::egui;
use eframe::egui::{RichText, Ui};
use slipstream_relay::StatsSnapshot;

use super::{App, theme, theme::pal};

const CARRIER_NOTE: &str = "Carriers' terms often count any data another device uses as hotspot data, \
     whatever carries it, and some detect it. Check your plan before relying on this.";

#[derive(Default)]
pub struct Relay {
    #[cfg(target_os = "android")]
    phone: Phone,
    #[cfg(not(target_os = "android"))]
    computer: Computer,
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

fn traffic(s: &StatsSnapshot) -> String {
    format!("{} connections · ↑ {} · ↓ {}", s.flows, megabytes(s.up), megabytes(s.down))
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    #[cfg(target_os = "android")]
    phone_ui(app, ui);
    #[cfg(not(target_os = "android"))]
    computer_ui(app, ui);
}

/// Per frame: keeps the phone's relay server in step with its Wi-Fi Direct
/// group (it must run whichever page is showing).
pub fn tick(app: &mut App) {
    #[cfg(target_os = "android")]
    app.relay.phone.tick(&app.rt);
    #[cfg(not(target_os = "android"))]
    let _ = app;
}

// ---------------------------------------------------------------- phone

/// The network's name after `DIRECT-ss-`: Android allows 32 bytes in all.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(22).collect::<String>().trim().to_string()
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn password_ok(p: &str) -> bool {
    (8..=63).contains(&p.len()) && p.chars().all(|c| c.is_ascii_graphic() || c == ' ')
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn new_password() -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    (0..10).map(|_| CHARS[rng.random_range(0..CHARS.len())] as char).collect()
}

#[cfg(target_os = "android")]
#[derive(Default)]
struct Phone {
    /// The relay for the group at this address, once it's listening.
    server: Arc<Mutex<Option<slipstream_relay::Server>>>,
    serving: Option<String>,
    error: Arc<Mutex<Option<String>>>,
}

#[cfg(target_os = "android")]
impl Phone {
    fn tick(&mut self, rt: &tokio::runtime::Runtime) {
        use slipstream_platform::android::RelayGroup;
        let ip = match slipstream_platform::android::relay_group() {
            RelayGroup::Up { ip, .. } => Some(ip),
            _ => None,
        };
        if ip == self.serving {
            return;
        }
        self.serving = ip.clone();
        let old = self.server.lock().unwrap().take();
        let (server, error) = (self.server.clone(), self.error.clone());
        *error.lock().unwrap() = None;
        rt.spawn(async move {
            if let Some(old) = old {
                old.stop().await;
            }
            let Some(ip) = ip else { return };
            let Ok(ip) = ip.parse() else {
                *error.lock().unwrap() = Some(format!("Unexpected Wi-Fi Direct address {ip}"));
                return;
            };
            // The group's address can take a moment to appear.
            let mut last = String::new();
            for _ in 0..20 {
                match slipstream_relay::Server::start(slipstream_relay::ServerOptions { ip, ..Default::default() }).await {
                    Ok(s) => {
                        *server.lock().unwrap() = Some(s);
                        return;
                    }
                    Err(e) => last = format!("{e:#}"),
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            tracing::warn!("Subspace Relay couldn't listen: {last}");
            *error.lock().unwrap() = Some(format!("The relay couldn't start: {last}"));
        });
    }
}

#[cfg(target_os = "android")]
fn phone_ui(app: &mut App, ui: &mut Ui) {
    use slipstream_platform::android::{self as phone, RelayGroup};
    let group = phone::relay_group();
    let (mut name, mut password) = app.client.with_settings(|s| (s.app.relay_name.clone(), s.app.relay_password.clone()));
    if name.is_empty() || password.is_empty() {
        if name.is_empty() {
            name = "Slipstream".into();
        }
        if password.is_empty() {
            password = new_password();
        }
        let (n, p) = (name.clone(), password.clone());
        app.client.update_settings(move |s| {
            s.app.relay_name = n;
            s.app.relay_password = p;
        });
    }
    theme::panel(ui, "Subspace Relay", pal::CYAN, |ui| {
        ui.label("Shares this phone's mobile data with a computer: the phone starts its own Wi-Fi network, the computer joins it, \
                  and Slipstream on the computer sends everything through the phone as this app's data.");
        ui.label(RichText::new(CARRIER_NOTE).small().color(pal::DIM));
        ui.add_space(8.0);
        let editable = matches!(group, RelayGroup::Off | RelayGroup::Failed(_));
        ui.label(RichText::new("NETWORK NAME").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.horizontal(|ui| {
            ui.label(RichText::new("DIRECT-ss-").font(theme::mono(15.0)).color(pal::DIM));
            let mut edit = name.clone();
            if ui.add_enabled(editable, theme::text_input(&mut edit).desired_width(200.0)).changed() {
                let clean = clean_name(&edit);
                app.client.update_settings(move |s| s.app.relay_name = clean);
            }
        });
        ui.label(RichText::new("PASSWORD").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.horizontal(|ui| {
            let mut edit = password.clone();
            if ui.add_enabled(editable, theme::text_input(&mut edit).desired_width(200.0)).changed() {
                let edit: String = edit.chars().take(63).collect();
                app.client.update_settings(move |s| s.app.relay_password = edit);
            }
            if ui.add_enabled_ui(editable, |ui| theme::pill(ui, "New", pal::BLUE)).inner.clicked() {
                let p = new_password();
                app.client.update_settings(move |s| s.app.relay_password = p);
            }
        });
        if !password_ok(&password) {
            ui.label(RichText::new("The password needs 8 to 63 characters.").color(pal::RED));
        }
        ui.add_space(8.0);
        match &group {
            RelayGroup::Off | RelayGroup::Failed(_) => {
                let ready = !clean_name(&name).is_empty() && password_ok(&password);
                if ui.add_enabled_ui(ready, |ui| theme::pill(ui, "Start relay", pal::GREEN)).inner.clicked()
                    && let Err(e) = phone::relay_start(&clean_name(&name), &password)
                {
                    app.toast(format!("Subspace Relay: {e:#}"), pal::RED);
                }
                if let RelayGroup::Failed(why) = &group {
                    ui.label(RichText::new(why).color(pal::RED));
                }
            }
            RelayGroup::Starting => {
                ui.label(RichText::new("Starting the Wi-Fi network…").color(pal::GOLD));
                if theme::pill(ui, "Stop", pal::RED).clicked() {
                    let _ = phone::relay_stop();
                }
            }
            RelayGroup::Up { name, passphrase, computers, .. } => {
                ui.label(RichText::new("RELAY ACTIVE").font(theme::display_bold(18.0)).color(pal::GREEN));
                ui.label(RichText::new(format!("Wi-Fi network: {name}")).font(theme::mono(16.0)).color(pal::TEXT));
                ui.label(RichText::new(format!("Password: {passphrase}")).font(theme::mono(16.0)).color(pal::TEXT));
                let error = app.relay.phone.error.lock().unwrap().clone();
                let stats = app.relay.phone.server.lock().unwrap().as_ref().map(|s| s.stats().snapshot());
                match (error, stats) {
                    (Some(e), _) => {
                        ui.label(RichText::new(e).color(pal::RED));
                    }
                    (None, Some(s)) => {
                        ui.label(format!("{computers} on the network · {} linked", s.computers));
                        ui.label(RichText::new(traffic(&s)).color(pal::BLUE));
                    }
                    (None, None) => {
                        ui.label(RichText::new("Starting the relay…").color(pal::GOLD));
                    }
                }
                ui.add_space(4.0);
                ui.label(RichText::new("On the computer: join this network, then open Slipstream's Relay page and press Connect.")
                    .small()
                    .color(pal::DIM));
                if theme::pill(ui, "Stop relay", pal::RED).clicked() {
                    let _ = phone::relay_stop();
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            }
        }
    });
}

// ---------------------------------------------------------------- computer

#[cfg(not(target_os = "android"))]
#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    Idle,
    /// Waiting for the administrator prompt.
    Elevating,
    Linking,
    Up(StatsSnapshot),
    Stopping,
    Ended(String),
    Failed(String),
}

#[cfg(not(target_os = "android"))]
#[derive(Default)]
struct Computer {
    state: Arc<Mutex<State>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(not(target_os = "android"))]
fn computer_ui(app: &mut App, ui: &mut Ui) {
    let state = app.relay.computer.state.lock().unwrap().clone();
    let (phone, dns) = app.client.with_settings(|s| (s.app.relay_phone.clone(), s.app.relay_dns.clone()));
    theme::panel(ui, "Subspace Relay", pal::CYAN, |ui| {
        ui.label("Uses an Android phone's mobile data for this whole computer, as the phone's own data rather than hotspot data. \
                  On the phone, open Slipstream's Relay page and press Start relay; join the Wi-Fi network it shows; then press Connect here.");
        ui.label(RichText::new(CARRIER_NOTE).small().color(pal::DIM));
        ui.add_space(8.0);
        let editable = matches!(state, State::Idle | State::Ended(_) | State::Failed(_));
        egui::Grid::new("relay-computer").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("PHONE").font(theme::display(15.0)).color(pal::LAVENDER));
            let mut edit = phone.clone();
            if ui.add_enabled(editable, theme::text_input(&mut edit).hint_text(slipstream_relay::proto::PHONE_IP.to_string()).desired_width(180.0)).changed() {
                app.client.update_settings(move |s| s.app.relay_phone = edit.trim().to_string());
            }
            ui.end_row();
            ui.label(RichText::new("DNS").font(theme::display(15.0)).color(pal::LAVENDER));
            let mut edit = dns.clone();
            if ui.add_enabled(editable, theme::text_input(&mut edit).hint_text("1.1.1.1").desired_width(180.0)).changed() {
                app.client.update_settings(move |s| s.app.relay_dns = edit.trim().to_string());
            }
            ui.end_row();
        });
        ui.add_space(8.0);
        let phone_ip = if phone.is_empty() { Ok(slipstream_relay::proto::PHONE_IP) } else { phone.parse::<std::net::IpAddr>() };
        let dns_ip = if dns.is_empty() { Ok(std::net::IpAddr::from([1, 1, 1, 1])) } else { dns.parse::<std::net::IpAddr>() };
        match &state {
            State::Idle | State::Ended(_) | State::Failed(_) => {
                let ready = phone_ip.is_ok() && dns_ip.is_ok();
                if ui.add_enabled_ui(ready, |ui| theme::pill(ui, "Connect", pal::GREEN)).inner.clicked()
                    && let (Ok(p), Ok(d)) = (phone_ip, dns_ip)
                {
                    connect(app, ui.ctx(), p, d);
                }
                if !ready {
                    ui.label(RichText::new("Enter IP addresses for the phone and DNS server.").color(pal::RED));
                }
                match &state {
                    State::Ended(why) => {
                        ui.label(RichText::new(format!("Disconnected: {why}. This computer's network settings are back as they were.")).color(pal::DIM));
                    }
                    State::Failed(why) => {
                        ui.label(RichText::new(why).color(pal::RED));
                    }
                    _ => {}
                }
            }
            State::Elevating => {
                ui.label(RichText::new("Waiting for administrator approval…").color(pal::GOLD));
                if theme::pill(ui, "Cancel", pal::RED).clicked() {
                    disconnect(app);
                }
            }
            State::Linking => {
                ui.label(RichText::new("Linking to the phone…").color(pal::GOLD));
                if theme::pill(ui, "Cancel", pal::RED).clicked() {
                    disconnect(app);
                }
            }
            State::Up(s) => {
                ui.label(RichText::new("CONNECTED THROUGH THE PHONE").font(theme::display_bold(18.0)).color(pal::GREEN));
                ui.label(RichText::new(traffic(s)).color(pal::BLUE));
                if theme::pill(ui, "Disconnect", pal::RED).clicked() {
                    disconnect(app);
                }
            }
            State::Stopping => {
                ui.label(RichText::new("Disconnecting and restoring network settings…").color(pal::GOLD));
            }
        }
        if !matches!(state, State::Idle | State::Ended(_) | State::Failed(_)) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    });
}

#[cfg(not(target_os = "android"))]
fn disconnect(app: &mut App) {
    if let Some(stop) = app.relay.computer.stop.take() {
        let _ = stop.send(());
    }
}

#[cfg(not(target_os = "android"))]
fn connect(app: &mut App, ctx: &egui::Context, phone: std::net::IpAddr, dns: std::net::IpAddr) {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    app.relay.computer.stop = Some(stop_tx);
    let state = app.relay.computer.state.clone();
    *state.lock().unwrap() = State::Elevating;
    let ctx = ctx.clone();
    app.rt.spawn(async move {
        let end = run_helper(&state, &ctx, phone, dns, stop_rx).await;
        let mut s = state.lock().unwrap();
        *s = match (end, &*s) {
            (Err(e), _) => State::Failed(format!("{e:#}")),
            (Ok(()), State::Up(_) | State::Linking | State::Stopping | State::Elevating) => State::Ended("stopped".into()),
            (Ok(()), other) => other.clone(),
        };
        ctx.request_repaint();
    });
}

/// Starts `slipstream relay connect` as administrator and follows it.
#[cfg(not(target_os = "android"))]
async fn run_helper(
    state: &Mutex<State>,
    ctx: &egui::Context,
    phone: std::net::IpAddr,
    dns: std::net::IpAddr,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    use anyhow::bail;
    use slipstream_relay::HelperStatus;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let set = |s: State| {
        *state.lock().unwrap() = s;
        ctx.request_repaint();
    };
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let token = format!("{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>());
    let args = vec![
        "relay".to_string(),
        "connect".into(),
        "--phone".into(),
        phone.to_string(),
        "--dns".into(),
        dns.to_string(),
        "--control".into(),
        listener.local_addr()?.to_string(),
        "--token".into(),
        token.clone(),
    ];
    let launcher = launch_elevated(args);
    tokio::pin!(launcher);
    let mut launched = false;
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(180));
    tokio::pin!(deadline);
    let (mut lines, mut to_helper) = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, peer) = accepted?;
                if !peer.ip().is_loopback() { continue; }
                let (read, write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                let first = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line()).await;
                if matches!(first, Ok(Ok(Some(ref t))) if *t == token) {
                    break (lines, write);
                }
            }
            done = &mut launcher, if !launched => {
                launched = true;
                done?;
                // macOS hands the helper to the background and returns at once.
                if !cfg!(target_os = "macos") {
                    bail!("The relay helper stopped before it linked to the phone (see the log).");
                }
            }
            _ = &mut stop => return Ok(()),
            _ = &mut deadline => bail!("Timed out waiting for administrator approval."),
        }
    };
    set(State::Linking);
    let mut stopping = false;
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => match serde_json::from_str::<HelperStatus>(&line) {
                    Ok(HelperStatus::Linking) => set(State::Linking),
                    Ok(HelperStatus::Up(s)) if !stopping => set(State::Up(s)),
                    Ok(HelperStatus::Up(_)) => {}
                    Ok(HelperStatus::Down(why)) => {
                        set(State::Ended(why));
                        return Ok(());
                    }
                    Ok(HelperStatus::Failed(why)) => bail!("{why}"),
                    Err(e) => tracing::debug!("relay helper said {line:?}: {e}"),
                },
                _ => return Ok(()),
            },
            _ = &mut stop, if !stopping => {
                stopping = true;
                set(State::Stopping);
                let _ = to_helper.shutdown().await;
            }
        }
    }
}

/// Runs this program with `args` as administrator (root): polkit on Linux,
/// UAC on Windows, the administrator prompt on macOS. Completes when the
/// helper exits, except on macOS, where it completes once it's started.
#[cfg(not(target_os = "android"))]
async fn launch_elevated(args: Vec<String>) -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    use anyhow::bail;
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "linux")]
    {
        let mut command = if unsafe { libc::geteuid() } == 0 {
            tokio::process::Command::new(&exe)
        } else {
            let mut c = tokio::process::Command::new("pkexec");
            c.arg(&exe);
            c
        };
        let status = command
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(false)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("couldn't ask for administrator rights (pkexec): {e}"))?;
        match status.code() {
            Some(126) => bail!("Administrator approval was cancelled."),
            Some(127) => bail!("Not authorized to change this computer's network settings."),
            _ => Ok(()),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let args = args.join(" ");
        let _ = exe;
        tokio::task::spawn_blocking(move || crate::service::run_elevated(&args)).await??;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
        let mut line = quote(&exe.display().to_string());
        for a in &args {
            line.push(' ');
            line.push_str(&quote(a));
        }
        line.push_str(" >/dev/null 2>&1 &");
        let script = format!("do shell script \"{}\" with administrator privileges", line.replace('\\', "\\\\").replace('"', "\\\""));
        let status = tokio::process::Command::new("osascript").arg("-e").arg(script).status().await?;
        if !status.success() {
            bail!("Administrator approval was cancelled.");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_names_and_passwords_suit_wifi_direct() {
        assert_eq!(clean_name("  Doug's Phone!  "), "Dougs Phone");
        assert_eq!(clean_name(&"x".repeat(40)).len(), 22);
        assert!(password_ok(&new_password()));
        assert!(!password_ok("short"));
        assert!(!password_ok(&"x".repeat(64)));
        assert!(!password_ok("naïve-password"));
    }
}
