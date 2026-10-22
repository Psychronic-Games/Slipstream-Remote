use super::*;
use slipstream_core::proto::DisplayInfo;

pub(super) struct Picker {
    job: Option<Pending<Vec<DisplayInfo>>>,
    choices: Vec<DisplayInfo>,
    selected: Option<u32>,
    error: Option<String>,
}
impl Picker {
    pub fn new(app: &App, ctx: &egui::Context) -> Self {
        let client = app.client.clone();
        Self {
            job: Some(pending(&app.rt, ctx, async move {
                client.meeting_displays().await
            })),
            choices: Vec::new(),
            selected: None,
            error: None,
        }
    }
    pub fn poll(&mut self) {
        if let Some(result) = ready(&mut self.job) {
            match result {
                Ok(choices) => {
                    self.selected = if choices.len() == 1 {
                        Some(choices[0].index)
                    } else {
                        None
                    };
                    self.choices = choices;
                }
                Err(error) => self.error = Some(format!("{error:#}")),
            }
        }
    }
}
pub(super) fn label(display: &DisplayInfo) -> String {
    format!(
        "Screen {} · {} · {} × {}{}",
        display.index + 1,
        display.name,
        display.width,
        display.height,
        if display.primary {
            " · main display"
        } else {
            ""
        }
    )
}
pub(super) fn ui(meetings: &mut Meetings, app: &App, ctx: &egui::Context) {
    let Some(picker) = &mut meetings.screen_picker else {
        return;
    };
    let mut close = false;
    let mut chosen = None;
    theme::modal("meeting-screen-picker", pal::BLUE).show(ctx, |ui| {
        ui.set_width(theme::dialog_width(ctx, 540.0));
        if theme::window_header_with_close(ui, "Choose a screen", pal::BLUE, pal::PEACH).close {
            close = true;
        }
        ui.label("Only the screen you choose will be shared with this meeting.");
        if picker.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Finding screens… approve your desktop's sharing prompt if shown.");
            });
        }
        if let Some(error) = &picker.error {
            ui.label(RichText::new(error).color(pal::GOLD));
        }
        if picker.job.is_none() && picker.choices.is_empty() && picker.error.is_none() {
            ui.label("No screens are available. Refresh the list to try again.");
        }
        arrangement(picker, ui);
        egui::ScrollArea::vertical()
            .id_salt("meeting-screen-choices")
            .max_height((ctx.content_rect().height() - 380.0).clamp(80.0, 300.0))
            .show(ui, |ui| {
                for display in &picker.choices {
                    if ui
                        .add(
                            egui::Button::new(label(display))
                                .selected(picker.selected == Some(display.index))
                                .wrap()
                                .min_size(egui::vec2(ui.available_width(), 54.0)),
                        )
                        .on_hover_text(format!("Desktop position: {}, {}", display.x, display.y))
                        .clicked()
                    {
                        picker.selected = Some(display.index);
                    }
                }
            });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(picker.selected.is_some(), |ui| {
                if theme::pill(ui, "Share selected screen", pal::GREEN).clicked() {
                    chosen = picker
                        .choices
                        .iter()
                        .find(|d| Some(d.index) == picker.selected)
                        .cloned();
                }
            });
            if theme::pill(ui, "Cancel", pal::PEACH).clicked() {
                close = true;
            }
            if picker.job.is_none() && theme::pill(ui, "Refresh screens", pal::BLUE).clicked() {
                *picker = Picker::new(app, ctx);
            }
        });
    });
    if let Some(display) = chosen {
        if let Some(active) = &mut meetings.active {
            active.next_video_name = label(&display);
            start_video(
                active,
                Source::Screen,
                display.index,
                &app.client,
                &app.rt,
                ctx,
            );
            active.tab = RoomTab::Live;
        }
        close = true;
    }
    if close {
        meetings.screen_picker = None;
    }
}

fn arrangement(picker: &mut Picker, ui: &mut Ui) {
    if picker.choices.len() < 2 {
        return;
    }
    let left = picker
        .choices
        .iter()
        .map(|d| d.x as f32)
        .fold(f32::INFINITY, f32::min);
    let top = picker
        .choices
        .iter()
        .map(|d| d.y as f32)
        .fold(f32::INFINITY, f32::min);
    let right = picker
        .choices
        .iter()
        .map(|d| d.x as f32 + d.width as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = picker
        .choices
        .iter()
        .map(|d| d.y as f32 + d.height as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 132.0),
        egui::Sense::hover(),
    );
    let scale =
        ((area.width() - 20.0) / (right - left).max(1.0)).min(112.0 / (bottom - top).max(1.0));
    let origin = area.center() - egui::vec2((right - left) * scale, (bottom - top) * scale) * 0.5;
    for display in &picker.choices {
        let start = origin
            + egui::vec2(
                (display.x as f32 - left) * scale,
                (display.y as f32 - top) * scale,
            );
        let rect = egui::Rect::from_min_size(
            start,
            egui::vec2(display.width as f32 * scale, display.height as f32 * scale),
        )
        .shrink(3.0);
        let selected = picker.selected == Some(display.index);
        let response = ui.interact(
            rect,
            ui.scope_id().with(("screen-map", display.index)),
            egui::Sense::click(),
        );
        ui.painter()
            .rect_filled(rect, 5.0, if selected { pal::BLUE } else { pal::PANEL });
        ui.painter().rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(
                if selected || response.hovered() {
                    2.0
                } else {
                    1.0
                },
                pal::BLUE,
            ),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            (display.index + 1).to_string(),
            theme::display(24.0),
            if selected {
                egui::Color32::BLACK
            } else {
                pal::TAN
            },
        );
        if response.on_hover_text(label(display)).clicked() {
            picker.selected = Some(display.index);
        }
    }
}
