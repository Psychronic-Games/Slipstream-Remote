//! Session tabs and cross-viewport dragging. Screen targets use physical pixels
//! so windows on monitors with different scale factors can share a drop target.
use std::{sync::{Arc, Mutex}, collections::BTreeMap};

use eframe::egui::{self, Align2, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, vec2};

use super::{SessionAction, remote::TabAction, theme::{self, pal}};

#[derive(Default)]
pub(super) struct Tabs {
    targets: BTreeMap<egui::ViewportId, Target>,
    drag: Option<Drag>,
    pointer_source: Option<egui::ViewportId>,
    focus_order: BTreeMap<egui::ViewportId,u64>,
    focused: Option<egui::ViewportId>,
    focus_serial: u64,
}

struct Target {
    rect: Rect,
    slots: Vec<(u64, Rect)>,
}

struct Drag {
    id: u64,
    source: egui::ViewportId,
    label: String,
    screen_pos: Option<Pos2>,
}

// winit stops supplying a position (and may omit button-up) after CursorLeft.
// Sample only during a tab drag, so release outside a source window is reliable.
#[cfg(all(not(test), target_os = "linux"))]
fn native_pointer() -> Option<(Pos2, bool)> {
    use x11rb::{connection::Connection, protocol::xproto::{ConnectionExt, KeyButMask}};
    thread_local! {
        static X: Option<(x11rb::rust_connection::RustConnection, u32)> = x11rb::connect(None).ok()
            .map(|(conn, screen)| {let root = conn.setup().roots[screen].root; (conn, root)});
    }
    X.with(|x| {
        let (conn, root) = x.as_ref()?;
        let pointer = conn.query_pointer(*root).ok()?.reply().ok()?;
        pointer.same_screen.then_some((egui::pos2(f32::from(pointer.root_x), f32::from(pointer.root_y)),
            pointer.mask.contains(KeyButMask::BUTTON1)))
    })
}

#[cfg(all(not(test), windows))]
fn native_pointer() -> Option<(Pos2, bool)> {
    use windows_sys::Win32::{Foundation::POINT, UI::{Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON}, WindowsAndMessaging::GetCursorPos}};
    let mut point = POINT {x: 0, y: 0};
    // Both APIs report this desktop's pointer; no capture or global hook is installed.
    unsafe {
        (GetCursorPos(&mut point) != 0).then(|| (egui::pos2(point.x as f32, point.y as f32), GetAsyncKeyState(i32::from(VK_LBUTTON)) < 0))
    }
}

#[cfg(any(test, not(any(target_os = "linux", windows))))]
fn native_pointer() -> Option<(Pos2, bool)> { None }

impl Tabs {
    pub fn dragging(&self) -> bool { self.drag.is_some() }
    pub fn hide_target(&mut self) { self.targets.remove(&egui::ViewportId::ROOT); }
    pub fn retain_targets(&mut self, live: &[egui::ViewportId]) {self.targets.retain(|id,_|live.contains(id));self.focus_order.retain(|id,_|live.contains(id));}
    pub fn cancel(&mut self, id: u64) {
        if self.drag.as_ref().is_some_and(|drag| drag.id == id) {
            self.drag = None;
            self.pointer_source = None;
        }
    }

    pub fn update_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if raw.focused && self.focused!=Some(raw.viewport_id) {
            self.focused=Some(raw.viewport_id);self.focus_serial+=1;self.focus_order.insert(raw.viewport_id,self.focus_serial);
        }
        if raw.viewport().minimized==Some(true) {self.targets.remove(&raw.viewport_id);}
        if self.pointer_source != Some(raw.viewport_id) {return;}
        if let Some((point, pressed)) = native_pointer() && let Some(rect) = raw.viewport().inner_rect {
            let scale = raw.viewport().native_pixels_per_point.unwrap_or(1.0) * ctx.zoom_factor();
            let pos = point / scale - rect.min.to_vec2();
            raw.events.retain(|event| !matches!(event, egui::Event::PointerGone | egui::Event::PointerMoved(_)
                | egui::Event::PointerButton {button: egui::PointerButton::Primary, ..}));
            raw.events.push(egui::Event::PointerMoved(pos));
            if !pressed {
                self.pointer_source = None;
                raw.events.push(egui::Event::PointerButton {pos, button: egui::PointerButton::Primary, pressed: false, modifiers: ctx.input(|i| i.modifiers)});
            }
        }
    }

    fn destination(&self, id: u64, pos: Pos2) -> Option<(egui::ViewportId, Option<u64>)> {
        let (viewport,target)=self.targets.iter().filter(|(_,target)|target.rect.contains(pos))
            .max_by_key(|(viewport,_)|self.focus_order.get(viewport).copied().unwrap_or(0))?;
        Some((*viewport,target.slots.iter().find(|(other,rect)|*other!=id && pos.x<rect.center().x).map(|(id,_)|*id)))
    }
}

/// Returns content below the strip and actions to apply after releasing UI locks.
pub(super) fn show(
    shared: &Arc<Mutex<Tabs>>, ui: &mut Ui, area: Rect,
    entries: &[(u64, String)], active: Option<u64>, root: bool, visible: bool,
) -> (Rect, Vec<SessionAction>) {
    let ctx = ui.ctx().clone();
    let viewport = ctx.viewport_id();
    let scale = ctx.pixels_per_point();
    let origin = ctx.input(|i| i.viewport().inner_rect.map(|r| r.min));
    let screen_rect = |r: Rect| origin.map(|p| r.translate(p.to_vec2()) * scale);
    let bar = area.with_max_y(area.top() + 37.0);
    ui.painter().rect_filled(bar, 5.0, pal::PANEL);
    let mut actions = Vec::new();
    let mut child = ui.new_child(UiBuilder::new().max_rect(bar.shrink2(vec2(4.0, 2.0))));
    // A slim scroll rail leaves the tab height intact when many links overflow.
    child.spacing_mut().scroll.bar_width=4.0;
    child.spacing_mut().scroll.bar_inner_margin=0.0;
    child.spacing_mut().scroll.bar_outer_margin=0.0;
    let mut slots = Vec::new();
    let mut local_slots = Vec::new();
    let mut started = None;
    egui::ScrollArea::horizontal().id_salt(("session-tabs", root)).auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if root && theme::pill(ui, "Bridge", if active.is_none() {pal::PEACH} else {pal::BLUE}).clicked() {
                    actions.push((0, TabAction::Bridge));
                }
                for (id, name) in entries {
                    let (rect, _) = ui.allocate_exact_size(vec2(180.0, 29.0), Sense::hover());
                    let close = rect.with_min_x(rect.right() - 27.0);
                    let body = rect.with_max_x(close.left());
                    let response = ui.interact(body, ui.scope_id().with(("session-tab", id)), Sense::click_and_drag())
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text(format!("{name}\nDrag to reorder or move between tabs and windows"));
                    let color = if active == Some(*id) {pal::LAVENDER} else if response.hovered() {pal::BLUE} else {pal::VIOLET};
                    ui.painter().rect_filled(rect, 6.0, color);
                    ui.painter().with_clip_rect(body.shrink(6.0)).text(body.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER,
                        name.to_uppercase(), theme::display(17.0), pal::BG);
                    if response.clicked() { actions.push((*id, TabAction::Activate)); }
                    response.context_menu(|ui| {
                        if !ui.ctx().embed_viewports() && ui.button(if root {"Open in separate window"} else {"Dock in main console"}).clicked() {
                            actions.push((*id, if root {TabAction::Detach} else {TabAction::Dock}));
                            ui.close();
                        }
                        if ui.button("End link").clicked() {
                            actions.push((*id, TabAction::Close));
                            ui.close();
                        }
                    });
                    if response.drag_started() {
                        started = Some(Drag { id: *id, source: viewport, label: name.clone(), screen_pos: None });
                    }
                    let end = ui.interact(close, ui.scope_id().with(("close-session-tab", id)), Sense::click()).on_hover_text("End this link");
                    ui.painter().text(close.center(), Align2::CENTER_CENTER, "×", theme::body(21.0), if end.hovered() {pal::RED} else {pal::BG});
                    if end.clicked() { actions.push((*id, TabAction::Close)); }
                    local_slots.push((*id, rect.intersect(ui.clip_rect())));
                    if let Some(rect) = screen_rect(rect.intersect(ui.clip_rect())) && rect.is_positive() { slots.push((*id, rect)); }
                }
                // The next tab belongs beside the existing tabs, including
                // directly after Bridge when the strip has no sessions.
                let (plus, add) = ui.allocate_exact_size(vec2(33.0, 29.0), Sense::click());
                let add = add.on_hover_text("New link · recent and saved systems");
                ui.painter().rect_filled(plus, 9.0, if add.hovered() {pal::TAN} else {pal::GREEN});
                for offset in [vec2(7.0,0.0), vec2(0.0,7.0)] {
                    ui.painter().line_segment([plus.center()-offset,plus.center()+offset],Stroke::new(2.0,pal::BG));
                }
                add.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button,true,"New link"));
                if add.clicked() { actions.push((0,TabAction::NewLink(viewport))); }
            });
        });
    let mut state = shared.lock().unwrap();
    if visible && let Some(rect)=screen_rect(bar) {state.targets.insert(viewport,Target {rect,slots});}
    else {state.targets.remove(&viewport);}
    if let Some(drag) = started {
        actions.push((drag.id, TabAction::Activate));
        state.pointer_source = Some(drag.source);
        state.drag = Some(drag);
    }
    // Some compositors deliver release to the destination instead of the source,
    // and Wayland deliberately withholds global window positions.
    if visible && state.drag.as_ref().is_some_and(|d| d.source != viewport)
        && let Some(pos) = ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary)
            .then(|| i.pointer.latest_pos()).flatten())
        && bar.contains(pos)
    {
        let drag = state.drag.take().unwrap();
        let before = local_slots.iter().find(|(id, rect)| *id != drag.id && rect.is_positive() && pos.x < rect.center().x).map(|(id, _)| *id);
        actions.push((drag.id, TabAction::PlaceTab(viewport,before)));
    }
    if state.drag.as_ref().is_some_and(|d| d.source == viewport) {
        let (pos, released, down, escape) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.button_released(egui::PointerButton::Primary),
            i.pointer.primary_down(), i.key_pressed(egui::Key::Escape)));
        if let Some(drag) = &mut state.drag {
            drag.screen_pos = pos.and_then(|p| origin.map(|o| (o + p.to_vec2()) * scale));
        }
        if escape || (!down && !released) {
            state.drag = None;
        } else if released {
            let drag = state.drag.take().unwrap();
            if let Some(point) = drag.screen_pos {
                if let Some((destination,before)) = state.destination(drag.id, point) {
                    actions.push((drag.id, TabAction::PlaceTab(destination,before)));
                } else if !ctx.embed_viewports() {
                    // Native window positions use the source viewport's point scale.
                    actions.push((drag.id, TabAction::FloatAt(point / scale - vec2(90.0, 52.0))));
                }
            } else if !ctx.embed_viewports() && pos.is_some_and(|p| !bar.contains(p)) {
                // Compositors that withhold screen positions still allow tear-out.
                actions.push((drag.id, TabAction::Detach));
            }
        } else if let Some(pos) = pos {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let drag = state.drag.as_ref().unwrap();
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("session-tab-drag")));
            let badge = Rect::from_min_size(pos + vec2(12.0, 14.0), vec2(210.0, 30.0));
            p.rect_filled(badge, 6.0, pal::LAVENDER);
            p.with_clip_rect(badge.shrink(5.0)).text(badge.center(), Align2::CENTER_CENTER, &drag.label, theme::display(17.0), pal::BG);
            for viewport in state.targets.keys() {ctx.request_repaint_of(*viewport);}
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }
    if let Some(drag) = &state.drag
        && let Some(point) = drag.screen_pos
        && let Some((destination,before)) = state.destination(drag.id, point)
        && destination==viewport
    {
        ui.painter().rect_stroke(bar.shrink(1.0), 5.0, Stroke::new(2.0, pal::GREEN), egui::StrokeKind::Inside);
        if let Some(target) = state.targets.get(&viewport) {
            let x = before.and_then(|id| target.slots.iter().find(|(other, _)| *other == id).map(|(_, r)| r.left()))
                .or_else(|| target.slots.last().map(|(_, r)| r.right()));
            if let (Some(x), Some(origin)) = (x, origin) {
                let x = x / scale - origin.x;
                ui.painter().line_segment([egui::pos2(x, bar.top()), egui::pos2(x, bar.bottom())], Stroke::new(3.0, pal::GREEN));
            }
        }
    }
    if state.drag.is_none() {state.pointer_source=None;}
    (area.with_min_y(bar.bottom() + 3.0), actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput, ViewportId, ViewportInfo, pos2};

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE}
    }

    struct Fixture {
        ctx: egui::Context,
        tabs: Arc<Mutex<Tabs>>,
    }

    impl Fixture {
        fn new() -> Self {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(false);
            theme::install(&ctx);
            Self {ctx, tabs: Default::default()}
        }

        fn frame(&self, root: bool, origin: Pos2, scale: f32, events: Vec<Event>) -> Vec<SessionAction> {
            let viewport = if root {ViewportId::ROOT} else {ViewportId::from_hash_of("floating-test")};
            let entries = if root {vec![(1, "First".into()), (2, "Second".into())]} else {vec![(3, "Third".into())]};
            self.frame_in(viewport,root,origin,scale,events,&entries)
        }
        fn frame_in(&self, viewport:ViewportId, root:bool, origin:Pos2, scale:f32, events:Vec<Event>, entries:&[(u64,String)])->Vec<SessionAction> {
            let area = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
            let mut raw = RawInput {viewport_id: viewport, screen_rect: Some(area), events, ..Default::default()};
            raw.viewports.insert(viewport, ViewportInfo {inner_rect: Some(area.translate(origin.to_vec2())), native_pixels_per_point: Some(scale), ..Default::default()});
            let mut actions = Vec::new();
            let mut output = self.ctx.run_ui(raw, |ui| {
                actions.extend(show(&self.tabs, ui, area, entries, entries.first().map(|(id,_)|*id), root, true).1);
            });
            output.textures_delta.clear();
            actions
        }
    }

    #[test]
    fn pointer_drag_reorders_tears_out_and_escape_cancels() {
        let f = Fixture::new();
        let origin = pos2(100.0, 100.0);
        for _ in 0..2 {f.frame(true, origin, 1.0, vec![]);}
        let start = f.tabs.lock().unwrap().targets.get(&ViewportId::ROOT).unwrap().slots[0].1.center() - origin.to_vec2();
        f.frame(true, origin, 1.0, vec![Event::PointerMoved(start), button(start, true)]);
        f.frame(true, origin, 1.0, vec![Event::PointerMoved(pos2(700.0, start.y))]);
        let actions = f.frame(true, origin, 1.0, vec![button(pos2(700.0, start.y), false)]);
        assert!(actions.iter().any(|(id, a)| *id == 1 && matches!(a, TabAction::PlaceTab(ViewportId::ROOT,None))));

        f.frame(true, origin, 1.0, vec![Event::PointerMoved(start), button(start, true)]);
        f.frame(true, origin, 1.0, vec![Event::PointerMoved(pos2(900.0, 300.0))]);
        let actions = f.frame(true, origin, 1.0, vec![button(pos2(900.0, 300.0), false)]);
        assert!(actions.iter().any(|(id, a)| *id == 1 && matches!(a, TabAction::FloatAt(_))));
        assert!(!f.tabs.lock().unwrap().dragging());

        f.frame(true, origin, 1.0, vec![Event::PointerMoved(start), button(start, true)]);
        f.frame(true, origin, 1.0, vec![Event::PointerMoved(pos2(900.0, 300.0))]);
        f.frame(true, origin, 1.0, vec![Event::Key {key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE}]);
        let actions = f.frame(true, origin, 1.0, vec![button(pos2(900.0, 300.0), false)]);
        assert!(!actions.iter().any(|(_, a)| matches!(a, TabAction::FloatAt(_) | TabAction::PlaceTab(_,_))));
    }

    #[test]
    fn floating_tab_docks_across_different_viewport_scales() {
        let f = Fixture::new();
        for _ in 0..2 {f.frame(true, pos2(100.0, 100.0), 2.0, vec![]);}
        let destination = f.tabs.lock().unwrap().targets.get(&ViewportId::ROOT).unwrap().slots[0].1.left_center() + vec2(4.0, 0.0);
        let origin = pos2(1000.0, 400.0);
        for _ in 0..2 {f.frame(false, origin, 1.0, vec![]);}
        let start = pos2(70.0, 15.0);
        f.frame(false, origin, 1.0, vec![Event::PointerMoved(start), button(start, true)]);
        let drop = destination - origin.to_vec2();
        f.frame(false, origin, 1.0, vec![Event::PointerMoved(drop)]);
        let actions = f.frame(false, origin, 1.0, vec![button(drop, false)]);
        assert!(actions.iter().any(|(id, a)| *id == 3 && matches!(a, TabAction::PlaceTab(ViewportId::ROOT,Some(1)))));
    }

    #[test]
    fn destination_can_receive_release_instead_of_the_drag_source() {
        let f = Fixture::new();
        let origin = pos2(100.0, 100.0);
        for _ in 0..2 {f.frame(true, origin, 1.0, vec![]);}
        let drop = f.tabs.lock().unwrap().targets.get(&ViewportId::ROOT).unwrap().slots[0].1.left_center() - origin.to_vec2() + vec2(3.0, 0.0);
        for _ in 0..2 {f.frame(false, pos2(1000.0, 400.0), 1.0, vec![]);}
        let start = pos2(70.0, 15.0);
        f.frame(false, pos2(1000.0, 400.0), 1.0, vec![Event::PointerMoved(start), button(start, true)]);
        f.frame(false, pos2(1000.0, 400.0), 1.0, vec![Event::PointerMoved(start + vec2(20.0, 0.0))]);
        let actions = f.frame(true, origin, 1.0, vec![Event::PointerMoved(drop), button(drop, false)]);
        assert!(actions.iter().any(|(id, a)| *id == 3 && matches!(a, TabAction::PlaceTab(ViewportId::ROOT,Some(1)))));
        assert!(!f.tabs.lock().unwrap().dragging());
    }

    #[test]
    fn floating_groups_receive_tabs_and_support_further_tear_out() {
        let f=Fixture::new();
        let first=ViewportId::from_hash_of("first-group");let second=ViewportId::from_hash_of("second-group");
        let origin=pos2(100.0,100.0);let destination=pos2(1100.0,100.0);
        for _ in 0..2 {
            f.frame_in(first,false,origin,1.0,vec![],&[(3,"Third".into())]);
            f.frame_in(second,false,destination,1.5,vec![],&[(4,"Fourth".into())]);
        }
        let drop=f.tabs.lock().unwrap().targets[&second].slots[0].1.left_center()+vec2(4.0,0.0)-origin.to_vec2();
        let start=pos2(60.0,15.0);
        f.frame_in(first,false,origin,1.0,vec![Event::PointerMoved(start),button(start,true)],&[(3,"Third".into())]);
        f.frame_in(first,false,origin,1.0,vec![Event::PointerMoved(drop)],&[(3,"Third".into())]);
        let actions=f.frame_in(first,false,origin,1.0,vec![button(drop,false)],&[(3,"Third".into())]);
        assert!(actions.iter().any(|(id,a)|*id==3 && matches!(a,TabAction::PlaceTab(v,Some(4)) if *v==second)));
        f.frame_in(first,false,origin,1.0,vec![Event::PointerMoved(start),button(start,true)],&[(3,"Third".into())]);
        f.frame_in(first,false,origin,1.0,vec![Event::PointerMoved(pos2(850.0,350.0))],&[(3,"Third".into())]);
        let actions=f.frame_in(first,false,origin,1.0,vec![button(pos2(850.0,350.0),false)],&[(3,"Third".into())]);
        assert!(actions.iter().any(|(id,a)|*id==3 && matches!(a,TabAction::FloatAt(_))));
        f.tabs.lock().unwrap().retain_targets(&[second]);
        assert_eq!(f.tabs.lock().unwrap().targets.len(),1);
    }

    #[test]
    fn overlapping_groups_choose_the_most_recently_focused_visible_window() {
        let mut tabs=Tabs::default();let first=ViewportId::from_hash_of("front");let second=ViewportId::from_hash_of("back");
        let area=Rect::from_min_size(Pos2::ZERO,vec2(500.0,30.0));
        for id in [first,second] {tabs.targets.insert(id,Target{rect:area,slots:vec![]});}
        tabs.focus_order.insert(first,2);tabs.focus_order.insert(second,1);
        assert_eq!(tabs.destination(3,pos2(100.0,15.0)),Some((first,None)));
        tabs.focus_order.insert(second,3);
        assert_eq!(tabs.destination(3,pos2(100.0,15.0)),Some((second,None)));
    }

    #[test]
    fn new_link_follows_tabs_and_bridge_when_empty() {
        for root in [true, false] {
            for count in [0, 1, 2] {
                let ctx = egui::Context::default();
                ctx.set_embed_viewports(false);
                theme::install(&ctx);
                let shared = Default::default();
                let viewport=if root {ViewportId::ROOT} else {ViewportId::from_hash_of("plus-group")};
                let area = Rect::from_min_size(Pos2::ZERO, vec2(800.0,360.0));
                let entries: Vec<_> = (1..=count).map(|id| (id,format!("Workstation {id}"))).collect();
                let mut point = Pos2::ZERO;
                let mut actions = Vec::new();
                for frame in 0..4 {
                    let events = match frame {
                        2 => vec![Event::PointerMoved(point),button(point,true)],
                        3 => vec![button(point,false)],
                        _ => vec![],
                    };
                    let mut raw=RawInput {viewport_id:viewport,screen_rect:Some(area),events,..Default::default()};
                    raw.viewports.insert(viewport,ViewportInfo {inner_rect:Some(area),..Default::default()});
                    let mut output = ctx.run_ui(raw, |ui| {
                        actions.extend(show(&shared,ui,area,&entries,Some(1),root,true).1);
                        // Root slots locate the last tab without depending on font widths.
                        point = if let Some((_,last))=shared.lock().unwrap().targets.get(&ViewportId::ROOT).and_then(|t|t.slots.last()) {
                            pos2(last.right()+20.0,15.0)
                        } else {
                            let bridge = if root {ui.painter().layout_no_wrap("BRIDGE".into(),theme::display(17.0),pal::BG).size().x+32.0+4.0} else {0.0};
                            pos2(4.0+bridge+count as f32*184.0+16.0,15.0)
                        };
                    });
                    output.textures_delta.clear();
                }
                assert_eq!(actions.iter().filter(|(_,a)|matches!(a,TabAction::NewLink(v) if *v==viewport)).count(),1,"root={root}, tabs={count}");
                assert!(!shared.lock().unwrap().dragging());
            }
        }
    }
}
