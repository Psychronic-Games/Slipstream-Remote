//! "Allow remote control?" for one-time-password links (see
//! `slipstream_session::approval`), with a plain warning about phone scams.
use super::{App, pal, theme};
use eframe::egui::{self, RichText};
use slipstream_session::approval::{LinkRequest, ReceiverGuard};

/// Shown beside the warning: the people scammers pretend to be.
pub const SCAM_WARNING: &str = "Only allow this if you asked someone you know for help. Banks, Microsoft, Apple, \
     internet providers and \"tech support\" never phone or message you out of the blue to connect to your computer. \
     If someone told you to install this app, decline and hang up.";

#[derive(Default)]
pub struct LinkApproval {
    /// This window can answer for the in-process host.
    receiver: Option<ReceiverGuard>,
    pub pending: Option<LinkRequest>,
    shown: Option<u64>,
}

pub(super) fn poll(app: &mut App, ctx: &egui::Context) {
    if !app.via_service {
        if app.link_approval.receiver.is_none() {
            app.link_approval.receiver = Some(app.client.approvals().receiver());
        }
        app.link_approval.pending = app.client.approvals().pending();
    }
    let Some(request) = app.link_approval.pending.clone() else {
        return;
    };
    let left = request.expires.saturating_sub(now());
    if left == 0 {
        return;
    }
    if app.link_approval.shown != Some(request.id) {
        app.link_approval.shown = Some(request.id);
        app.set_visible(ctx, true);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
    }
    let mut answer = None;
    theme::modal("link-approval", pal::RED).show(ctx, |ui| {
        ui.set_width(theme::dialog_width(ctx, 480.0));
        if theme::window_header_with_close(ui, "Allow remote control?", pal::RED, pal::PEACH).close {
            answer = Some(false);
        }
        ui.label(RichText::new(&request.peer).font(theme::display(24.0)).color(pal::TAN));
        ui.label(if request.public {
            "wants to see and control this computer through Slipstream Public."
        } else {
            "wants to see and control this computer."
        });
        ui.label(RichText::new("The name above is chosen by whoever is connecting.").small().color(pal::DIM));
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(pal::RED.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.5, pal::RED))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.label(RichText::new("⚠ BEWARE OF SCAMS").font(theme::display_bold(17.0)).color(pal::RED));
                ui.label(SCAM_WARNING);
            });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if theme::pill(ui, "Decline", pal::RED).clicked() {
                answer = Some(false);
            }
            if theme::pill(ui, "Allow", pal::GREEN).clicked() {
                answer = Some(true);
            }
            ui.label(RichText::new(format!("Declines by itself in {left} s")).small().color(pal::DIM));
        });
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    if let Some(accepted) = answer {
        answer_link(app, request.id, accepted);
        app.link_approval.pending = None;
    }
}

fn answer_link(app: &App, id: u64, accepted: bool) {
    #[cfg(windows)]
    if app.via_service {
        if let Some(bridge) = &app.service_chat {
            bridge.sender().answer_link(id, accepted);
        }
        return;
    }
    app.client.approvals().answer(id, accepted);
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
