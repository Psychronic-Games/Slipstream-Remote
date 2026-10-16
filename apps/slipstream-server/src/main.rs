//! slipstream-server: the ID directory and relay for Slipstream Remote.
//!
//! One binary, two services:
//! * a directory on UDP (QUIC) that hands out 9-digit IDs and tracks which
//!   hosts are online;
//! * an iroh relay (HTTP/HTTPS) that forwards encrypted traffic when two
//!   devices can't reach each other directly.

mod db;
mod directory;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use clap::Parser;
use iroh::{Endpoint, RelayMode, RelayUrl, SecretKey, endpoint::presets};
use iroh_relay::server as relay;
use slipstream_core::directory::{ALPN_DIRECTORY, DEFAULT_DIRECTORY_PORT};
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(version, about = "Slipstream Remote ID directory and relay server")]
struct Cli {
    /// Where to keep the server key and database.
    #[arg(long, default_value = "./slipstream-data", env = "SLIPSTREAM_DATA")]
    data_dir: PathBuf,

    /// UDP port for the directory.
    #[arg(long, default_value_t = DEFAULT_DIRECTORY_PORT)]
    port: u16,

    /// Hostname or IP that clients use to reach this server.
    #[arg(long, env = "SLIPSTREAM_PUBLIC_HOST")]
    public_host: Option<String>,

    /// Disable the embedded relay (clients then rely on public relays).
    #[arg(long)]
    no_relay: bool,

    /// Plain-HTTP relay listen address (also serves the captive-portal probe).
    #[arg(long, default_value = "[::]:21117")]
    relay_http: SocketAddr,

    /// Get a TLS certificate from Let's Encrypt for this domain and serve the
    /// relay over HTTPS. Requires ports 80 and 443 to be reachable.
    #[arg(long)]
    acme_domain: Option<String>,

    /// Contact e-mail for Let's Encrypt.
    #[arg(long)]
    acme_email: Option<String>,

    /// Use the Let's Encrypt staging environment (for testing).
    #[arg(long)]
    acme_staging: bool,

    /// PEM certificate chain for HTTPS (instead of Let's Encrypt).
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,

    /// PEM private key for HTTPS.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,

    /// HTTPS relay listen address, used when TLS is configured.
    #[arg(long, default_value = "[::]:443")]
    relay_https: SocketAddr,

    /// Extra relay URLs to advertise to clients (e.g. other regions).
    #[arg(long)]
    advertise_relay: Vec<RelayUrl>,

    /// Message shown to clients when they connect.
    #[arg(long)]
    motd: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,iroh=warn,iroh_relay=warn,noq=warn".into()),
        )
        .init();
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.data_dir).context("creating data dir")?;
    let secret = load_or_create_key(&cli.data_dir.join("server.key"))?;
    let public_host = cli.public_host.clone().unwrap_or_else(|| {
        warn!("--public-host not set; using 'localhost' (only local clients will work)");
        "localhost".into()
    });

    // Relay.
    let mut relays = cli.advertise_relay.clone();
    let _relay = if cli.no_relay {
        None
    } else {
        let (server, url) = spawn_relay(&cli, &public_host).await?;
        info!("relay listening, advertised as {url}");
        relays.insert(0, url);
        Some(server)
    };

    // Directory.
    let db = db::Db::open(&cli.data_dir.join("directory.sqlite3"))?;
    info!("{} devices known", db.device_count()?);
    let dir = Arc::new(directory::Directory::new(db, relays, cli.motd.clone()));
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(vec![ALPN_DIRECTORY.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], cli.port)))?
        .bind_addr(SocketAddr::from(([0u16; 8], cli.port)))?
        .bind()
        .await
        .context("binding directory port")?;

    let host_part = if public_host.contains(':') { format!("[{public_host}]") } else { public_host.clone() };
    let address = format!("{}@{}:{}", endpoint.id(), host_part, cli.port);
    println!();
    println!("  ╭─ SLIPSTREAM SERVER ONLINE ─────────────────────────────");
    println!("  │ Paste this into each client's Settings → Server:");
    println!("  │");
    println!("  │   {address}");
    println!("  │");
    println!("  ╰────────────────────────────────────────────────────────");
    println!();
    std::fs::write(cli.data_dir.join("server-address.txt"), format!("{address}\n")).ok();

    let stats = dir.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            tick.tick().await;
            info!("{} hosts online", stats.online_count());
        }
    });

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let dir = dir.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => dir.handle_connection(conn).await,
                        Err(e) => tracing::debug!("incoming connection failed: {e:#}"),
                    }
                });
            }
        }
    }
    info!("shutting down");
    endpoint.close().await;
    Ok(())
}

async fn spawn_relay(cli: &Cli, public_host: &str) -> Result<(relay::Server, RelayUrl)> {
    let mut relay_cfg = relay::RelayConfig::new(cli.relay_http);
    let host_part = if public_host.contains(':') { format!("[{public_host}]") } else { public_host.to_string() };
    let https_port = cli.relay_https.port();
    let https_suffix = if https_port == 443 { String::new() } else { format!(":{https_port}") };

    let url = if let Some(domain) = &cli.acme_domain {
        let Some(email) = &cli.acme_email else { bail!("--acme-domain needs --acme-email") };
        let acme = relay::AcmeConfig::letsencrypt(!cli.acme_staging)
            .domains(vec![domain.clone()])
            .contact(vec![format!("mailto:{email}")])
            .cache_path(cli.data_dir.join("acme"));
        relay_cfg.tls = Some(relay::TlsConfig::new(
            cli.relay_https,
            relay::CertConfig::LetsEncrypt { acme_config: acme, server_config_builder: rustls_builder() },
        ));
        format!("https://{domain}{https_suffix}")
    } else if let (Some(cert), Some(key)) = (&cli.tls_cert, &cli.tls_key) {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
        let certs = CertificateDer::pem_file_iter(cert)
            .context("reading --tls-cert")?
            .collect::<Result<Vec<_>, _>>()
            .context("parsing --tls-cert")?;
        let key = PrivateKeyDer::from_pem_file(key).context("reading --tls-key")?;
        let server_config = rustls_builder().with_single_cert(certs, key)?;
        relay_cfg.tls = Some(relay::TlsConfig::new(cli.relay_https, relay::CertConfig::Manual { server_config }));
        format!("https://{host_part}{https_suffix}")
    } else {
        format!("http://{host_part}:{}", cli.relay_http.port())
    };

    let mut cfg = relay::ServerConfig::default();
    cfg.relay = Some(relay_cfg);
    let server = relay::Server::spawn(cfg).await.context("starting relay")?;
    Ok((server, url.parse()?))
}

fn rustls_builder() -> rustls::ConfigBuilder<rustls::ServerConfig, rustls::server::WantsServerCert> {
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports default protocols")
        .with_no_client_auth()
}

fn load_or_create_key(path: &std::path::Path) -> Result<SecretKey> {
    if let Ok(bytes) = std::fs::read(path)
        && let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    std::fs::write(path, key.to_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}
