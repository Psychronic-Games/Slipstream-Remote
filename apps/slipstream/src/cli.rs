//! Headless modes, handy for servers, scripting and automated tests.

use std::time::Duration;

use anyhow::{Context, Result};
use slipstream_core::{config::Store, directory::DeviceId};
use slipstream_session::{Client, ClientEvent, ClientOptions, DirStatus, ViewerEvent};

pub fn set_server(address: &str) -> Result<()> {
    let store = Store::open_default()?;
    let mut s = store.load_settings();
    slipstream_core::directory::normalize_server_url(address).context("invalid server address")?;
    s.server.url = Some(address.trim().to_string());
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
        Some(d) => println!("server       : {d}"),
        None => println!("server       : (none — run `slipstream set-server relay.example.com`)"),
    }
    Ok(())
}

pub async fn host(test_pattern: bool, password: Option<String>) -> Result<()> {
    let store = Store::open_default()?;
    if let Some(pw) = &password {
        let mut s = store.load_settings();
        s.host.set_permanent_password(Some(pw));
        store.save_settings(&s)?;
    }
    let (client, mut events) = Client::start(store, ClientOptions { test_pattern, accept_sessions: true }).await?;
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
                    ClientEvent::Notice(n) => println!("notice: {n}"),
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

pub async fn connect(id: &str, password: &str, seconds: u64) -> Result<()> {
    let id: DeviceId = id.parse()?;
    let store = Store::open_default()?;
    let (client, _events) = Client::start(store, ClientOptions::default()).await?;
    let (session, mut events) = client.connect(id, password, |p| println!("… {p}")).await?;
    let info = session.info().clone();
    println!("connected to {} ({}), {} display(s)", info.host_name, info.os, info.displays.len());
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
