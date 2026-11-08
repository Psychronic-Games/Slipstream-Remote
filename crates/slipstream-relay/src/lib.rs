//! Subspace Relay: a computer's whole connection through a phone's mobile
//! data (see `docs/issues/19-subspace-relay.md`).
//!
//! The phone runs a [`Server`] on its Wi-Fi Direct group. The computer
//! finds it ([`find_phone`]), links to it ([`Client::connect`]) and hands it
//! each TCP connection and UDP flow its apps make ([`Client::forward_tcp`],
//! [`Client::forward_udp`]); the phone opens the real connections with its
//! own sockets. On desktops, [`tunnel`] captures the computer's traffic.

pub mod proto;
mod client;
mod server;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub mod tunnel;

use std::{
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

pub use client::{Client, find_phone};

/// What `slipstream relay connect --control` (running as administrator)
/// tells the app that started it over a local TCP connection: first the
/// token the app gave it (so the app knows it's its helper), then one JSON
/// line per status. The helper stops when the app closes its side.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperStatus {
    Linking,
    Up(StatsSnapshot),
    /// The tunnel stopped (routes and DNS restored), and why.
    Down(String),
    Failed(String),
}

pub use server::{Server, ServerOptions, load_or_create_key};

/// Data moved and links in use, for status displays. "Up" is from the
/// computer towards the internet, "down" back to it.
#[derive(Debug, Default)]
pub struct Stats {
    pub up: AtomicU64,
    pub down: AtomicU64,
    pub flows: AtomicUsize,
    pub computers: AtomicUsize,
}

/// A copy of [`Stats`] at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StatsSnapshot {
    pub up: u64,
    pub down: u64,
    pub flows: usize,
    pub computers: usize,
}

impl Stats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            up: self.up.load(Ordering::Relaxed),
            down: self.down.load(Ordering::Relaxed),
            flows: self.flows.load(Ordering::Relaxed),
            computers: self.computers.load(Ordering::Relaxed),
        }
    }
}

/// Counts a value up for as long as it lives.
pub(crate) struct Gauge<'a>(&'a AtomicUsize);

impl<'a> Gauge<'a> {
    pub(crate) fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for Gauge<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// QUIC settings for the link between computer and phone: one stream per
/// TCP connection or UDP flow (browsers open hundreds), windows sized for a
/// fast local Wi-Fi link, and keep-alives so an idle link stays up.
pub(crate) fn transport_config() -> iroh::endpoint::QuicTransportConfig {
    let mut builder = iroh::endpoint::QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(4096u32.into())
        .max_concurrent_uni_streams(0u32.into())
        .stream_receive_window((4u32 << 20).into())
        .receive_window((64u32 << 20).into())
        .send_window(64 << 20)
        .keep_alive_interval(Duration::from_secs(5));
    // As in `slipstream_core::net`: some Android UDP stacks stall on
    // segmentation offload.
    if cfg!(target_os = "android") {
        builder = builder.enable_segmentation_offload(false);
    }
    builder.build()
}

/// Copies one direction of a stream, counting bytes, then half-closes the
/// writer so the other side sees the end.
pub(crate) async fn pump<R, W>(mut from: R, mut to: W, counter: &AtomicU64) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n]).await?;
        counter.fetch_add(n as u64, Ordering::Relaxed);
    }
    to.shutdown().await
}
