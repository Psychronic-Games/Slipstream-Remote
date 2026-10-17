//! Length-prefixed `postcard` messages over QUIC streams.

use anyhow::{Context, Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};
use tokio::{sync::mpsc, task::JoinHandle};

/// Upper bound on a single message (a 4K keyframe fits comfortably).
pub const MAX_MESSAGE: usize = 32 * 1024 * 1024;

pub async fn write_msg<T: Serialize>(send: &mut SendStream, msg: &T) -> Result<()> {
    // Serialize after a length placeholder, so the body isn't copied again.
    let mut buf = postcard::to_extend(msg, vec![0u8; 4])?;
    let len = buf.len() - 4;
    if len > MAX_MESSAGE {
        bail!("message too large: {len} bytes");
    }
    buf[..4].copy_from_slice(&(len as u32).to_le_bytes());
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

/// Reads a stream's messages on a task of its own, so a loop can wait for
/// them in `select!` next to other events. [`read_msg`] can't be used there:
/// it isn't cancel-safe, so a message half read when another branch wins
/// would be lost and the stream would fall out of step.
pub struct MsgReader<T> {
    rx: mpsc::Receiver<Result<Option<T>>>,
    task: JoinHandle<()>,
}

impl<T: DeserializeOwned + Send + 'static> MsgReader<T> {
    pub fn spawn(mut recv: RecvStream) -> Self {
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            loop {
                let msg = read_msg::<T>(&mut recv).await;
                let last = !matches!(msg, Ok(Some(_)));
                if tx.send(msg).await.is_err() || last {
                    break;
                }
            }
        });
        Self { rx, task }
    }

    /// The next message; `Ok(None)` once the stream has ended. Cancel-safe.
    pub async fn next(&mut self) -> Result<Option<T>> {
        self.rx.recv().await.unwrap_or(Ok(None))
    }
}

impl<T> Drop for MsgReader<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Like [`read_msg`] but treats end of stream as an error.
pub async fn expect_msg<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<T> {
    read_msg(recv).await?.context("peer closed the stream")
}
