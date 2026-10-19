//! Native session windows share their existing connection with the Links hub.
use std::sync::{Arc, Mutex, mpsc};

use eframe::egui::{self, UiBuilder};

use super::{
    remote::{RemoteTab, TabAction},
    theme,
};

pub(super) fn show(
    shared: &Arc<Mutex<RemoteTab>>,
    ctx: &egui::Context,
    rt: &Arc<tokio::runtime::Runtime>,
    actions: &mpsc::Sender<(u64, TabAction)>,
    time: f64,
    app_title: &str,
) {
    let (id, host) = {
        let tab = shared.lock().unwrap();
        if !tab.detached {
            return;
        }
        (tab.id, tab.info.host_name.clone())
    };
    let (shared, rt, actions, app_title) = (
        shared.clone(),
        rt.clone(),
        actions.clone(),
        app_title.to_owned(),
    );
    let registered = std::time::Instant::now();
    let builder = egui::ViewportBuilder::default()
        .with_title(format!("{host} · {app_title}"))
        .with_app_id("slipstream-remote")
        .with_decorations(false)
        .with_transparent(theme::transparent_window())
        .with_icon(Arc::new(theme::icon(64)))
        .with_inner_size([1180.0, 780.0])
        .with_min_inner_size([520.0, 360.0]);
    ctx.show_viewport_deferred(RemoteTab::window_id(id), builder, move |ui, class| {
        let ctx = ui.ctx().clone();
        let desktop = class == egui::ViewportClass::Deferred;
        let mut tab = shared.lock().unwrap();
        if !tab.detached {
            theme::dismiss_tool_viewport(&ctx, desktop);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            tab.end_link();
            let _ = actions.send((id, TabAction::Close));
            theme::dismiss_tool_viewport(&ctx, desktop);
            return;
        }
        let body = theme::native_session_title_bar(ui, &app_title, Some(&tab.info.host_name));
        let ui = &mut ui.new_child(UiBuilder::new().max_rect(body));
        let action = tab.ui(ui, body, time + registered.elapsed().as_secs_f64(), &rt);
        if matches!(action, TabAction::Close | TabAction::Dock) {
            tab.release_keyboard();
            tab.leave_fullscreen(&ctx);
            tab.detached = false;
            theme::dismiss_tool_viewport(&ctx, desktop);
        }
        if !matches!(action, TabAction::None) {
            let _ = actions.send((id, action));
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(
            if ctx.input(|i| i.focused) { 16 } else { 100 },
        ));
    });
}
