//! Viewer side of a session.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, EndpointAddr, endpoint::Connection};
use slipstream_codec::{Decoder, YuvFrame};
use slipstream_core::{
    auth::ViewerAuth,
    config::ViewerSettings,
    directory::DeviceId,
    framing::{MsgReader, expect_msg, read_msg, write_msg},
    proto::{
        ALPN_REMOTE, ClipboardData, DisplayInfo, HostMsg, InputEvent, PROTOCOL_VERSION, SessionInfo, StreamSettings,
        VideoFrame, ViewerMsg,
    },
};
use slipstream_platform::clipboard::{Clipboard, ClipboardSource};
use tokio::sync::mpsc;
use tracing::debug;

use crate::files::RemoteFs;

/// Commands from the UI.
#[derive(Debug)]
pub enum ViewerCmd {
    Input(InputEvent),
    SelectDisplay(u32),
    Settings(StreamSettings),
    Chat(String),
    /// Push text to the remote clipboard now (e.g. right before a paste).
    Clipboard(String),
    /// Play the remote computer's sound, or stop.
    Audio(bool),
    Disconnect,
}

/// Where the viewer plays the remote computer's sound.
#[derive(Debug, Clone, Default)]
pub enum AudioOut {
    #[default]
    Speakers,
    /// Collect the decoded samples (tests).
    Memory(Arc<Mutex<Vec<i16>>>),
}

/// Session events for the UI.
#[derive(Debug, Clone)]
pub enum ViewerEvent {
    StreamStarted { display: u32, width: u32, height: u32 },
    Displays(Vec<DisplayInfo>),
    Chat(String),
    Notice(String),
    Stats(ViewerStats),
    Disconnected { reason: String },
}

#[derive(Debug, Clone, Default)]
pub struct ViewerStats {
    pub fps: f32,
    pub kbps: f32,
    pub rtt_ms: f32,
    pub relayed: bool,
    pub width: u32,
    pub height: u32,
    /// Host capture → displayed on this machine is not measurable without
    /// synced clocks; this is decode time per frame instead.
    pub decode_ms: f32,
}

/// Latest decoded frame, shared with the renderer.
#[derive(Default)]
pub struct FrameShared {
    latest: Mutex<Option<Arc<YuvFrame>>>,
    serial: AtomicU64,
    repaint: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl FrameShared {
    /// The newest frame and its serial number (increments per frame).
    pub fn latest(&self) -> (u64, Option<Arc<YuvFrame>>) {
        (self.serial.load(Ordering::Acquire), self.latest.lock().unwrap().clone())
    }

    /// Called whenever a new frame arrives (e.g. `egui::Context::request_repaint`).
    pub fn set_repaint(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.repaint.lock().unwrap() = Some(Box::new(f));
    }

    fn publish(&self, frame: YuvFrame) {
        *self.latest.lock().unwrap() = Some(Arc::new(frame));
        self.serial.fetch_add(1, Ordering::Release);
        if let Some(f) = &*self.repaint.lock().unwrap() {
            f();
        }
    }
}

/// A live remote-control session. Dropping it disconnects.
pub struct ViewerSession {
    cmds: mpsc::UnboundedSender<ViewerCmd>,
    fs: RemoteFs,
    frames: Arc<FrameShared>,
    info: SessionInfo,
    peer: DeviceId,
}

impl ViewerSession {
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn peer_id(&self) -> DeviceId {
        self.peer
    }

    pub fn frames(&self) -> Arc<FrameShared> {
        self.frames.clone()
    }

    pub fn send(&self, cmd: ViewerCmd) {
        let _ = self.cmds.send(cmd);
    }

    pub fn input(&self, ev: InputEvent) {
        self.send(ViewerCmd::Input(ev));
    }

    /// The remote computer's files (browse, upload, download).
    pub fn fs(&self) -> RemoteFs {
        self.fs.clone()
    }

    pub fn command_sender(&self) -> mpsc::UnboundedSender<ViewerCmd> {
        self.cmds.clone()
    }
}

impl Drop for ViewerSession {
    fn drop(&mut self) {
        let _ = self.cmds.send(ViewerCmd::Disconnect);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn connect(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    peer: DeviceId,
    password: &str,
    viewer_name: String,
    settings: ViewerSettings,
    clipboard: ClipboardSource,
    audio_out: AudioOut,
    on_progress: Box<dyn Fn(String) + Send + Sync>,
) -> Result<(ViewerSession, mpsc::UnboundedReceiver<ViewerEvent>)> {
    on_progress("Opening encrypted channel…".into());
    let host_id = addr.id;
    let conn = tokio::time::timeout(Duration::from_secs(20), endpoint.connect(addr, ALPN_REMOTE))
        .await
        .context("timed out reaching the device")?
        .context("could not reach the device")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.set_priority(slipstream_core::proto::PRIORITY_CONTROL).ok();
    write_msg(&mut send, &ViewerMsg::Hello { version: PROTOCOL_VERSION, viewer_name }).await?;

    on_progress("Authenticating…".into());
    let pakes = match expect_msg::<HostMsg>(&mut recv).await? {
        HostMsg::AuthChallenge { pakes } => pakes,
        HostMsg::AuthFailed { reason, retry_after_secs } => bail!(auth_error(reason, retry_after_secs)),
        other => bail!("unexpected message from host: {other:?}"),
    };
    let (auth, pakes, confirms) = ViewerAuth::respond(password, &pakes, &host_id, &endpoint.id())?;
    write_msg(&mut send, &ViewerMsg::AuthResponse { pakes, confirms }).await?;

    let info = loop {
        match expect_msg::<HostMsg>(&mut recv).await? {
            HostMsg::AuthOk { confirm, session } => {
                auth.verify_host(&confirm)?;
                break session;
            }
            HostMsg::Notice(n) => on_progress(n),
            HostMsg::AuthFailed { reason, retry_after_secs } => bail!(auth_error(reason, retry_after_secs)),
            other => bail!("unexpected message from host: {other:?}"),
        }
    };
    on_progress("Connected".into());

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let frames = Arc::new(FrameShared::default());
    let display = info.displays.iter().find(|d| d.primary).or(info.displays.first()).map(|d| d.index).unwrap_or(0);
    let stream_settings = StreamSettings {
        bitrate_kbps: settings.bitrate_kbps,
        max_fps: settings.max_fps.clamp(1, 144),
        ..Default::default()
    };
    let clipboard = (settings.sync_clipboard && info.capabilities.clipboard).then_some(clipboard);
    let audio = settings.play_audio && info.capabilities.audio;
    let fs = RemoteFs::new(conn.clone());
    tokio::spawn(run(
        conn,
        send,
        recv,
        cmd_rx,
        ev_tx,
        frames.clone(),
        display,
        stream_settings,
        clipboard,
        audio,
        audio_out,
    ));
    Ok((ViewerSession { cmds: cmd_tx, fs, frames, info, peer }, ev_rx))
}

/// Opens the speakers and asks the host for its sound. A speaker problem is
/// reported as a notice, not an error: the session carries on silently.
async fn start_audio(
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    events: &mpsc::UnboundedSender<ViewerEvent>,
    out: &AudioOut,
) -> Result<Option<Abort<()>>> {
    let player = match out {
        AudioOut::Speakers => slipstream_audio::Player::start(),
        AudioOut::Memory(m) => Ok(slipstream_audio::Player::memory(m.clone())),
    };
    match player {
        Ok(player) => {
            let task = Abort(tokio::spawn(receive_audio(conn.clone(), player)));
            write_msg(send, &ViewerMsg::SetAudio(true)).await?;
            Ok(Some(task))
        }
        Err(e) => {
            let _ = events.send(ViewerEvent::Notice(format!("Can't play sound here: {e:#}")));
            Ok(None)
        }
    }
}

/// Decodes the host's audio datagrams into `player`. Up to three lost
/// packets in a row are concealed; late or repeated ones are dropped.
async fn receive_audio(conn: Connection, player: slipstream_audio::Player) {
    use slipstream_audio::opus::{Decoder, FRAME_SAMPLES};
    let Ok(mut decoder) = Decoder::new() else { return };
    let mut pcm = [0i16; FRAME_SAMPLES];
    let mut expect: Option<u16> = None;
    while let Ok(d) = conn.read_datagram().await {
        if d.len() < 3 || d[0] != slipstream_core::proto::AUDIO_DATAGRAM {
            continue;
        }
        let seq = u16::from_le_bytes([d[1], d[2]]);
        if let Some(next) = expect {
            match seq.wrapping_sub(next) {
                0 => {}
                gap @ 1..=3 => {
                    for _ in 0..gap {
                        if decoder.decode(None, &mut pcm).is_ok() {
                            player.push(&pcm);
                        }
                    }
                }
                0x8000.. => continue, // late or repeated
                _ => {}               // a long gap: just carry on
            }
        }
        expect = Some(seq.wrapping_add(1));
        if decoder.decode(Some(&d[3..]), &mut pcm).is_ok() {
            player.push(&pcm);
        }
    }
}

fn auth_error(reason: String, retry_after: u32) -> String {
    if retry_after > 0 { format!("{reason} (wait {retry_after}s)") } else { reason }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    conn: Connection,
    mut send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    mut cmds: mpsc::UnboundedReceiver<ViewerCmd>,
    events: mpsc::UnboundedSender<ViewerEvent>,
    frames: Arc<FrameShared>,
    display: u32,
    stream_settings: StreamSettings,
    sync_clipboard: Option<ClipboardSource>,
    audio: bool,
    audio_out: AudioOut,
) {
    let mut recv = MsgReader::<HostMsg>::spawn(recv);
    let reason = match session(&conn, &mut send, &mut recv, &mut cmds, &events, frames, display, stream_settings, sync_clipboard, audio, audio_out)
        .await
    {
        Ok(r) => r,
        Err(e) => format!("{e:#}"),
    };
    let _ = write_msg(&mut send, &ViewerMsg::Bye).await;
    crate::flush(&mut send).await;
    conn.close(0u32.into(), b"bye");
    let _ = events.send(ViewerEvent::Disconnected { reason });
}

/// Messages the decoder thread hands back to the async side.
enum DecoderOut {
    Ack(u64),
    NeedKeyframe,
}

#[allow(clippy::too_many_arguments)]
async fn session(
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut MsgReader<HostMsg>,
    cmds: &mut mpsc::UnboundedReceiver<ViewerCmd>,
    events: &mpsc::UnboundedSender<ViewerEvent>,
    frames: Arc<FrameShared>,
    display: u32,
    settings: StreamSettings,
    sync_clipboard: Option<ClipboardSource>,
    play_audio: bool,
    audio_out: AudioOut,
) -> Result<String> {
    write_msg(send, &ViewerMsg::StartStream { display, settings }).await?;

    // Sound: datagrams → decoder → player, while wanted.
    let mut _audio: Option<Abort<()>> = None;
    if play_audio {
        _audio = start_audio(conn, send, events, &audio_out).await?;
    }

    // Video: network → decoder thread.
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<VideoFrame>();
    let (dec_tx, mut dec_rx) = mpsc::unbounded_channel::<DecoderOut>();
    let bytes_rx = Arc::new(AtomicU64::new(0));
    let decode_us = Arc::new(AtomicU64::new(0));
    let decoded = Arc::new(AtomicU64::new(0));
    let dims = Arc::new(Mutex::new((0u32, 0u32)));
    {
        let (decode_us, decoded, dims) = (decode_us.clone(), decoded.clone(), dims.clone());
        thread::Builder::new()
            .name("decoder".into())
            .spawn(move || decode_loop(frame_rx, dec_tx, frames, decode_us, decoded, dims))?;
    }
    let video_conn = conn.clone();
    let video_bytes = bytes_rx.clone();
    let video_task = tokio::spawn(async move {
        let mut stream = video_conn.accept_uni().await?;
        while let Some(frame) = read_msg::<VideoFrame>(&mut stream).await? {
            video_bytes.fetch_add(frame.data.len() as u64 + 32, Ordering::Relaxed);
            if frame_tx.send(frame).is_err() {
                break;
            }
        }
        anyhow::Ok(())
    });
    let _video_guard = Abort(video_task);

    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel();
    let clipboard = sync_clipboard.and_then(|source| Clipboard::start(clip_tx, &source));

    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_stats = (Instant::now(), 0u64, 0u64);

    loop {
        tokio::select! {
            msg = recv.next() => {
                let msg = match msg {
                    Ok(m) => m,
                    // The read fails first when the connection closes.
                    Err(e) => return conn.close_reason().map(|r| crate::closed_reason(&r, "the host")).ok_or(e),
                };
                let Some(msg) = msg else { return Ok("the host ended the session".into()) };
                match msg {
                    HostMsg::StreamStarted { display, width, height } => {
                        let _ = events.send(ViewerEvent::StreamStarted { display, width, height });
                    }
                    HostMsg::Displays(d) => { let _ = events.send(ViewerEvent::Displays(d)); }
                    HostMsg::Clipboard(ClipboardData::Text(t)) => {
                        if let Some(c) = &clipboard { c.set_text(t); }
                    }
                    HostMsg::Chat(t) => { let _ = events.send(ViewerEvent::Chat(t)); }
                    HostMsg::Notice(n) => { let _ = events.send(ViewerEvent::Notice(n)); }
                    HostMsg::Bye { reason } => return Ok(reason),
                    other => debug!("ignoring {other:?}"),
                }
            }
            Some(out) = dec_rx.recv() => match out {
                DecoderOut::Ack(seq) => write_msg(send, &ViewerMsg::FrameAck { seq }).await?,
                DecoderOut::NeedKeyframe => write_msg(send, &ViewerMsg::RequestKeyframe).await?,
            },
            Some(text) = clip_rx.recv() => {
                if text.len() <= 1 << 20 {
                    write_msg(send, &ViewerMsg::Clipboard(ClipboardData::Text(text))).await?;
                }
            }
            cmd = cmds.recv() => match cmd {
                Some(ViewerCmd::Input(ev)) => write_msg(send, &ViewerMsg::Input(ev)).await?,
                Some(ViewerCmd::SelectDisplay(d)) => {
                    write_msg(send, &ViewerMsg::StartStream { display: d, settings }).await?;
                }
                Some(ViewerCmd::Settings(s)) => write_msg(send, &ViewerMsg::SetSettings(s)).await?,
                Some(ViewerCmd::Chat(t)) => write_msg(send, &ViewerMsg::Chat(t)).await?,
                Some(ViewerCmd::Clipboard(t)) => {
                    write_msg(send, &ViewerMsg::Clipboard(ClipboardData::Text(t))).await?;
                }
                Some(ViewerCmd::Audio(on)) => {
                    _audio = None;
                    if on {
                        _audio = start_audio(conn, send, events, &audio_out).await?;
                    } else {
                        write_msg(send, &ViewerMsg::SetAudio(false)).await?;
                    }
                }
                Some(ViewerCmd::Disconnect) | None => return Ok("disconnected".into()),
            },
            _ = stats_tick.tick() => {
                let (t0, b0, f0) = last_stats;
                let (b1, f1) = (bytes_rx.load(Ordering::Relaxed), decoded.load(Ordering::Relaxed));
                let secs = t0.elapsed().as_secs_f32().max(0.001);
                let (rtt, relayed) = conn
                    .paths()
                    .iter()
                    .find(|p| p.is_selected())
                    .map(|p| (p.rtt(), p.is_relay()))
                    .unwrap_or_default();
                let (width, height) = *dims.lock().unwrap();
                let frames = (f1 - f0).max(1);
                let _ = events.send(ViewerEvent::Stats(ViewerStats {
                    fps: (f1 - f0) as f32 / secs,
                    kbps: (b1 - b0) as f32 * 8.0 / 1000.0 / secs,
                    rtt_ms: rtt.as_secs_f32() * 1000.0,
                    relayed,
                    width,
                    height,
                    decode_ms: decode_us.swap(0, Ordering::Relaxed) as f32 / 1000.0 / frames as f32,
                }));
                last_stats = (Instant::now(), b1, f1);
            }
            reason = conn.closed() => return Ok(crate::closed_reason(&reason, "the host")),
        }
    }
}

fn decode_loop(
    rx: std::sync::mpsc::Receiver<VideoFrame>,
    out: mpsc::UnboundedSender<DecoderOut>,
    frames: Arc<FrameShared>,
    decode_us: Arc<AtomicU64>,
    decoded: Arc<AtomicU64>,
    dims: Arc<Mutex<(u32, u32)>>,
) {
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("decoder init failed: {e:#}");
            return;
        }
    };
    let mut waiting_for_key = false;
    while let Ok(frame) = rx.recv() {
        let seq = frame.seq;
        if waiting_for_key && !frame.keyframe {
            let _ = out.send(DecoderOut::Ack(seq));
            continue;
        }
        let t = Instant::now();
        match decoder.decode(&frame.data) {
            Ok(Some(yuv)) => {
                waiting_for_key = false;
                decode_us.fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                decoded.fetch_add(1, Ordering::Relaxed);
                *dims.lock().unwrap() = (yuv.width, yuv.height);
                frames.publish(yuv);
            }
            Ok(None) => {}
            Err(e) => {
                debug!("decode error: {e:#}; requesting keyframe");
                waiting_for_key = true;
                let _ = out.send(DecoderOut::NeedKeyframe);
                if let Ok(d) = Decoder::new() {
                    decoder = d;
                }
            }
        }
        if out.send(DecoderOut::Ack(seq)).is_err() {
            return;
        }
    }
}

struct Abort<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for Abort<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
