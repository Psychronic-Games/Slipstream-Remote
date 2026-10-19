//! X11/Xwayland keeps ordinary pointer motion grabbed by the drag source.
//! winit delivers file events without coordinates, so sample the native
//! pointer while a drop is active instead of reusing a stale egui position.
use eframe::egui;
use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

pub(super) fn update_pointer(ctx: &egui::Context, raw: &mut egui::RawInput) {
    if raw.hovered_files.is_empty() && raw.dropped_files.is_empty()
    {
        return;
    }
    thread_local! {
        static X: Option<(x11rb::rust_connection::RustConnection, u32)> = x11rb::connect(None).ok()
            .map(|(conn, screen)| { let root = conn.setup().roots[screen].root; (conn, root) });
    }
    let position = X.with(|x| {
        let (conn, root) = x.as_ref()?;
        let pointer = conn.query_pointer(*root).ok()?.reply().ok()?;
        if !pointer.same_screen {
            return None;
        }
        let rect = raw.viewports.get(&raw.viewport_id)?.inner_rect?;
        let global = egui::pos2(f32::from(pointer.root_x), f32::from(pointer.root_y))
            / (raw.viewport().native_pixels_per_point.unwrap_or(1.0)*ctx.zoom_factor());
        rect.contains(global).then_some(global - rect.min.to_vec2())
    });
    raw.events.push(
        position
            .map(egui::Event::PointerMoved)
            .unwrap_or(egui::Event::PointerGone),
    );
    if !raw.hovered_files.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}
