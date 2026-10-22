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
    microphone_device: String,
    camera_index: u32,
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
pub(super) fn ui(app: &mut App, ui: &mut Ui) {
    let mut meetings = std::mem::take(&mut app.meetings);
    let ctx = ui.ctx().clone();
    #[cfg(target_os = "android")]
    if !meetings.notifications_requested {
        meetings.notifications_requested = true;
        let _ = slipstream_platform::android::action(16);
    }
    if let Some(error) = &meetings.error {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(error).color(pal::GOLD));
        });
        if theme::pill(ui, "Dismiss", pal::BLUE).clicked() {
            meetings.error = None;
        }
    }
    if let Some(call) = &meetings.outgoing {
        let text = match &*call.state.borrow() {
            calls::Progress::Calling => "Calling…".into(),
            calls::Progress::Ringing => "Ringing · waiting for an answer".into(),
            calls::Progress::Answered => "Call answered · joining your meeting".into(),
            calls::Progress::Ended(reason) => reason.clone(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(text).color(pal::BLUE));
            if theme::pill(ui, "Close call", pal::PEACH).clicked() {
                meetings.outgoing = None;
            }
        });
    }
    if meetings.joining.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Preparing meeting…");
            if theme::pill(ui, "Cancel", pal::PEACH).clicked() {
                meetings.joining = None;
                meetings.target = None;
            }
        });
    } else if meetings.active.is_none() {
        theme::panel(ui, "Subspace meetings", pal::GREEN, |ui| {
            ui.label("Talk, share a screen, or show your camera. The organizer approves people joining by invitation.");
            ui.label("Your name");
            ui.text_edit_singleline(&mut meetings.name);
            ui.add_space(8.0);
            ui.label("Meeting title");
            ui.text_edit_singleline(&mut meetings.title);
            ui.horizontal(|ui| {
                ui.label("Participant limit");
                ui.add(egui::DragValue::new(&mut meetings.capacity).range(2..=32));
            });
            if theme::pill(ui, "Create meeting", pal::GREEN).clicked() {
                meetings.host(&app.client, &app.rt, &ctx);
            }
            ui.add_space(12.0);
            ui.label("Meeting invitation");
            ui.add(
                egui::TextEdit::multiline(&mut meetings.join_text)
                    .desired_rows(2)
                    .desired_width(ui.available_width())
                    .hint_text("Paste a Slipstream meeting invitation"),
            );
            if theme::pill(ui, "Join meeting", pal::BLUE).clicked() {
                match Invitation::parse(&meetings.join_text) {
                    Ok(invite) => meetings.join(invite, &app.client, &app.rt, &ctx),
                    Err(e) => meetings.error = Some(format!("{e:#}")),
                }
            }
            ui.add_space(10.0);
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
        call_form(&mut meetings, app, ui);
    }
    if let Some(mut active) = meetings.active.take() {
        let state = active.state();
        let mut leave = false;
        theme::panel(ui, &state.title, pal::GREEN, |ui| {
            if let Some(reason) = &state.ended {
                ui.label(RichText::new(reason).color(pal::GOLD));
                if theme::pill(ui, "Close meeting", pal::PEACH).clicked() {
                    leave = true;
                }
                return;
            }
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
            if state.waiting {
                ui.label(RichText::new("Waiting for the organizer to admit you…").color(pal::GOLD));
                if theme::pill(ui, "Cancel", pal::PEACH).clicked() {
                    leave = true;
                }
                return;
            }
            ui.horizontal_wrapped(|ui| {
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
                        active.microphone = None;
                        active.announce();
                        #[cfg(target_os = "android")]
                        {
                            let _ = slipstream_platform::android::action(15);
                        }
                    } else if active.mic_job.is_none() {
                        if let Some(sound) = &active.sound {
                            let player = sound.player.clone();
                            let publisher = active.meeting.publisher.clone();
                            let device = (!meetings.microphone_device.trim().is_empty())
                                .then(|| meetings.microphone_device.trim().to_owned());
                            let cancelled: std::sync::Arc<std::sync::atomic::AtomicBool> =
                                Default::default();
                            let worker_cancel = cancelled.clone();
                            let mut job = pending(&app.rt, &ctx, async move {
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
                                            if worker_cancel
                                                .load(std::sync::atomic::Ordering::Acquire)
                                            {
                                                anyhow::bail!("Microphone cancelled");
                                            }
                                            if start.elapsed() > Duration::from_secs(60) {
                                                let _ = slipstream_platform::android::action(15);
                                                anyhow::bail!(
                                                    "Microphone permission did not arrive in time"
                                                );
                                            }
                                            std::thread::sleep(Duration::from_millis(50));
                                        }
                                    }
                                    anyhow::ensure!(
                                        !worker_cancel.load(std::sync::atomic::Ordering::Acquire),
                                        "Microphone cancelled"
                                    );
                                    let microphone =
                                        Microphone::start(player, device, move |packet| {
                                            publisher.audio(packet.into())
                                        })?;
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
                        } else {
                            meetings.error =
                                Some("Wait for the speakers to be ready before unmuting".into());
                        }
                    }
                }
                if active.mic_job.is_some() {
                    ui.spinner();
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
                }
                let camera_label = if cfg!(target_os = "android") {
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
                if theme::pill(ui, camera_label, pal::VIOLET).clicked() {
                    if active.source == Source::Camera {
                        active.sender = None;
                        active.source = Source::None;
                        active.announce();
                    } else {
                        start_video(
                            &mut active,
                            Source::Camera,
                            meetings.camera_index,
                            &app.client,
                            &app.rt,
                            &ctx,
                        );
                    }
                }
                if theme::pill(
                    ui,
                    if active.source == Source::Screen {
                        "Stop screen share"
                    } else {
                        "Share screen"
                    },
                    pal::BLUE,
                )
                .clicked()
                {
                    if active.source == Source::Screen {
                        active.sender = None;
                        active.source = Source::None;
                        active.announce();
                    } else {
                        start_video(&mut active, Source::Screen, 0, &app.client, &app.rt, &ctx);
                    }
                }
                if active.video_job.is_some() {
                    ui.spinner();
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
                }
                if state.host && theme::pill(ui, "Copy invitation", pal::LAVENDER).clicked() {
                    match active.meeting.invitation.encode() {
                        Ok(invite) => ui.ctx().copy_text(invite),
                        Err(e) => meetings.error = Some(format!("{e:#}")),
                    }
                }
                if state.host
                    && theme::pill(
                        ui,
                        if state.locked {
                            "Unlock meeting"
                        } else {
                            "Lock meeting"
                        },
                        pal::GOLD,
                    )
                    .clicked()
                {
                    active.meeting.command(Command::Lock(!state.locked));
                }
                if theme::pill(
                    ui,
                    if state.host { "End meeting" } else { "Leave" },
                    pal::RED,
                )
                .clicked()
                {
                    leave = true;
                }
            });
            if active.microphone.is_none() {
                ui.label(RichText::new("Your microphone is muted").color(pal::GOLD));
            }
            ui.label(RichText::new("Support sound sharing pauses during meeting playback to prevent feedback.").small().color(pal::DIM));
            if active.sound.is_none()
                && active.sound_job.is_none()
                && theme::pill(ui, "Retry speakers", pal::BLUE).clicked()
            {
                active.sound_attempted = false;
            }
            ui.collapsing("Camera and microphone", |ui| {
                ui.label("Microphone device (empty uses the default)");
                ui.text_edit_singleline(&mut meetings.microphone_device);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Camera");
                    ui.add(egui::DragValue::new(&mut meetings.camera_index).range(0..=15));
                    #[cfg(target_os = "android")]
                    {
                        ui.label("0 Rear · 1 Front");
                        if theme::pill(ui, "Switch camera", pal::BLUE).clicked() {
                            meetings.camera_index = 1 - meetings.camera_index.min(1);
                            start_video(
                                &mut active,
                                Source::Camera,
                                meetings.camera_index,
                                &app.client,
                                &app.rt,
                                &ctx,
                            );
                        }
                        if theme::pill(ui, "Torch", pal::GOLD).clicked() {
                            let _ = slipstream_platform::android::action(13);
                        }
                    }
                });
            });
        });
        if !state.waiting && state.ended.is_none() {
            ui.add_space(8.0);
            room_video(&mut active, &state, ui);
            if state.host {
                call_form(&mut meetings, app, ui);
            }
            theme::panel(ui, "Participants", pal::BLUE, |ui| {
                for member in &state.participants {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{}{} · {}",
                            member.name,
                            if member.id == state.local {
                                " (you)"
                            } else {
                                ""
                            },
                            if member.microphone { "mic on" } else { "muted" }
                        ));
                        if member.source != Source::None
                            && member.id != state.local
                            && theme::pill(ui, "Pin video", pal::BLUE).clicked()
                        {
                            active.pin = Some(member.id);
                        }
                        if state.host
                            && member.id != state.local
                            && theme::pill(ui, "Remove", pal::PEACH).clicked()
                        {
                            active.meeting.command(Command::Remove(member.id));
                        }
                    });
                }
                for member in &state.lobby {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{} · waiting", member.name));
                        if theme::pill(ui, "Admit", pal::GREEN).clicked() {
                            active.meeting.command(Command::Admit(member.id));
                        }
                        if theme::pill(ui, "Decline", pal::RED).clicked() {
                            active.meeting.command(Command::Reject(member.id));
                        }
                    });
                }
            });
            theme::panel(ui, "Subspace chat", pal::VIOLET, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("meeting-chat")
                    .max_height(220.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &active.chat {
                            ui.label(RichText::new(&line.name).color(pal::LAVENDER));
                            ui.label(&line.text);
                            ui.add_space(5.0);
                        }
                    });
                ui.add(
                    egui::TextEdit::multiline(&mut active.message)
                        .desired_rows(2)
                        .desired_width(ui.available_width())
                        .hint_text("Message everyone in this meeting"),
                );
                if theme::pill(ui, "Send", pal::BLUE).clicked() && !active.message.trim().is_empty()
                {
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
        if leave {
            active.meeting.command(Command::Leave);
            meetings.outgoing = None;
            meetings.target = None;
            video::retain_streams(&app.render_state, |id| id < STREAM_BASE);
        } else {
            meetings.active = Some(active);
        }
    }
    app.meetings = meetings;
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
            client.capture_for_meeting().await?
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
fn call_form(meetings: &mut Meetings, app: &App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Call a system");
        ui.add(
            egui::TextEdit::singleline(&mut meetings.call_id)
                .hint_text("9-digit ID")
                .desired_width(150.0),
        );
        if theme::pill(ui, "Meeting call", pal::GREEN).clicked() {
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
