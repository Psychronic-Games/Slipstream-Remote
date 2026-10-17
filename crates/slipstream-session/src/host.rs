//! Host side of a session: authenticate the viewer, share the screen,
//! apply its input.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, SendStream};
use slipstream_core::{
    auth::{HostAuth, password_secret},
    framing::{MsgReader, expect_msg, write_msg},
    proto::{
        Capabilities, ClipboardData, DisplayInfo, HostMsg, PROTOCOL_VERSION, SessionInfo, StreamSettings, VideoFrame,
        ViewerMsg,
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

async fn fail(send: &mut SendStream, reason: impl Into<String>, retry_after_secs: u32) -> Result<()> {
    write_msg(send, &HostMsg::AuthFailed { reason: reason.into(), retry_after_secs }).await?;
    send.finish().ok();
    Ok(())
}

pub async fn run_session(inner: Arc<Inner>, conn: Connection) -> Result<()> {
    let peer = conn.remote_id();
    let (mut send, mut recv) = conn.accept_bi().await.context("waiting for control stream")?;
    send.set_priority(slipstream_core::proto::PRIORITY_CONTROL).ok();
    let ViewerMsg::Hello { version, viewer_name } = expect_msg(&mut recv).await? else {
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
    let confirm = {
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
        let (auth, pakes) = HostAuth::start(&secrets, &inner.endpoint.id(), &peer);
        let exchange = async {
            write_msg(&mut send, &HostMsg::AuthChallenge { pakes }).await?;
            expect_msg(&mut recv).await
        };
        let ViewerMsg::AuthResponse { pakes, confirms } = tokio::time::timeout(AUTH_RESPONSE_WAIT, exchange)
            .await
            .context("viewer didn't answer the password challenge")??
        else {
            bail!("expected AuthResponse");
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

    // Open (or reuse) the screen session. On Wayland the first time this
    // shows the desktop's "allow screen sharing" dialog on this machine.
    let (displays, cursor_in_video, privacy) = {
        let mut backend = inner.backend.lock().await;
        if backend.is_none() {
            let opts = OpenOptions {
                test_pattern: inner.opts.test_pattern,
                restore_token: settings.host.portal_restore_token.clone(),
            };
            let open = slipstream_platform::open(&opts);
            tokio::pin!(open);
            // Only mention the prompt if there evidently is one (opening
            // is instant on X11, Windows, or with a remembered permission).
            let opened = tokio::select! {
                r = &mut open => r,
                _ = tokio::time::sleep(Duration::from_millis(700)) => {
                    write_msg(&mut send, &HostMsg::Notice("Waiting for the remote side to allow screen sharing…".into()))
                        .await?;
                    open.await
                }
            };
            match opened {
                Ok(b) => {
                    if let Some(token) = b.restore_token() {
                        set_restore_token(&inner, token);
                    }
                    info!(backend = b.name(), "screen backend ready");
                    *backend = Some(b);
                }
                Err(e) => {
                    warn!("screen sharing unavailable: {e:#}");
                    fail(&mut send, format!("screen sharing unavailable: {e:#}"), 0).await?;
                    return Ok(());
                }
            }
        }
        backend.as_ref().map(|b| (b.displays(), b.cursor_in_video(), b.privacy_supported())).unwrap_or_default()
    };

    let capabilities = Capabilities {
        input: settings.host.allow_input,
        clipboard: settings.host.allow_clipboard,
        file_transfer: settings.host.allow_file_transfer,
        audio: settings.host.allow_audio && (inner.opts.test_pattern || slipstream_audio::capture_supported()),
        privacy,
        tunnels: settings.host.allow_tunnels,
        chat: true,
        cursor_in_video,
    };
    let session = SessionInfo {
        host_name: settings.device_name.clone(),
        os: slipstream_core::os_name(),
        displays: displays.clone(),
        capabilities: capabilities.clone(),
    };
    write_msg(&mut send, &HostMsg::AuthOk { confirm, session }).await?;

    let id = inner.next_session_id();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    inner.host_sessions.lock().unwrap().insert(id, cmd_tx);
    inner.emit(ClientEvent::HostSessionStarted { session: id, peer: viewer_name.clone() });

    let reason = match session_loop(&inner, &conn, id, &viewer_name, send, recv, displays, capabilities, cmd_rx).await {
        Ok(r) => r,
        Err(e) => format!("{e:#}"),
    };
    inner.host_sessions.lock().unwrap().remove(&id);
    info!(%peer, %reason, "session ended");
    inner.emit(ClientEvent::HostSessionEnded { session: id, reason });
    // A new one-time password after every session, like TeamViewer.
    let len = inner.settings.lock().unwrap().host.one_time_password_len.clamp(4, 32);
    let pw = slipstream_core::auth::generate_password(len);
    *inner.password.lock().unwrap() = pw.clone();
    inner.emit(ClientEvent::Password(pw));
    conn.close(0u32.into(), b"bye");
    Ok(())
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
    displays: Vec<DisplayInfo>,
    caps: Capabilities,
    mut cmds: mpsc::UnboundedReceiver<HostSessionCmd>,
) -> Result<String> {
    let mut recv = MsgReader::<ViewerMsg>::spawn(recv);
    // Video writer: owns the uni-stream, fed by the encoder thread.
    let (video_tx, mut video_rx) = mpsc::channel::<VideoFrame>(4);
    let video_conn = conn.clone();
    let writer = tokio::spawn(async move {
        let mut stream = video_conn.open_uni().await?;
        stream.set_priority(slipstream_core::proto::PRIORITY_VIDEO).ok();
        while let Some(frame) = video_rx.recv().await {
            write_msg(&mut stream, &frame).await?;
        }
        stream.finish().ok();
        anyhow::Ok(())
    });
    let _writer_guard = AbortOnDrop(writer);

    // File browsing and transfer requests from the viewer.
    let files_inner = inner.clone();
    let policy = files::FilePolicy { browse: caps.file_transfer, receive: caps.file_transfer, tunnels: caps.tunnels };
    let on_saved: Arc<dyn Fn(std::path::PathBuf) + Send + Sync> =
        Arc::new(move |path| files_inner.emit(ClientEvent::FileReceived { session: id, path }));
    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel();
    let clipboard = if caps.clipboard { Clipboard::start(clip_tx, &inner.opts.clipboard) } else { None };
    // Pasted files go onto this clipboard (only when clipboard sharing is on).
    let _files_guard = AbortOnDrop(tokio::spawn(files::serve(conn.clone(), policy, on_saved, clipboard.clone())));

    let flow = Flow::new();
    let (err_tx, mut err_rx) = mpsc::unbounded_channel::<String>();
    let mut pipeline: Option<Pipeline> = None;
    let mut display = displays.iter().find(|d| d.primary).map(|d| d.index).unwrap_or(0);
    // The host's sound, while the viewer wants it (capture stops on drop).
    let mut _audio: Option<slipstream_audio::Capture> = None;
    // Privacy mode, lifted however the session ends.
    let mut privacy = PrivacyGuard { inner: inner.clone(), on: false };

    loop {
        tokio::select! {
            msg = recv.next() => {
                let msg = match msg {
                    Ok(m) => m,
                    // The read fails first when the connection closes.
                    Err(e) => return conn.close_reason().map(|r| crate::closed_reason(&r, "they")).ok_or(e),
                };
                let Some(msg) = msg else { return Ok("they ended the session".into()) };
                match msg {
                    ViewerMsg::StartStream { display: d, settings } => {
                        if !displays.iter().any(|x| x.index == d) {
                            write_msg(&mut send, &HostMsg::Notice(format!("no display {d}"))).await?;
                            continue;
                        }
                        display = d;
                        let stream_settings = sanitize_settings(settings);
                        drop(pipeline.take()); // stop the old capture first
                        match open_capture(inner, display, stream_settings.max_fps).await? {
                            Ok(c) => {
                                pipeline = Some(Pipeline::start(c, flow.clone(), stream_settings, video_tx.clone(), err_tx.clone()));
                                let info = &displays[display as usize];
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
                        if let Some(c) = &clipboard { c.set_text(t); }
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
            Some(clip) = clip_rx.recv() => match clip {
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
            },
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
