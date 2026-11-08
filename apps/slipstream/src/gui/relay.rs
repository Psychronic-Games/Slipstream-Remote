//! The Subspace Relay page: on a phone, share its mobile data over a
//! Wi-Fi Direct network; on a computer, send all of its traffic through such
//! a phone (see `docs/issues/19-subspace-relay.md`).

#[cfg(not(target_os = "android"))]
use std::sync::{Arc, Mutex};

#[cfg(not(target_os = "android"))]
use eframe::egui;
use eframe::egui::{RichText, Ui};

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

fn traffic(connections: u64, up: u64, down: u64) -> String {
    format!("{connections} connections · ↑ {} · ↓ {}", megabytes(up), megabytes(down))
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    #[cfg(target_os = "android")]
    phone_ui(app, ui);
    #[cfg(not(target_os = "android"))]
    computer_ui(app, ui);
}

/// Per frame. On a computer: looks for a phone's relay on the network it
/// has joined, and connects to one it knows by itself.
pub fn tick(app: &mut App, ctx: &eframe::egui::Context) {
    #[cfg(not(target_os = "android"))]
    computer_tick(app, ctx);
    #[cfg(target_os = "android")]
    let _ = (app, ctx);
}

/// A heading that folds a section open and shut.
fn fold_button(open: bool) -> eframe::egui::Button<'static> {
    let text = if open { "▾ ADVANCED" } else { "▸ ADVANCED" };
    eframe::egui::Button::new(RichText::new(text).font(theme::display(15.0)).color(pal::LAVENDER)).frame(false)
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

/// What's being typed: saved as it changes, never replaced while the app
/// runs (an emptied password stays empty until something is typed or
/// **New** is pressed).
#[cfg(target_os = "android")]
#[derive(Default)]
struct Phone {
    loaded: bool,
    name: String,
    password: String,
    advanced: bool,
}

#[cfg(target_os = "android")]
fn phone_ui(app: &mut App, ui: &mut Ui) {
    use slipstream_platform::android::{self as phone, RelayGroup};
    let group = phone::relay_group();
    let band = app.client.with_settings(|s| s.app.relay_band.clone());
    if !app.relay.phone.loaded {
        let (mut name, mut password) = app.client.with_settings(|s| (s.app.relay_name.clone(), s.app.relay_password.clone()));
        // The first time only: a name and a password to start from.
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
        app.relay.phone = Phone { loaded: true, name, password, advanced: false };
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
            let field = &mut app.relay.phone.name;
            if ui.add_enabled(editable, theme::text_input(field).desired_width(200.0)).changed() {
                let clean = clean_name(field);
                app.client.update_settings(move |s| s.app.relay_name = clean);
            }
        });
        ui.label(RichText::new("PASSWORD").font(theme::display(15.0)).color(pal::LAVENDER));
        ui.horizontal(|ui| {
            let field = &mut app.relay.phone.password;
            if ui.add_enabled(editable, theme::text_input(field).desired_width(200.0)).changed() {
                let p: String = field.chars().take(63).collect();
                *field = p.clone();
                app.client.update_settings(move |s| s.app.relay_password = p);
            }
            if ui.add_enabled_ui(editable, |ui| theme::pill(ui, "New", pal::BLUE)).inner.clicked() {
                let p = new_password();
                app.relay.phone.password = p.clone();
                app.client.update_settings(move |s| s.app.relay_password = p);
            }
        });
        let (name, password) = (clean_name(&app.relay.phone.name), app.relay.phone.password.clone());
        if !password_ok(&password) {
            ui.label(RichText::new("The password needs 8 to 63 characters (letters, digits, symbols).").color(pal::RED));
        }

        // Advanced: the Wi-Fi band.
        ui.add_space(4.0);
        if ui.add(fold_button(app.relay.phone.advanced)).clicked() {
            app.relay.phone.advanced ^= true;
        }
        if app.relay.phone.advanced {
            ui.label(RichText::new("WI-FI BAND").font(theme::display(15.0)).color(pal::LAVENDER));
            ui.horizontal_wrapped(|ui| {
                for (value, label) in [("", "2.4 GHz"), ("5", "5 GHz"), ("auto", "Automatic")] {
                    let chosen = band == value || (value.is_empty() && band == "2.4");
                    let color = if chosen { pal::GREEN } else { pal::DIM };
                    if ui.add_enabled_ui(editable, |ui| theme::pill(ui, label, color)).inner.clicked() {
                        let v = value.to_string();
                        app.client.update_settings(move |s| s.app.relay_band = v);
                    }
                }
            });
            ui.label(RichText::new("2.4 GHz works with every computer. 5 GHz is faster, but on it Android tells computers to cut their \
                                    transmit power, which Linux computers obey and then can't stay connected. Takes effect when the relay starts.")
                .small()
                .color(pal::DIM));
        }
        ui.add_space(8.0);
        match &group {
            RelayGroup::Off | RelayGroup::Failed(_) => {
                let ready = !name.is_empty() && password_ok(&password);
                if ui.add_enabled_ui(ready, |ui| theme::pill(ui, "Start relay", pal::GREEN)).inner.clicked()
                    && let Err(e) = phone::relay_start(&name, &password, &band)
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
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            }
            RelayGroup::Up { name, passphrase, joined, linked, connections, up, down, .. } => {
                ui.label(RichText::new("RELAY ACTIVE").font(theme::display_bold(18.0)).color(pal::GREEN));
                ui.label(RichText::new(format!("Wi-Fi network: {name}")).font(theme::mono(16.0)).color(pal::TEXT));
                ui.label(RichText::new(format!("Password: {passphrase}")).font(theme::mono(16.0)).color(pal::TEXT));
                ui.label(format!("{joined} on the network · {linked} linked"));
                ui.label(RichText::new(traffic(*connections, *up, *down)).color(pal::BLUE));
                ui.add_space(4.0);
                ui.label(RichText::new("On the computer: join this network and open Slipstream. It connects by itself once \
                                        you've pressed Connect on its Relay page the first time. The relay keeps running when \
                                        you close this app; stop it here or from its notification.")
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
    /// Looking for the phone before asking for administrator rights.
    Finding,
    /// Waiting for the administrator prompt.
    Elevating,
    Linking,
    Up(slipstream_relay::StatsSnapshot),
    Stopping,
    Ended(String),
    Failed(String),
}

#[cfg(not(target_os = "android"))]
impl State {
    fn idle(&self) -> bool {
        matches!(self, State::Idle | State::Ended(_) | State::Failed(_))
    }
}

#[cfg(not(target_os = "android"))]
#[derive(Default)]
struct Computer {
    state: Arc<Mutex<State>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    advanced: bool,
    /// The phone relay last seen on this computer's network (its ID).
    found: Arc<Mutex<Option<String>>>,
    probing: Arc<std::sync::atomic::AtomicBool>,
    next_probe: Option<std::time::Instant>,
    /// Not connected by itself again until this phone has gone away
    /// (after Disconnect or an attempt).
    held_off: Option<String>,
    announced: Option<String>,
}

#[cfg(not(target_os = "android"))]
fn addresses(app: &App) -> (std::net::IpAddr, std::net::IpAddr) {
    let (phone, dns) = app.client.with_settings(|s| (s.app.relay_phone.clone(), s.app.relay_dns.clone()));
    (
        phone.parse().unwrap_or(slipstream_relay::proto::PHONE_IP),
        dns.parse().unwrap_or(std::net::IpAddr::from([1, 1, 1, 1])),
    )
}

/// Whether this computer is on the phone's network: the address it would
/// send from shares the phone's /24 (no packet is sent).
#[cfg(not(target_os = "android"))]
fn on_phone_network(phone: std::net::IpAddr) -> bool {
    let Ok(socket) = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)) else { return false };
    if socket.connect((phone, slipstream_relay::proto::PORT)).is_err() {
        return false;
    }
    match (socket.local_addr().map(|a| a.ip()), phone) {
        (Ok(std::net::IpAddr::V4(me)), std::net::IpAddr::V4(phone)) => me.octets()[..3] == phone.octets()[..3] && me != phone,
        _ => false,
    }
}

#[cfg(not(target_os = "android"))]
fn computer_tick(app: &mut App, ctx: &egui::Context) {
    use std::sync::atomic::Ordering;
    let now = std::time::Instant::now();
    let idle = app.relay.computer.state.lock().unwrap().idle();
    if idle && !app.relay.computer.probing.load(Ordering::Relaxed) && app.relay.computer.next_probe.is_none_or(|t| now >= t) {
        app.relay.computer.next_probe = Some(now + std::time::Duration::from_secs(3));
        app.relay.computer.probing.store(true, Ordering::Relaxed);
        let (phone, _) = addresses(app);
        let (found, probing, wake) = (app.relay.computer.found.clone(), app.relay.computer.probing.clone(), ctx.clone());
        app.rt.spawn(async move {
            let id = if on_phone_network(phone) {
                let hello = std::net::SocketAddr::new(phone, slipstream_relay::proto::PORT);
                tokio::time::timeout(std::time::Duration::from_secs(2), slipstream_relay::find_phone(hello))
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .map(|(id, _)| id.to_string())
            } else {
                None
            };
            let changed = *found.lock().unwrap() != id;
            *found.lock().unwrap() = id;
            probing.store(false, Ordering::Relaxed);
            if changed {
                wake.request_repaint();
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_secs(3));
    }
    let found = app.relay.computer.found.lock().unwrap().clone();
    let Some(id) = found else {
        // The phone has gone: connect by itself again next time it's here.
        app.relay.computer.held_off = None;
        app.relay.computer.announced = None;
        return;
    };
    if !idle {
        return;
    }
    let (trusted, manual) = app.client.with_settings(|s| (s.app.relay_trusted.contains(&id), s.app.relay_manual));
    if trusted && !manual && app.relay.computer.held_off.as_ref() != Some(&id) {
        app.relay.computer.held_off = Some(id);
        connect(app, ctx);
        app.toast("Subspace Relay: connecting through your phone", pal::CYAN);
    } else if !trusted && app.relay.computer.announced.as_ref() != Some(&id) {
        app.relay.computer.announced = Some(id);
        app.toast("A phone's Subspace Relay is on this network: Connect on the Relay page", pal::CYAN);
    }
}

#[cfg(not(target_os = "android"))]
fn computer_ui(app: &mut App, ui: &mut Ui) {
    let state = app.relay.computer.state.lock().unwrap().clone();
    let found = app.relay.computer.found.lock().unwrap().clone();
    theme::panel(ui, "Subspace Relay", pal::CYAN, |ui| {
        ui.label("Uses an Android phone's mobile data for this whole computer, as the phone's own data rather than hotspot data. \
                  On the phone, open Slipstream's Relay page and press Start relay, then join the Wi-Fi network it shows here. \
                  Press Connect the first time; after that this computer connects by itself whenever it joins that phone's network.");
        ui.label(RichText::new(CARRIER_NOTE).small().color(pal::DIM));
        ui.add_space(8.0);
        match &state {
            State::Idle | State::Ended(_) | State::Failed(_) => {
                match &found {
                    Some(_) => ui.label(RichText::new("◆ PHONE RELAY FOUND ON THIS NETWORK").font(theme::display(16.0)).color(pal::GREEN)),
                    None => ui.label(RichText::new("No phone relay on this network yet: join the phone's DIRECT-ss-… Wi-Fi network.").color(pal::DIM)),
                };
                if theme::pill(ui, "Connect", pal::GREEN).clicked() {
                    connect(app, ui.ctx());
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
            State::Finding => {
                ui.label(RichText::new("Looking for the phone…").color(pal::GOLD));
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
                ui.label(RichText::new(traffic(s.flows as u64, s.up, s.down)).color(pal::BLUE));
                if theme::pill(ui, "Disconnect", pal::RED).clicked() {
                    disconnect(app);
                }
            }
            State::Stopping => {
                ui.label(RichText::new("Disconnecting and restoring network settings…").color(pal::GOLD));
            }
        }

        // Advanced: connecting by itself, and the addresses used.
        ui.add_space(6.0);
        if ui.add(fold_button(app.relay.computer.advanced)).clicked() {
            app.relay.computer.advanced ^= true;
        }
        if app.relay.computer.advanced {
            let editable = state.idle();
            let mut automatic = !app.client.with_settings(|s| s.app.relay_manual);
            if ui.checkbox(&mut automatic, "Connect by itself when this computer joins a phone it has connected through").changed() {
                app.client.update_settings(move |s| s.app.relay_manual = !automatic);
            }
            let (phone, dns) = app.client.with_settings(|s| (s.app.relay_phone.clone(), s.app.relay_dns.clone()));
            egui::Grid::new("relay-computer").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("PHONE").font(theme::display(15.0)).color(pal::LAVENDER));
                let mut edit = phone.clone();
                let hint = slipstream_relay::proto::PHONE_IP.to_string();
                if ui.add_enabled(editable, theme::text_input(&mut edit).hint_text(hint).desired_width(180.0)).changed() {
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
            ui.label(RichText::new("Leave these empty for Android's usual address and Cloudflare's DNS.").small().color(pal::DIM));
            let bad = |v: &str| !v.is_empty() && v.parse::<std::net::IpAddr>().is_err();
            if bad(&phone) || bad(&dns) {
                ui.label(RichText::new("Not an IP address: the usual one is used instead.").color(pal::RED));
            }
        }
        if !state.idle() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    });
}

#[cfg(not(target_os = "android"))]
fn disconnect(app: &mut App) {
    if let Some(stop) = app.relay.computer.stop.take() {
        let _ = stop.send(());
    }
    // Don't connect straight back by itself.
    app.relay.computer.held_off = app.relay.computer.found.lock().unwrap().clone();
}

#[cfg(not(target_os = "android"))]
fn connect(app: &mut App, ctx: &egui::Context) {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    app.relay.computer.stop = Some(stop_tx);
    let (phone, dns) = addresses(app);
    let state = app.relay.computer.state.clone();
    *state.lock().unwrap() = State::Finding;
    let (ctx, client) = (ctx.clone(), app.client.clone());
    app.rt.spawn(async move {
        let end = run_helper(&state, &ctx, &client, phone, dns, stop_rx).await;
        let mut s = state.lock().unwrap();
        *s = match (end, &*s) {
            (Err(e), _) => State::Failed(format!("{e:#}")),
            (Ok(()), State::Ended(_) | State::Failed(_)) => s.clone(),
            (Ok(()), State::Finding) => State::Idle,
            (Ok(()), _) => State::Ended("stopped".into()),
        };
        ctx.request_repaint();
    });
}

/// Finds the phone, then starts `slipstream relay connect` as administrator
/// and follows it.
#[cfg(not(target_os = "android"))]
async fn run_helper(
    state: &Mutex<State>,
    ctx: &egui::Context,
    client: &slipstream_session::Client,
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
    // Before any administrator prompt: is the phone there at all?
    let hello = std::net::SocketAddr::new(phone, slipstream_relay::proto::PORT);
    let phone_id = match slipstream_relay::find_phone(hello).await {
        Ok((id, _)) => id.to_string(),
        Err(_) => bail!("No phone relay answered at {phone}. Start the relay on the phone and join its DIRECT-ss-… Wi-Fi network first."),
    };
    set(State::Elevating);
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
    let mut remembered = false;
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => match serde_json::from_str::<HelperStatus>(&line) {
                    Ok(HelperStatus::Linking) => set(State::Linking),
                    Ok(HelperStatus::Up(s)) if !stopping => {
                        // Connected through it once: it may be used again by itself.
                        if !remembered {
                            remembered = true;
                            let id = phone_id.clone();
                            client.update_settings(move |settings| {
                                if !settings.app.relay_trusted.contains(&id) {
                                    settings.app.relay_trusted.push(id);
                                }
                            });
                        }
                        set(State::Up(s));
                    }
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

    #[cfg(not(target_os = "android"))]
    #[test]
    fn loopback_is_never_the_phone_network() {
        assert!(!on_phone_network(std::net::IpAddr::from([127, 0, 0, 1])));
    }
}
