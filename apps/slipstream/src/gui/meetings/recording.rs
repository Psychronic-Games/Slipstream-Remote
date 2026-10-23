//! Recording a meeting: the choices (size, frame rate, quality, file
//! format), "Save as", then feeding the recorder the large picture, the mix
//! and our own voice until Stop (or leaving the meeting) finishes the file.
use super::{Active, Pending, pending, ready, room};
use crate::gui::{App, theme, theme::pal};
use eframe::egui::{self, RichText, Ui};
use slipstream_core::{config::RecordingPrefs, meeting::State};
use slipstream_session::meetings::record::{Container, Recorder, Settings};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct Recording {
    pub recorder: Option<Recorder>,
    /// Choosing settings (the dialog is open).
    pub choosing: Option<RecordingPrefs>,
    /// Waiting for "Save as" (the path, if one was chosen) …
    saving: Option<Pending<Option<PathBuf>>>,
    /// … to record with these settings.
    prefs: RecordingPrefs,
    /// Finishing the file after Stop.
    finishing: Option<Pending<PathBuf>>,
}

/// Bitrate for a size, frame rate and quality: 3, 5 or 8 Mbit/s at 720p30.
fn settings(p: RecordingPrefs) -> Settings {
    let height = p.height.clamp(360, 2160);
    let width = (height * 16 / 9) & !1;
    let fps = p.fps.clamp(10, 60);
    let base = [3000.0, 5000.0, 8000.0][p.quality.min(2) as usize];
    let kbps = base * (width * height) as f64 / (1280.0 * 720.0) * fps as f64 / 30.0;
    Settings { width, height, fps, kbps: (kbps as u32).clamp(1000, 40_000), container: if p.mkv { Container::Mkv } else { Container::Mp4 } }
}

/// A file name from the meeting's title and the time.
fn suggested_name(title: &str, mkv: bool) -> String {
    let title: String = title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    let now = chrono::Local::now().format("%Y-%m-%d %H%M");
    format!("{} {now}.{}", title.trim(), if mkv { "mkv" } else { "mp4" })
}

/// The Record / Stop button for the meeting's header.
pub(super) fn button(active: &mut Active, app: &App, ui: &mut Ui) {
    let r = &mut active.recording;
    if let Some(recorder) = &r.recorder {
        let secs = recorder.started.elapsed().as_secs();
        let label = format!("Stop · {:02}:{:02}", secs / 60, secs % 60);
        if theme::console_button(ui, &label, pal::RED, theme::ConsoleIcon::Record, false)
            .on_hover_text(format!("Recording to {}", recorder.path.display()))
            .clicked()
        {
            stop(active, app, ui.ctx());
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    } else if r.finishing.is_some() || r.saving.is_some() {
        ui.add_enabled_ui(false, |ui| theme::console_button(ui, "Record", pal::PEACH, theme::ConsoleIcon::Record, false));
    } else if theme::console_button(ui, "Record", pal::PEACH, theme::ConsoleIcon::Record, false)
        .on_hover_text("Record the meeting to a video file")
        .clicked()
    {
        r.choosing = Some(app.client.settings().app.meeting_recording);
    }
}

pub(super) fn stop(active: &mut Active, app: &App, ctx: &egui::Context) {
    let Some(recorder) = active.recording.recorder.take() else { return };
    active.meeting.publisher.tap_audio(None);
    if let Some(sound) = &active.sound {
        sound.tap(None);
    }
    // Finishing writes the last second and flushes: off the UI thread.
    active.recording.finishing = Some(pending(&app.rt, ctx, async move { tokio::task::spawn_blocking(move || recorder.finish()).await? }));
}

/// The settings dialog, "Save as", starting, and feeding a running recorder.
pub(super) fn poll(active: &mut Active, state: &State, app: &mut App, ctx: &egui::Context) {
    if let Some(mut prefs) = active.recording.choosing.take() {
        let (mut open, mut go) = (true, false);
        theme::modal("meeting-recording", pal::RED).show(ctx, |ui| {
            ui.set_width(theme::dialog_width(ctx, 460.0));
            if theme::window_header_with_close(ui, "Record meeting", pal::RED, pal::PEACH).close {
                open = false;
            }
            ui.label("Records the large picture (the speaker, a pinned or spotlighted person, or a shared screen) and everyone's voices.");
            ui.add_space(6.0);
            let row = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui)| {
                ui.horizontal_wrapped(|ui| {
                    ui.add_sized([110.0, 24.0], egui::Label::new(RichText::new(label).color(pal::TAN)));
                    add(ui);
                });
            };
            row(ui, "Size", &mut |ui| {
                for (h, name) in [(720, "720p"), (1080, "1080p"), (1440, "1440p")] {
                    ui.selectable_value(&mut prefs.height, h, name);
                }
            });
            row(ui, "Frame rate", &mut |ui| {
                for fps in [15, 30, 60] {
                    ui.selectable_value(&mut prefs.fps, fps, format!("{fps} fps"));
                }
            });
            row(ui, "Quality", &mut |ui| {
                for (q, name) in [(0, "Standard"), (1, "High"), (2, "Maximum")] {
                    ui.selectable_value(&mut prefs.quality, q, name);
                }
            });
            row(ui, "File format", &mut |ui| {
                ui.selectable_value(&mut prefs.mkv, false, "MP4");
                ui.selectable_value(&mut prefs.mkv, true, "MKV");
            });
            row(ui, "Codecs", &mut |ui| {
                ui.label(RichText::new("H.264 video · Opus sound").color(pal::DIM));
            });
            let s = settings(prefs);
            let per_hour = (s.kbps as f64 + 96.0) * 3600.0 / 8.0 / 1024.0 / 1024.0;
            ui.label(
                RichText::new(format!("{}×{} at {} fps, about {:.1} GB an hour.", s.width, s.height, s.fps, per_hour))
                    .small()
                    .color(pal::DIM),
            );
            ui.label(
                RichText::new(if prefs.mkv {
                    "MKV plays in VLC, mpv and Windows' Media Player."
                } else {
                    "MP4 plays in VLC, Windows' Media Player, web browsers and mpv; on a Mac use VLC or IINA (QuickTime can't play Opus sound)."
                })
                .small()
                .color(pal::DIM),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                go = theme::pill(ui, "Choose where to save…", pal::RED).clicked();
                if theme::pill(ui, "Cancel", pal::BLUE).clicked() {
                    open = false;
                }
            });
        });
        if go {
            app.client.update_settings(|s| s.app.meeting_recording = prefs);
            let name = suggested_name(&state.title, prefs.mkv);
            let (label, extension) = if prefs.mkv { ("Matroska video", "mkv") } else { ("MP4 video", "mp4") };
            // Automated UI checks have no desktop portal: they name the file.
            let preset = std::env::var_os("SLIPSTREAM_TEST_RECORD_TO").map(PathBuf::from);
            let job = pending(&app.rt, ctx, async move {
                if let Some(path) = preset {
                    return Ok(Some(path));
                }
                let file = rfd::AsyncFileDialog::new().set_title("Save the meeting recording").set_file_name(&name).add_filter(label, &[extension]).save_file().await;
                Ok(file.map(|f| f.path().to_path_buf()))
            });
            active.recording.saving = Some(job);
            active.recording.prefs = prefs;
        } else if open {
            active.recording.choosing = Some(prefs);
        }
    }
    if let Some(result) = ready(&mut active.recording.saving) {
        start(active, app, result);
    }
    if let Some(result) = ready(&mut active.recording.finishing) {
        match result {
            Ok(path) => app.toast(format!("Recording saved: {}", path.display()), pal::GREEN),
            Err(e) => app.toast(format!("The recording couldn't be finished: {e:#}"), pal::RED),
        }
    }
    if let Some(recorder) = &active.recording.recorder {
        if recorder.failed() {
            app.toast("Recording stopped: the file couldn't be written (disk full or removed?)", pal::RED);
            stop(active, app, ctx);
            return;
        }
        recorder.picture(room::main_picture(active, state));
    }
}

fn start(active: &mut Active, app: &mut App, chosen: anyhow::Result<Option<PathBuf>>) {
    let mut path = match chosen {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(e) => return app.toast(format!("Can't record: {e:#}"), pal::RED),
    };
    let s = settings(active.recording.prefs);
    if path.extension().is_none() {
        path.set_extension(s.container.extension());
    }
    match Recorder::start(&path, s) {
        Ok(recorder) => {
            active.meeting.publisher.tap_audio(Some(recorder.voice_input()));
            if let Some(sound) = &active.sound {
                sound.tap(Some(recorder.speakers_input()));
            }
            app.toast(format!("Recording to {}", path.display()), pal::RED);
            active.recording.recorder = Some(recorder);
        }
        Err(e) => app.toast(format!("Can't record: {e:#}"), pal::RED),
    }
}
