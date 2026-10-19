//! Port forwarding: a TCP port on the viewer's computer that reaches
//! `host:port` as seen from the remote computer (its own RDP, a web admin
//! page, another machine on its network). Each TCP connection gets a QUIC
//! stream of its own, opened with [`StreamOpen::Tunnel`]; the host has to
//! allow forwarding (off by default).

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use slipstream_core::{
    framing::{expect_msg, write_msg},
    proto::{StreamOpen, StreamReply},
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

/// Copies bytes both ways until both sides are done, passing on each
/// side's end of stream (half-close) to the other.
pub(crate) async fn splice(tcp: TcpStream, mut send: SendStream, mut recv: RecvStream) {
    let _ = tcp.set_nodelay(true);
    let stopped = send.stopped();
    let (mut from_tcp, mut to_tcp) = tcp.into_split();
    let up = async {
        let _ = tokio::io::copy(&mut recv, &mut to_tcp).await;
        let _ = to_tcp.shutdown().await;
    };
    let down = async {
        let _ = tokio::io::copy(&mut from_tcp, &mut send).await;
        let _ = send.finish();
    };
    let copying = async {tokio::join!(up,down);};
    tokio::pin!(copying);
    tokio::select! {
        _ = &mut copying => {},
        reason = stopped => {
            // A peer cancelling its receive half closes the whole forward.
            // A clean FIN acknowledgement still allows the opposite half
            // to finish (e.g. a response after the requester half-closes).
            if matches!(reason,Ok(None)) {copying.await;}
        }
    }
}

/// What the UI shows about a tunnel.
#[derive(Debug, Default, Clone)]
pub struct TunnelStatus {
    /// Connections open right now.
    pub active: usize,
    /// Connections made so far.
    pub total: usize,
    /// The remote side's last refusal ("can't reach …"), if any.
    pub last_error: Option<String>,
}

/// A forwarded port; closes when dropped.
pub struct Tunnel {
    pub local_port: u16,
    pub host: String,
    pub port: u16,
    status: Arc<Mutex<TunnelStatus>>,
    task: tokio::task::JoinHandle<()>,
}

impl Tunnel {
    pub fn status(&self) -> TunnelStatus {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Listens on 127.0.0.1:`local_port` (0 = any free port) and forwards each
/// connection to `host:port` on the far side of `conn`.
pub async fn open(conn: Connection, local_port: u16, host: String, port: u16) -> Result<Tunnel> {
    let listener = TcpListener::bind(("127.0.0.1", local_port))
        .await
        .with_context(|| format!("port {local_port} is already in use on this computer"))?;
    let local_port = listener.local_addr()?.port();
    let status = Arc::new(Mutex::new(TunnelStatus::default()));
    let task = {
        let (status, host) = (status.clone(), host.clone());
        tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = conn.closed() => break,
                    _ = connections.join_next(), if !connections.is_empty() => {},
                    incoming = listener.accept(), if connections.len() < 64 => {
                        let Ok((tcp,_)) = incoming else {break};
                        let (conn,status,host) = (conn.clone(),status.clone(),host.clone());
                        connections.spawn(async move {
                            let _active = ActiveConnection::new(status.clone());
                            if let Err(e) = forward(&conn,tcp,host,port).await {
                                status.lock().unwrap().last_error = Some(format!("{e:#}"));
                            }
                        });
                    }
                }
            }
            // Aborting the listener also aborts every live forwarded socket.
        })
    };
    tracing::info!(local_port, %host, port, "tunnel listening");
    Ok(Tunnel { local_port, host, port, status, task })
}

struct ActiveConnection(Arc<Mutex<TunnelStatus>>);
impl ActiveConnection {
    fn new(status:Arc<Mutex<TunnelStatus>>)->Self {
        {let mut s=status.lock().unwrap();s.active+=1;s.total+=1;}
        Self(status)
    }
}
impl Drop for ActiveConnection {
    fn drop(&mut self) {self.0.lock().unwrap().active-=1;}
}

async fn forward(conn: &Connection, tcp: TcpStream, host: String, port: u16) -> Result<()> {
    let (mut send, mut recv) = conn.open_bi().await.context("the link closed")?;
    write_msg(&mut send, &StreamOpen::Tunnel { host, port }).await?;
    match expect_msg::<StreamReply>(&mut recv).await? {
        StreamReply::Ready => {
            splice(tcp, send, recv).await;
            Ok(())
        }
        StreamReply::Error(e) => anyhow::bail!(e),
        other => anyhow::bail!("unexpected reply {other:?}"),
    }
}
