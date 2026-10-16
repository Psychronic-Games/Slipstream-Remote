//! Length-prefixed `postcard` messages over QUIC streams.

use anyhow::{Context, Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};

/// Upper bound on a single message (a 4K keyframe fits comfortably).
pub const MAX_MESSAGE: usize = 32 * 1024 * 1024;

pub async fn write_msg<T: Serialize>(send: &mut SendStream, msg: &T) -> Result<()> {
    let body = postcard::to_stdvec(msg)?;
    if body.len() > MAX_MESSAGE {
        bail!("message too large: {} bytes", body.len());
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    send.write_all(&buf).await.context("stream write")?;
    Ok(())
}

/// Reads one message; `Ok(None)` on a clean end of stream.
pub async fn read_msg<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<Option<T>> {
    let mut len = [0u8; 4];
    match recv.read_exact(&mut len).await {
        Ok(()) => {}
        Err(iroh::endpoint::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e).context("stream read"),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE {
        bail!("peer sent oversized message: {len} bytes");
    }
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await.context("stream read")?;
    Ok(Some(postcard::from_bytes(&body)?))
}

/// Like [`read_msg`] but treats end of stream as an error.
pub async fn expect_msg<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<T> {
    read_msg(recv).await?.context("peer closed the stream")
}
