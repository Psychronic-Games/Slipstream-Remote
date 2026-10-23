//! Headless modes, handy for servers, scripting and automated tests.

use std::time::Duration;

use anyhow::{Context, Result};
use slipstream_core::{config::Store, directory::DeviceId};
use slipstream_session::{Client, ClientEvent, ClientOptions, DirStatus, ViewerEvent};

pub fn set_server(address: &str, key: Option<String>) -> Result<()> {
    let store = Store::open_default()?;
    let mut s = store.load_settings();
    slipstream_core::directory::normalize_server_url(address).context("invalid server address")?;
    if s.server.url.as_deref() != Some(address.trim()) { s.host.last_device_id = None; }
    s.server.url = Some(address.trim().to_string());
    s.server.key = key.filter(|k| !k.trim().is_empty());
    s.server.provisioned = false;
    store.save_settings(&s)?;
    println!("server set; settings in {}", store.dir().display());
    Ok(())
}

pub fn set_password(password: Option<&str>) -> Result<()> {
    if let Some(p) = password {
        anyhow::ensure!(p.chars().count() >= 8, "use at least 8 characters");
    }
    let store = Store::open_default()?;
    let mut s = store.load_settings();
    s.host.set_permanent_password(password);
    store.save_settings(&s)?;
    println!("{}", if password.is_some() { "unattended password set" } else { "unattended password removed" });
    Ok(())
}

pub async fn info() -> Result<()> {
    let store = Store::open_default()?;
    let s = store.load_settings();
    println!("settings dir : {}", store.dir().display());
    println!("device name  : {}", s.device_name);
    let custom = s.server.url.as_deref().is_some_and(|url| !url.trim().is_empty() && !slipstream_core::config::is_public_station(url));
    let secret = store.identity()?;
    let endpoint = if custom { secret.public() } else { slipstream_core::config::public_identity(&secret).public() };
    println!("endpoint id  : {endpoint}");
    match if custom { s.host.last_device_id } else { s.host.public_device_id } {
        Some(id) => println!("device id    : {id}"),
        None => println!("device id    : (not registered yet)"),
    }
    match s.server.url {
        Some(d) => println!(
            "server       : {d}{}{}",
            if s.server.key.is_some() { " (private, key set)" } else { "" },
            s.server.org_name.as_deref().map(|o| format!(" — {o}")).unwrap_or_default()
        ),
        None => println!("server       : {} (built-in public station)", slipstream_core::config::PUBLIC_STATION_URL),
    }
    if custom && let Some(id) = s.host.public_device_id { println!("public id    : {id} ({})", slipstream_core::config::PUBLIC_STATION_URL); }
    match slipstream_core::release::trusted_keys().as_slice() {
        [] => println!("updates      : off (this build has no update key)"),
        keys => {
            let keys: Vec<String> = keys.iter().map(|k| k.fmt_short().to_string()).collect();
            println!("updates      : signed by {}", keys.join(", "));
        }
    }
    Ok(())
}

/// Headless host. `service`: started by the Windows service, so it installs
/// updates itself and exits for the service to restart it.
pub async fn host(test_pattern: bool, password: Option<String>, service: bool) -> Result<()> {
    let store = Store::open_default()?;
    if let Some(pw) = &password {
        let mut s = store.load_settings();
        s.host.set_permanent_password(Some(pw));
        store.save_settings(&s)?;
    }
    let (client, mut events) = Client::start(store, ClientOptions { test_pattern, accept_sessions: true, public_station: (!test_pattern).then(slipstream_core::config::ServerSettings::public_station), ..Default::default() }).await?;
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && client.settings().host.enabled {
        let preparing = client.clone();
        tokio::spawn(async move {
            if let Err(e) = preparing.prepare_screen_access().await {
                tracing::warn!("approve screen access locally before connecting: {e:#}");
            }
        });
    }
    #[cfg(windows)]
    let mut chat_bridge=if service {match crate::service_chat::Server::start(client.clone()){Ok(bridge)=>Some(bridge),Err(e)=>{tracing::warn!("host chat unavailable: {e:#}");None}}} else {None};
    println!("hosting; endpoint {}", client.endpoint_id());
    loop {
        tokio::select! {
            _ = shutdown_signal() => break,
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                #[cfg(windows)]
                if let Some(bridge)=&mut chat_bridge {bridge.observe(&ev);}
                match ev {
                    ClientEvent::Directory(DirStatus::Online { id }) => println!("ID       {id}"),
                    ClientEvent::PublicDirectory(status) => println!("public   {status:?}"),
                    ClientEvent::Directory(other) => println!("server   {other:?}"),
                    ClientEvent::Password(pw) => if !service { println!("PASSWORD {pw}"); },
                    ClientEvent::HostSessionStarted { session, peer } => println!("session {session} started by {peer}"),
                    ClientEvent::HostSessionEnded { session, reason } => println!("session {session} ended: {reason}"),
                    ClientEvent::HostChat { text, .. } => if service {println!("chat message received");} else {println!("chat: {text}");}
                    ClientEvent::HostStatus { .. } => println!("support status updated"),
                    ClientEvent::FileReceived { path, .. } => println!("received file {}", path.display()),
                    ClientEvent::Motd(m) => println!("server says: {m}"),
                    ClientEvent::Org(Some(o)) => println!("station  {o}"),
                    ClientEvent::Org(None) => {}
                    ClientEvent::Notice(n) => println!("notice: {n}"),
                    ClientEvent::CallChanged => {},
                    ClientEvent::LinkApproval => if !service && client.approvals().pending().is_some() {
                        println!("a one-time-password link needs approval at a window; use the unattended password here");
                    },
                    ClientEvent::UpdateAvailable(u) if service => {
                        #[cfg(windows)]
                        let _guard = match crate::service::UpdateGuard::acquire() {
                            Ok(guard) => guard,
                            Err(error) => { tracing::info!("service update deferred: {error:#}");continue; }
                        };
                        tracing::info!(version = %u.label(), "installing update");
                        match client.install_update(&u, |_, _| {}).await {
                            Ok(_) => break, // the service restarts with the new program
                            Err(e) => tracing::warn!("update failed: {e:#}"),
                        }
                    }
                    ClientEvent::UpdateAvailable(u) => println!("update available: {} (run `slipstream update`)", u.label()),
                }
            }
        }
    }
    client.shutdown().await;
    Ok(())
}

/// Ctrl+C, or SIGTERM (systemd stop) on Unix.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// One-shot file operations for `slipstream connect`.
pub struct FileOps {
    pub ls: Option<String>,
    pub put: Option<std::path::PathBuf>,
    pub to: Option<String>,
    pub get: Option<String>,
}

async fn file_ops(fs: &slipstream_session::RemoteFs, ops: FileOps) -> Result<()> {
    use slipstream_core::proto::UploadTarget;
    if let Some(path) = ops.ls {
        let l = fs.list(Some(path).filter(|p| !p.is_empty())).await?;
        println!("{}  (parent: {:?}, separator {:?})", l.path, l.parent, l.sep);
        for e in &l.entries {
            println!("  {} {:>12}  {}", if e.is_dir { "DIR " } else { "FILE" }, e.size, e.name);
        }
        let places = fs.places().await?;
        println!("places: {}", places.iter().map(|p| format!("{}={}", p.label, p.path)).collect::<Vec<_>>().join(", "));
    }
    if let Some(local) = ops.put {
        let name = local.file_name().context("not a file")?.to_string_lossy().into_owned();
        let target = ops.to.map(UploadTarget::Folder).unwrap_or(UploadTarget::Default);
        let saved = fs.upload(&local, target, name, |_| {}).await?;
        println!("uploaded to {saved}");
    }
    if let Some(remote) = ops.get {
        let name = remote.rsplit(['/', '\\']).next().unwrap_or("download").to_string();
        let dest = std::env::current_dir()?.join(name);
        let saved = fs.download(remote, &dest, |_| {}).await?;
        println!("downloaded to {}", saved.display());
    }
    Ok(())
}

/// Parses `LOCAL:HOST:PORT` or `LOCAL:PORT` (the remote's localhost).
pub fn parse_forward(spec: &str) -> Result<(u16, String, u16)> {
    let parts: Vec<&str> = spec.split(':').collect();
    let num = |s: &str| s.parse::<u16>().with_context(|| format!("{s:?} is not a port number"));
    match parts.as_slice() {
        [local, port] => Ok((num(local)?, "localhost".into(), num(port)?)),
        [local, host, port] if !host.is_empty() => Ok((num(local)?, host.to_string(), num(port)?)),
        _ => anyhow::bail!("expected LOCAL:HOST:PORT or LOCAL:PORT, got {spec:?}"),
    }
}

pub async fn connect(
    id: &str,
    password: Option<String>,
    station: Option<String>,
    seconds: u64,
    ops: FileOps,
    forward: Vec<String>,
) -> Result<()> {
    let forward = forward.iter().map(|f| parse_forward(f)).collect::<Result<Vec<_>>>()?;
    let id: DeviceId = id.parse()?;
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::production()).await?;
    let attempt = match &station {
        Some(st) => match account_for(&client, st) {
            Ok(r) => client.connect_via(&r, id, password, |p| println!("… {p}")).await,
            Err(e) => Err(e),
        },
        None => match &password {
            Some(pw) => client.connect(id, pw, |p| println!("… {p}")).await,
            None => Err(anyhow::anyhow!("--password is needed (or --station, for a saved one)")),
        },
    };
    let (session, mut events) = match attempt {
        Ok(s) => s,
        Err(e) => {
            client.shutdown().await;
            return Err(e);
        }
    };
    let info = session.info().clone();
    println!("connected to {} ({}), {} display(s)", info.host_name, info.os, info.displays.len());
    if !forward.is_empty() {
        let mut tunnels = Vec::new();
        for (local, host, port) in forward {
            match session.tunnel(local, host.clone(), port).await {
                Ok(t) => {
                    println!(
                        "forwarding 127.0.0.1:{} → {host}:{port} on {} (Ctrl+C to stop)",
                        t.local_port, info.host_name
                    );
                    tunnels.push(t);
                }
                Err(e) => {
                    drop(session);
                    client.shutdown().await;
                    return Err(e);
                }
            }
        }
        let _ = tokio::signal::ctrl_c().await;
        drop(tunnels);
        drop(session);
        client.shutdown().await;
        return Ok(());
    }
    if ops.ls.is_some() || ops.put.is_some() || ops.get.is_some() {
        let r = file_ops(&session.fs(), ops).await;
        drop(session);
        client.shutdown().await;
        return r;
    }
    let deadline = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(deadline);
    let mut total_frames = 0.0;
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            ev = events.recv() => match ev {
                Some(ViewerEvent::Stats(s)) => {
                    total_frames += s.fps;
                    println!(
                        "{:>4}x{:<4} {:>5.1} fps {:>7.0} kbps  rtt {:>5.1} ms  decode {:>4.1} ms  {}{}",
                        s.width, s.height, s.fps, s.kbps, s.rtt_ms, s.decode_ms,
                        if s.relayed { "RELAY" } else { "DIRECT" },
                        s.host
                            .iter()
                            .map(|h| if h.fps == 0.0 && h.ms == 0.0 { format!("  | {}", h.name) } else { format!("  | {} {:.0} fps {:.1} ms", h.name, h.fps, h.ms) })
                            .collect::<String>()
                    );
                }
                Some(ViewerEvent::Disconnected { reason, .. }) => { println!("disconnected: {reason}"); break; }
                Some(other) => println!("{other:?}"),
                None => break,
            }
        }
    }
    println!("received ~{total_frames:.0} frames");
    drop(session);
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.shutdown().await;
    Ok(())
}

/// Checks the station for a newer build and installs it.
/// The sign-in for a station address.
fn account_for(client: &Client, station: &str) -> Result<slipstream_session::AccountRef> {
    client
        .settings()
        .accounts
        .iter()
        .find(|a| a.is_station(station))
        .map(slipstream_session::AccountRef::of)
        .ok_or_else(|| anyhow::anyhow!("not signed in to {station}: use `slipstream sign-in {station} USERNAME`"))
}

pub async fn sign_in(station: &str, username: &str, password: Option<String>) -> Result<()> {
    let password = match password {
        Some(p) => p,
        None => {
            eprint!("password for {username} on {station}: ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::production()).await?;
    let result = client.sign_in(station, username, &password).await;
    let accounts = client.settings().accounts;
    client.shutdown().await;
    let r = result?;
    let a = accounts.iter().find(|a| a.is_station(&r.server)).expect("just signed in");
    println!(
        "signed in to {} as {}{}: {} saved device(s), {} shared",
        a.label(),
        a.user.name,
        if a.user.admin { " (station admin)" } else { "" },
        a.devices.len(),
        a.shared.len()
    );
    Ok(())
}

pub async fn sign_out(station: &str) -> Result<()> {
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::production()).await?;
    let result = account_for(&client, station);
    if let Ok(r) = &result {
        client.sign_out(r).await;
        println!("signed out of {station}");
    }
    client.shutdown().await;
    result.map(|_| ())
}

pub struct SaveOptions {
    pub name: String,
    pub notes: String,
    pub group: Option<String>,
    pub password: Option<String>,
    pub shared: bool,
    pub remove: bool,
}

pub async fn save(id: &str, station: &str, options: SaveOptions) -> Result<()> {
    let SaveOptions { name, notes, group, password, shared, remove } = options;
    let id: DeviceId = id.parse()?;
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::production()).await?;
    let result = async {
        let r = account_for(&client, station)?;
        if remove {
            client.delete_device(&r, id, shared).await?;
            println!("removed {id}");
        } else {
            client.sync_account(&r).await?;
            let settings = client.settings();
            let account = settings.accounts.iter().find(|a| a.is_station(&r.server) && a.user.username == r.username)
                .context("station account no longer available")?;
            let existing = if shared { &account.shared } else { &account.devices };
            let group = group.unwrap_or_else(|| existing.iter().find(|d| d.id == id).map(|d| d.group.clone()).unwrap_or_default());
            let device = slipstream_core::account::SavedDevice { id, name, notes, group, secret: None, added_by: None };
            client.save_device(&r, device, password, shared).await?;
            println!("saved {id}");
        }
        anyhow::Ok(())
    }
    .await;
    client.shutdown().await;
    result
}

pub async fn saved() -> Result<()> {
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::production()).await?;
    client.sync_accounts().await;
    let accounts = client.settings().accounts;
    if accounts.is_empty() {
        println!("not signed in to any station (see `slipstream sign-in`)");
    }
    for a in &accounts {
        let r = slipstream_session::AccountRef::of(a);
        println!("{} ({}) · {}{}", a.label(), a.server, a.user.name, if a.signed_out { " · SIGNED OUT, sign in again" } else { "" });
        let ids: Vec<DeviceId> = a.devices.iter().chain(&a.shared).map(|d| d.id).collect();
        let presence = client.presence(&r, &ids).await;
        let state = |id| presence.iter().find(|(i, _)| *i == id).map(|(_, p)| format!("{p:?}").to_lowercase()).unwrap_or_default();
        for (list, devices) in [("mine", &a.devices), ("shared", &a.shared)] {
            for d in devices {
                println!(
                    "  {:<7} {}  {:<28} {:<8}{}{}{}",
                    list,
                    d.id,
                    d.name,
                    state(d.id),
                    if d.secret.is_some() { " · password saved" } else { "" },
                    if d.group.is_empty() { String::new() } else { format!(" · [{}]", d.group) },
                    if d.notes.is_empty() { String::new() } else { format!(" · {}", d.notes.replace('\n', " / ")) }
                );
            }
        }
    }
    client.shutdown().await;
    Ok(())
}

pub async fn update() -> Result<()> {
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::production()).await?;
    println!("this is Slipstream {}", slipstream_core::version_string());
    let result = async {
        let info = client.check_update().await?;
        #[cfg(windows)]
        let info = if info.is_none() && crate::service::needs_app_update()? { client.available_update().await? } else { info };
        match info {
            None => println!("up to date"),
            Some(info) => {
                #[cfg(windows)]
                if info.version_key() == slipstream_session::update::UpdateInfo::current_version_key() {
                    crate::update::sync_service(&info).await?;
                    println!("Windows service updated from this app; no app restart needed");
                    return Ok(());
                }
                println!("installing {}…", info.label());
                let exe = crate::update::install(&client, &info, |done, total| {
                        if total > 0 && done == total {
                            println!("downloaded {} bytes", total);
                        }
                    })
                    .await?;
                println!("updated {} — restart Slipstream to use it", exe.display());
            }
        }
        anyhow::Ok(())
    }
    .await;
    client.shutdown().await;
    result
}
