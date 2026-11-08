//! The computer's side: links to the phone and hands it connections.

use std::{
    net::SocketAddr,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode,
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use slipstream_core::framing::{expect_msg_max, write_msg};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};

use crate::{
    Gauge, Stats,
    proto::{self, MAX_CONTROL, Open, Reply},
};

/// Asks the phone at `addr` (its TCP hello) for its relay ID and QUIC port.
pub async fn find_phone(addr: SocketAddr) -> Result<(EndpointId, SocketAddr)> {
    let line = tokio::time::timeout(Duration::from_secs(5), async {
        let mut socket = TcpStream::connect(addr).await?;
        let mut line = String::new();
        (&mut socket).take(256).read_to_string(&mut line).await?;
        std::io::Result::Ok(line)
    })
    .await
    .with_context(|| format!("no answer from {addr}"))?
    .with_context(|| format!("couldn't reach Subspace Relay at {addr}"))?;
    let (id, port) = proto::parse_hello(&line).with_context(|| format!("{addr} isn't a Subspace Relay"))?;
    Ok((id, SocketAddr::new(addr.ip(), port)))
}

/// A link to a phone's relay.
#[derive(Clone)]
pub struct Client {
    endpoint: Endpoint,
    conn: Connection,
    phone: SocketAddr,
    stats: Arc<Stats>,
}

impl Client {
    /// Finds the phone at `hello` and links to it.
    pub async fn connect(hello: SocketAddr) -> Result<Self> {
        let (id, quic) = find_phone(hello).await?;
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .transport_config(crate::transport_config())
            .bind()
            .await
            .context("starting the relay link")?;
        let addr = EndpointAddr::new(id).with_ip_addr(quic);
        let conn = tokio::time::timeout(Duration::from_secs(10), endpoint.connect(addr, proto::ALPN))
            .await
            .context("the phone didn't answer")?
            .context("couldn't link to the phone")?;
        Ok(Self { endpoint, conn, phone: quic, stats: Arc::default() })
    }

    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    /// The phone's QUIC address (kept out of the tunnel).
    pub fn phone_addr(&self) -> SocketAddr {
        self.phone
    }

    /// Why the link ended, once it has.
    pub async fn closed(&self) -> String {
        self.conn.closed().await.to_string()
    }

    pub async fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
        self.endpoint.close().await;
    }

    async fn open(&self, open: Open) -> Result<(SendStream, RecvStream)> {
        let (mut send, mut recv) = self.conn.open_bi().await.context("relay link closed")?;
        write_msg(&mut send, &open).await?;
        match tokio::time::timeout(Duration::from_secs(20), expect_msg_max::<Reply>(&mut recv, MAX_CONTROL))
            .await
            .context("the phone didn't answer")??
        {
            Reply::Opened => Ok((send, recv)),
            Reply::Refused(reason) => bail!("{reason}"),
        }
    }

    /// Carries one TCP connection to `target` through the phone until
    /// either side closes it.
    pub async fn forward_tcp<S: AsyncRead + AsyncWrite>(&self, target: SocketAddr, local: S) -> Result<()> {
        let _flow = Gauge::new(&self.stats.flows);
        let (send, recv) = self.open(Open::Tcp(target)).await?;
        let (from_app, to_app) = tokio::io::split(local);
        tokio::try_join!(crate::pump(from_app, send, &self.stats.up), crate::pump(recv, to_app, &self.stats.down))?;
        Ok(())
    }

    /// Carries one UDP flow to `target` through the phone. Each read from
    /// `local` is one datagram, and each write one datagram back (as with
    /// `ipstack`'s UDP streams); it ends when `local` does.
    pub async fn forward_udp<S: AsyncRead + AsyncWrite>(&self, target: SocketAddr, local: S) -> Result<()> {
        let _flow = Gauge::new(&self.stats.flows);
        let (mut send, mut recv) = self.open(Open::Udp(target)).await?;
        let (mut from_app, mut to_app) = tokio::io::split(local);
        let up = async {
            let mut buf = vec![0u8; 65536];
            loop {
                // A read error is the app's flow timing out: end quietly.
                let n = match from_app.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                proto::write_datagram(&mut send, &buf[..n]).await?;
                self.stats.up.fetch_add(n as u64, Ordering::Relaxed);
            }
            let _ = send.finish();
            anyhow::Ok(())
        };
        let down = async {
            let mut buf = Vec::with_capacity(2048);
            while let Some(n) = proto::read_datagram(&mut recv, &mut buf).await? {
                to_app.write_all(&buf[..n]).await?;
                self.stats.down.fetch_add(n as u64, Ordering::Relaxed);
            }
            anyhow::Ok(())
        };
        // Either direction ending ends the flow.
        tokio::select! {
            r = up => r,
            r = down => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::net::{TcpListener, UdpSocket};

    use super::*;
    use crate::{Server, ServerOptions};

    async fn relay() -> (Server, Client) {
        let server = Server::start(ServerOptions { ip: Ipv4Addr::LOCALHOST.into(), port: 0, allow_local_targets: true })
            .await
            .unwrap();
        let client = Client::connect(server.addr()).await.unwrap();
        (server, client)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_connections_go_through_the_phone_both_ways() {
        let (server, client) = relay().await;
        // A "website": answers what it's sent in upper case, then closes.
        let site = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = site.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = site.accept().await.unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).await.unwrap();
            s.write_all(&got.to_ascii_uppercase()).await.unwrap();
        });
        // The app's end of the connection, as the tunnel would hand it over.
        let (mut app, relayed) = tokio::io::duplex(1 << 16);
        let c = client.clone();
        let flow = tokio::spawn(async move { c.forward_tcp(target, relayed).await });
        let body = vec![b'x'; 300_000];
        app.write_all(&body).await.unwrap();
        app.shutdown().await.unwrap();
        let mut back = Vec::new();
        app.read_to_end(&mut back).await.unwrap();
        assert_eq!(back.len(), body.len());
        assert!(back.iter().all(|&b| b == b'X'));
        flow.await.unwrap().unwrap();
        let (c, s) = (client.stats().snapshot(), server.stats().snapshot());
        assert_eq!((c.up, c.down), (300_000, 300_000));
        assert_eq!((s.up, s.down), (300_000, 300_000));
        assert_eq!(s.computers, 1);
        assert_eq!((c.flows, s.flows), (0, 0), "flows counted down when closed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refusals_reach_the_computer() {
        let (_server, client) = relay().await;
        // Nothing listens on port 1.
        let (_app, relayed) = tokio::io::duplex(1024);
        let err = client.forward_tcp("127.0.0.1:1".parse().unwrap(), relayed).await.unwrap_err();
        assert!(err.to_string().contains("127.0.0.1:1"), "{err:#}");

        let strict = Server::start(ServerOptions { ip: Ipv4Addr::LOCALHOST.into(), port: 0, allow_local_targets: false })
            .await
            .unwrap();
        let client = Client::connect(strict.addr()).await.unwrap();
        let (_app, relayed) = tokio::io::duplex(1024);
        let err = client.forward_tcp("127.0.0.1:80".parse().unwrap(), relayed).await.unwrap_err();
        assert!(err.to_string().contains("isn't reachable"), "{err:#}");
    }

    /// A UDP flow as `ipstack` presents it: one read, one datagram.
    struct Datagrams {
        rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
        tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    }

    impl AsyncRead for Datagrams {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.rx.poll_recv(cx) {
                std::task::Poll::Ready(Some(d)) => {
                    buf.put_slice(&d);
                    std::task::Poll::Ready(Ok(()))
                }
                std::task::Poll::Ready(None) => std::task::Poll::Ready(Ok(())),
                std::task::Poll::Pending => std::task::Poll::Pending,
            }
        }
    }

    impl AsyncWrite for Datagrams {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let _ = self.tx.try_send(buf.to_vec());
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn udp_flows_keep_datagrams_whole() {
        let (server, client) = relay().await;
        // A "DNS server": answers each datagram with its reverse.
        let dns = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = dns.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let (n, from) = dns.recv_from(&mut buf).await.unwrap();
                buf[..n].reverse();
                dns.send_to(&buf[..n], from).await.unwrap();
            }
        });
        let (to_flow, rx) = tokio::sync::mpsc::channel(8);
        let (tx, mut from_flow) = tokio::sync::mpsc::channel(8);
        let c = client.clone();
        let flow = tokio::spawn(async move { c.forward_udp(target, Datagrams { rx, tx }).await });
        for query in [b"abc".to_vec(), vec![1u8, 2, 3, 4], vec![9u8; 1200]] {
            to_flow.send(query.clone()).await.unwrap();
            let answer = tokio::time::timeout(Duration::from_secs(5), from_flow.recv()).await.unwrap().unwrap();
            assert_eq!(answer, query.iter().rev().copied().collect::<Vec<_>>());
        }
        drop(to_flow);
        flow.await.unwrap().unwrap();
        assert_eq!(server.stats().snapshot().up, 3 + 4 + 1200);
    }
}
