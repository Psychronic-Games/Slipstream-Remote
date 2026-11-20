//! Viewer side of a session.

use std::{
    collections::HashMap,
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
    framing::{MsgReader, expect_msg_max, expect_msg_with_extras, read_msg, write_msg},
    proto::{
        ALPN_REMOTE, Capabilities, ClipFile, ClipboardData, DisplayInfo, HelloExtras, HostExtras, HostMsg, InputEvent,
        PROTOCOL_VERSION, SessionInfo, StreamSettings, UploadTarget, VideoFrame, ViewerMsg,
    },
};
use slipstream_platform::clipboard::{Clip, Clipboard, ClipboardSource};
use tokio::sync::mpsc;
use tracing::debug;

use crate::files::RemoteFs;

/// Commands from the UI.
#[derive(Debug)]
pub enum ViewerCmd {
    /// Tests: end as if the network had cut the link off.
    #[cfg(test)]
    Interrupt,
    Input(InputEvent),
    /// Input on one display: pointer positions are on that display (hosts
    /// that stream one display at a time apply it to the one streaming).
    InputOn(u32, InputEvent),
    SelectDisplay(u32),
    /// Stream these displays at once (the ones on view). Hosts that stream
    /// one display at a time switch to the first, if it isn't the one
    /// streaming already.
    ShowDisplays(Vec<u32>),
    Settings(StreamSettings),
    Chat(String),
    /// Push text to the remote clipboard now (e.g. right before a paste).
    Clipboard(String),
    /// Play the remote computer's sound, or stop.
    Audio(bool),
    /// Black out the remote computer's own screen, or stop.
    Privacy(bool),
    /// Turn a chat-only link into a full session: start the picture, and
    /// the clipboard and sound the viewer's settings ask for. Does nothing
    /// once the picture has started.
    StartViewing,
    Disconnect,
}

/// What a new link starts with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkMode {
    /// Picture, clipboard and sound as the viewer's settings say.
    #[default]
    Full,
    /// Chat only: authenticated as usual, but the host captures nothing and
    /// no clipboard or sound is exchanged until [`ViewerCmd::StartViewing`].
    ChatOnly,
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
    StreamStarted {
        display: u32,
        width: u32,
        height: u32,
    },
    Displays(Vec<DisplayInfo>),
    /// What the host can do changed (a chat-only link's screen opened).
    Capabilities(Capabilities),
    Chat(String),
    Notice(String),
    Stats(ViewerStats),
    Disconnected {
        reason: String,
        retryable: bool,
    },
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
/// it says it can't (older hosts). An unanswered request means the host
/// stopped responding only if nothing else arrived from it meanwhile
/// (`received` counts what the connection received): on a slow, busy link
/// the answer can just be late.
async fn poll_host_pace(
    fs: RemoteFs,
    out: Arc<Mutex<Vec<slipstream_core::proto::PaceStage>>>,
    received: impl Fn() -> u64,
) -> Option<String> {
    loop {
        let before = received();
        match tokio::time::timeout(Duration::from_secs(6), fs.ask(slipstream_core::proto::StreamOpen::Pace)).await {
            Ok(Ok((_, _, slipstream_core::proto::StreamReply::Pace(stages)))) => *out.lock().unwrap() = stages,
            Err(_) if received() != before => {}
            Err(_) => return Some("The remote host stopped responding; it may be restarting".into()),
            // Older hosts can decline this optional request. A real connection
            // close is handled by the control stream, not mistaken for a hang.
            _ => return None,
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// "screen capture (GDI) 21 fps 44.0 ms, video encoding 21 fps 30.1 ms"
fn host_pace_text(stages: &[slipstream_core::proto::PaceStage]) -> String {
    stages
        .iter()
        .map(|s| {
            if s.fps == 0.0 && s.ms == 0.0 {
                s.name.clone()
            } else {
                format!("{} {:.0} fps {:.1} ms", s.name, s.fps, s.ms)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Latest decoded frame, shared with the renderer.
#[derive(Default)]
pub struct FrameShared {
    latest: Mutex<Option<Arc<YuvFrame>>>,
    /// A frame already shown and no longer held by anything: the decoder
    /// fills the next one into its buffers.
    spare: Mutex<Option<YuvFrame>>,
    serial: AtomicU64,
    repaint: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl FrameShared {
    /// The newest frame and its serial number (increments per frame).
    pub fn latest(&self) -> (u64, Option<Arc<YuvFrame>>) {
        let latest = self.latest.lock().unwrap();
        (self.serial.load(Ordering::Acquire), latest.clone())
    }

    /// Called whenever a new frame arrives (e.g. `egui::Context::request_repaint`).
    fn take_spare(&self) -> Option<YuvFrame> {
        self.spare.lock().unwrap().take()
    }

    pub fn set_repaint(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.repaint.lock().unwrap() = Some(Arc::new(f));
    }

    fn publish(&self, frame: YuvFrame) {
        let old = {
            let mut latest = self.latest.lock().unwrap();
            let old = latest.replace(Arc::new(frame));
            self.serial.fetch_add(1, Ordering::Release);
            old
        };
        // Drawn already (the renderer let go of it): its buffers are reused.
        if let Some(old) = old
            && let Ok(old) = Arc::try_unwrap(old)
        {
            *self.spare.lock().unwrap() = Some(old);
        }
        let repaint = self.repaint.lock().unwrap().clone();
        if let Some(f) = repaint {
            f();
        }
    }
}

/// Each display's latest frame, for hosts that stream several at once.
type ScreenFrames = Arc<Mutex<HashMap<u32, Arc<FrameShared>>>>;

fn screen_frames(screens: &ScreenFrames, display: u32) -> Arc<FrameShared> {
    screens.lock().unwrap().entry(display).or_default().clone()
}

/// A live remote-control session. Dropping it disconnects.
pub struct ViewerSession {
    cmds: mpsc::UnboundedSender<ViewerCmd>,
    conn: Connection,
    fs: RemoteFs,
    frames: Arc<FrameShared>,
    screens: ScreenFrames,
    extras: HostExtras,
    /// The display shown first.
    display: u32,
    info: SessionInfo,
    peer: DeviceId,
    /// The frame rate this viewer asked for (Config → Max frame rate).
    max_fps: u32,
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
    pub fn system_info(
        &self,
    ) -> impl Future<Output = Result<Vec<slipstream_core::proto::InfoSection>>> + Send + 'static {
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

    /// The frame rate this viewer asked for when it connected.
    pub fn max_fps(&self) -> u32 {
        self.max_fps
    }

    /// The pictures of the display shown first (see [`Self::frames_for`]).
    pub fn frames(&self) -> Arc<FrameShared> {
        self.frames_for(self.display)
    }

    /// `display`'s pictures. A host that streams one display at a time has
    /// one picture for all of them: whichever is streaming
    /// ([`ViewerEvent::StreamStarted`] says which).
    pub fn frames_for(&self, display: u32) -> Arc<FrameShared> {
        if self.extras.multi_display { screen_frames(&self.screens, display) } else { self.frames.clone() }
    }

    /// The host streams several displays at once ([`ViewerCmd::ShowDisplays`]).
    pub fn multi_display(&self) -> bool {
        self.extras.multi_display
    }

    /// The host types characters ([`InputEvent::Text`]); otherwise text goes
    /// as the keys of a US keyboard.
    pub fn text_input(&self) -> bool {
        self.extras.text_input
    }

    pub fn send(&self, cmd: ViewerCmd) {
        let _ = self.cmds.send(cmd);
    }

    pub fn input(&self, ev: InputEvent) {
        self.send(ViewerCmd::Input(ev));
    }

    /// Input on `display` (pointer positions are on it).
    pub fn input_on(&self, display: u32, ev: InputEvent) {
        self.send(ViewerCmd::InputOn(display, ev));
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
        // Clears out pastes from over a day ago, as the host does.
        let dir = tokio::task::spawn_blocking(|| crate::fsops::make_paste_dir(rand::random())).await?;
        // The host's summary stops counting after a few seconds (a slow
        // drive), so the limits are checked again on what's really there.
        let mut files = Vec::new();
        let (mut bytes, mut count) = (0u64, 0u64);
        for item in &items {
            for entry in fs.walk(item.path.clone(), item.is_dir, item.name.clone(), item.size).await? {
                bytes += entry.2;
                count += 1;
                if bytes > AUTO_FETCH_BYTES || count > AUTO_FETCH_FILES {
                    bail!("they're too many to copy through the clipboard — use Files instead");
                }
                files.push(entry);
            }
        }
        for (remote, rel, _) in files {
            let dest = dir.join(crate::fsops::safe_relative(&rel)?);
            fs.download(remote, &dest, |_| {}).await?;
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
    mode: LinkMode,
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
    // A chat-only link asks the host not to open its screen yet (older hosts
    // ignore the extras and open it as before).
    let hello = ViewerMsg::Hello { version: PROTOCOL_VERSION, viewer_name };
    write_msg(&mut send, &(hello, HelloExtras { chat_only: mode == LinkMode::ChatOnly })).await?;

    on_progress("Authenticating…".into());
    let pakes = match expect_msg_max::<HostMsg>(&mut recv, PRE_AUTH_MESSAGE).await? {
        HostMsg::AuthChallenge { pakes } => pakes,
        HostMsg::AuthFailed { reason, retry_after_secs } => return Err(auth_error(reason, retry_after_secs)),
        other => bail!("unexpected message from host: {other:?}"),
    };
    let (auth, pakes, confirms) = ViewerAuth::respond(password, &pakes, &host_id, &endpoint.id())?;
    write_msg(&mut send, &ViewerMsg::AuthResponse { pakes, confirms }).await?;

    let (info, extras) = loop {
        match expect_msg_with_extras::<HostMsg, HostExtras>(&mut recv, PRE_AUTH_MESSAGE).await? {
            (HostMsg::AuthOk { confirm, session }, extras) => {
                auth.verify_host(&confirm)
                    .map_err(|e| AuthRejected(format!("Remote identity verification failed: {e:#}")))?;
                break (session, extras);
            }
            (HostMsg::Notice(n), _) => on_progress(n),
            (HostMsg::AuthFailed { reason, retry_after_secs }, _) => return Err(auth_error(reason, retry_after_secs)),
            (other, _) => bail!("unexpected message from host: {other:?}"),
        }
    };
    on_progress("Connected".into());

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let frames = Arc::new(FrameShared::default());
    let screens = ScreenFrames::default();
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
        Video { frames: frames.clone(), screens: screens.clone(), multi: extras.multi_display },
        display,
        stream_settings,
        clipboard,
        audio,
        audio_out,
        mode == LinkMode::Full,
    ));
    let max_fps = stream_settings.max_fps;
    Ok((
        ViewerSession { cmds: cmd_tx, conn: tunnel_conn, fs, frames, screens, extras, display, info, peer, max_fps },
        ev_rx,
    ))
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
/// The next command for the host. Pointer moves queued up behind each other
/// (the link was slow to take them) go as one, the latest: the pointer jumps
/// to where it is now rather than replaying its whole path late.
async fn next_cmd(pending: &mut Option<ViewerCmd>, cmds: &mut mpsc::UnboundedReceiver<ViewerCmd>) -> Option<ViewerCmd> {
    if let Some(cmd) = pending.take() {
        return Some(cmd);
    }
    let mut cmd = cmds.recv().await?;
    while pointer_move(&cmd).is_some() {
        match cmds.try_recv() {
            Ok(next) if pointer_move(&next).is_some() && pointer_move(&next) == pointer_move(&cmd) => cmd = next,
            Ok(next) => {
                *pending = Some(next);
                break;
            }
            Err(_) => break,
        }
    }
    Some(cmd)
}

/// Which display a pointer move is on (`Some(None)`: the one streaming).
fn pointer_move(cmd: &ViewerCmd) -> Option<Option<u32>> {
    match cmd {
        ViewerCmd::Input(InputEvent::MouseMove { .. }) => Some(None),
        ViewerCmd::InputOn(d, InputEvent::MouseMove { .. }) => Some(Some(*d)),
        _ => None,
    }
}

/// Copied text up to this long (about a hundred pages) goes over as soon
/// as it's copied; longer text only when it's pasted over there with the
/// keyboard. It shares the stream input and frame acknowledgements take,
/// and a megabyte held them up for seconds on a slow link.
const AUTO_CLIP_TEXT: usize = 256 << 10;
/// Pasted text longer than this isn't sent (messages stop at 32 MiB, and
/// one that size would hold up input for minutes on a slow link).
const MAX_CLIP_TEXT: usize = 8 << 20;

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

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
                // A little late (reordered) or repeated.
                0xFFCD.. => continue,
                // Far behind: the host started its sound again (muted, then
                // unmuted), counting from 0. Before, these were all dropped
                // as late until the count caught up: minutes of silence.
                0x8000.. => {}
                _ => {} // a long gap: just carry on
            }
        }
        expect = Some(seq.wrapping_add(1));
        if decoder.decode(Some(&d[3..]), &mut pcm).is_ok() {
            player.push(&pcm);
        }
    }
}

#[derive(Debug)]
pub(crate) struct AuthRejected(pub String);
impl std::fmt::Display for AuthRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AuthRejected {}
/// A refusal ends automatic reconnects ([`AuthRejected`]) unless the host
/// says it's only for now: busy with another sign-in, or no screen yet.
fn auth_error(reason: String, retry_after: u32) -> anyhow::Error {
    let transient = reason.contains("busy with another sign-in")
        || reason.starts_with("screen sharing unavailable")
        || reason.starts_with("No screen is currently available")
        || reason.starts_with("On the phone, tap Start sharing");
    let text = if retry_after > 0 { format!("{reason} (wait {retry_after}s)") } else { reason };
    if transient { anyhow::anyhow!(text) } else { AuthRejected(text).into() }
}

/// Host messages before the session starts are small (the session info
/// lists displays and capabilities).
const PRE_AUTH_MESSAGE: usize = 1024 * 1024;

struct SessionEnd {
    reason: String,
    retryable: bool,
}
impl SessionEnd {
    fn deliberate(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), retryable: false }
    }
    fn interrupted(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), retryable: true }
    }
    fn connection(reason: &iroh::endpoint::ConnectionError) -> Self {
        if matches!(reason,iroh::endpoint::ConnectionError::ApplicationClosed(c) if c.error_code==slipstream_core::proto::RESTART_CLOSE_CODE.into())
        {
            return Self::interrupted(slipstream_core::proto::HOST_RESTARTING);
        }
        Self {
            reason: crate::closed_reason(reason, "the host"),
            retryable: !matches!(reason, iroh::endpoint::ConnectionError::ApplicationClosed(_)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    conn: Connection,
    mut send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    mut cmds: mpsc::UnboundedReceiver<ViewerCmd>,
    events: mpsc::UnboundedSender<ViewerEvent>,
    video: Video,
    display: u32,
    stream_settings: StreamSettings,
    sync_clipboard: Option<ClipboardSource>,
    audio: bool,
    audio_out: AudioOut,
    viewing: bool,
) {
    let mut recv = MsgReader::<HostMsg>::spawn(recv);
    let ended = match session(
        &conn,
        &mut send,
        &mut recv,
        &mut cmds,
        &events,
        video,
        display,
        stream_settings,
        sync_clipboard,
        audio,
        audio_out,
        viewing,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => conn
            .close_reason()
            .map(|r| SessionEnd::connection(&r))
            .unwrap_or_else(|| SessionEnd::interrupted(format!("{e:#}"))),
    };
    if ended.retryable {
        // Cut off, not ended: no Bye, and a close that tells the host this
        // viewer means to reconnect (it keeps its one-time password for it).
        conn.close(slipstream_core::proto::INTERRUPTED_CLOSE_CODE.into(), b"interrupted");
    } else {
        let _ = tokio::time::timeout(Duration::from_millis(250), write_msg(&mut send, &ViewerMsg::Bye)).await;
        crate::flush(&mut send).await;
        conn.close(0u32.into(), b"bye");
    }
    let _ = events.send(ViewerEvent::Disconnected { reason: ended.reason, retryable: ended.retryable });
}

/// Where the pictures go: one picture (`frames`), or one per display when
/// the host streams several at once (`multi`).
struct Video {
    frames: Arc<FrameShared>,
    screens: ScreenFrames,
    multi: bool,
}

/// Messages a decoder thread hands back to the async side, with the
/// display it decodes when the host streams several.
enum DecoderOut {
    Ack(Option<u32>, u64),
    NeedKeyframe(Option<u32>),
}

/// The figures every decoder adds to: bytes received, decoding time,
/// frames decoded, the latest picture size.
#[derive(Clone, Default)]
struct VideoCounters {
    bytes: Arc<AtomicU64>,
    decode_us: Arc<AtomicU64>,
    decoded: Arc<AtomicU64>,
    dims: Arc<Mutex<(u32, u32)>>,
    /// Decoders running: the frame rate shown is per picture.
    live: Arc<AtomicU64>,
}

/// Starts a decoder thread for one stream's frames.
fn spawn_decoder(
    display: Option<u32>,
    frames: Arc<FrameShared>,
    out: mpsc::UnboundedSender<DecoderOut>,
    counters: &VideoCounters,
) -> Result<mpsc::Sender<VideoFrame>> {
    let (tx, rx) = mpsc::channel::<VideoFrame>(2);
    let counters = counters.clone();
    thread::Builder::new().name("decoder".into()).spawn(move || decode_loop(display, rx, out, frames, counters))?;
    Ok(tx)
}

/// Several displays at once: each arrives on its own stream, which starts
/// with the display's index.
async fn receive_screens(
    conn: Connection,
    screens: ScreenFrames,
    out: mpsc::UnboundedSender<DecoderOut>,
    counters: VideoCounters,
) -> Result<()> {
    loop {
        let mut stream = conn.accept_uni().await?;
        let (screens, out, counters) = (screens.clone(), out.clone(), counters.clone());
        tokio::spawn(async move {
            let Ok(Some(display)) = read_msg::<u32>(&mut stream).await else { return };
            let Ok(tx) = spawn_decoder(Some(display), screen_frames(&screens, display), out, &counters) else { return };
            while let Ok(Some(frame)) = read_msg::<VideoFrame>(&mut stream).await {
                counters.bytes.fetch_add(frame.data.len() as u64 + 32, Ordering::Relaxed);
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn session(
    conn: &Connection,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut MsgReader<HostMsg>,
    cmds: &mut mpsc::UnboundedReceiver<ViewerCmd>,
    events: &mpsc::UnboundedSender<ViewerEvent>,
    video: Video,
    display: u32,
    settings: StreamSettings,
    sync_clipboard: Option<ClipboardSource>,
    play_audio: bool,
    audio_out: AudioOut,
    mut viewing: bool,
) -> Result<SessionEnd> {
    let (mut display, mut settings) = (display, settings);
    let multi = video.multi;
    // Sound: datagrams → decoder → player, while wanted.
    let mut _audio: Option<Abort<()>> = None;
    if viewing {
        write_msg(send, &start_message(multi, vec![display], settings)).await?;
        if play_audio {
            _audio = start_audio(conn, send, events, &audio_out).await?;
        }
    }

    // Video: network → decoder thread(s).
    let (dec_tx, mut dec_rx) = mpsc::unbounded_channel::<DecoderOut>();
    let counters = VideoCounters::default();
    let (bytes_rx, decode_us, decoded, dims, live) = (
        counters.bytes.clone(),
        counters.decode_us.clone(),
        counters.decoded.clone(),
        counters.dims.clone(),
        counters.live.clone(),
    );
    let video_task = if multi {
        tokio::spawn(receive_screens(conn.clone(), video.screens, dec_tx, counters))
    } else {
        let frame_tx = spawn_decoder(None, video.frames, dec_tx, &counters)?;
        let video_conn = conn.clone();
        tokio::spawn(async move {
            let mut stream = video_conn.accept_uni().await?;
            while let Some(frame) = read_msg::<VideoFrame>(&mut stream).await? {
                counters.bytes.fetch_add(frame.data.len() as u64 + 32, Ordering::Relaxed);
                if frame_tx.send(frame).await.is_err() {
                    break;
                }
            }
            anyhow::Ok(())
        })
    };
    let _video_guard = Abort(video_task);

    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel();
    // A chat-only link starts the clipboard with the picture (StartViewing).
    let mut clipboard = if viewing {
        sync_clipboard.as_ref().and_then(|source| Clipboard::start(clip_tx.clone(), source))
    } else {
        None
    };
    // The latest fetch of files copied over there (a newer copy replaces it).
    let mut _clip_fetch: Option<Abort<()>> = None;
    // The text last put on the host's clipboard, so a paste doesn't send it again.
    let mut clip_sent: Option<u64> = None;
    // A command read while merging pointer moves (see `next_cmd`).
    let mut pending_cmd: Option<ViewerCmd> = None;

    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_stats = (Instant::now(), 0u64, 0u64);
    // Every 30 s the link's figures go to the log, with the host's.
    let mut last_log = (Instant::now(), 0u64, 0u64);
    let host_pace = Arc::new(Mutex::new(Vec::new()));
    let (health_tx, mut health_rx) = mpsc::channel(1);
    let fs = RemoteFs::new(conn.clone());
    let pace_out = host_pace.clone();
    let pace_conn = conn.clone();
    let _pace_task = Abort(tokio::spawn(async move {
        if let Some(reason) = poll_host_pace(fs, pace_out, move || pace_conn.stats().udp_rx.bytes).await {
            let _ = health_tx.send(reason).await;
        }
    }));

    loop {
        tokio::select! {
            msg = recv.next() => {
                let msg = match msg {
                    Ok(m) => m,
                    // The read fails first when the connection closes.
                    Err(e) => return conn.close_reason().map(|r|SessionEnd::connection(&r)).ok_or(e),
                };
                let Some(msg) = msg else { return Ok(SessionEnd::interrupted("The host connection ended unexpectedly")) };
                match msg {
                    HostMsg::StreamStarted { display, width, height } => {
                        let _ = events.send(ViewerEvent::StreamStarted { display, width, height });
                    }
                    HostMsg::Displays(d) => { let _ = events.send(ViewerEvent::Displays(d)); }
                    HostMsg::Capabilities(c) => { let _ = events.send(ViewerEvent::Capabilities(c)); }
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
                    HostMsg::Bye { reason } => return Ok(if reason==slipstream_core::proto::HOST_RESTARTING {SessionEnd::interrupted(reason)}else{SessionEnd::deliberate(reason)}),
                    other => debug!("ignoring {other:?}"),
                }
            }
            Some(out) = dec_rx.recv() => match out {
                DecoderOut::Ack(None, seq) => write_msg(send, &ViewerMsg::FrameAck { seq }).await?,
                DecoderOut::Ack(Some(display), seq) => write_msg(send, &ViewerMsg::FrameAckOn { display, seq }).await?,
                DecoderOut::NeedKeyframe(None) => write_msg(send, &ViewerMsg::RequestKeyframe).await?,
                DecoderOut::NeedKeyframe(Some(display)) => write_msg(send, &ViewerMsg::RequestKeyframeOn { display }).await?,
            },
            Some(clip) = clip_rx.recv() => {
                // Local files go over on paste (`paste_files`), not on copy.
                // So does longer text: it shares the stream that carries
                // input and frame acknowledgements, and a big copy sent on
                // every copy froze the mouse and picture on a slow link.
                if let Clip::Text(text) = clip
                    && text.len() <= AUTO_CLIP_TEXT
                {
                    clip_sent = Some(text_hash(&text));
                    write_msg(send, &ViewerMsg::Clipboard(ClipboardData::Text(text))).await?;
                }
            }
            cmd = next_cmd(&mut pending_cmd, cmds) => match cmd {
                Some(ViewerCmd::Input(ev)) => write_msg(send, &ViewerMsg::Input(ev)).await?,
                Some(ViewerCmd::InputOn(d, event)) => {
                    let msg = if multi { ViewerMsg::InputOn { display: d, event } } else { ViewerMsg::Input(event) };
                    write_msg(send, &msg).await?;
                }
                Some(ViewerCmd::SelectDisplay(d)) if viewing => {
                    display = d;
                    write_msg(send, &start_message(multi, vec![d], settings)).await?;
                }
                // Nothing on view: a host streaming several displays pauses
                // them all; one streaming a single display carries on.
                Some(ViewerCmd::ShowDisplays(shown)) if viewing || shown.is_empty() => {
                    if multi {
                        write_msg(send, &ViewerMsg::ShowDisplays { displays: shown, settings }).await?;
                    } else if let Some(&d) = shown.first() && !shown.contains(&display) {
                        display = d;
                        write_msg(send, &ViewerMsg::StartStream { display: d, settings }).await?;
                    }
                }
                // Remembered, so switching displays keeps the chosen quality.
                Some(ViewerCmd::Settings(s)) => {
                    settings = s;
                    write_msg(send, &ViewerMsg::SetSettings(s)).await?;
                }
                Some(ViewerCmd::Chat(t)) => write_msg(send, &ViewerMsg::Chat(t)).await?,
                // Pasting there: the text goes over unless it already has.
                Some(ViewerCmd::Clipboard(t)) => {
                    let hash = text_hash(&t);
                    if t.len() > MAX_CLIP_TEXT {
                        let _ = events.send(ViewerEvent::Notice(format!(
                            "The copied text ({} MB) is too long to paste over there; send it as a file with Files",
                            t.len() >> 20
                        )));
                    } else if clip_sent != Some(hash) {
                        clip_sent = Some(hash);
                        write_msg(send, &ViewerMsg::Clipboard(ClipboardData::Text(t))).await?;
                    }
                }
                Some(ViewerCmd::Privacy(on)) => write_msg(send, &ViewerMsg::SetPrivacy(on)).await?,
                // Choosing a display on a chat-only link starts viewing it.
                Some(cmd @ (ViewerCmd::StartViewing | ViewerCmd::SelectDisplay(_) | ViewerCmd::ShowDisplays(_))) => if !viewing {
                    let shown = match cmd {
                        ViewerCmd::SelectDisplay(d) => vec![d],
                        ViewerCmd::ShowDisplays(shown) => shown,
                        _ => vec![display],
                    };
                    if let Some(&d) = shown.first() { display = d; }
                    viewing = true;
                    write_msg(send, &start_message(multi, shown, settings)).await?;
                    clipboard = sync_clipboard.as_ref().and_then(|source| Clipboard::start(clip_tx.clone(), source));
                    if play_audio && _audio.is_none() {
                        _audio = start_audio(conn, send, events, &audio_out).await?;
                    }
                },
                Some(ViewerCmd::Audio(on)) => {
                    _audio = None;
                    if on {
                        _audio = start_audio(conn, send, events, &audio_out).await?;
                    } else {
                        write_msg(send, &ViewerMsg::SetAudio(false)).await?;
                    }
                }
                Some(ViewerCmd::Disconnect) | None => return Ok(SessionEnd::deliberate("disconnected")),
                #[cfg(test)]
                Some(ViewerCmd::Interrupt) => return Ok(SessionEnd::interrupted("cut off")),
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
                // Several screens at once: the rate of each picture (the
                // bandwidth stays the total).
                let pictures = live.load(Ordering::Relaxed).max(1) as f32;
                let rtt_ms = rtt.as_secs_f32() * 1000.0;
                if !viewing {
                    last_log = (Instant::now(), b1, f1);
                } else if last_log.0.elapsed() >= Duration::from_secs(30) {
                    let secs = last_log.0.elapsed().as_secs_f32();
                    tracing::info!(
                        "link: {width}x{height}, {:.1} fps, {:.0} kbps, round trip {rtt_ms:.0} ms {}, decode {decode_ms:.1} ms; host: {}",
                        (f1 - last_log.2) as f32 / secs / pictures,
                        (b1 - last_log.1) as f32 * 8.0 / 1000.0 / secs,
                        if relayed { "relayed" } else { "direct" },
                        if host.is_empty() { "no figures (older version)".to_string() } else { host_pace_text(&host) }
                    );
                    last_log = (Instant::now(), b1, f1);
                }
                let _ = events.send(ViewerEvent::Stats(ViewerStats {
                    fps: (f1 - f0) as f32 / secs / pictures,
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
            Some(reason)=health_rx.recv() => return Ok(SessionEnd::interrupted(reason)),
            reason = conn.closed() => return Ok(SessionEnd::connection(&reason)),
        }
    }
}

/// What starts the picture: the displays to show at once, or the first of
/// them on a host that streams one at a time.
fn start_message(multi: bool, displays: Vec<u32>, settings: StreamSettings) -> ViewerMsg {
    if multi {
        ViewerMsg::ShowDisplays { displays, settings }
    } else {
        ViewerMsg::StartStream { display: displays.first().copied().unwrap_or(0), settings }
    }
}

fn decode_loop(
    display: Option<u32>,
    mut rx: mpsc::Receiver<VideoFrame>,
    out: mpsc::UnboundedSender<DecoderOut>,
    frames: Arc<FrameShared>,
    counters: VideoCounters,
) {
    let VideoCounters { decode_us, decoded, dims, live, .. } = counters;
    live.fetch_add(1, Ordering::Relaxed);
    struct Live(Arc<AtomicU64>);
    impl Drop for Live {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let _live = Live(live);
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("decoder init failed: {e:#}");
            return;
        }
    };
    let mut waiting_for_key = false;
    while let Some(frame) = rx.blocking_recv() {
        let seq = frame.seq;
        if waiting_for_key && !frame.keyframe {
            let _ = out.send(DecoderOut::Ack(display, seq));
            continue;
        }
        let t = Instant::now();
        match decoder.decode_reusing(&frame.data, frames.take_spare()) {
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
                let _ = out.send(DecoderOut::NeedKeyframe(display));
                if let Ok(d) = Decoder::new() {
                    decoder = d;
                }
            }
        }
        if out.send(DecoderOut::Ack(display, seq)).is_err() {
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

#[cfg(test)]
mod health_tests {
    use super::*;

    #[test]
    fn frame_serials_match_pixels_and_repaint_callbacks_can_reenter() {
        let shared = Arc::new(FrameShared::default());
        let weak = Arc::downgrade(&shared);
        shared.set_repaint(move || {
            let shared = weak.upgrade().unwrap();
            let _ = shared.latest();
            shared.set_repaint(|| {});
        });
        let producer = shared.clone();
        let task = std::thread::spawn(move || {
            for n in 1..=2000u64 {
                producer.publish(YuvFrame {
                    width: 2,
                    height: 2,
                    y: n.to_le_bytes().to_vec(),
                    u: vec![0],
                    v: vec![0],
                    y_stride: 2,
                    uv_stride: 1,
                });
            }
        });
        while !task.is_finished() {
            let (serial, frame) = shared.latest();
            if let Some(frame) = frame {
                assert_eq!(serial, u64::from_le_bytes(frame.y[..8].try_into().unwrap()));
            }
        }
        task.join().unwrap();
        assert_eq!(shared.latest().0, 2000);
    }

    #[tokio::test]
    async fn stalled_probe_is_detected_but_unsupported_probe_is_not_an_outage() {
        slipstream_core::init_crypto();
        let config = slipstream_core::config::ServerSettings { use_public_relays: false, ..Default::default() };
        for unsupported in [false, true] {
            let host = slipstream_core::net::bind_client(iroh::SecretKey::generate(), &config, true).await.unwrap();
            let viewer = slipstream_core::net::bind_client(iroh::SecretKey::generate(), &config, false).await.unwrap();
            let (out, incoming) =
                tokio::join!(viewer.connect(host.addr(), ALPN_REMOTE), async { host.accept().await.unwrap().await });
            let out = out.unwrap();
            let incoming = incoming.unwrap();
            let worker_conn = incoming.clone();
            let worker = tokio::spawn(async move {
                let (_send, mut recv) = worker_conn.accept_bi().await.unwrap();
                assert!(matches!(
                    slipstream_core::framing::expect_msg::<slipstream_core::proto::StreamOpen>(&mut recv)
                        .await
                        .unwrap(),
                    slipstream_core::proto::StreamOpen::Pace
                ));
                if !unsupported {
                    std::future::pending::<()>().await;
                }
            });
            let answer = tokio::time::timeout(
                Duration::from_secs(8),
                poll_host_pace(RemoteFs::new(out.clone()), Arc::default(), || 0),
            )
            .await
            .unwrap();
            assert_eq!(answer.is_none(), unsupported);
            worker.abort();
            let _ = worker.await;
            out.close(0u32.into(), b"test complete");
            incoming.close(0u32.into(), b"test complete");
            viewer.close().await;
            host.close().await;
        }
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[tokio::test]
    async fn queued_pointer_moves_go_as_the_latest_one() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let moved = |x: f32| ViewerCmd::Input(InputEvent::MouseMove { x, y: 0.5 });
        for x in [0.1, 0.2, 0.3] {
            tx.send(moved(x)).unwrap();
        }
        tx.send(ViewerCmd::Input(InputEvent::Key { code: "KeyA".into(), down: true })).unwrap();
        tx.send(moved(0.4)).unwrap();
        let mut pending = None;
        let first = next_cmd(&mut pending, &mut rx).await.unwrap();
        assert!(matches!(first, ViewerCmd::Input(InputEvent::MouseMove { x, .. }) if x == 0.3), "{first:?}");
        let second = next_cmd(&mut pending, &mut rx).await.unwrap();
        assert!(matches!(second, ViewerCmd::Input(InputEvent::Key { .. })), "the key isn't lost: {second:?}");
        let third = next_cmd(&mut pending, &mut rx).await.unwrap();
        assert!(matches!(third, ViewerCmd::Input(InputEvent::MouseMove { x, .. }) if x == 0.4));
    }
}
