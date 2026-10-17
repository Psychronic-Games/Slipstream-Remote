//! Headless modes, handy for servers, scripting and automated tests.

use std::time::Duration;

use anyhow::{Context, Result};
use slipstream_core::{config::Store, directory::DeviceId};
use slipstream_session::{Client, ClientEvent, ClientOptions, DirStatus, ViewerEvent};

pub fn set_server(address: &str, key: Option<String>) -> Result<()> {
    let store = Store::open_default()?;
    let mut s = store.load_settings();
    slipstream_core::directory::normalize_server_url(address).context("invalid server address")?;
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
    println!("endpoint id  : {}", store.identity()?.public());
    match s.host.last_device_id {
        Some(id) => println!("device id    : {id}"),
        None => println!("device id    : (not registered yet)"),
    }
    match s.server.url {
        Some(d) => println!(
            "server       : {d}{}{}",
            if s.server.key.is_some() { " (private, key set)" } else { "" },
            s.server.org_name.as_deref().map(|o| format!(" — {o}")).unwrap_or_default()
        ),
        None => println!("server       : (none — run `slipstream set-server relay.example.com`)"),
    }
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
    let (client, mut events) = Client::start(store, ClientOptions { test_pattern, accept_sessions: true, ..Default::default() }).await?;
    println!("hosting; endpoint {}", client.endpoint_id());
    loop {
        tokio::select! {
            _ = shutdown_signal() => break,
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    ClientEvent::Directory(DirStatus::Online { id }) => println!("ID       {id}"),
                    ClientEvent::Directory(other) => println!("server   {other:?}"),
                    ClientEvent::Password(pw) => println!("PASSWORD {pw}"),
                    ClientEvent::HostSessionStarted { session, peer } => println!("session {session} started by {peer}"),
                    ClientEvent::HostSessionEnded { session, reason } => println!("session {session} ended: {reason}"),
                    ClientEvent::HostChat { text, .. } => println!("chat: {text}"),
                    ClientEvent::FileReceived { path, .. } => println!("received file {}", path.display()),
                    ClientEvent::Motd(m) => println!("server says: {m}"),
                    ClientEvent::Org(Some(o)) => println!("station  {o}"),
                    ClientEvent::Org(None) => {}
                    ClientEvent::Notice(n) => println!("notice: {n}"),
                    ClientEvent::UpdateAvailable(u) if service => {
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

pub async fn connect(id: &str, password: &str, seconds: u64, ops: FileOps, forward: Vec<String>) -> Result<()> {
    let forward = forward.iter().map(|f| parse_forward(f)).collect::<Result<Vec<_>>>()?;
    let id: DeviceId = id.parse()?;
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::default()).await?;
    let (session, mut events) = match client.connect(id, password, |p| println!("… {p}")).await {
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
                        "{:>4}x{:<4} {:>5.1} fps {:>7.0} kbps  rtt {:>5.1} ms  decode {:>4.1} ms  {}",
                        s.width, s.height, s.fps, s.kbps, s.rtt_ms, s.decode_ms,
                        if s.relayed { "RELAY" } else { "DIRECT" }
                    );
                }
                Some(ViewerEvent::Disconnected { reason }) => { println!("disconnected: {reason}"); break; }
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
pub async fn software_renderer() -> Result<()> {
    let dir = slipstream_session::softgl::dir();
    if slipstream_session::softgl::installed() {
        println!("the software renderer is already in {}", dir.display());
        return Ok(());
    }
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::default()).await?;
    let result = client
        .install_software_renderer(|done, total| {
            if total > 0 && done == total {
                println!("downloaded {total} bytes");
            }
        })
        .await;
    client.shutdown().await;
    result?;
    println!("installed Mesa {} in {}: start Slipstream for the full window", slipstream_session::softgl::version(), dir.display());
    Ok(())
}

pub async fn update() -> Result<()> {
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::default()).await?;
    println!("this is Slipstream {}", slipstream_core::version_string());
    let result = async {
        match client.check_update().await? {
            None => println!("up to date"),
            Some(info) => {
                println!("installing {}…", info.label());
                let exe = client
                    .install_update(&info, |done, total| {
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
