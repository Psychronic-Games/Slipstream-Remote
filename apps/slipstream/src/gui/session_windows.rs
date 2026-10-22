//! Native tab groups retain their existing connections while moving windows.
use super::{
    remote::{RemoteTab, TabAction},
    session_tabs, theme,
};
use eframe::egui::{self, UiBuilder};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
};

pub(super) fn show(
    all: &[Arc<Mutex<RemoteTab>>],
    ctx: &egui::Context,
    rt: &Arc<tokio::runtime::Runtime>,
    actions: &mpsc::Sender<(u64, TabAction)>,
    tabs: &Arc<Mutex<session_tabs::Tabs>>,
    time: f64,
    app_title: &str,
) {
    let mut groups = BTreeMap::<egui::ViewportId, Vec<Arc<Mutex<RemoteTab>>>>::new();
    for shared in all {
        let tab = shared.lock().unwrap();
        if tab.detached {
            groups
                .entry(tab.viewport_id())
                .or_default()
                .push(shared.clone());
        }
    }
    for (viewport, members) in groups {
        let active = members
            .iter()
            .find(|tab| tab.lock().unwrap().window_active)
            .cloned()
            .unwrap_or_else(|| members[0].clone());
        active.lock().unwrap().window_active = true;
        let (host, position) = {
            let mut tab = active.lock().unwrap();
            (tab.info.host_name.clone(), tab.window_position.take())
        };
        let (rt, actions, tabs, app_title) = (
            rt.clone(),
            actions.clone(),
            tabs.clone(),
            app_title.to_owned(),
        );
        let registered = std::time::Instant::now();
        let mut builder = egui::ViewportBuilder::default()
            .with_title(format!("{host} · {app_title}"))
            .with_app_id("slipstream-remote")
            .with_decorations(false)
            .with_transparent(theme::transparent_window())
            .with_icon(Arc::new(theme::icon(64)))
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([520.0, 360.0]);
        if let Some(position) = position {
            builder = builder.with_position(position);
        }
        ctx.show_viewport_deferred(viewport, builder, move |ui, class| {
            let ctx = ui.ctx().clone();
            let desktop = class == egui::ViewportClass::Deferred;
            let members: Vec<_> = members
                .iter()
                .filter(|tab| {
                    let tab = tab.lock().unwrap();
                    tab.detached && tab.viewport_id() == viewport
                })
                .cloned()
                .collect();
            if members.is_empty() {
                theme::dismiss_tool_viewport(&ctx, desktop);
                return;
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                for tab in &members {
                    let mut tab = tab.lock().unwrap();
                    tab.end_link();
                    let _ = actions.send((tab.id, TabAction::Close));
                }
                theme::dismiss_tool_viewport(&ctx, desktop);
                return;
            }
            let active = members
                .iter()
                .find(|tab| tab.lock().unwrap().window_active)
                .cloned()
                .unwrap_or_else(|| members[0].clone());
            let entries: Vec<_> = members
                .iter()
                .map(|tab| {
                    let tab = tab.lock().unwrap();
                    (tab.id, tab.info.host_name.clone())
                })
                .collect();
            let (id, host) = {
                let tab = active.lock().unwrap();
                (tab.id, tab.info.host_name.clone())
            };
            let body = theme::native_session_title_bar(ui, &app_title, Some(&host));
            let visible = ctx.input(|i| i.viewport().minimized != Some(true));
            let (body, tab_actions) =
                session_tabs::show(&tabs, ui, body, &entries, Some(id), false, visible);
            for (id, action) in tab_actions {
                for tab in &members {
                    tab.lock().unwrap().release_keyboard();
                }
                let _ = actions.send((id, action));
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            let mut tab = active.lock().unwrap();
            let ui = &mut ui.new_child(UiBuilder::new().max_rect(body));
            let action = tab.ui(ui, body, time + registered.elapsed().as_secs_f64(), &rt);
            if !matches!(action, TabAction::None) {
                tab.release_keyboard();
                tab.leave_fullscreen(&ctx);
                let _ = actions.send((id, action));
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(
                if ctx.input(|i| i.focused) { 16 } else { 100 },
            ));
        });
    }
}
