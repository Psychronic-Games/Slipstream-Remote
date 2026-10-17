//! Host side of a session: authenticate the viewer, share the screen,
//! apply its input.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, SendStream};
use slipstream_core::{
    auth::{HostAuth, password_secret},
    framing::{expect_msg, read_msg, write_msg},
    proto::{
        Capabilities, ClipboardData, DisplayInfo, HostMsg, PROTOCOL_VERSION, SessionInfo, StreamSettings, VideoFrame,
        ViewerMsg,
    },
};
use slipstream_platform::{OpenOptions, clipboard::Clipboard};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    ClientEvent, HostSessionCmd, Inner, files,
    video::{Flow, Pipeline},
};

async fn fail(send: &mut SendStream, reason: impl Into<String>, retry_after_secs: u32) -> Result<()> {
    write_msg(send, &HostMsg::AuthFailed { reason: reason.into(), retry_after_secs }).await?;
    send.finish().ok();
    Ok(())
}

pub async fn run_session(inner: Arc<Inner>, conn: Connection) -> Result<()> {
    let peer = conn.remote_id();
    let (mut send, mut recv) = conn.accept_bi().await.context("waiting for control stream")?;
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
    let locked = inner.throttle.lock().unwrap().locked();
    if let Some(wait) = locked {
        fail(&mut send, "too many wrong passwords — try again shortly", wait.as_secs() as u32 + 1).await?;
        return Ok(());
    }

    // Password check (see slipstream_core::auth).
    let mut secrets = vec![password_secret(&inner.one_time_password())];
    if let Some(p) = settings.host.permanent_secret() {
        secrets.push(p);
    }
    let (auth, pakes) = HostAuth::start(&secrets, &inner.endpoint.id(), &peer);
    write_msg(&mut send, &HostMsg::AuthChallenge { pakes }).await?;
    let ViewerMsg::AuthResponse { pakes, confirms } = expect_msg(&mut recv).await? else {
        bail!("expected AuthResponse");
    };
    let confirm = match auth.finish(&pakes, &confirms) {
        Ok(c) => {
            inner.throttle.lock().unwrap().record_success();
            c
        }
        Err(_) => {
            let wait = {
                let mut t = inner.throttle.lock().unwrap();
                t.record_failure();
                t.locked().map(|d| d.as_secs() as u32 + 1).unwrap_or(0)
            };
            warn!(%peer, "wrong password");
            fail(&mut send, "wrong password", wait).await?;
            return Ok(());
        }
    };

    // Open (or reuse) the screen session. On Wayland the first time this
    // shows the desktop's "allow screen sharing" dialog on this machine.
    let (displays, cursor_in_video) = {
        let mut backend = inner.backend.lock().await;
        if backend.is_none() {
            write_msg(&mut send, &HostMsg::Notice("Waiting for the remote side to allow screen sharing…".into()))
                .await?;
            let opts = OpenOptions {
                test_pattern: inner.opts.test_pattern,
                restore_token: settings.host.portal_restore_token.clone(),
            };
            match slipstream_platform::open(&opts).await {
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
        backend.as_ref().map(|b| (b.displays(), b.cursor_in_video())).unwrap_or_default()
    };

    let capabilities = Capabilities {
        input: settings.host.allow_input,
        clipboard: settings.host.allow_clipboard,
        file_transfer: settings.host.allow_file_transfer,
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

    let reason = match session_loop(&inner, &conn, id, send, recv, displays, capabilities, cmd_rx).await {
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

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    inner: &Arc<Inner>,
    conn: &Connection,
    id: u64,
    mut send: SendStream,
    mut recv: iroh::endpoint::RecvStream,
    displays: Vec<DisplayInfo>,
    caps: Capabilities,
    mut cmds: mpsc::UnboundedReceiver<HostSessionCmd>,
) -> Result<String> {
    // Video writer: owns the uni-stream, fed by the encoder thread.
    let (video_tx, mut video_rx) = mpsc::channel::<VideoFrame>(4);
    let video_conn = conn.clone();
    let writer = tokio::spawn(async move {
        let mut stream = video_conn.open_uni().await?;
        while let Some(frame) = video_rx.recv().await {
            write_msg(&mut stream, &frame).await?;
        }
        stream.finish().ok();
        anyhow::Ok(())
    });
    let _writer_guard = AbortOnDrop(writer);

    // File browsing and transfer requests from the viewer.
    let files_inner = inner.clone();
    let policy = files::FilePolicy { browse: caps.file_transfer, receive: caps.file_transfer };
    let on_saved: Arc<dyn Fn(std::path::PathBuf) + Send + Sync> =
        Arc::new(move |path| files_inner.emit(ClientEvent::FileReceived { session: id, path }));
    let _files_guard = AbortOnDrop(tokio::spawn(files::serve(conn.clone(), policy, on_saved)));

    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel();
    let clipboard = if caps.clipboard { Clipboard::start(clip_tx, &inner.opts.clipboard) } else { None };

    let flow = Flow::new();
    let (err_tx, mut err_rx) = mpsc::unbounded_channel::<String>();
    let mut pipeline: Option<Pipeline> = None;
    let mut display = displays.iter().find(|d| d.primary).map(|d| d.index).unwrap_or(0);

    loop {
        tokio::select! {
            msg = read_msg::<ViewerMsg>(&mut recv) => {
                let Some(msg) = msg? else { return Ok("viewer closed the session".into()) };
                match msg {
                    ViewerMsg::StartStream { display: d, settings } => {
                        if !displays.iter().any(|x| x.index == d) {
                            write_msg(&mut send, &HostMsg::Notice(format!("no display {d}"))).await?;
                            continue;
                        }
                        display = d;
                        let stream_settings = sanitize_settings(settings);
                        drop(pipeline.take()); // stop the old capture first
                        let capture = {
                            let mut b = inner.backend.lock().await;
                            let b = b.as_mut().context("screen backend gone")?;
                            b.capture(display, stream_settings.max_fps)
                        };
                        match capture {
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
                        if let Some(p) = &pipeline { p.set_settings(sanitize_settings(s)); }
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
                    ViewerMsg::Chat(text) => {
                        let text: String = text.chars().take(4000).collect();
                        inner.emit(ClientEvent::HostChat { session: id, text });
                    }
                    ViewerMsg::Bye => return Ok("viewer disconnected".into()),
                    ViewerMsg::Hello { .. } | ViewerMsg::AuthResponse { .. } => bail!("protocol violation"),
                }
            }
            Some(text) = clip_rx.recv() => {
                if text.len() <= 1 << 20 {
                    write_msg(&mut send, &HostMsg::Clipboard(ClipboardData::Text(text))).await?;
                }
            }
            Some(err) = err_rx.recv() => {
                drop(pipeline.take());
                // The portal session may have been revoked; reopen next time.
                *inner.backend.lock().await = None;
                write_msg(&mut send, &HostMsg::Bye { reason: format!("screen capture stopped: {err}") }).await.ok();
                return Ok(format!("capture stopped: {err}"));
            }
            cmd = cmds.recv() => match cmd {
                Some(HostSessionCmd::Chat(text)) => write_msg(&mut send, &HostMsg::Chat(text)).await?,
                Some(HostSessionCmd::Disconnect) | None => {
                    write_msg(&mut send, &HostMsg::Bye { reason: "the host ended the session".into() }).await.ok();
                    send.finish().ok();
                    return Ok("ended by host".into());
                }
            },
            reason = conn.closed() => return Ok(format!("connection lost: {reason}")),
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
