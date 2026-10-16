//! slipstream-server: the ID directory and relay for Slipstream Remote, as
//! one self-contained binary on one HTTP port.
//!
//! * `POST /api/` — directory (same protocol as the PHP drop-in server)
//! * `/relay`, `/ping`, `/generate_204` — iroh relay for when two devices
//!   can't connect directly
//! * `GET /` — status page
//!
//! Run it directly, or behind Apache/Nginx/Caddy for HTTPS.

mod api;
mod db;
mod relay;

use std::{
    collections::HashSet,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::State,
    response::Html,
    routing::{any, get, post},
};
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
#[command(version, about = "Slipstream Remote ID directory and relay server")]
struct Cli {
    /// Where to keep the device database.
    #[arg(long, default_value = "./slipstream-data", env = "SLIPSTREAM_DATA")]
    data_dir: PathBuf,

    /// Address to listen on. Use 127.0.0.1:21117 behind a reverse proxy.
    #[arg(long, default_value = "[::]:8080", env = "SLIPSTREAM_LISTEN")]
    listen: SocketAddr,

    /// Public URL of this server (e.g. https://relay.example.com/). Used to
    /// advertise the relay; by default it's taken from each request's Host.
    #[arg(long, env = "SLIPSTREAM_PUBLIC_URL")]
    public_url: Option<String>,

    /// Directory only — don't run a relay (clients use public relays).
    #[arg(long)]
    no_relay: bool,

    /// Relay only — don't serve the directory API (e.g. next to the PHP directory).
    #[arg(long, conflicts_with = "no_relay")]
    relay_only: bool,

    /// Extra relay URLs to advertise to clients (other regions).
    #[arg(long)]
    advertise_relay: Vec<String>,

    /// Message shown on clients' Bridge page.
    #[arg(long, env = "SLIPSTREAM_MOTD")]
    motd: Option<String>,

    /// Make this a private station: clients must present this key to use
    /// the directory and the relay.
    #[arg(long, env = "SLIPSTREAM_ACCESS_KEY")]
    access_key: Option<String>,

    /// Organisation name shown in clients ("Acme IT Support").
    #[arg(long, env = "SLIPSTREAM_ORG_NAME")]
    org_name: Option<String>,

    /// Tell clients not to fall back to the public n0 relays.
    #[arg(long)]
    no_public_relays: bool,
}

pub struct App {
    db: db::Db,
    relay_enabled: bool,
    relay_addr: Option<SocketAddr>,
    public_url: Option<String>,
    extra_relays: Vec<String>,
    motd: Option<String>,
    access_key: Option<String>,
    org_name: Option<String>,
    public_relays: bool,
    limits: api::RateLimiter,
    seen: Mutex<HashSet<String>>,
}

impl App {
    /// True the first time an endpoint registers in this process (for logs).
    fn seen_once(&self, endpoint: &str) -> bool {
        self.seen.lock().unwrap().insert(endpoint.to_string())
    }
}

/// When a station's PHP starts us, we inherit whatever the web server had
/// open (FastCGI pipes, listening sockets). Holding those would pin ports
/// and confuse the web server's process management, so close everything
/// beyond stdin/stdout/stderr before doing anything else.
#[cfg(target_os = "linux")]
fn close_inherited_fds() {
    let fds: Vec<i32> = std::fs::read_dir("/proc/self/fd")
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default();
    for fd in fds.into_iter().filter(|&fd| fd > 2) {
        // SAFETY: nothing in this process uses these yet (we haven't opened
        // anything); the read_dir handle itself is already closed.
        unsafe { libc::close(fd) };
    }
}

fn main() -> Result<()> {
    #[cfg(target_os = "linux")]
    close_inherited_fds();
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run())
}

async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,iroh=warn,iroh_relay=warn,noq=warn".into()),
        )
        .init();
    slipstream_core::init_crypto();
    let cli = Cli::parse();
    let app = build(&cli).await?;
    let router = router(app.clone(), cli.relay_only);

    let listener = tokio::net::TcpListener::bind(cli.listen).await.with_context(|| format!("binding {}", cli.listen))?;
    let (total, online) = app.db.counts()?;
    info!("{total} devices known, {online} online");
    let shown = cli.public_url.clone().unwrap_or_else(|| format!("http://{}/", cli.listen));
    println!();
    println!("  ╭─ SLIPSTREAM SERVER ONLINE ──────────────────────────────");
    println!("  │ Listening on {}", cli.listen);
    println!("  │ Directory: {}", if cli.relay_only { "off" } else { "on" });
    println!("  │ Relay:     {}", if app.relay_enabled { "on" } else { "off (public relays)" });
    println!("  │ Access:    {}", if app.access_key.is_some() { "private (access key required)" } else { "open" });
    println!("  │");
    println!("  │ In each app: CONFIG → Server address → {shown}");
    println!("  ╰─────────────────────────────────────────────────────────");
    println!();
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn build(cli: &Cli) -> Result<Arc<App>> {
    std::fs::create_dir_all(&cli.data_dir).context("creating data dir")?;
    let db = db::Db::open(&cli.data_dir.join("directory.sqlite3"))?;
    let relay_addr = if cli.no_relay {
        None
    } else {
        let (server, addr) = relay::spawn(cli.access_key.clone().filter(|k| !k.is_empty())).await?;
        // Keep the relay alive for the life of the process.
        Box::leak(Box::new(server));
        Some(addr)
    };
    Ok(Arc::new(App {
        db,
        relay_enabled: relay_addr.is_some(),
        relay_addr,
        public_url: cli.public_url.clone().map(|u| if u.ends_with('/') { u } else { format!("{u}/") }),
        extra_relays: cli.advertise_relay.clone(),
        motd: cli.motd.clone(),
        access_key: cli.access_key.clone().filter(|k| !k.is_empty()),
        org_name: cli.org_name.clone(),
        public_relays: !cli.no_public_relays,
        limits: Default::default(),
        seen: Mutex::new(HashSet::new()),
    }))
}

fn router(app: Arc<App>, relay_only: bool) -> Router {
    let mut r = Router::new()
        .route("/relay", any(relay::proxy))
        .route("/ping", any(relay::proxy))
        .route("/generate_204", any(relay::proxy))
        .route("/", get(status));
    if !relay_only {
        r = r.route("/api/", post(api::handle)).route("/api", post(api::handle));
    }
    r.with_state(app)
}

async fn status(State(app): State<Arc<App>>) -> Html<String> {
    let (total, online) = app.db.counts().unwrap_or_default();
    Html(format!(
        r#"<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Slipstream server</title>
<style>body{{background:#020308;color:#ffd6aa;font:18px/1.5 system-ui,sans-serif;margin:0;padding:24px}}
.bar{{background:#ff9900;border-radius:28px 0 0 0;height:28px;margin-bottom:24px}}
h1{{color:#ff9900;letter-spacing:.06em;text-transform:uppercase;margin:0 0 12px}}
.k{{color:#cc99cc;text-transform:uppercase;font-size:14px;letter-spacing:.08em}}
.ok{{color:#66ffaa}}</style>
<div class="bar"></div><h1>Slipstream Remote server</h1>
<p class="ok">● ONLINE · v{}</p>
<p><span class="k">Devices known</span><br>{total}</p>
<p><span class="k">Online now</span><br>{online}</p>
<p><span class="k">Relay</span><br>{}</p>
<p>Enter this site's address in the Slipstream app under CONFIG → Server address.</p>"#,
        slipstream_core::APP_VERSION,
        if app.relay_enabled { "built-in relay active" } else { "public relays" }
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::{Endpoint, RelayMap, RelayMode, endpoint::presets};
    use slipstream_core::directory::{DeviceInfo, DirectoryClient, Lookup};

    use super::*;

    async fn start() -> (String, Arc<App>) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("slipstream-server-test-{}-{n}", std::process::id()));
        let cli = Cli::parse_from(["slipstream-server", "--data-dir", dir.to_str().unwrap()]);
        let app = build(&cli).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let r = router(app.clone(), false);
        tokio::spawn(async move {
            axum::serve(listener, r.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        (url, app)
    }

    /// Directory over HTTP: hello, signed register, lookup, unregister.
    #[tokio::test]
    async fn directory_roundtrip() {
        let (url, _app) = start().await;
        let mut dc = DirectoryClient::new(&url, None).unwrap();
        let hello = dc.hello().await.unwrap();
        assert_eq!(hello.relays.len(), 1, "relay should be advertised");
        let secret = iroh::SecretKey::generate();
        let addr = iroh::EndpointAddr::from_parts(secret.public(), []);
        let info = DeviceInfo { name: "test-host".into(), os: "Linux".into(), app_version: "0".into() };
        let id = dc.register(&secret, addr.clone(), info.clone()).await.unwrap();
        assert_eq!(dc.register(&secret, addr, info).await.unwrap(), id, "IDs are stable");
        match dc.lookup(id).await.unwrap() {
            Lookup::Online { addr, info } => {
                assert_eq!(addr.id, secret.public());
                assert_eq!(info.name, "test-host");
            }
            other => panic!("expected online, got {other:?}"),
        }
        dc.unregister(&secret).await.unwrap();
        assert!(matches!(dc.lookup(id).await.unwrap(), Lookup::Offline { .. }));
        assert!(matches!(dc.lookup(slipstream_core::directory::DeviceId(123456789)).await.unwrap(), Lookup::NotFound));
    }

    /// Two endpoints that can only talk through our proxied relay.
    #[tokio::test]
    async fn relay_through_proxy() {
        let (url, _app) = start().await;
        let relay_url: iroh::RelayUrl = url.parse().unwrap();
        let ep = |alpn: bool| {
            let map = RelayMap::from_iter([relay_url.clone()]);
            let b = Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Custom(map))
                .clear_ip_transports(); // relay only: no direct path possible
            let b = if alpn { b.alpns(vec![b"test".to_vec()]) } else { b };
            b.bind()
        };
        let a = ep(true).await.unwrap();
        let b = ep(false).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), a.online()).await.expect("a reached the relay");
        let addr = iroh::EndpointAddr::from_parts(a.id(), [iroh::TransportAddr::Relay(relay_url)]);
        let accept = tokio::spawn(async move {
            let conn = a.accept().await.unwrap().await.unwrap();
            let (mut s, mut r) = conn.accept_bi().await.unwrap();
            let msg = r.read_to_end(100).await.unwrap();
            s.write_all(&msg).await.unwrap();
            s.finish().unwrap();
            conn.closed().await;
        });
        let conn = tokio::time::timeout(Duration::from_secs(10), b.connect(addr, b"test")).await.unwrap().unwrap();
        let (mut s, mut r) = conn.open_bi().await.unwrap();
        s.write_all(b"hello via relay").await.unwrap();
        s.finish().unwrap();
        let echo = r.read_to_end(100).await.unwrap();
        assert_eq!(echo, b"hello via relay");
        conn.close(0u32.into(), b"done");
        let _ = tokio::time::timeout(Duration::from_secs(2), accept).await;
    }
}
