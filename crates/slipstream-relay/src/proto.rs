//! What computer and phone say to each other.
//!
//! The phone answers a TCP connection to [`PORT`] with one line,
//! `SLIPSTREAM-RELAY 1 <endpoint id> <QUIC port>`, and closes it. The
//! computer then links over QUIC ([`ALPN`]). Every bi-stream it opens starts
//! with an [`Open`] (postcard, length-prefixed as in
//! `slipstream_core::framing`), and the phone answers a [`Reply`]. A TCP
//! stream then carries the connection's bytes; a UDP stream carries
//! datagrams, each a little-endian `u16` length and the payload.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"slipstream/relay/1";

/// The phone's TCP port for finding it, and its usual QUIC (UDP) port.
pub const PORT: u16 = 47100;

/// Android's address on its own Wi-Fi Direct group.
pub const PHONE_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 49, 1));

pub(crate) const HELLO: &str = "SLIPSTREAM-RELAY 1";

/// Opening a flow: where the computer's app wanted to go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Open {
    Tcp(SocketAddr),
    Udp(SocketAddr),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Opened,
    Refused(String),
}

/// `Open` and `Reply` are a few dozen bytes.
pub(crate) const MAX_CONTROL: usize = 1024;

pub(crate) fn hello_line(id: &iroh::EndpointId, quic_port: u16) -> String {
    format!("{HELLO} {id} {quic_port}\n")
}

/// The endpoint ID and QUIC port in a phone's hello line.
pub(crate) fn parse_hello(line: &str) -> Option<(iroh::EndpointId, u16)> {
    let rest = line.trim().strip_prefix(HELLO)?.trim();
    let (id, port) = rest.split_once(' ')?;
    Some((id.parse().ok()?, port.parse().ok()?))
}

/// Writes one UDP datagram onto a stream.
pub(crate) async fn write_datagram<W: tokio::io::AsyncWrite + Unpin>(to: &mut W, data: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let len = u16::try_from(data.len()).map_err(|_| std::io::Error::other("datagram too large"))?;
    let mut frame = Vec::with_capacity(data.len() + 2);
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(data);
    to.write_all(&frame).await
}

/// Reads one UDP datagram from a stream into `buf`; `None` at its end.
pub(crate) async fn read_datagram<R: tokio::io::AsyncRead + Unpin>(
    from: &mut R,
    buf: &mut Vec<u8>,
) -> std::io::Result<Option<usize>> {
    use tokio::io::AsyncReadExt;
    let mut len = [0u8; 2];
    match from.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u16::from_le_bytes(len) as usize;
    buf.resize(len, 0);
    from.read_exact(&mut buf[..len]).await?;
    Ok(Some(len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trips() {
        let id = iroh::SecretKey::generate().public();
        assert_eq!(parse_hello(&hello_line(&id, 47100)), Some((id, 47100)));
        assert_eq!(parse_hello("HTTP/1.1 400 Bad Request"), None);
        assert_eq!(parse_hello("SLIPSTREAM-RELAY 1 nonsense 1"), None);
    }

    #[tokio::test]
    async fn datagrams_keep_their_boundaries() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        write_datagram(&mut a, b"one").await.unwrap();
        write_datagram(&mut a, b"").await.unwrap();
        write_datagram(&mut a, &[7u8; 1400]).await.unwrap();
        drop(a);
        let mut buf = Vec::new();
        assert_eq!(read_datagram(&mut b, &mut buf).await.unwrap(), Some(3));
        assert_eq!(&buf[..3], b"one");
        assert_eq!(read_datagram(&mut b, &mut buf).await.unwrap(), Some(0));
        assert_eq!(read_datagram(&mut b, &mut buf).await.unwrap(), Some(1400));
        assert_eq!(read_datagram(&mut b, &mut buf).await.unwrap(), None);
    }
}
