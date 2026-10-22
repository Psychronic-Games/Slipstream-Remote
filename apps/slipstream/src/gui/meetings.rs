//! Meeting UI and local media ownership. A room lives independently of support tabs.
use super::{App, Page, theme, theme::pal, video};
use anyhow::Result;
use eframe::egui::{self, RichText, Ui};
use slipstream_audio::voice::Microphone;
use slipstream_core::{
    config::ServerSettings,
    directory::DeviceId,
    meeting::{CAPACITY, Invitation, Source, State},
};
use slipstream_session::{
    Client,
    meetings::{
        Command, Event, Meeting,
        calls::{self, IncomingCall},
        media::{Sound, VideoReceiver, VideoSender},
    },
};
use std::{collections::VecDeque, time::Duration};
use tokio::{runtime::Runtime, sync::oneshot};
mod devices;
mod screens;

pub(super) const STREAM_BASE: u64 = 1 << 63;

struct Pending<T> {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    rx: oneshot::Receiver<Result<T>>,
    task: tokio::task::JoinHandle<()>,
}
impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.task.abort();
    }
}
fn pending<T: Send + 'static>(
    rt: &Runtime,
    ctx: &egui::Context,
    work: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> Pending<T> {
    let (tx, rx) = oneshot::channel();
    let wake = ctx.clone();
    let task = rt.spawn(async move {
        let _ = tx.send(work.await);
        wake.request_repaint();
    });
    Pending {
        rx,
        task,
        cancelled: Default::default(),
    }
}
fn ready<T>(job: &mut Option<Pending<T>>) -> Option<Result<T>> {
    let result = match job.as_mut()?.rx.try_recv() {
        Ok(result) => result,
        Err(oneshot::error::TryRecvError::Empty) => return None,
        Err(_) => Err(anyhow::anyhow!("Meeting operation was cancelled")),
    };
    job.take();
    Some(result)
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum RoomTab {
    #[default]
    Live,
    Chat,
    Participants,
    Devices,
}
#[derive(Default)]
pub(super) struct Meetings {
    name: String,
    title: String,
    pub join_text: String,
    call_id: String,
    active: Option<Active>,
    joining: Option<Pending<Meeting>>,
    error: Option<String>,
    pub incoming: Option<IncomingCall>,
    last_call: Option<u64>,
    receiver: Option<calls::ReceiverGuard>,
    target: Option<(DeviceId, ServerSettings)>,
    outgoing: Option<calls::Outgoing>,
    calling: Option<Pending<calls::Outgoing>>,
    microphone_device: Option<String>,
    camera_index: Option<u32>,
    devices: devices::Devices,
    preferences_loaded: bool,
    setup_devices: bool,
    screen_picker: Option<screens::Picker>,
    confirm_end: bool,
    confirm_remove: Option<(u64, String)>,
    capacity: usize,
    #[cfg(target_os = "android")]
    notifications_requested: bool,
}
struct Active {
    meeting: Meeting,
    video: VideoReceiver,
    sound: Option<Sound>,
    microphone: Option<Microphone>,
    sender: Option<VideoSender>,
    sound_job: Option<Pending<Sound>>,
    mic_job: Option<Pending<Microphone>>,
    video_job: Option<Pending<VideoSender>>,
    source: Source,
    next_source: Source,
    subscriptions: Vec<u64>,
    sound_attempted: bool,
    chat: VecDeque<slipstream_core::meeting::Chat>,
    message: String,
    pin: Option<u64>,
    tab: RoomTab,
    unread: usize,
    microphone_name: String,
    video_name: String,
    next_video_name: String,
}
impl Active {
    fn new(meeting: Meeting) -> Result<Self> {
        let commands = meeting.command_sender();
        let video = VideoReceiver::new(move |id| {
            let _ = commands.try_send(Command::Keyframe(id));
        })?;
        Ok(Self {
            meeting,
            video,
            sound: None,
            microphone: None,
            sender: None,
            sound_job: None,
            mic_job: None,
            video_job: None,
            source: Source::None,
            next_source: Source::None,
            subscriptions: Vec::new(),
            sound_attempted: false,
            chat: VecDeque::new(),
            message: String::new(),
            pin: None,
            tab: RoomTab::Live,
            unread: 0,
            microphone_name: String::new(),
            video_name: String::new(),
            next_video_name: String::new(),
        })
    }
    fn state(&self) -> State {
        self.meeting.state.borrow().clone()
    }
    fn announce(&self) {
        self.meeting.command(Command::Media {
            microphone: self.microphone.is_some(),
            source: self.source,
        });
    }
    fn stop_media(&mut self) {
        self.mic_job = None;
        self.video_job = None;
        self.microphone = None;
        self.sender = None;
        self.source = Source::None;
        #[cfg(target_os = "android")]
        {
            let _ = slipstream_platform::android::action(12);
            let _ = slipstream_platform::android::action(15);
        }
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        self.stop_media();
    }
}

impl Meetings {
    pub(super) fn call_target(&mut self, id: DeviceId, server: ServerSettings) {
        self.target = Some((id, server));
    }
    fn host(&mut self, client: &Client, rt: &Runtime, ctx: &egui::Context) {
        if self.active.is_some() || self.joining.is_some() {
            return;
        }
        let settings = client.settings();
        let name = self.name.clone();
        let title = self.title.clone();
        let capacity = self.capacity.max(2);
        self.error = None;
        self.joining = Some(pending(rt, ctx, async move {
            Meeting::host(&settings.server, &name, &title, capacity).await
        }));
    }
    fn join(&mut self, invite: Invitation, client: &Client, rt: &Runtime, ctx: &egui::Context) {
        if self.active.is_some() || self.joining.is_some() {
            self.error = Some("Leave your current meeting before joining another".into());
            return;
        }
        let settings = client.settings();
        let name = self.name.clone();
        self.error = None;
        self.joining = Some(pending(rt, ctx, async move {
            Meeting::join(invite, &settings.server, &name).await
        }));
    }
    fn poll(&mut self, app: &mut App, ctx: &egui::Context) {
        if !self.preferences_loaded {
            self.preferences_loaded = true;
            let settings = app.client.settings();
            self.microphone_device = settings.app.meeting_microphone;
            self.camera_index = settings.app.meeting_camera;
        }
        self.devices.poll();
        if let Some(saved)=self.camera_index
            && let [(index,_)]=self.devices.cameras.as_slice()
            && *index!=saved
        {
            self.camera_index=Some(*index);
            app.client.update_settings(|s|s.app.meeting_camera=self.camera_index);
        }
        if app.page != Page::Meetings {self.screen_picker=None;}
        if let Some(picker) = &mut self.screen_picker {
            picker.poll(app, ctx);
        }
        let progress = self
            .outgoing
            .as_ref()
            .map(|call| call.state.borrow().clone());
        match progress {
            Some(calls::Progress::Answered) => {
                self.outgoing = None;
                app.toast("Call answered", pal::GREEN);
            }
            Some(calls::Progress::Ended(reason)) => {
                self.outgoing = None;
                app.toast(reason, pal::GOLD);
            }
            _ => {}
        }
        if self.name.is_empty() {
            self.name = app.client.settings().device_name;
        }
        if self.title.is_empty() {
            self.title = "Subspace meeting".into();
        }
        if self.capacity == 0 {
            self.capacity = CAPACITY;
        }
        if self.receiver.is_none() {
            self.receiver = Some(app.client.call_broker().receiver());
        }
        if !app.via_service {
            self.incoming = app.client.call_broker().incoming();
        }
        if self
            .incoming
            .as_ref()
            .is_some_and(|call| call.expires <= slipstream_core::meeting::now())
        {
            self.incoming = None;
        }
        if let Some(call) = self.incoming.clone()
            && self.last_call != Some(call.id)
        {
            self.last_call = Some(call.id);
            if self.active.is_some() || self.joining.is_some() {
                answer(app, call.id, false);
                self.incoming = None;
            } else {
                app.set_visible(ctx, true);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        if let Some(result) = ready(&mut self.joining) {
            match result.and_then(Active::new) {
                Ok(active) => {
                    self.active = Some(active);
                    app.page = Page::Meetings;
                }
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    self.target = None;
                }
            }
        }
        if self.target.is_some() && self.active.is_none() && self.joining.is_none() {
            self.host(&app.client, &app.rt, ctx);
        }
        if let Some(result) = ready(&mut self.calling) {
            match result {
                Ok(call) => self.outgoing = Some(call),
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
        if let Some(active) = self.active.as_mut() {
            let state = active.state();
            if state.ended.is_some() {
                self.screen_picker = None;
                self.calling = None;
                self.outgoing = None;
                self.confirm_end = false;
                self.confirm_remove = None;
                active.stop_media();
                active.sound_job = None;
                active.sound = None;
                self.target = None;
            } else {
                if !state.waiting && !active.sound_attempted {
                    active.sound_attempted = true;
                    active.sound_job = Some(pending(&app.rt, ctx, async {
                        tokio::task::spawn_blocking(Sound::start).await?
                    }));
                }
                if let Some(result) = ready(&mut active.sound_job) {
                    match result {
                        Ok(sound) => active.sound = Some(sound),
                        Err(e) => self.error = Some(format!("Speakers: {e:#}")),
                    }
                }
                let cancelled = active
                    .mic_job
                    .as_ref()
                    .is_some_and(|job| job.cancelled.load(std::sync::atomic::Ordering::Acquire));
                if let Some(result) = ready(&mut active.mic_job)
                    && !cancelled
                {
                    match result {
                        Ok(mic) => {
                            active.microphone = Some(mic);
                            active.announce();
                        }
                        Err(e) => self.error = Some(format!("Microphone: {e:#}")),
                    }
                }
                let cancelled = active
                    .video_job
                    .as_ref()
                    .is_some_and(|job| job.cancelled.load(std::sync::atomic::Ordering::Acquire));
                if let Some(result) = ready(&mut active.video_job)
                    && !cancelled
                {
                    match result {
                        Ok(sender) => {
                            active.sender = Some(sender);
                            active.source = active.next_source;
                            active.video_name = active.next_video_name.clone();
                            active.announce();
                        }
                        Err(e) => self.error = Some(format!("Sharing: {e:#}")),
                    }
                }
                #[cfg(target_os = "android")]
                if active.microphone.is_some() && !slipstream_platform::android::microphone_ready()
                {
                    active.microphone = None;
                    active.announce();
                }
                if let Some(error) = active
                    .sender
                    .as_ref()
                    .and_then(|sender| sender.error.lock().unwrap().clone())
                {
                    active.sender = None;
                    active.source = Source::None;
                    active.announce();
                    self.error = Some(error);
                }
                while let Ok(event) = active.meeting.events.try_recv() {
                    match event {
                        Event::Chat(chat) => {
                            if (active.tab != RoomTab::Chat || app.page != Page::Meetings)
                                && chat.sender != state.local
                            {
                                active.unread = active.unread.saturating_add(1);
                            }
                            active.chat.push_back(chat);
                            while active.chat.len() > 128 {
                                active.chat.pop_front();
                            }
                        }
                        Event::Keyframe => {
                            if let Some(sender) = &active.sender {
                                sender
                                    .keyframe
                                    .store(true, std::sync::atomic::Ordering::Release);
                            }
                        }
                        Event::Notice(text) => self.error = Some(text),
                    }
                }
                let mut subscriptions = Vec::new();
                if let Some(pin) = active.pin
                    && state
                        .participants
                        .iter()
                        .any(|p| p.id == pin && p.source != Source::None)
                {
                    subscriptions.push(pin);
                }
                for source in [Source::Screen, Source::Camera] {
                    for member in &state.participants {
                        if member.id != state.local
                            && member.source == source
                            && !subscriptions.contains(&member.id)
                            && subscriptions.len() < 4
                        {
                            subscriptions.push(member.id);
                        }
                    }
                }
                if subscriptions != active.subscriptions {
                    active
                        .meeting
                        .command(Command::Subscribe(subscriptions.clone()));
                    active.video.subscribe(subscriptions.clone());
                    active.subscriptions = subscriptions;
                }
                while let Ok(frame) = active.meeting.video.try_recv() {
                    active.video.push(frame);
                }
                if let Some(sound) = &active.sound {
                    sound.members(
                        state
                            .participants
                            .iter()
                            .filter(|p| p.id != state.local)
                            .map(|p| p.id)
                            .collect(),
                    );
                    while let Ok(packet) = active.meeting.audio.try_recv() {
                        sound.push(packet);
                    }
                } else {
                    while active.meeting.audio.try_recv().is_ok() {}
                }
                if self.target.is_some()
                    && (state.locked || state.participants.len() >= state.capacity)
                {
                    self.target = None;
                    self.error = Some(
                        if state.locked {
                            "Reopen admission in Participants before calling someone."
                        } else {
                            "This meeting is full."
                        }
                        .into(),
                    );
                }
                if state.host
                    && self.calling.is_none()
                    && let Some((id, server)) = self.target.take()
                {
                    let commands = active.meeting.command_sender();
                    let client = app.client.clone();
                    self.calling = Some(pending(&app.rt, ctx, async move {
                        let (tx, rx) = oneshot::channel();
                        commands.send(Command::CallInvitation(tx)).await?;
                        let invite = tokio::time::timeout(Duration::from_secs(3), rx).await??;
                        Ok(calls::Outgoing::start(client, server, id, invite))
                    }));
                } else if !state.host && self.target.take().is_some() {
                    self.error =
                        Some("Only the organizer can call people into this meeting".into());
                }
            }
        }
        if self.active.is_some()
            || self.joining.is_some()
            || self.incoming.is_some()
            || self.outgoing.is_some()
            || self.calling.is_some()
            || self.devices.busy()
            || self.screen_picker.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
    }
}
pub(super) fn poll(app: &mut App, ctx: &egui::Context) {
    let mut meetings = std::mem::take(&mut app.meetings);
    meetings.poll(app, ctx);
    incoming(&mut meetings, app, ctx);
    app.meetings = meetings;
}
fn answer(app: &App, id: u64, accepted: bool) -> bool {
    #[cfg(windows)]
    if app.via_service {
        return app
            .service_chat
            .as_ref()
            .is_some_and(|s| s.sender().answer_call(id, accepted));
    }
    app.client.call_broker().answer(id, accepted);
    true
}
fn incoming(meetings: &mut Meetings, app: &mut App, ctx: &egui::Context) {
    let Some(call) = meetings.incoming.clone() else {
        return;
    };
    let mut accepted = None;
    theme::modal("incoming-meeting-call",pal::GREEN).show(ctx,|ui|{
        ui.set_width(theme::dialog_width(ctx,440.0));if theme::window_header_with_close(ui,"Incoming call",pal::GREEN,pal::PEACH).close{accepted=Some(false);}
        ui.label(RichText::new(&call.name).font(theme::display(24.0)).color(pal::TAN));
        ui.label(format!("Inviting you to {}",call.invitation.title));
        ui.label("Answer with your microphone muted. You can unmute or share your camera in the meeting.");
        ui.horizontal_wrapped(|ui|{
            if theme::pill(ui,"Answer",pal::GREEN).clicked(){accepted=Some(true);}
            if theme::pill(ui,"Decline",pal::RED).clicked(){accepted=Some(false);}
        });
    });
    if let Some(accepted) = accepted {
        if answer(app, call.id, accepted) {
            meetings.incoming = None;
            if accepted {
                meetings.join(call.invitation, &app.client, &app.rt, ctx);
                app.page = Page::Meetings;
            }
        } else {
            meetings.error =
                Some("The service disconnected; the call could not be answered".into());
        }
    }
}
fn start_microphone(active: &mut Active, meetings: &mut Meetings, app: &App, ctx: &egui::Context) {
    if active.mic_job.is_some() {
        return;
    }
    let Some(sound) = &active.sound else {
        meetings.error = Some("Speakers are not ready. Retry speakers before unmuting.".into());
        return;
    };
    active.microphone_name = meetings
        .devices
        .microphone_name(meetings.microphone_device.as_deref());
    let player = sound.player.clone();
    let publisher = active.meeting.publisher.clone();
    let device = meetings.microphone_device.clone().or_else(|| {
        meetings
            .devices
            .microphones
            .iter()
            .find(|d| d.is_default)
            .map(|d| d.id.clone())
    });
    let cancelled: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
    let worker_cancel = cancelled.clone();
    let mut job = pending(&app.rt, ctx, async move {
        tokio::task::spawn_blocking(move || {
            anyhow::ensure!(
                !worker_cancel.load(std::sync::atomic::Ordering::Acquire),
                "Microphone cancelled"
            );
            #[cfg(target_os = "android")]
            {
                slipstream_platform::android::action(14)?;
                let start = std::time::Instant::now();
                while !slipstream_platform::android::microphone_ready() {
                    if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                        anyhow::bail!("Microphone cancelled");
                    }
                    if start.elapsed() > Duration::from_secs(60) {
                        let _ = slipstream_platform::android::action(15);
                        anyhow::bail!("Microphone permission did not arrive in time");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            anyhow::ensure!(
                !worker_cancel.load(std::sync::atomic::Ordering::Acquire),
                "Microphone cancelled"
            );
            let microphone =
                Microphone::start(player, device, move |packet| publisher.audio(packet.into()))?;
            anyhow::ensure!(
                !worker_cancel.load(std::sync::atomic::Ordering::Acquire),
                "Microphone cancelled"
            );
            Ok(microphone)
        })
        .await?
    });
    job.cancelled = cancelled;
    active.mic_job = Some(job);
}
fn start_video(
    active: &mut Active,
    source: Source,
    index: u32,
    client: &Client,
    rt: &Runtime,
    ctx: &egui::Context,
) {
    if active.video_job.is_some() {
        return;
    }
    active.sender = None;
    active.source = Source::None;
    active.announce();
    active.next_source = source;
    let client = client.clone();
    let publisher = active.meeting.publisher.clone();
    let cancelled: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
    let worker_cancel = cancelled.clone();
    let mut job = pending(rt, ctx, async move {
        let capture = if source == Source::Camera {
            let cancel = worker_cancel.clone();
            tokio::task::spawn_blocking(move || client.camera_for_meeting(index, cancel)).await??
        } else {
            client.capture_for_meeting(index).await?
        };
        anyhow::ensure!(
            !worker_cancel.load(std::sync::atomic::Ordering::Acquire),
            "Sharing cancelled"
        );
        VideoSender::start(capture, publisher)
    });
    job.cancelled = cancelled;
    active.video_job = Some(job);
}
fn call_form(meetings: &mut Meetings, app: &App, ui: &mut Ui, allow: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label("System ID");
        ui.add(
            egui::TextEdit::singleline(&mut meetings.call_id)
                .hint_text("9-digit ID")
                .desired_width(150.0),
        );
        let available = allow && meetings.calling.is_none() && meetings.outgoing.is_none();
        if ui
            .add_enabled_ui(available, |ui| theme::pill(ui, "Call system", pal::GREEN))
            .inner
            .clicked()
        {
            match meetings.call_id.parse::<DeviceId>() {
                Ok(id) => meetings.call_target(id, app.client.settings().server),
                Err(e) => meetings.error = Some(format!("{e:#}")),
            }
        }
    });
}
fn room_video(active: &mut Active, state: &State, ui: &mut Ui) {
    let frames = active.video.frames.lock().unwrap().clone();
    let mut tiles = Vec::new();
    for id in &active.subscriptions {
        if let Some((serial, frame)) = frames.get(id) {
            let name = state
                .participants
                .iter()
                .find(|p| p.id == *id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            tiles.push((*id, *serial, frame.clone(), name));
        }
    }
    if let Some(sender) = &active.sender
        && let Some((serial, frame)) = sender.preview.lock().unwrap().clone()
    {
        tiles.push((0, serial, frame, "You · preview".into()));
    }
    if tiles.is_empty() {
        ui.label(RichText::new("Cameras and shared screens appear here").color(pal::DIM));
        return;
    }
    let columns = if ui.available_width() > 720.0 && tiles.len() > 1 {
        2
    } else {
        1
    };
    for row in tiles.chunks(columns) {
        ui.columns(columns, |cols| {
            for (ui, (id, serial, frame, name)) in cols.iter_mut().zip(row) {
                ui.label(RichText::new(name).color(pal::TAN));
                let width = ui.available_width();
                let height = (width * 0.6).clamp(100.0, 380.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, egui::Color32::BLACK);
                let fit = video::fit_rect(rect, (frame.width, frame.height));
                ui.painter()
                    .add(eframe::egui_wgpu::Callback::new_paint_callback(
                        fit,
                        video::VideoCallback {
                            stream: STREAM_BASE + id,
                            serial: *serial,
                            frame: frame.clone(),
                        },
                    ));
            }
        });
    }
}

fn stop_microphone(active: &mut Active) {
    active.microphone = None;
    active.announce();
    #[cfg(target_os = "android")]
    {
        let _ = slipstream_platform::android::action(15);
    }
}
fn stop_video(active: &mut Active) {
    active.sender = None;
    active.source = Source::None;
    active.video_name.clear();
    active.announce();
}
fn start_camera(active: &mut Active, meetings: &mut Meetings, app: &App, ctx: &egui::Context) {
    if let Some(index) = meetings.devices.camera(meetings.camera_index) {
        active.next_video_name = meetings.devices.camera_name(meetings.camera_index);
        start_video(active, Source::Camera, index, &app.client, &app.rt, ctx);
    } else {
        meetings.error =
            Some("No camera is available. Open Devices to refresh or select a camera.".into());
    }
}
pub(super) fn ui(app: &mut App, ui: &mut Ui) {
    let mut meetings = std::mem::take(&mut app.meetings);
    let ctx = ui.ctx().clone();
    meetings.devices.load(app, &ctx);
    #[cfg(target_os = "android")]
    if !meetings.notifications_requested {
        meetings.notifications_requested = true;
        let _ = slipstream_platform::android::action(16);
    }
    if let Some(error) = meetings.error.clone() {
        theme::panel(ui, "Meeting notice", pal::GOLD, |ui| {
            ui.label(error);
            if theme::pill(ui, "Dismiss", pal::BLUE).clicked() {
                meetings.error = None;
            }
        });
    }
    if meetings.calling.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label("Preparing call…");
            if theme::pill(ui, "Cancel call", pal::PEACH).clicked() {
                meetings.calling = None;
                meetings.target = None;
            }
        });
    }
    if let Some(call) = &meetings.outgoing {
        let progress = call.state.borrow().clone();
        if matches!(
            progress,
            calls::Progress::Calling | calls::Progress::Ringing
        ) {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(if matches!(progress, calls::Progress::Ringing) {
                    "Ringing · waiting for an answer"
                } else {
                    "Calling…"
                });
                if theme::pill(ui, "Cancel call", pal::PEACH)
                    .on_hover_text("Stop ringing this person. Your meeting stays open.")
                    .clicked()
                {
                    meetings.outgoing = None;
                }
            });
        }
    }
    if meetings.joining.is_some() {
        theme::panel(ui, "Joining meeting", pal::GREEN, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label("Preparing meeting…");
                if theme::pill(ui, "Cancel", pal::PEACH).clicked() {
                    meetings.joining = None;
                    meetings.target = None;
                }
            });
        });
    } else if meetings.active.is_none() {
        ui.horizontal_wrapped(|ui| {
            if theme::pill(
                ui,
                "Start or join",
                if !meetings.setup_devices {
                    pal::GREEN
                } else {
                    pal::BLUE
                },
            )
            .clicked()
            {
                meetings.setup_devices = false;
            }
            if theme::pill(
                ui,
                "Devices",
                if meetings.setup_devices {
                    pal::GREEN
                } else {
                    pal::BLUE
                },
            )
            .clicked()
            {
                meetings.setup_devices = true;
            }
        });
        ui.add_space(8.0);
        if meetings.setup_devices {
            devices::ui(&mut meetings, None, app, ui);
        } else {
            setup(&mut meetings, app, ui);
        }
    }
    let mut leave = false;
    if let Some(mut active) = meetings.active.take() {
        let state = active.state();
        theme::panel(ui, &state.title, pal::GREEN, |ui| {
            if let Some(reason) = &state.ended {
                ui.label(RichText::new(reason).color(pal::GOLD));
                if theme::pill(ui, "Close meeting", pal::PEACH).clicked() {
                    leave = true;
                }
            } else if state.waiting {
                ui.label(RichText::new("Waiting for the organizer to admit you…").color(pal::GOLD));
                ui.label("Your microphone and camera are off while you wait.");
                if theme::pill(ui, "Cancel join request", pal::PEACH).clicked() {
                    leave = true;
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "{} / {} participants{}",
                        state.participants.len(),
                        state.capacity,
                        if state.host {
                            " · You are the organizer"
                        } else {
                            ""
                        }
                    ));
                    if state.locked {
                        ui.label(RichText::new("Joining closed").color(pal::GOLD));
                    }
                    if !state.lobby.is_empty()
                        && theme::pill(
                            ui,
                            &format!("{} waiting to join", state.lobby.len()),
                            pal::GOLD,
                        )
                        .clicked()
                    {
                        active.tab = RoomTab::Participants;
                    }
                });
                toolbar(&mut meetings, &mut active, &state, app, ui, &mut leave);
            }
        });
        if !state.waiting && state.ended.is_none() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let chat = if active.unread > 0 {
                    format!("Chat · {} new", active.unread)
                } else {
                    "Chat".into()
                };
                let people = format!("Participants · {}", state.participants.len());
                for (tab, label) in [
                    (RoomTab::Live, "Live meeting"),
                    (RoomTab::Chat, chat.as_str()),
                    (RoomTab::Participants, people.as_str()),
                    (RoomTab::Devices, "Devices"),
                ] {
                    if theme::pill(
                        ui,
                        label,
                        if active.tab == tab {
                            pal::GREEN
                        } else {
                            pal::BLUE
                        },
                    )
                    .clicked()
                    {
                        active.tab = tab;
                        if tab == RoomTab::Chat {
                            active.unread = 0;
                        }
                    }
                }
            });
            ui.add_space(10.0);
            match active.tab {
                RoomTab::Live => {
                    theme::panel(ui, "Live meeting", pal::BLUE, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("meeting-gallery")
                            .max_height(body_height(ui, 110.0))
                            .show(ui, |ui| {
                                room_video(&mut active, &state, ui);
                            });
                        if active.sender.is_none() && active.subscriptions.is_empty() {
                            ui.add_space(12.0);
                            ui.label(
                                RichText::new("You're in the meeting")
                                    .font(theme::display(24.0))
                                    .color(pal::TAN),
                            );
                            ui.label("Turn on your camera or share a screen. Voice and Subspace chat work with cameras off.");
                            if state.host
                                && theme::pill(ui, "Invite participants", pal::LAVENDER).clicked()
                            {
                                active.tab = RoomTab::Participants;
                            }
                            ui.add_space(12.0);
                        }
                    });
                }
                RoomTab::Chat => {
                    active.unread = 0;
                    chat(&mut meetings, &mut active, ui);
                }
                RoomTab::Participants => {
                    egui::ScrollArea::vertical()
                        .id_salt("meeting-people")
                        .max_height(body_height(ui, 70.0))
                        .show(ui, |ui| {
                            participants(&mut meetings, &mut active, &state, app, ui);
                        });
                }
                RoomTab::Devices => {
                    egui::ScrollArea::vertical()
                        .id_salt("meeting-device-page")
                        .max_height(body_height(ui, 70.0))
                        .show(ui, |ui| {
                            devices::ui(&mut meetings, Some(&mut active), app, ui);
                        });
                }
            }
        }
        meetings.active = Some(active);
    }
    if meetings.confirm_end {
        theme::modal("end-meeting-confirm",pal::RED).show(&ctx,|ui| {
            ui.set_width(theme::dialog_width(&ctx,430.0));
            if theme::window_header_with_close(ui,"End meeting for everyone?",pal::RED,pal::PEACH).close {meetings.confirm_end=false;}
            ui.label("Everyone will be disconnected from this meeting. Remote support links stay connected.");
            ui.horizontal_wrapped(|ui| {
                if theme::pill(ui,"End for everyone",pal::RED).clicked(){meetings.confirm_end=false;leave=true;}
                if theme::pill(ui,"Keep meeting open",pal::BLUE).clicked(){meetings.confirm_end=false;}
            });
        });
    }
    if let Some((id, name)) = meetings.confirm_remove.clone() {
        theme::modal("remove-meeting-participant", pal::PEACH).show(&ctx, |ui| {
            ui.set_width(theme::dialog_width(&ctx, 400.0));
            if theme::window_header_with_close(ui, "Remove participant?", pal::PEACH, pal::BLUE)
                .close
            {
                meetings.confirm_remove = None;
            }
            ui.label(format!("Remove {name} from this meeting?"));
            ui.horizontal_wrapped(|ui| {
                if theme::pill(ui, "Remove participant", pal::RED).clicked() {
                    if let Some(active) = &meetings.active {
                        active.meeting.command(Command::Remove(id));
                    }
                    meetings.confirm_remove = None;
                }
                if theme::pill(ui, "Cancel", pal::BLUE).clicked() {
                    meetings.confirm_remove = None;
                }
            });
        });
    }
    if leave {
        if let Some(active) = meetings.active.take() {
            active.meeting.command(Command::Leave);
        }
        meetings.outgoing = None;
        meetings.calling = None;
        meetings.target = None;
        meetings.screen_picker = None;
        meetings.confirm_end = false;
        meetings.confirm_remove = None;
        video::retain_streams(&app.render_state, |id| id < STREAM_BASE);
    }
    screens::ui(&mut meetings, app, &ctx);
    app.meetings = meetings;
}
fn setup(meetings: &mut Meetings, app: &App, ui: &mut Ui) {
    theme::panel(ui, "Start a meeting", pal::GREEN, |ui| {
        ui.label("Talk, share a screen, or show your camera. Everyone joins muted with their camera off.");
        ui.label("Your name");
        ui.text_edit_singleline(&mut meetings.name);
        ui.label("Meeting title");
        ui.text_edit_singleline(&mut meetings.title);
        ui.horizontal_wrapped(|ui| {
            ui.label("Participant limit");
            ui.add(egui::DragValue::new(&mut meetings.capacity).range(2..=32));
        });
        if theme::pill(ui, "Create meeting", pal::GREEN).clicked() {
            meetings.host(&app.client, &app.rt, ui.ctx());
        }
        ui.add_space(10.0);
        ui.label(format!(
            "Microphone: {}",
            meetings
                .devices
                .microphone_name(meetings.microphone_device.as_deref())
        ));
        ui.label(format!(
            "Camera: {}",
            meetings.devices.camera_name(meetings.camera_index)
        ));
        if theme::pill(ui, "Choose devices", pal::BLUE).clicked() {
            meetings.setup_devices = true;
        }
    });
    ui.add_space(10.0);
    theme::panel(ui, "Join a meeting", pal::BLUE, |ui| {
        ui.label("Paste the invitation your organizer shared with you.");
        ui.add(
            egui::TextEdit::multiline(&mut meetings.join_text)
                .desired_rows(2)
                .desired_width(ui.available_width())
                .hint_text("Slipstream meeting invitation"),
        );
        if theme::pill(ui, "Join meeting", pal::BLUE).clicked() {
            match Invitation::parse(&meetings.join_text) {
                Ok(invite) => meetings.join(invite, &app.client, &app.rt, ui.ctx()),
                Err(error) => meetings.error = Some(format!("{error:#}")),
            }
        }
    });
    ui.add_space(10.0);
    theme::panel(ui, "Call a system", pal::LAVENDER, |ui| {
        ui.label("Ring someone by their Slipstream ID, or use Meeting beside a saved system.");
        call_form(meetings, app, ui, true);
        let mut allowed = !app.client.settings().app.block_meeting_calls;
        if ui
            .checkbox(&mut allowed, "Allow incoming meeting calls")
            .changed()
        {
            app.client
                .update_settings(|s| s.app.block_meeting_calls = !allowed);
            #[cfg(windows)]
            if app.via_service
                && let Some(bridge) = &app.service_chat
            {
                bridge.sender().meeting_calls(allowed);
            }
        }
    });
}
fn toolbar(
    meetings: &mut Meetings,
    active: &mut Active,
    state: &State,
    app: &App,
    ui: &mut Ui,
    leave: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(
            active.mic_job.is_none() && (active.microphone.is_some() || active.sound.is_some()),
            |ui| {
                if theme::pill(
                    ui,
                    if active.microphone.is_some() {
                        "Mute"
                    } else {
                        "Unmute"
                    },
                    if active.microphone.is_some() {
                        pal::GREEN
                    } else {
                        pal::BLUE
                    },
                )
                .clicked()
                {
                    if active.microphone.is_some() {
                        stop_microphone(active);
                    } else {
                        start_microphone(active, meetings, app, ui.ctx());
                    }
                }
            },
        );
        ui.add_enabled_ui(active.video_job.is_none(), |ui| {
            let label = if cfg!(target_os = "android") {
                if active.source == Source::Camera {
                    "Stop Show me"
                } else {
                    "Show me"
                }
            } else if active.source == Source::Camera {
                "Stop camera"
            } else {
                "Camera"
            };
            if theme::pill(ui, label, pal::VIOLET).clicked() {
                if active.source == Source::Camera {
                    stop_video(active);
                } else {
                    start_camera(active, meetings, app, ui.ctx());
                }
            }
            if theme::pill(
                ui,
                if active.source == Source::Screen {
                    "Change screen"
                } else {
                    "Share screen"
                },
                pal::BLUE,
            )
            .clicked()
            {
                meetings.screen_picker = Some(screens::Picker::new(app, ui.ctx()));
            }
        });
        if active.source == Source::Screen && theme::pill(ui, "Stop sharing", pal::PEACH).clicked()
        {
            stop_video(active);
        }
        if theme::pill(
            ui,
            if state.host {
                "End meeting"
            } else {
                "Leave meeting"
            },
            pal::RED,
        )
        .clicked()
        {
            if state.host {
                meetings.confirm_end = true;
            } else {
                *leave = true;
            }
        }
    });
    if active.mic_job.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label("Starting microphone…");
            if theme::pill(ui, "Cancel microphone", pal::PEACH).clicked() {
                if let Some(job) = &active.mic_job {
                    job.cancelled
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                #[cfg(target_os = "android")]
                {
                    let _ = slipstream_platform::android::action(15);
                }
            }
        });
    }
    if active.video_job.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label("Preparing sharing…");
            if theme::pill(ui, "Cancel sharing", pal::PEACH).clicked() {
                if let Some(job) = &active.video_job {
                    job.cancelled
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                #[cfg(target_os = "android")]
                {
                    let _ = slipstream_platform::android::action(12);
                }
            }
        });
    }
    ui.label(
        RichText::new(if active.microphone.is_some() {
            format!("Microphone on · {}", active.microphone_name)
        } else {
            format!(
                "Microphone muted · {}",
                meetings
                    .devices
                    .microphone_name(meetings.microphone_device.as_deref())
            )
        })
        .color(if active.microphone.is_some() {
            pal::GREEN
        } else {
            pal::GOLD
        }),
    );
    if active.source != Source::None {
        ui.label(format!("Sharing: {}", active.video_name));
    }
    if active.sound_job.is_some() {
        ui.label("Preparing speakers…");
    } else if active.sound.is_none() && theme::pill(ui, "Retry speakers", pal::BLUE).clicked() {
        active.sound_attempted = false;
    }
}
fn participants(
    meetings: &mut Meetings,
    active: &mut Active,
    state: &State,
    app: &mut App,
    ui: &mut Ui,
) {
    if state.host {
        theme::panel(ui, "Invite and admission", pal::LAVENDER, |ui| {
            if theme::pill(ui, "Copy invitation", pal::LAVENDER).clicked() {
                match active.meeting.invitation.encode() {
                    Ok(invite) => {
                        ui.ctx().copy_text(invite);
                        app.toast("Meeting invitation copied", pal::GREEN);
                    }
                    Err(error) => meetings.error = Some(format!("{error:#}")),
                }
            }
            call_form(
                meetings,
                app,
                ui,
                !state.locked && state.participants.len() < state.capacity,
            );
            let mut allow = !state.locked;
            if ui.checkbox(&mut allow,"Allow new join requests").on_hover_text("Closing admission blocks new arrivals. People already connected stay in the meeting, and you can still admit people already waiting.").changed(){active.meeting.command(Command::Lock(!allow));}
            ui.label(if state.locked {
                "Admission is closed. New people cannot join; current participants stay connected."
            } else {
                "Admission is open. Invitation guests wait here for your approval."
            });
        });
        ui.add_space(10.0);
        if !state.lobby.is_empty() {
            theme::panel(ui, "Waiting to join", pal::GOLD, |ui| {
                for member in &state.lobby {
                    ui.push_id(member.id, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&member.name).color(pal::TAN));
                            if theme::pill(ui, "Admit", pal::GREEN).clicked() {
                                active.meeting.command(Command::Admit(member.id));
                            }
                            if theme::pill(ui, "Decline", pal::RED).clicked() {
                                active.meeting.command(Command::Reject(member.id));
                            }
                        });
                    });
                }
            });
            ui.add_space(10.0);
        }
    }
    theme::panel(ui, "Participants", pal::BLUE, |ui| {
        if active.pin.is_some() && theme::pill(ui, "Unpin video", pal::BLUE).clicked() {
            active.pin = None;
        }
        for member in &state.participants {
            ui.push_id(member.id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}{}{}",
                            member.name,
                            if member.id == state.local {
                                " (you)"
                            } else {
                                ""
                            },
                            if member.id == 1 { " · organizer" } else { "" }
                        ))
                        .color(pal::TAN),
                    );
                    ui.label(if member.microphone {
                        "Microphone on"
                    } else {
                        "Muted"
                    });
                    ui.label(match member.source {
                        Source::None => "Camera off",
                        Source::Camera => "Camera on",
                        Source::Screen => "Sharing a screen",
                    });
                    if member.source != Source::None
                        && member.id != state.local
                        && theme::pill(
                            ui,
                            if active.pin == Some(member.id) {
                                "Pinned"
                            } else {
                                "Pin video"
                            },
                            pal::BLUE,
                        )
                        .clicked()
                    {
                        active.pin = Some(member.id);
                        active.tab = RoomTab::Live;
                    }
                    if state.host
                        && member.id != state.local
                        && theme::pill(ui, "Remove", pal::PEACH).clicked()
                    {
                        meetings.confirm_remove = Some((member.id, member.name.clone()));
                    }
                });
                ui.separator();
            });
        }
    });
}
fn chat(meetings: &mut Meetings, active: &mut Active, ui: &mut Ui) {
    theme::panel(ui, "Subspace chat", pal::VIOLET, |ui| {
        ui.label("Messages are shared with everyone in this meeting.");
        egui::ScrollArea::vertical()
            .id_salt("meeting-chat")
            .max_height(body_height(ui, 180.0))
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if active.chat.is_empty() {
                    ui.label(
                        RichText::new("No messages yet. Say hello to the meeting.").color(pal::DIM),
                    );
                }
                for line in &active.chat {
                    ui.label(RichText::new(&line.name).color(pal::LAVENDER));
                    ui.label(&line.text);
                    ui.add_space(8.0);
                }
            });
        ui.add_space(8.0);
        let response = ui.add(
            egui::TextEdit::multiline(&mut active.message)
                .desired_rows(2)
                .desired_width(ui.available_width())
                .hint_text("Message everyone · Enter to send · Shift+Enter for a new line")
                .return_key(egui::KeyboardShortcut::new(
                    egui::Modifiers::SHIFT,
                    egui::Key::Enter,
                )),
        );
        let enter = response.has_focus()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let can_send = !active.message.trim().is_empty();
        let mut send = false;
        ui.add_enabled_ui(can_send, |ui| {
            send = theme::pill(ui, "Send", pal::BLUE).clicked();
        });
        if can_send && (send || enter) {
            if active.message.len() <= 4000 {
                active
                    .meeting
                    .command(Command::Chat(std::mem::take(&mut active.message)));
            } else {
                meetings.error = Some("Keep messages under 4000 bytes".into());
            }
        }
    });
}

fn body_height(ui: &Ui, reserved: f32) -> f32 {
    (ui.ctx().content_rect().bottom() - ui.cursor().top() - reserved).clamp(60.0, 720.0)
}
