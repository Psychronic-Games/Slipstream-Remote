use super::*;
use slipstream_audio::devices::MicrophoneDevice;

#[derive(Default)]
pub(super) struct Devices {
    pub microphones: Vec<MicrophoneDevice>,
    pub cameras: Vec<(u32, String)>,
    pub microphone_error: Option<String>,
    pub camera_error: Option<String>,
    microphone_job: Option<Pending<Vec<MicrophoneDevice>>>,
    camera_job: Option<Pending<Vec<(u32, String)>>>,
    loaded: bool,
}
impl Devices {
    pub fn poll(&mut self) {
        if let Some(result) = ready(&mut self.microphone_job) {
            match result {
                Ok(devices) => {
                    self.microphones = devices;
                    self.microphone_error = None;
                }
                Err(error) => self.microphone_error = Some(format!("{error:#}")),
            }
        }
        if let Some(result) = ready(&mut self.camera_job) {
            match result {
                Ok(devices) => {
                    self.cameras = devices;
                    self.camera_error = None;
                }
                Err(error) => self.camera_error = Some(format!("{error:#}")),
            }
        }
    }
    pub fn load(&mut self, app: &App, ctx: &egui::Context) {
        if !self.loaded {
            self.refresh(app, ctx);
        }
    }
    pub fn refresh(&mut self, app: &App, ctx: &egui::Context) {
        self.loaded = true;
        if self.microphone_job.is_none() {
            self.microphone_job = Some(pending(&app.rt, ctx, async {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::task::spawn_blocking(slipstream_audio::devices::microphones),
                )
                .await
                .map_err(|_| anyhow::anyhow!("Microphone discovery timed out"))??
            }));
        }
        if self.camera_job.is_none() {
            let client = app.client.clone();
            self.camera_job = Some(pending(&app.rt, ctx, async move {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::task::spawn_blocking(move || client.meeting_cameras()),
                )
                .await
                .map_err(|_| anyhow::anyhow!("Camera discovery timed out"))??
            }));
        }
    }
    pub fn busy(&self) -> bool {
        self.microphone_job.is_some() || self.camera_job.is_some()
    }
    pub fn microphone_name(&self, selected: Option<&str>) -> String {
        match selected {
            Some(id) => self
                .microphones
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "Saved microphone unavailable".into()),
            None => self
                .microphones
                .iter()
                .find(|d| d.is_default)
                .map(|d| format!("System default · {}", d.name))
                .unwrap_or_else(|| "System default microphone".into()),
        }
    }
    pub fn camera(&self, selected: Option<u32>) -> Option<u32> {
        match selected {
            Some(id) if self.cameras.iter().any(|(index, _)| *index == id) => Some(id),
            Some(_) if self.cameras.len() != 1 => None,
            _ => self.cameras.first().map(|(id, _)| *id),
        }
    }
    pub fn camera_name(&self, selected: Option<u32>) -> String {
        self.camera(selected)
            .and_then(|id| self.cameras.iter().find(|(index, _)| *index == id))
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| {
                if self.camera_job.is_some() {
                    "Finding cameras…".into()
                } else {
                    "No camera available".into()
                }
            })
    }
}

/// Choices apply immediately to an active source; selecting a device while
/// muted/off changes only the next explicit start. Discovery never captures.
pub(super) fn ui(meetings: &mut Meetings, mut active: Option<&mut Active>, app: &App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    meetings.devices.load(app, &ctx);
    theme::panel(ui, "Your devices", pal::BLUE, |ui| {
        ui.label("Choose your microphone and camera. Your choices are remembered on this device.");
        if meetings.devices.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Finding devices…");
            });
        }
        if theme::pill(ui, "Refresh devices", pal::BLUE).clicked() {
            meetings.devices.refresh(app, &ctx);
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Microphone").color(pal::TAN));
        let previous = meetings.microphone_device.clone();
        let busy = active.as_ref().is_some_and(|a| a.mic_job.is_some());
        ui.add_enabled_ui(!busy, |ui| {
            egui::ComboBox::from_id_salt("meeting-microphone")
                .width(ui.available_width().min(480.0))
                .wrap_mode(egui::TextWrapMode::Truncate)
                .selected_text(
                    meetings
                        .devices
                        .microphone_name(meetings.microphone_device.as_deref()),
                )
                .show_ui(ui, |ui| {
                    ui.set_max_width(480.0_f32.min(ui.ctx().content_rect().width() - 40.0));
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.selectable_value(
                        &mut meetings.microphone_device,
                        None,
                        meetings.devices.microphone_name(None),
                    );
                    for device in &meetings.devices.microphones {
                        let label = format!(
                            "{}{}",
                            device.name,
                            if device.is_default { " · default" } else { "" }
                        );
                        ui.selectable_value(
                            &mut meetings.microphone_device,
                            Some(device.id.clone()),
                            label,
                        );
                    }
                });
        });
        if let Some(error) = &meetings.devices.microphone_error {
            ui.label(RichText::new(error).color(pal::GOLD));
        } else if !meetings.devices.busy() && meetings.devices.microphones.is_empty() {
            ui.label("No microphone was found. Connect one and refresh devices.");
        }
        if previous != meetings.microphone_device {
            app.client
                .update_settings(|s| s.app.meeting_microphone = meetings.microphone_device.clone());
            if let Some(a) = active.as_deref_mut()
                && a.microphone.is_some()
            {
                stop_microphone(a);
                start_microphone(a, meetings, app, &ctx);
            }
        }
        if let Some(a) = active.as_ref() {
            ui.label(if a.microphone.is_some() {
                format!("Using: {}", a.microphone_name)
            } else {
                "Muted · unmute to use this microphone".into()
            });
        } else {
            ui.label("You will join with your microphone muted.");
        }
        ui.add_space(14.0);
        ui.label(RichText::new("Camera").color(pal::TAN));
        let previous = meetings.camera_index;
        let busy = active.as_ref().is_some_and(|a| a.video_job.is_some());
        ui.add_enabled_ui(!busy && !meetings.devices.cameras.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("meeting-camera")
                .width(ui.available_width().min(480.0))
                .wrap_mode(egui::TextWrapMode::Truncate)
                .selected_text(meetings.devices.camera_name(meetings.camera_index))
                .show_ui(ui, |ui| {
                    for (id, name) in &meetings.devices.cameras {
                        ui.selectable_value(&mut meetings.camera_index, Some(*id), name);
                    }
                });
        });
        if let Some(error) = &meetings.devices.camera_error {
            ui.label(RichText::new(error).color(pal::GOLD));
        } else if !meetings.devices.busy() && meetings.devices.cameras.is_empty() {
            ui.label("No camera was found. Connect one and refresh devices.");
        }
        if previous != meetings.camera_index {
            app.client
                .update_settings(|s| s.app.meeting_camera = meetings.camera_index);
            if let Some(a) = active.as_deref_mut()
                && a.source == Source::Camera
            {
                start_camera(a, meetings, app, &ctx);
            }
        }
        ui.label("Your camera stays off until you turn it on in the meeting.");
        if let Some(a) = active.as_ref()
            && a.source == Source::Camera
        {
            ui.label(format!("Sharing: {}", a.video_name));
        }
        #[cfg(target_os = "android")]
        if let Some(a) = active.as_ref()
            && a.source == Source::Camera
            && theme::pill(ui, "Torch", pal::GOLD)
                .on_hover_text("Toggle the rear camera's light, when available")
                .clicked()
        {
            let _ = slipstream_platform::android::action(13);
        }
    });
}
