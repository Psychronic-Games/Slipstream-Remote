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
        ALPN_REMOTE, ClipFile, ClipboardData, DisplayInfo, HostMsg, InputEvent, PROTOCOL_VERSION, SessionInfo,
        StreamSettings, UploadTarget, VideoFrame, ViewerMsg,
    },
};
use slipstream_platform::clipboard::{Clip, Clipboard, ClipboardSource};
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
    /// Black out the remote computer's own screen, or stop.
    Privacy(bool),
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
    /// The host's video stages (empty for hosts from before 0.6.2).
    pub host: Vec<slipstream_core::proto::PaceStage>,
}

/// Asks the host how fast its video stages run, every few seconds, until
/// it says it can't (older hosts).
async fn poll_host_pace(fs: RemoteFs, out: Arc<Mutex<Vec<slipstream_core::proto::PaceStage>>>) {
    loop {
        match fs.ask(slipstream_core::proto::StreamOpen::Pace).await {
            Ok((_, _, slipstream_core::proto::StreamReply::Pace(stages))) => *out.lock().unwrap() = stages,
            _ => return,
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// "screen capture (GDI) 21 fps 44.0 ms, video encoding 21 fps 30.1 ms"
fn host_pace_text(stages: &[slipstream_core::proto::PaceStage]) -> String {
    stages
        .iter()
        .map(|s| if s.fps == 0.0 && s.ms == 0.0 { s.name.clone() } else { format!("{} {:.0} fps {:.1} ms", s.name, s.fps, s.ms) })
        .collect::<Vec<_>>()
        .join(", ")
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
    conn: Connection,
    fs: RemoteFs,
    frames: Arc<FrameShared>,
    info: SessionInfo,
    peer: DeviceId,
}

impl ViewerSession {
    /// Forwards 127.0.0.1:`local_port` (0 = any) here to `host:port` as
    /// seen from the remote computer, while the session lasts (see `tunnel`).
    pub async fn tunnel(&self, local_port: u16, host: String, port: u16) -> Result<crate::tunnel::Tunnel> {
        if !self.info.capabilities.tunnels {
            bail!("that computer doesn't allow port forwarding (Config → Permissions there)");
        }
        crate::tunnel::open(self.conn.clone(), local_port, host, port).await
    }

    /// The remote computer's hardware and system, for the System
    /// information window.
    pub fn system_info(&self) -> impl Future<Output = Result<Vec<slipstream_core::proto::InfoSection>>> + Send + 'static {
        let fs = self.fs.clone();
        async move {
            // Hosts from before this request drop the stream unanswered.
            let reply = fs
                .ask(slipstream_core::proto::StreamOpen::SystemInfo)
                .await
                .map_err(|e| anyhow::anyhow!("no answer ({e:#}): that computer may need a newer Slipstream"))?;
            match reply.2 {
                slipstream_core::proto::StreamReply::SystemInfo(s) => Ok(s),
                other => bail!("unexpected reply {other:?}"),
            }
        }
    }

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

    /// Copies local files onto the remote computer's clipboard, ready for
    /// the paste that follows (Ctrl+V there pastes them into the folder or
    /// app in front). `progress` gets (bytes sent, bytes in all).
    pub fn paste_files(
        &self,
        paths: Vec<std::path::PathBuf>,
        mut progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> impl Future<Output = Result<usize>> + Send + 'static {
        let caps = &self.info.capabilities;
        let allowed = caps.clipboard && caps.file_transfer;
        let fs = self.fs.clone();
        async move {
            if !allowed {
                bail!("that computer doesn't accept files (Config → Permissions there)");
            }
            let files = tokio::task::spawn_blocking(move || {
                let mut all = Vec::new();
                for p in &paths {
                    all.extend(crate::fsops::walk_local(p)?);
                }
                anyhow::Ok(all)
            })
            .await??;
            let total: u64 = files.iter().map(|f| f.2).sum();
            let batch = rand::random::<u64>();
            let mut sent = 0;
            for (local, rel, _) in &files {
                fs.upload(local, UploadTarget::Clipboard { batch }, rel.clone(), |n| {
                    sent += n;
                    progress(sent, total);
                })
                .await?;
            }
            let (_send, _recv, reply) = fs.ask(slipstream_core::proto::StreamOpen::ClipboardFiles { batch }).await?;
            if !matches!(reply, slipstream_core::proto::StreamReply::Done) {
                bail!("unexpected reply {reply:?}");
            }
            Ok(files.len())
        }
    }
}

/// Biggest copy fetched automatically when the remote computer's clipboard
/// holds files; larger ones are left to the Files window.
const AUTO_FETCH_BYTES: u64 = 64 << 20;
const AUTO_FETCH_FILES: u64 = 500;

/// Fetches files copied on the remote computer and puts them on this
/// computer's clipboard.
async fn fetch_clipboard_files(
    fs: RemoteFs,
    clipboard: Clipboard,
    items: Vec<ClipFile>,
    events: mpsc::UnboundedSender<ViewerEvent>,
) {
    let result = async {
        let dir = tokio::task::spawn_blocking(|| crate::fsops::paste_dir(rand::random())).await?;
        let mut count = 0;
        for item in &items {
            for (remote, rel, _) in fs.walk(item.path.clone(), item.is_dir, item.name.clone(), item.size).await? {
                let dest = dir.join(crate::fsops::safe_relative(&rel)?);
                fs.download(remote, &dest, |_| {}).await?;
                count += 1;
            }
        }
        let top: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        if top.is_empty() {
            bail!("nothing to copy");
        }
        clipboard.set_files(top);
        anyhow::Ok(count)
    }
    .await;
    let notice = match result {
        Ok(1) => "Copied 1 file from the remote computer — ready to paste here".to_string(),
        Ok(n) => format!("Copied {n} files from the remote computer — ready to paste here"),
        Err(e) => format!("Couldn't copy the remote computer's files: {e:#}"),
    };
    let _ = events.send(ViewerEvent::Notice(notice));
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
    let tunnel_conn = conn.clone();
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
    Ok((ViewerSession { cmds: cmd_tx, conn: tunnel_conn, fs, frames, info, peer }, ev_rx))
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
    // The latest fetch of files copied over there (a newer copy replaces it).
    let mut _clip_fetch: Option<Abort<()>> = None;

    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_stats = (Instant::now(), 0u64, 0u64);
    // Every 30 s the link's figures go to the log, with the host's.
    let mut last_log = (Instant::now(), 0u64, 0u64);
    let host_pace = Arc::new(Mutex::new(Vec::new()));
    let _pace_task = Abort(tokio::spawn(poll_host_pace(RemoteFs::new(conn.clone()), host_pace.clone())));

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
                    HostMsg::Clipboard(ClipboardData::Files { items, total_bytes, file_count }) => {
                        let Some(c) = &clipboard else { continue };
                        if total_bytes > AUTO_FETCH_BYTES || file_count > AUTO_FETCH_FILES {
                            let _ = events.send(ViewerEvent::Notice(format!(
                                "Files copied over there ({file_count} files, {} MB) are too many to copy through the clipboard — use Files instead",
                                total_bytes >> 20
                            )));
                        } else if !items.is_empty() {
                            _clip_fetch = Some(Abort(tokio::spawn(fetch_clipboard_files(
                                RemoteFs::new(conn.clone()),
                                c.clone(),
                                items,
                                events.clone(),
                            ))));
                        }
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
            Some(clip) = clip_rx.recv() => {
                // Local files go over on paste (`paste_files`), not on copy.
                if let Clip::Text(text) = clip
                    && text.len() <= 1 << 20
                {
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
                Some(ViewerCmd::Privacy(on)) => write_msg(send, &ViewerMsg::SetPrivacy(on)).await?,
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
                let host = host_pace.lock().unwrap().clone();
                let decode_ms = decode_us.swap(0, Ordering::Relaxed) as f32 / 1000.0 / frames as f32;
                let rtt_ms = rtt.as_secs_f32() * 1000.0;
                if last_log.0.elapsed() >= Duration::from_secs(30) {
                    let secs = last_log.0.elapsed().as_secs_f32();
                    tracing::info!(
                        "link: {width}x{height}, {:.1} fps, {:.0} kbps, round trip {rtt_ms:.0} ms {}, decode {decode_ms:.1} ms; host: {}",
                        (f1 - last_log.2) as f32 / secs,
                        (b1 - last_log.1) as f32 * 8.0 / 1000.0 / secs,
                        if relayed { "relayed" } else { "direct" },
                        if host.is_empty() { "no figures (older version)".to_string() } else { host_pace_text(&host) }
                    );
                    last_log = (Instant::now(), b1, f1);
                }
                let _ = events.send(ViewerEvent::Stats(ViewerStats {
                    fps: (f1 - f0) as f32 / secs,
                    kbps: (b1 - b0) as f32 * 8.0 / 1000.0 / secs,
                    rtt_ms,
                    relayed,
                    width,
                    height,
                    decode_ms,
                    host,
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
