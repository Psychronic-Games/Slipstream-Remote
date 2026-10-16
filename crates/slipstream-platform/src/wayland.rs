//! Wayland backend: one xdg-desktop-portal RemoteDesktop session provides
//! both screen streams (frames arrive over PipeWire) and input injection.
//! Works on KDE Plasma, GNOME and wlroots desktops with a portal backend.

use std::{
    os::fd::OwnedFd,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use ashpd::desktop::{
    PersistMode, Session,
    remote_desktop::{Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions},
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use pipewire as pw;
use pw::{properties::properties, spa};
use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::{DisplayInfo, InputEvent, MouseButton};
use tokio::sync::mpsc;

use crate::{Backend, FrameSlot, FrameStream, HeldInputs, keymap};

struct PortalStream {
    node_id: u32,
    info: DisplayInfo,
}

pub struct WaylandBackend {
    streams: Vec<PortalStream>,
    pipewire_fd: OwnedFd,
    input: mpsc::UnboundedSender<InputCmd>,
    restore_token: Option<String>,
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
        let rd = RemoteDesktop::new().await.context("RemoteDesktop portal unavailable")?;
        let sc = Screencast::new().await.context("ScreenCast portal unavailable")?;
        let session = rd.create_session(Default::default()).await.context("creating portal session")?;

        let mut devices = SelectDevicesOptions::default()
            .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
            .set_persist_mode(PersistMode::ExplicitlyRevoked);
        if let Some(token) = restore_token {
            devices = devices.set_restore_token(token);
        }
        rd.select_devices(&session, devices).await?.response().context("selecting input devices")?;
        sc.select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                .set_multiple(true),
        )
        .await?
        .response()
        .context("selecting screens")?;

        let started = rd
            .start(&session, None, Default::default())
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
        let pipewire_fd = sc.open_pipe_wire_remote(&session, Default::default()).await?;

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(input_task(rd, session, rx));
        tracing::info!("portal session started with {} screen(s)", streams.len());
        Ok(Self { streams, pipewire_fd, input: tx, restore_token: token })
    }
}

impl Backend for WaylandBackend {
    fn name(&self) -> &'static str {
        "wayland-portal"
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.streams.iter().map(|s| s.info.clone()).collect()
    }

    fn capture(&mut self, display: u32, max_fps: u32) -> Result<FrameStream> {
        let stream = self.streams.get(display as usize).context("no such screen")?;
        let fd = self.pipewire_fd.try_clone()?;
        let node = stream.node_id;
        let slot = FrameSlot::new();
        let producer = slot.clone();
        let min_interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);
        thread::Builder::new().name("pipewire".into()).spawn(move || {
            if let Err(e) = run_pipewire(fd, node, producer.clone(), min_interval) {
                tracing::error!("pipewire capture failed: {e:#}");
                producer.fail(format!("{e:#}"));
            }
        })?;
        Ok(FrameStream::new(slot))
    }

    fn inject(&mut self, display: u32, event: InputEvent) -> Result<()> {
        let s = self.streams.get(display as usize).context("no such screen")?;
        self.input
            .send(InputCmd { node_id: s.node_id, width: s.info.width as f64, height: s.info.height as f64, event })
            .map_err(|_| anyhow!("portal session closed"))
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

async fn input_task(rd: RemoteDesktop, session: Session<RemoteDesktop>, mut rx: mpsc::UnboundedReceiver<InputCmd>) {
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

struct CaptureState {
    format: spa::param::video::VideoInfoRaw,
    slot: Arc<FrameSlot>,
    last: Instant,
    min_interval: Duration,
}

fn run_pipewire(fd: OwnedFd, node: u32, slot: Arc<FrameSlot>, min_interval: Duration) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "slipstream-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let state = CaptureState {
        format: Default::default(),
        slot: slot.clone(),
        last: Instant::now() - min_interval,
        min_interval,
    };

    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, _, old, new| tracing::debug!("pipewire stream {old:?} -> {new:?}"))
        .param_changed(|_, st, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param) else { return };
            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if st.format.parse(param).is_ok() {
                let size = st.format.size();
                tracing::info!(
                    "capture format {:?} {}x{}",
                    st.format.format(),
                    size.width,
                    size.height
                );
            }
        })
        .process(|stream, st| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            // Allow 20% early: frames arrive with jitter, and a strict check
            // would drop every other frame at the monitor's refresh rate.
            if st.last.elapsed() < st.min_interval.mul_f32(0.8) {
                return;
            }
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let (size, offset, stride) = {
                let c = d.chunk();
                (c.size() as usize, c.offset() as usize, c.stride())
            };
            let fmt = st.format.format();
            let format = match fmt {
                spa::param::video::VideoFormat::BGRx | spa::param::video::VideoFormat::BGRA => PixelFormat::Bgra,
                spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA => PixelFormat::Rgba,
                _ => return,
            };
            let dims = st.format.size();
            let (w, h) = (dims.width, dims.height);
            let stride = if stride > 0 { stride as u32 } else { w * 4 };
            let Some(bytes) = d.data() else { return };
            let needed = (stride * h.saturating_sub(1) + w * 4) as usize;
            if size == 0 || w == 0 || h == 0 || offset + needed > bytes.len() {
                return; // cursor-only or corrupt buffer
            }
            st.last = Instant::now();
            st.slot.put(RawFrame {
                width: w,
                height: h,
                stride,
                format,
                data: bytes[offset..offset + needed].to_vec(),
            });
        })
        .register()?;

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRA,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle { width: 1920, height: 1080 },
            pw::spa::utils::Rectangle { width: 1, height: 1 },
            pw::spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 60, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| anyhow!("pod serialize: {e:?}"))?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&values).context("pod")?];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    // Poll for the consumer going away.
    let ml = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        if slot.is_closed() {
            ml.quit();
        }
    });
    timer.update_timer(Some(Duration::from_millis(100)), Some(Duration::from_millis(100)));
    mainloop.run();
    Ok(())
}
