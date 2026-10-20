//! Wayland backend: one xdg-desktop-portal RemoteDesktop session provides
//! both screen streams (frames arrive over PipeWire) and input injection.
//! Works on KDE Plasma, GNOME and wlroots desktops with a portal backend.

use std::{future::poll_fn, os::fd::OwnedFd, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use ashpd::desktop::{
    PersistMode, Session,
    remote_desktop::{Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions},
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use slipstream_core::proto::{DisplayInfo, InputEvent, MouseButton};
use futures_core::Stream;
use tokio::sync::mpsc;

use crate::{Backend, FrameSlot, FrameStream, HeldInputs, keymap, pw};

struct PortalStream {
    node_id: u32,
    info: DisplayInfo,
}

pub struct WaylandBackend {
    streams: Vec<PortalStream>,
    pipewire_fd: OwnedFd,
    input: mpsc::UnboundedSender<InputCmd>,
    restore_token: Option<String>,
    live: Arc<AtomicBool>,
}

/// Close incomplete requests when setup fails, times out, or is cancelled.
struct PendingSession(Option<Arc<Session<RemoteDesktop>>>);

impl Drop for PendingSession {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() && let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { let _ = tokio::time::timeout(Duration::from_secs(3), session.close()).await; });
        }
    }
}

struct InputCmd {
    node_id: u32,
    width: f64,
    height: f64,
    event: InputEvent,
}

impl WaylandBackend {
    /// Asks the desktop for screen + input access. Shows a permission
    /// dialog unless `restore_token` is still valid.
    pub async fn open(restore_token: Option<&str>) -> Result<Self> {
        // Check before asking the user for permission we couldn't use.
        pw::available()?;
        crate::portal_health::prepare().await?;
        let (rd, sc, session) = tokio::time::timeout(Duration::from_secs(15), async {
            let rd = RemoteDesktop::new().await.context("RemoteDesktop portal unavailable")?;
            let sc = Screencast::new().await.context("ScreenCast portal unavailable")?;
            let session = rd.create_session(Default::default()).await.context("creating portal session")?;
            anyhow::Ok((rd, sc, Arc::new(session)))
        }).await.context("The desktop screen-sharing service did not respond. Sign out and back in, then choose Config → Screen permission → Approve now")??;
        let mut pending = PendingSession(Some(session));
        let session = pending.0.as_ref().unwrap();
        // Subscribe before Start: closing/revoking access must invalidate the
        // cached backend even when nobody is currently watching the screen.
        let live = Arc::new(AtomicBool::new(true));
        let (watch, ended) = (session.clone(), live.clone());
        let (ready, subscribed) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            match watch.receive_closed().await {
                Ok(closed) => {
                    let _ = ready.send(Ok(()));
                    let mut closed = Box::pin(closed);
                    poll_fn(|cx| closed.as_mut().poll_next(cx)).await;
                    tracing::info!("desktop revoked or closed the screen-sharing session");
                },
                Err(error) => { let _ = ready.send(Err(error)); },
            }
            ended.store(false, Ordering::Release);
        });
        tokio::time::timeout(Duration::from_secs(3), subscribed).await
            .context("desktop session monitoring timed out")???;
        let (streams, pipewire_fd, token) = tokio::time::timeout(Duration::from_secs(120), async {

        let mut devices = SelectDevicesOptions::default()
            .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
            .set_persist_mode(PersistMode::ExplicitlyRevoked);
        if let Some(token) = restore_token {
            devices = devices.set_restore_token(token);
        }
        rd.select_devices(session, devices).await?.response().context("selecting input devices")?;
        sc.select_sources(
            session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                .set_multiple(true),
        )
        .await?
        .response()
        .context("selecting screens")?;

        let started = rd
            .start(session, None, Default::default())
            .await?
            .response()
            .context("screen sharing was declined")?;
        let token = started.restore_token().map(str::to_string);
        if !started.devices().contains(DeviceType::Pointer) {
            tracing::warn!("portal did not grant pointer control; view-only");
        }
        let streams: Vec<PortalStream> = started
            .streams()
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (w, h) = s.size().unwrap_or((1920, 1080));
                let (x, y) = s.position().unwrap_or((0, 0));
                PortalStream {
                    node_id: s.pipe_wire_node_id(),
                    info: DisplayInfo {
                        index: i as u32,
                        name: format!("Screen {}", i + 1),
                        width: w.max(1) as u32,
                        height: h.max(1) as u32,
                        x,
                        y,
                        primary: i == 0,
                    },
                }
            })
            .collect();
        if streams.is_empty() {
            bail!("no screens were shared");
        }
        let pipewire_fd = sc.open_pipe_wire_remote(session, Default::default()).await?;
        anyhow::Ok((streams, pipewire_fd, token))
        }).await.context("Screen-sharing approval timed out. Choose Config → Screen permission → Approve now. If no dialog appears, sign out and back in to refresh the desktop")??;

        let (tx, rx) = mpsc::unbounded_channel();
        let session = pending.0.take().unwrap();
        tokio::spawn(input_task(rd, session, rx));
        tracing::info!("portal session started with {} screen(s)", streams.len());
        Ok(Self { streams, pipewire_fd, input: tx, restore_token: token, live })
    }
}

impl Backend for WaylandBackend {
    fn name(&self) -> &'static str {
        "wayland-portal"
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        if !self.live.load(Ordering::Acquire) { return Vec::new(); }
        self.streams.iter().map(|s| s.info.clone()).collect()
    }

    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream> {
        let stream = self.streams.get(display as usize).context("no such screen")?;
        let fd = self.pipewire_fd.try_clone()?;
        let node = stream.node_id;
        let slot = FrameSlot::new();
        let min_interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);
        let capture = pw::Capture::start(fd, node, slot.clone(), min_interval)?;
        Ok(FrameStream::new(slot).with_cleanup(move || drop(capture)))
    }

    fn inject(&mut self, display: u32, event: InputEvent) -> Result<()> {
        let s = self.streams.get(display as usize).context("no such screen")?;
        self.input
            .send(InputCmd { node_id: s.node_id, width: s.info.width as f64, height: s.info.height as f64, event })
            .map_err(|_| anyhow!("portal session closed"))
    }

    fn cursor_in_video(&self) -> bool {
        true // CursorMode::Embedded
    }

    fn restore_token(&self) -> Option<String> {
        self.restore_token.clone()
    }
}

// Linux input-event-codes.h
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;
const BTN_SIDE: i32 = 0x113;
const BTN_EXTRA: i32 = 0x114;

async fn input_task(rd: RemoteDesktop, session: Arc<Session<RemoteDesktop>>, mut rx: mpsc::UnboundedReceiver<InputCmd>) {
    let mut held = HeldInputs::default();
    let mut wheel = (0.0f32, 0.0f32);
    let mut last_node = 0;
    while let Some(mut cmd) = rx.recv().await {
        // Coalesce bursts of pointer motion: only the latest position matters.
        while matches!(cmd.event, InputEvent::MouseMove { .. }) {
            match rx.try_recv() {
                Ok(next) if matches!(next.event, InputEvent::MouseMove { .. }) => cmd = next,
                Ok(next) => {
                    send(&rd, &session, &mut held, &mut wheel, cmd).await;
                    cmd = next;
                    break;
                }
                Err(_) => break,
            }
        }
        last_node = cmd.node_id;
        send(&rd, &session, &mut held, &mut wheel, cmd).await;
    }
    for event in held.release_all() {
        let cmd = InputCmd { node_id: last_node, width: 1.0, height: 1.0, event };
        apply(&rd, &session, &mut wheel, cmd).await.ok();
    }
    session.close().await.ok();
    tracing::info!("portal session closed");
}

async fn send(
    rd: &RemoteDesktop,
    session: &Session<RemoteDesktop>,
    held: &mut HeldInputs,
    wheel: &mut (f32, f32),
    cmd: InputCmd,
) {
    let InputCmd { node_id, width, height, event } = cmd;
    for event in held.process(event) {
        if let Err(e) = apply(rd, session, wheel, InputCmd { node_id, width, height, event }).await {
            tracing::debug!("portal input failed: {e}");
        }
    }
}

async fn apply(
    rd: &RemoteDesktop,
    session: &Session<RemoteDesktop>,
    wheel: &mut (f32, f32),
    cmd: InputCmd,
) -> ashpd::Result<()> {
    match cmd.event {
        InputEvent::MouseMove { x, y } => {
            let px = x.clamp(0.0, 1.0) as f64 * (cmd.width - 1.0).max(0.0);
            let py = y.clamp(0.0, 1.0) as f64 * (cmd.height - 1.0).max(0.0);
            rd.notify_pointer_motion_absolute(session, cmd.node_id, px, py, Default::default()).await
        }
        InputEvent::MouseButton { button, down } => {
            let code = match button {
                MouseButton::Left => BTN_LEFT,
                MouseButton::Right => BTN_RIGHT,
                MouseButton::Middle => BTN_MIDDLE,
                MouseButton::Back => BTN_SIDE,
                MouseButton::Forward => BTN_EXTRA,
            };
            let state = if down { KeyState::Pressed } else { KeyState::Released };
            rd.notify_pointer_button(session, code, state, Default::default()).await
        }
        InputEvent::Wheel { dx, dy } => {
            wheel.0 += dx;
            wheel.1 += dy;
            let (sx, sy) = (wheel.0.trunc() as i32, wheel.1.trunc() as i32);
            wheel.0 -= sx as f32;
            wheel.1 -= sy as f32;
            // Protocol: positive = up/left. Portal: positive = down/right.
            if sy != 0 {
                rd.notify_pointer_axis_discrete(session, Axis::Vertical, -sy, Default::default()).await?;
            }
            if sx != 0 {
                rd.notify_pointer_axis_discrete(session, Axis::Horizontal, -sx, Default::default()).await?;
            }
            Ok(())
        }
        InputEvent::Key { code, down } => {
            let Some(k) = keymap::lookup(&code) else { return Ok(()) };
            if k.evdev == 0 {
                return Ok(());
            }
            let state = if down { KeyState::Pressed } else { KeyState::Released };
            rd.notify_keyboard_keycode(session, k.evdev as i32, state, Default::default()).await
        }
        InputEvent::ReleaseAll => Ok(()),
    }
}
