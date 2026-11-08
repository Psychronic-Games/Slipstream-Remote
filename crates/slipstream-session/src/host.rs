//! Host side of a session: authenticate the viewer, share the screen,
//! apply its input.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, SendStream};
use slipstream_core::{
    auth::{HostAuth, password_secret},
    framing::{MsgReader, expect_msg_max, expect_msg_with_extras, write_msg},
    proto::{
        Capabilities, ClipboardData, DisplayInfo, HelloExtras, HostMsg, InputEvent, PROTOCOL_VERSION, SessionInfo,
        StreamSettings, VideoFrame, ViewerMsg,
    },
};
use slipstream_platform::{
    OpenOptions,
    clipboard::{Clip, Clipboard},
};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    ClientEvent, HostSessionCmd, Inner, files,
    video::{Flow, Pipeline},
};

/// How long a viewer may queue behind another viewer's password check.
const AUTH_QUEUE_WAIT: Duration = Duration::from_secs(15);
/// How long the challenge and answer may take. The answer is computed, not
/// typed, so this only cuts off stalled connections holding up the queue.
const AUTH_RESPONSE_WAIT: Duration = Duration::from_secs(10);
/// Answering the challenge needs only a round trip while the throttle is
/// held, so a stalled answer counts as a failed attempt after this long.
const AUTH_EXCHANGE_WAIT: Duration = Duration::from_secs(5);
/// Messages before authentication are tiny (a name, SPAKE2 values); never
/// let an unauthenticated peer make this host allocate more.
const PRE_AUTH_MESSAGE: usize = 64 * 1024;

/// Whether a link has asked to see or use this computer. Chat-only links
/// haven't; anything beyond chat and health checks counts, so the person
/// here is told (`ClientEvent::HostSessionViewing`) even if a modified
/// viewer skips the picture.
pub(crate) struct Viewing {
    flag: std::sync::atomic::AtomicBool,
    inner: Arc<Inner>,
    session: u64,
}

impl Viewing {
    pub(crate) fn note(&self) {
        if !self.flag.swap(true, std::sync::atomic::Ordering::AcqRel) {
            self.inner.emit(ClientEvent::HostSessionViewing { session: self.session });
        }
    }

    fn get(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::Acquire)
    }
}

async fn fail(send: &mut SendStream, reason: impl Into<String>, retry_after_secs: u32) -> Result<()> {
    write_msg(send, &HostMsg::AuthFailed { reason: reason.into(), retry_after_secs }).await?;
    send.finish().ok();
    Ok(())
}

pub async fn run_session(inner: Arc<Inner>, conn: Connection, host_id: iroh::EndpointId, public: bool) -> Result<()> {
    let peer = conn.remote_id();
    let (mut send, mut recv, hello) = tokio::time::timeout(AUTH_RESPONSE_WAIT, async {
        let (send, mut recv) = conn.accept_bi().await.context("waiting for control stream")?;
        send.set_priority(slipstream_core::proto::PRIORITY_CONTROL).ok();
        let hello = expect_msg_with_extras::<ViewerMsg, HelloExtras>(&mut recv, PRE_AUTH_MESSAGE).await?;
        anyhow::Ok((send, recv, hello))
    }).await.context("viewer did not start authentication")??;
    let (ViewerMsg::Hello { version, viewer_name }, extras) = hello else {
        bail!("expected Hello");
    };
    let viewer_name: String = viewer_name.chars().take(64).collect();
    info!(%peer, %viewer_name, "incoming session");

    if version != PROTOCOL_VERSION {
        fail(&mut send, format!("version mismatch (host v{PROTOCOL_VERSION}, viewer v{version}) — please update"), 0)
            .await?;
        return Ok(());
    }
    let settings = inner.settings.lock().unwrap().clone();
    if !settings.host.enabled {
        fail(&mut send, "this device is not accepting connections", 0).await?;
        return Ok(());
    }
    // Password check (see slipstream_core::auth), one at a time: the
    // throttle stays locked until this attempt is counted, so parallel
    // connections can't slip extra guesses in before the lockout starts.
    let (confirm, matched) = {
        let Ok(mut throttle) = tokio::time::timeout(AUTH_QUEUE_WAIT, inner.throttle.lock()).await else {
            fail(&mut send, "the device is busy with another sign-in — try again", 5).await?;
            return Ok(());
        };
        if let Some(wait) = throttle.locked() {
            fail(&mut send, "too many wrong passwords — try again shortly", wait.as_secs() as u32 + 1).await?;
            return Ok(());
        }
        let mut secrets = vec![password_secret(&inner.one_time_password())];
        if let Some(p) = settings.host.permanent_secret() {
            secrets.push(p);
        }
        let (auth, pakes) = HostAuth::start(&secrets, &host_id, &peer);
        let exchange = async {
            write_msg(&mut send, &HostMsg::AuthChallenge { pakes }).await?;
            expect_msg_max(&mut recv, PRE_AUTH_MESSAGE).await
        };
        // A stalled or malformed answer is a failed attempt: otherwise a
        // peer could hold the queue for free, again and again.
        let (pakes, confirms) = match tokio::time::timeout(AUTH_EXCHANGE_WAIT, exchange).await {
            Ok(Ok(ViewerMsg::AuthResponse { pakes, confirms })) => (pakes, confirms),
            other => {
                throttle.record_failure();
                drop(throttle);
                return Err(match other {
                    Err(_) => anyhow::anyhow!("viewer didn't answer the password challenge"),
                    Ok(Err(e)) => e,
                    Ok(Ok(_)) => anyhow::anyhow!("expected AuthResponse"),
                });
            }
        };
        match auth.finish(&pakes, &confirms) {
            Ok(c) => {
                throttle.record_success();
                c
            }
            Err(_) => {
                throttle.record_failure();
                let wait = throttle.locked().map(|d| d.as_secs() as u32 + 1).unwrap_or(0);
                drop(throttle);
                warn!(%peer, "wrong password");
                fail(&mut send, "wrong password", wait).await?;
                return Ok(());
            }
        }
    };

    // A one-time password may have been read out to a stranger on the phone:
    // on Slipstream Public, and wherever the station or user asks for it,
    // someone at this computer has to allow the link (see `approval`).
    let one_time = matched == 0;
    if one_time && (public || settings.host.confirm_links || settings.server.station_approves_links) {
        write_msg(&mut send, &HostMsg::Notice(crate::approval::WAITING.into())).await?;
        let gone = async {
            conn.closed().await;
        };
        if let Err(reason) = crate::approval::ask(&inner, &viewer_name, public, gone).await {
            info!(%peer, %reason, "link not allowed");
            // The code may be in the wrong hands: never accept it again.
            rotate_password(&inner);
            fail(&mut send, reason, 0).await?;
            return Ok(());
        }
    }

    // A chat-only link from a current viewer doesn't open the screen until it
    // asks to see or control it (`session_loop`), so chatting shows no
    // screen-sharing prompt here and works on a phone that isn't sharing.
    // Until then it gets no displays, and input as the permission says.
    let deferred = extras.chat_only;
    let (backend, screen) = if deferred {
        (None, Screen { input_supported: true, ..Default::default() })
    } else {
        match open_screen(&inner, &mut send).await? {
            Ok((backend, screen)) => (Some(backend), screen),
            Err(reason) => {
                fail(&mut send, reason, 0).await?;
                return Ok(());
            }
        }
    };

    let mut capabilities = Capabilities {
        input: settings.host.allow_input && screen.input_supported,
        clipboard: settings.host.allow_clipboard && !cfg!(target_os="android"),
        file_transfer: settings.host.allow_file_transfer && !cfg!(target_os="android"),
        audio: settings.host.allow_audio && (inner.opts.test_pattern || slipstream_audio::capture_supported()),
        tunnels: settings.host.allow_tunnels,
        network_scan: settings.host.allow_network_scan && !cfg!(target_os="android"),
        chat: true,
        cursor_in_video: screen.cursor_in_video,
        privacy: screen.privacy,
    };
    let session = SessionInfo {
        host_name: settings.device_name.clone(),
        os: slipstream_core::os_name(),
        displays: screen.displays.clone(),
        capabilities: capabilities.clone(),
    };
    write_msg(&mut send, &HostMsg::AuthOk { confirm, session }).await?;

    let id = inner.next_session_id();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    inner.host_sessions.lock().unwrap().insert(id, cmd_tx);
    // Register the viewer before allowing a local reset to acquire this lock.
    // Otherwise reset could discard its backend just as authentication ends.
    drop(backend);
    inner.emit(ClientEvent::HostSessionStarted { session: id, peer: viewer_name.clone() });

    let viewing = Arc::new(Viewing { flag: Default::default(), inner: inner.clone(), session: id });
    let screen = ScreenState { open: !deferred, displays: screen.displays, allow_input: settings.host.allow_input };
    let reason = match session_loop(&inner, &conn, id, &viewer_name, send, recv, screen, &mut capabilities, cmd_rx, viewing.clone()).await {
        Ok(r) => r,
        // The video writer may observe a peer close before the control reader.
        Err(e) => conn.close_reason().map(|r| crate::closed_reason(&r, "they")).unwrap_or_else(|| format!("{e:#}")),
    };
    // The backend is cached across connections, so dropping a viewer does
    // not drop its held keyboard/mouse state. Release it on every exit path
    // (a chat-only link never controlled anything: leave other sessions be).
    if capabilities.input && viewing.get() && let Some(backend) = inner.backend.lock().await.as_mut() {
        let display = backend.displays().first().map(|d| d.index).unwrap_or(0);
        let _ = backend.inject(display, InputEvent::ReleaseAll);
    }
    inner.host_sessions.lock().unwrap().remove(&id);
    info!(%peer, %reason, "session ended");
    let restarting=reason==slipstream_core::proto::HOST_RESTARTING;
    inner.emit(ClientEvent::HostSessionEnded { session: id, reason });
    // A new one-time password after every session, like TeamViewer.
    rotate_password(&inner);
    conn.close((if restarting {slipstream_core::proto::RESTART_CLOSE_CODE}else{0}).into(), b"bye");
    Ok(())
}

fn rotate_password(inner: &Inner) {
    let len = inner.settings.lock().unwrap().host.one_time_password_len.clamp(4, 32);
    let pw = slipstream_core::auth::generate_password(len);
    *inner.password.lock().unwrap() = pw.clone();
    inner.emit(ClientEvent::Password(pw));
}

/// What the opened screen offers a session.
#[derive(Default)]
struct Screen {
    displays: Vec<DisplayInfo>,
    cursor_in_video: bool,
    privacy: bool,
    input_supported: bool,
}

type BackendGuard<'a> = tokio::sync::MutexGuard<'a, Option<Box<dyn slipstream_platform::Backend>>>;

/// Opens (or reuses) the screen session. On Wayland the first time this
/// shows the desktop's "allow screen sharing" dialog on this machine. The
/// outer error is the control stream failing; the inner one, why the screen
/// can't be shared (for the viewer).
async fn open_screen<'a>(inner: &'a Inner, send: &mut SendStream) -> Result<Result<(BackendGuard<'a>, Screen), String>> {
    let open = screen_backend(inner);
    tokio::pin!(open);
    // Only mention the prompt if there evidently is one (opening
    // is instant on X11, Windows, or with a remembered permission).
    let opened = tokio::select! {
        r = &mut open => r,
        _ = tokio::time::sleep(Duration::from_millis(700)) => {
            write_msg(send, &HostMsg::Notice("Preparing screen sharing on the remote computer…".into())).await?;
            open.await
        }
    };
    let backend = match opened {
        Ok(backend) => backend,
        Err(e) => {
            warn!("screen sharing unavailable: {e:#}");
            return Ok(Err(format!("screen sharing unavailable: {e:#}")));
        }
    };
    let screen = backend.as_ref().map(|b| Screen {
        displays: b.displays(),
        cursor_in_video: b.cursor_in_video(),
        privacy: b.privacy_supported(),
        input_supported: b.input_supported(),
    }).unwrap_or_default();
    if screen.displays.is_empty() {
        let reason = if cfg!(target_os = "android") {
            "On the phone, tap Start sharing and approve Android's screen-sharing prompt"
        } else {
            "No screen is currently available on the remote device"
        };
        return Ok(Err(reason.into()));
    }
    Ok(Ok((backend, screen)))
}

/// A session's view of the screen: a chat-only link starts with it closed.
struct ScreenState {
    open: bool,
    displays: Vec<DisplayInfo>,
    /// Host permission, applied once the screen says whether it takes input.
    allow_input: bool,
}

impl ScreenState {
    /// Opens a chat-only link's screen on its first request to see or
    /// control it, and tells the viewer what it now has. False (after a
    /// notice) if it can't be opened; the chat carries on and a later
    /// request tries again.
    async fn ensure_open(&mut self, inner: &Inner, send: &mut SendStream, caps: &mut Capabilities) -> Result<bool> {
        if self.open {
            return Ok(true);
        }
        let screen = match open_screen(inner, send).await? {
            Ok((backend, screen)) => {
                drop(backend);
                screen
            }
            Err(reason) => {
                write_msg(send, &HostMsg::Notice(reason)).await?;
                return Ok(false);
            }
        };
        caps.input = self.allow_input && screen.input_supported;
        caps.privacy = screen.privacy;
        caps.cursor_in_video = screen.cursor_in_video;
        self.displays = screen.displays;
        self.open = true;
        write_msg(send, &HostMsg::Capabilities(caps.clone())).await?;
        write_msg(send, &HostMsg::Displays(self.displays.clone())).await?;
        Ok(true)
    }
}

/// Shared by local approval and incoming sessions, so preparing access never
/// opens a second portal or replaces a backend a viewer is already using.
pub(crate) async fn screen_backend(inner: &Inner) -> Result<tokio::sync::MutexGuard<'_, Option<Box<dyn slipstream_platform::Backend>>>> {
    let mut backend = inner.backend.lock().await;
    // Android can revoke capture with no viewer left to report its failure.
    if backend.as_ref().is_some_and(|b| b.displays().is_empty()) {
        *backend = None;
    }
    if backend.is_none() {
        let restore_token = inner.settings.lock().unwrap().host.portal_restore_token.clone();
        let opened = slipstream_platform::open(&OpenOptions {
            test_pattern: inner.opts.test_pattern,
            restore_token,
        }).await?;
        if let Some(token) = opened.restore_token() {
            set_restore_token(inner, token);
        }
        info!(backend = opened.name(), "screen backend ready");
        *backend = Some(opened);
    }
    Ok(backend)
}

fn set_restore_token(inner: &Inner, token: String) {
    let changed = {
        let mut s = inner.settings.lock().unwrap();
        let changed = s.host.portal_restore_token.as_deref() != Some(token.as_str());
        s.host.portal_restore_token = Some(token);
        changed
    };
    if changed {
        inner.save_settings();
    }
}

fn fsops_summary(paths: &[std::path::PathBuf]) -> (Vec<slipstream_core::proto::ClipFile>, u64, u64) {
    crate::fsops::summarize(paths, 10_000)
}

const LOCKED_NOTICE: &str = "That computer is showing its lock screen or an administrator prompt, which Windows hides \
     from apps. Install the Slipstream service there (Config → Windows service) to reach those.";
const SAS_NOTICE: &str = "Windows only takes Ctrl+Alt+Del from a service: install the Slipstream service on that \
     computer (Config → Windows service).";

fn desktop_hidden() -> bool {
    #[cfg(windows)]
    return slipstream_platform::windesk::input_desktop_hidden();
    #[allow(unreachable_code)]
    false
}

pub(crate) fn service_mode() -> bool {
    #[cfg(windows)]
    return slipstream_platform::windesk::following();
    #[allow(unreachable_code)]
    false
}

/// Tracks Ctrl/Alt; true when this event completes Ctrl+Alt+Del.
fn wants_sas(held: &mut Vec<String>, ev: &InputEvent) -> bool {
    let InputEvent::Key { code, down } = ev else {
        if matches!(ev, InputEvent::ReleaseAll) {
            held.clear();
        }
        return false;
    };
    if code.starts_with("Control") || code.starts_with("Alt") {
        held.retain(|k| k != code);
        if *down {
            held.push(code.clone());
        }
        return false;
    }
    *down
        && code == "Delete"
        && held.iter().any(|k| k.starts_with("Control"))
        && held.iter().any(|k| k.starts_with("Alt"))
}

/// Turns privacy mode off when a session that switched it on ends.
struct PrivacyGuard {
    inner: Arc<Inner>,
    on: bool,
}

impl Drop for PrivacyGuard {
    fn drop(&mut self) {
        if self.on {
            let inner = self.inner.clone();
            tokio::spawn(async move {
                if let Some(b) = inner.backend.lock().await.as_mut() {
                    let _ = b.set_privacy(false, "");
                }
            });
        }
    }
}

/// Captures this computer's sound and sends it to the viewer: Opus, one
/// datagram per 20 ms (see `proto::AUDIO_DATAGRAM`).
/// With the test pattern it sends a test tone instead (no feedback loop when
/// host and viewer share a machine).
fn send_audio(conn: Connection, test_tone: bool) -> Result<slipstream_audio::Capture> {
    let mut encoder = slipstream_audio::opus::Encoder::new(96_000)?;
    let mut seq = 0u16;
    let start = if test_tone { slipstream_audio::Capture::test_tone } else { slipstream_audio::Capture::start };
    start(Box::new(move |frame: &[i16]| match encoder.encode(frame) {
        Ok(packet) => {
            let mut d = Vec::with_capacity(packet.len() + 3);
            d.push(slipstream_core::proto::AUDIO_DATAGRAM);
            d.extend_from_slice(&seq.to_le_bytes());
            d.extend_from_slice(packet);
            seq = seq.wrapping_add(1);
            // A full send queue drops the frame, which is the right thing for audio.
            let _ = conn.send_datagram(d.into());
        }
        Err(e) => debug!("audio encode: {e:#}"),
    }))
}

/// Starts capturing `display`. The outer error means the backend is gone
/// (fatal); the inner one that this capture failed.
async fn open_capture(
    inner: &Inner,
    display: u32,
    max_fps: u32,
) -> Result<Result<slipstream_platform::FrameStream>> {
    let mut b = inner.backend.lock().await;
    let b = b.as_mut().context("screen backend gone")?;
    Ok(b.capture(display, max_fps))
}

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    inner: &Arc<Inner>,
    conn: &Connection,
    id: u64,
    viewer: &str,
    mut send: SendStream,
    recv: iroh::endpoint::RecvStream,
    mut screen: ScreenState,
    caps: &mut Capabilities,
    mut cmds: mpsc::UnboundedReceiver<HostSessionCmd>,
    viewing: Arc<Viewing>,
) -> Result<String> {
    let mut recv = MsgReader::<ViewerMsg>::spawn(recv);
    // Video writer: owns the uni-stream, fed by the encoder thread.
    let (video_tx, mut video_rx) = mpsc::channel::<VideoFrame>(4);
    let video_conn = conn.clone();
    let writer = tokio::spawn(async move {
        let mut stream = video_conn.open_uni().await?;
        stream.set_priority(slipstream_core::proto::PRIORITY_VIDEO).ok();
        let stopped = stream.stopped();
        tokio::pin!(stopped);
        loop {
            // An exhausted frame-credit window leaves the encoder idle. Still
            // observe peer cancellation without needing another frame/write.
            let frame = tokio::select! {
                result = &mut stopped => {
                    let code = result.context("video stream closed")?;
                    bail!("the viewer stopped receiving video ({code:?})");
                }
                frame = video_rx.recv() => frame,
            };
            let Some(frame) = frame else { break; };
            write_msg(&mut stream, &frame).await?;
        }
        stream.finish().ok();
        anyhow::Ok(())
    });
    let mut writer_guard = AbortOnDrop(writer);

    // File browsing and transfer requests from the viewer.
    let files_inner = inner.clone();
    let policy = files::FilePolicy { browse: caps.file_transfer, receive: caps.file_transfer, tunnels: caps.tunnels, network_scan: caps.network_scan, remote_input: caps.input };
    let on_saved: Arc<dyn Fn(std::path::PathBuf) + Send + Sync> =
        Arc::new(move |path| files_inner.emit(ClientEvent::FileReceived { session: id, path }));
    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel();
    let clipboard = if caps.clipboard { Clipboard::start(clip_tx, &inner.opts.clipboard) } else { None };
    // Pasted files go onto this clipboard (only when clipboard sharing is on).
    let status_inner = inner.clone();
    let support_settings = inner.settings.lock().unwrap().clone();
    let tools = Arc::new(crate::support::HostTools {
        station: support_settings.server,
        allow_wake: support_settings.host.allow_wake_on_lan,
        status: Arc::new(move |text| status_inner.emit(ClientEvent::HostStatus { session: id, text })),
    });
    let _files_guard = AbortOnDrop(tokio::spawn(files::serve(conn.clone(), policy, on_saved, clipboard.clone(), tools, viewing.clone())));

    let flow = Flow::new();
    let (err_tx, mut err_rx) = mpsc::unbounded_channel::<String>();
    let mut pipeline: Option<Pipeline> = None;
    let mut display = screen.displays.iter().find(|d| d.primary).map(|d| d.index).unwrap_or(0);
    // The host's sound, while the viewer wants it (capture stops on drop).
    let mut _audio: Option<slipstream_audio::Capture> = None;
    // Privacy mode, lifted however the session ends.
    let mut privacy = PrivacyGuard { inner: inner.clone(), on: false };
    // Windows without the service: say so when the screen it can't reach
    // (lock screen, UAC prompt) is up, or Ctrl+Alt+Del is asked for.
    let mut desktop_tick = tokio::time::interval(Duration::from_secs(3));
    let mut hidden_since: Option<Instant> = None;
    let mut held_mods: Vec<String> = Vec::new();

    loop {
        tokio::select! {
            result = &mut writer_guard.0 => {
                // Hanging up stops the video stream too, sometimes before
                // the viewer's Bye arrives: give that a moment to say why.
                let hung_up = tokio::time::timeout(Duration::from_millis(500), async {
                    loop {
                        match recv.next().await {
                            Ok(Some(ViewerMsg::Bye)) | Ok(None) => return Some("they ended the session".to_string()),
                            Ok(Some(_)) => {}
                            Err(_) => return conn.close_reason().map(|r| crate::closed_reason(&r, "they")),
                        }
                    }
                })
                .await
                .ok()
                .flatten();
                if let Some(reason) = hung_up {
                    return Ok(reason);
                }
                let reason=match result {
                    Ok(Err(error))=>format!("Video stream stopped: {error:#}"),
                    Err(error)=>format!("Video stream stopped: {error}"),
                    Ok(Ok(()))=>"Video stream ended unexpectedly".into(),
                };
                write_msg(&mut send,&HostMsg::Bye {reason:reason.clone()}).await?;
                crate::flush(&mut send).await;
                return Ok(reason);
            }
            _ = desktop_tick.tick() => {
                // A chat-only link isn't looking at the screen.
                if viewing.get() && desktop_hidden() {
                    let due = hidden_since.is_none_or(|t| t.elapsed() >= Duration::from_secs(15));
                    if due {
                        hidden_since = Some(Instant::now());
                        write_msg(&mut send, &HostMsg::Notice(LOCKED_NOTICE.into())).await?;
                    }
                } else {
                    hidden_since = None;
                }
            }
            msg = recv.next() => {
                let msg = match msg {
                    Ok(m) => m,
                    // The read fails first when the connection closes.
                    Err(e) => return conn.close_reason().map(|r| crate::closed_reason(&r, "they")).ok_or(e),
                };
                let Some(msg) = msg else { return Ok("they ended the session".into()) };
                match msg {
                    ViewerMsg::StartStream { display: d, settings } => {
                        viewing.note();
                        let was_open = screen.open;
                        if !screen.ensure_open(inner, &mut send, caps).await? {
                            continue;
                        }
                        let info = screen.displays.iter().find(|x| x.index == d).or_else(|| {
                            // A chat-only link asked before it knew the displays: show the main one.
                            if was_open { return None; }
                            screen.displays.iter().find(|x| x.primary).or(screen.displays.first())
                        }).cloned();
                        let Some(info) = info else {
                            write_msg(&mut send, &HostMsg::Notice(format!("no display {d}"))).await?;
                            continue;
                        };
                        display = info.index;
                        let stream_settings = sanitize_settings(settings);
                        drop(pipeline.take()); // stop the old capture first
                        match open_capture(inner, display, stream_settings.max_fps).await? {
                            Ok(c) => {
                                pipeline = Some(Pipeline::start(c, flow.clone(), stream_settings, video_tx.clone(), err_tx.clone()));
                                write_msg(&mut send, &HostMsg::StreamStarted { display, width: info.width, height: info.height }).await?;
                            }
                            Err(e) => {
                                write_msg(&mut send, &HostMsg::Notice(format!("capture failed: {e:#}"))).await?;
                            }
                        }
                    }
                    ViewerMsg::SetSettings(s) => {
                        let s = sanitize_settings(s);
                        let Some(p) = &pipeline else { continue };
                        if s.max_fps == p.settings().max_fps {
                            p.set_settings(s);
                            continue;
                        }
                        // Capture is paced at the frame rate it started with.
                        drop(pipeline.take());
                        match open_capture(inner, display, s.max_fps).await? {
                            Ok(c) => pipeline = Some(Pipeline::start(c, flow.clone(), s, video_tx.clone(), err_tx.clone())),
                            Err(e) => write_msg(&mut send, &HostMsg::Notice(format!("capture failed: {e:#}"))).await?,
                        }
                    }
                    ViewerMsg::FrameAck { seq } => flow.ack(seq),
                    ViewerMsg::RequestKeyframe => {
                        if let Some(p) = &pipeline { p.request_keyframe(); }
                    }
                    ViewerMsg::Input(ev) => {
                        // Nothing to control before the viewer has asked to see the screen.
                        if !screen.open { continue; }
                        if caps.input { viewing.note(); }
                        if cfg!(windows) && caps.input && wants_sas(&mut held_mods, &ev) && !service_mode() {
                            write_msg(&mut send, &HostMsg::Notice(SAS_NOTICE.into())).await?;
                        }
                        if caps.input {
                            let mut b = inner.backend.lock().await;
                            if let Some(b) = b.as_mut()
                                && let Err(e) = b.inject(display, ev)
                            {
                                debug!("inject failed: {e:#}");
                            }
                        }
                    }
                    ViewerMsg::Clipboard(ClipboardData::Text(t)) => {
                        // Chat-only links don't share clipboards either way.
                        if viewing.get() && let Some(c) = &clipboard { c.set_text(t); }
                    }
                    // Viewers paste files with uploads + StreamOpen::ClipboardFiles.
                    ViewerMsg::Clipboard(ClipboardData::Files { .. }) => {}
                    ViewerMsg::Chat(text) => {
                        let text: String = text.chars().take(4000).collect();
                        inner.emit(ClientEvent::HostChat { session: id, text });
                    }
                    ViewerMsg::Bye => return Ok("they ended the session".into()),
                    ViewerMsg::SetPrivacy(on) => {
                        if caps.privacy && on != privacy.on {
                            if on { viewing.note(); }
                            let r = match inner.backend.lock().await.as_mut() {
                                Some(b) => b.set_privacy(on, viewer),
                                None => Err(anyhow::anyhow!("screen backend gone")),
                            };
                            match r {
                                Ok(()) => privacy.on = on,
                                Err(e) => write_msg(&mut send, &HostMsg::Notice(format!("Privacy mode: {e:#}"))).await?,
                            }
                        }
                    }
                    ViewerMsg::SetAudio(on) => {
                        _audio = None;
                        if on && caps.audio {
                            viewing.note();
                            match send_audio(conn.clone(), inner.opts.test_pattern) {
                                Ok(a) => _audio = Some(a),
                                Err(e) => {
                                    warn!("sound capture: {e:#}");
                                    write_msg(&mut send, &HostMsg::Notice(format!("This computer's sound can't be shared: {e:#}"))).await?;
                                }
                            }
                        }
                    }
                    ViewerMsg::Hello { .. } | ViewerMsg::AuthResponse { .. } => bail!("protocol violation"),
                }
            }
            // Nothing copied here goes to a chat-only link.
            Some(clip) = clip_rx.recv() => if viewing.get() { match clip {
                Clip::Text(text) if text.len() <= 1 << 20 => {
                    write_msg(&mut send, &HostMsg::Clipboard(ClipboardData::Text(text))).await?;
                }
                Clip::Text(_) => {}
                // Copied files: tell the viewer what they are (it decides
                // whether to fetch them), if it may have them.
                Clip::Files(paths) if caps.file_transfer => {
                    let summary = tokio::task::spawn_blocking(move || fsops_summary(&paths)).await?;
                    let (items, total_bytes, file_count) = summary;
                    write_msg(&mut send, &HostMsg::Clipboard(ClipboardData::Files { items, total_bytes, file_count })).await?;
                }
                Clip::Files(_) => {}
            }},
            Some(err) = err_rx.recv() => {
                drop(pipeline.take());
                // The portal session may have been revoked; reopen next time.
                *inner.backend.lock().await = None;
                write_msg(&mut send, &HostMsg::Bye { reason: format!("screen capture stopped: {err}") }).await.ok();
                crate::flush(&mut send).await;
                return Ok(format!("screen capture stopped: {err}"));
            }
            cmd = cmds.recv() => match cmd {
                Some(HostSessionCmd::Chat(text)) => write_msg(&mut send, &HostMsg::Chat(text)).await?,
                Some(HostSessionCmd::Restart) => {
                    let reason=slipstream_core::proto::HOST_RESTARTING.to_string();
                    write_msg(&mut send,&HostMsg::Bye{reason:reason.clone()}).await.ok();
                    crate::flush(&mut send).await;
                    return Ok(reason);
                }
                Some(HostSessionCmd::Disconnect) | None => {
                    write_msg(&mut send, &HostMsg::Bye { reason: "the host ended the session".into() }).await.ok();
                    crate::flush(&mut send).await;
                    return Ok("you ended the session".into());
                }
            },
            reason = conn.closed() => return Ok(crate::closed_reason(&reason, "they")),
        }
    }
}

fn sanitize_settings(mut s: StreamSettings) -> StreamSettings {
    s.max_fps = s.max_fps.clamp(1, 144);
    s.max_in_flight = s.max_in_flight.clamp(1, 8);
    s.bitrate_kbps = s.bitrate_kbps.min(100_000);
    s
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_alt_del_is_recognised() {
        let key = |code: &str, down| InputEvent::Key { code: code.into(), down };
        let mut held = Vec::new();
        assert!(!wants_sas(&mut held, &key("Delete", true)), "Delete alone");
        wants_sas(&mut held, &key("ControlLeft", true));
        assert!(!wants_sas(&mut held, &key("Delete", true)), "Ctrl+Delete");
        wants_sas(&mut held, &key("AltRight", true));
        assert!(wants_sas(&mut held, &key("Delete", true)));
        assert!(!wants_sas(&mut held, &key("Delete", false)), "the release");
        wants_sas(&mut held, &key("AltRight", false));
        assert!(!wants_sas(&mut held, &key("Delete", true)), "Alt let go");
        wants_sas(&mut held, &key("AltLeft", true));
        wants_sas(&mut held, &InputEvent::ReleaseAll);
        assert!(!wants_sas(&mut held, &key("Delete", true)), "after ReleaseAll");
    }
}
