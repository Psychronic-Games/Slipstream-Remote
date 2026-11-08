//! The phone's side: accepts computers on its Wi-Fi Direct group and opens
//! their connections with its own sockets.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointId, RelayMode,
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use slipstream_core::framing::{expect_msg_max, write_msg};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream, UdpSocket},
    task::JoinSet,
};
use tracing::{debug, info, warn};

use crate::{
    Gauge, Stats,
    proto::{self, MAX_CONTROL, Open, Reply},
};

/// How long the phone tries to reach a computer's destination.
const CONNECT_WAIT: Duration = Duration::from_secs(15);
/// A UDP flow with no datagrams either way for this long is closed.
const UDP_IDLE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// The address to listen on: the phone's address on its group, so the
    /// relay can't be reached from any other network.
    pub ip: IpAddr,
    /// Port for finding the phone (TCP) and for QUIC (UDP); 0 picks free
    /// ones (tests).
    pub port: u16,
    /// Let computers reach this device's own loopback and link-local
    /// addresses (tests only).
    pub allow_local_targets: bool,
    /// The relay's identity. Keep it (see [`load_or_create_key`]) so
    /// computers recognise this phone and can connect by themselves.
    pub secret_key: Option<iroh::SecretKey>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self { ip: proto::PHONE_IP, port: proto::PORT, allow_local_targets: false, secret_key: None }
    }
}

/// The relay key kept in `path`, made on first use.
pub fn load_or_create_key(path: &std::path::Path) -> Result<iroh::SecretKey> {
    if let Ok(bytes) = std::fs::read(path)
        && let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok(iroh::SecretKey::from_bytes(&bytes));
    }
    let key = iroh::SecretKey::generate();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, key.to_bytes()).with_context(|| format!("saving {}", path.display()))?;
    Ok(key)
}

/// A running relay; stops when dropped.
pub struct Server {
    endpoint: Endpoint,
    stats: Arc<Stats>,
    hello_addr: SocketAddr,
    _tasks: JoinSet<()>,
}

impl Server {
    pub async fn start(opts: ServerOptions) -> Result<Self> {
        let hello = TcpListener::bind((opts.ip, opts.port)).await.with_context(|| format!("listening on {}:{}", opts.ip, opts.port))?;
        let hello_addr = hello.local_addr()?;
        let mut builder = Endpoint::builder(presets::Minimal);
        if let Some(key) = opts.secret_key.clone() {
            builder = builder.secret_key(key);
        }
        let endpoint = builder
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr(SocketAddr::new(opts.ip, opts.port))
            .context("relay address")?
            .alpns(vec![proto::ALPN.to_vec()])
            .transport_config(crate::transport_config())
            .bind()
            .await
            .context("starting the relay")?;
        let quic_port = endpoint.bound_sockets().first().map(|a| a.port()).context("relay has no socket")?;
        let stats = Arc::new(Stats::default());
        let mut tasks = JoinSet::new();
        tasks.spawn(answer_hellos(hello, proto::hello_line(&endpoint.id(), quic_port)));
        tasks.spawn(accept(endpoint.clone(), stats.clone(), opts.allow_local_targets));
        info!(%hello_addr, quic_port, id = %endpoint.id(), "Subspace Relay listening");
        Ok(Self { endpoint, stats, hello_addr, _tasks: tasks })
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Where computers find this relay (its TCP hello).
    pub fn addr(&self) -> SocketAddr {
        self.hello_addr
    }

    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    pub async fn stop(self) {
        self.endpoint.close().await;
    }
}

async fn answer_hellos(listener: TcpListener, line: String) {
    loop {
        let Ok((mut socket, peer)) = listener.accept().await else { continue };
        debug!(%peer, "relay hello");
        let line = line.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                socket.write_all(line.as_bytes()).await?;
                socket.shutdown().await
            }).await;
        });
    }
}

async fn accept(endpoint: Endpoint, stats: Arc<Stats>, allow_local: bool) {
    while let Some(incoming) = endpoint.accept().await {
        let stats = stats.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => return debug!("relay link failed: {e:#}"),
            };
            info!(computer = %conn.remote_id(), "computer linked through Subspace Relay");
            serve_computer(conn, stats, allow_local).await;
        });
    }
}

async fn serve_computer(conn: Connection, stats: Arc<Stats>, allow_local: bool) {
    let _linked = Gauge::new(&stats.computers);
    let mut flows = JoinSet::new();
    loop {
        tokio::select! {
            stream = conn.accept_bi() => match stream {
                Ok((send, recv)) => {
                    let stats = stats.clone();
                    flows.spawn(async move {
                        if let Err(e) = serve_flow(send, recv, &stats, allow_local).await {
                            debug!("relay flow ended: {e:#}");
                        }
                    });
                }
                Err(e) => {
                    info!(computer = %conn.remote_id(), "computer left Subspace Relay: {e}");
                    break;
                }
            },
            Some(_) = flows.join_next(), if !flows.is_empty() => {}
        }
    }
}

/// Whether the phone should open a connection for a computer to `ip`. The
/// relay is for reaching the internet (and networks the phone is on), not
/// the phone's own services.
fn target_allowed(ip: IpAddr, allow_local: bool) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    if allow_local {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_link_local() || v4.is_broadcast()),
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unicast_link_local()),
    }
}

async fn serve_flow(mut send: SendStream, mut recv: RecvStream, stats: &Stats, allow_local: bool) -> Result<()> {
    let open: Open = tokio::time::timeout(Duration::from_secs(10), expect_msg_max(&mut recv, MAX_CONTROL))
        .await
        .context("no request")??;
    let target = match &open {
        Open::Tcp(a) | Open::Udp(a) => *a,
    };
    if !target_allowed(target.ip(), allow_local) {
        write_msg(&mut send, &Reply::Refused(format!("{} isn't reachable through the relay", target.ip()))).await?;
        let _ = send.finish();
        return Ok(());
    }
    let _flow = Gauge::new(&stats.flows);
    match open {
        Open::Tcp(addr) => {
            let socket = match tokio::time::timeout(CONNECT_WAIT, TcpStream::connect(addr)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return refuse(&mut send, format!("{addr}: {e}")).await,
                Err(_) => return refuse(&mut send, format!("{addr}: no answer")).await,
            };
            let _ = socket.set_nodelay(true);
            write_msg(&mut send, &Reply::Opened).await?;
            let (from_net, to_net) = socket.into_split();
            tokio::try_join!(crate::pump(recv, to_net, &stats.up), crate::pump(from_net, send, &stats.down))?;
        }
        Open::Udp(addr) => {
            let local: SocketAddr = match addr {
                SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
                SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
            };
            let socket = match async {
                let s = UdpSocket::bind(local).await?;
                s.connect(addr).await?;
                std::io::Result::Ok(s)
            }.await {
                Ok(s) => s,
                Err(e) => return refuse(&mut send, format!("{addr}: {e}")).await,
            };
            write_msg(&mut send, &Reply::Opened).await?;
            relay_udp(socket, send, recv, stats).await?;
        }
    }
    Ok(())
}

async fn refuse(send: &mut SendStream, reason: String) -> Result<()> {
    debug!("relay refused: {reason}");
    write_msg(send, &Reply::Refused(reason)).await?;
    let _ = send.finish();
    Ok(())
}

async fn relay_udp(socket: UdpSocket, mut send: SendStream, mut recv: RecvStream, stats: &Stats) -> Result<()> {
    use std::sync::atomic::Ordering;
    let mut from_computer = Vec::with_capacity(2048);
    let mut from_net = vec![0u8; 65536];
    loop {
        tokio::select! {
            d = proto::read_datagram(&mut recv, &mut from_computer) => match d? {
                Some(n) => {
                    if let Err(e) = socket.send(&from_computer[..n]).await {
                        warn!("relay UDP send: {e}");
                    }
                    stats.up.fetch_add(n as u64, Ordering::Relaxed);
                }
                None => break,
            },
            r = socket.recv(&mut from_net) => {
                // ICMP errors (port unreachable) surface here: skip them.
                let Ok(n) = r else { continue };
                proto::write_datagram(&mut send, &from_net[..n]).await?;
                stats.down.fetch_add(n as u64, Ordering::Relaxed);
            }
            _ = tokio::time::sleep(UDP_IDLE) => break,
        }
    }
    let _ = send.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_outside_addresses_are_relayed() {
        let ok = |s: &str| target_allowed(s.parse().unwrap(), false);
        assert!(ok("1.1.1.1"));
        assert!(ok("192.168.1.20"), "networks the phone is on");
        assert!(ok("2606:4700::1111"));
        assert!(!ok("127.0.0.1"));
        assert!(!ok("::1"));
        assert!(!ok("::ffff:127.0.0.1"), "mapped loopback");
        assert!(!ok("169.254.1.1"));
        assert!(!ok("0.0.0.0"));
        assert!(!ok("224.0.0.251"));
        assert!(!ok("255.255.255.255"));
        assert!(target_allowed("127.0.0.1".parse().unwrap(), true));
    }

    #[test]
    fn the_relay_keeps_its_identity_between_starts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.key");
        let first = load_or_create_key(&path).unwrap();
        assert_eq!(load_or_create_key(&path).unwrap().public(), first.public());
        std::fs::write(&path, b"damaged").unwrap();
        assert_ne!(load_or_create_key(&path).unwrap().public(), first.public(), "a damaged key is replaced");
    }
}
