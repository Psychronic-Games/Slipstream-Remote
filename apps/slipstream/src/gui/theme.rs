//! The Slipstream look: a starship-console aesthetic — flat colour blocks
//! and swept "elbow" frames on black, condensed type, plus a few modern
//! touches (neon glow, scan lines, live readouts).

use std::f32::consts::{FRAC_PI_2, PI};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Mesh, Pos2, Rect, Response,
    Sense, Shadow, Shape, Stroke, Ui, Vec2, epaint::PathShape, pos2, vec2,
};

pub mod pal {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(2, 3, 8);
    pub const PANEL: Color32 = Color32::from_rgb(8, 11, 20);
    pub const ORANGE: Color32 = Color32::from_rgb(255, 153, 0);
    pub const PEACH: Color32 = Color32::from_rgb(255, 170, 120);
    pub const TAN: Color32 = Color32::from_rgb(255, 204, 153);
    pub const GOLD: Color32 = Color32::from_rgb(255, 204, 102);
    pub const LAVENDER: Color32 = Color32::from_rgb(204, 153, 204);
    pub const VIOLET: Color32 = Color32::from_rgb(153, 119, 221);
    pub const PERIWINKLE: Color32 = Color32::from_rgb(153, 153, 255);
    pub const BLUE: Color32 = Color32::from_rgb(153, 204, 255);
    pub const CYAN: Color32 = Color32::from_rgb(80, 220, 255);
    pub const RED: Color32 = Color32::from_rgb(255, 85, 85);
    pub const GREEN: Color32 = Color32::from_rgb(102, 255, 170);
    pub const TEXT: Color32 = Color32::from_rgb(255, 214, 170);
    pub const DIM: Color32 = Color32::from_rgb(110, 120, 150);
    pub const GRID: Color32 = Color32::from_rgb(14, 30, 48);
}

pub const ANTONIO: &str = "antonio";
pub const ANTONIO_BOLD: &str = "antonio-bold";

pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ANTONIO.into()))
}

pub fn display_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ANTONIO_BOLD.into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Fonts + global visuals. Call once at startup.
// The bundled fonts (SIL OFL), also used by the compact Windows window.
pub const FONT_ANTONIO: &[u8] = include_bytes!("../../assets/fonts/Antonio-Regular.ttf");
pub const FONT_ANTONIO_BOLD: &[u8] = include_bytes!("../../assets/fonts/Antonio-Bold.ttf");
pub const FONT_BARLOW: &[u8] = include_bytes!("../../assets/fonts/BarlowSemiCondensed-Regular.ttf");
pub const FONT_BARLOW_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/BarlowSemiCondensed-SemiBold.ttf");
pub const FONT_MONO: &[u8] = include_bytes!("../../assets/fonts/ShareTechMono-Regular.ttf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    };
    add(&mut fonts, "Antonio-Regular", FONT_ANTONIO);
    add(&mut fonts, "Antonio-Bold", FONT_ANTONIO_BOLD);
    add(&mut fonts, "Barlow", FONT_BARLOW);
    add(&mut fonts, "Barlow-SemiBold", FONT_BARLOW_SEMIBOLD);
    add(&mut fonts, "ShareTechMono", FONT_MONO);

    // Keep egui's defaults as fallbacks for symbols and emoji.
    let fallback_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let with = |first: &str, rest: &[String]| {
        let mut v = vec![first.to_string()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with("Barlow", &fallback_prop));
    fonts.families.insert(FontFamily::Monospace, with("ShareTechMono", &fallback_mono));
    fonts.families.insert(FontFamily::Name(ANTONIO.into()), with("Antonio-Regular", &fallback_prop));
    fonts.families.insert(FontFamily::Name(ANTONIO_BOLD.into()), with("Antonio-Bold", &fallback_prop));
    ctx.set_fonts(fonts);

    // The console look is always dark. egui keeps separate dark and light
    // styles and follows the OS by default, so pin dark mode and style both:
    // otherwise a light-mode Windows got egui's stock sizes (12.5 px text).
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        use egui::TextStyle::*;
        style.text_styles = [
            (Heading, display_bold(26.0)),
            (Body, body(17.0)),
            (Button, display(17.0)),
            (Monospace, mono(16.0)),
            (Small, body(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(14.0, 6.0);
        style.spacing.interact_size.y = 30.0;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = pal::BG;
        v.window_fill = pal::PANEL;
        v.extreme_bg_color = Color32::from_rgb(4, 6, 12);
        v.faint_bg_color = pal::PANEL;
        v.override_text_color = Some(pal::TEXT);
        v.hyperlink_color = pal::CYAN;
        v.selection.bg_fill = pal::ORANGE.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, pal::ORANGE);
        v.text_cursor.stroke = Stroke::new(2.0, pal::ORANGE);
        v.window_stroke = Stroke::new(1.0, pal::VIOLET.gamma_multiply(0.6));
        v.window_corner_radius = CornerRadius::same(18);
        v.window_shadow = Shadow { offset: [0, 0], blur: 24, spread: 0, color: pal::VIOLET.gamma_multiply(0.25) };
        v.popup_shadow = v.window_shadow;
        let r = CornerRadius::same(14);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal::GRID);
        v.widgets.inactive.bg_fill = Color32::from_rgb(16, 20, 34);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(16, 20, 34);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, pal::VIOLET.gamma_multiply(0.5));
        v.widgets.hovered.bg_fill = Color32::from_rgb(28, 30, 52);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(28, 30, 52);
        v.widgets.hovered.bg_stroke = Stroke::new(1.5, pal::ORANGE);
        v.widgets.active.bg_fill = pal::ORANGE.gamma_multiply(0.6);
        v.widgets.active.bg_stroke = Stroke::new(2.0, pal::ORANGE);
        v.widgets.open.bg_fill = Color32::from_rgb(28, 30, 52);
        // Console scrollbars: a wide rounded bar like the frames' rails
        // (lavender, orange while hovered or dragged) on a dark track,
        // instead of egui's thin grey strip. The handle takes the widgets'
        // foreground colour, as do check marks and arrows.
        style.spacing.scroll = egui::style::ScrollStyle {
            bar_width: 14.0,
            handle_min_length: 40.0,
            bar_inner_margin: 6.0,
            bar_outer_margin: 2.0,
            foreground_color: true,
            dormant_handle_opacity: 1.0,
            active_handle_opacity: 1.0,
            interact_handle_opacity: 1.0,
            dormant_background_opacity: 1.0,
            active_background_opacity: 1.0,
            interact_background_opacity: 1.0,
            ..egui::style::ScrollStyle::solid()
        };
        v.widgets.inactive.fg_stroke = Stroke::new(1.5, pal::LAVENDER);
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, pal::ORANGE);
        v.widgets.active.fg_stroke = Stroke::new(2.0, pal::ORANGE);
    });
}

/// Soft neon halo around a rounded rect.
pub fn glow(painter: &egui::Painter, rect: Rect, radius: f32, color: Color32, strength: f32) {
    let shadow = Shadow {
        offset: [0, 0],
        blur: (18.0 * strength) as u8,
        spread: (2.0 * strength) as u8,
        color: color.gamma_multiply(0.35 * strength.min(1.5)),
    };
    painter.add(shadow.as_shape(rect, CornerRadius::same(radius.min(255.0) as u8)));
}

fn lighten(c: Color32, amt: f32) -> Color32 {
    let l = |x: u8| (x as f32 + (255.0 - x as f32) * amt) as u8;
    Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
}

/// Which corner an elbow turns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    BottomLeft,
}

/// The signature swept frame corner: a vertical arm of width `arm_w` meets a
/// horizontal arm of height `arm_h`, with a big outer radius and a concave
/// inner fillet. `rect` spans both arms.
pub fn elbow(painter: &egui::Painter, rect: Rect, arm_w: f32, arm_h: f32, corner: Corner, color: Color32) {
    let outer = (arm_w.min(rect.height()) * 0.9).min(arm_w);
    let inner = (arm_h * 0.9).max(10.0);
    let r_outer = outer.min(255.0) as u8;
    match corner {
        Corner::TopLeft => {
            // Corner block with the outer sweep.
            let block = Rect::from_min_max(rect.min, pos2(rect.min.x + arm_w, rect.min.y + arm_h.max(outer)));
            painter.rect_filled(block, CornerRadius { nw: r_outer, ..Default::default() }, color);
            // Vertical arm.
            painter.rect_filled(
                Rect::from_min_max(pos2(rect.min.x, block.max.y - 1.0), pos2(rect.min.x + arm_w, rect.max.y)),
                0.0,
                color,
            );
            // Horizontal arm.
            painter.rect_filled(
                Rect::from_min_max(pos2(rect.min.x + arm_w - 1.0, rect.min.y), pos2(rect.max.x, rect.min.y + arm_h)),
                0.0,
                color,
            );
            fillet(painter, pos2(rect.min.x + arm_w, rect.min.y + arm_h), inner, 1.0, 1.0, color);
        }
        Corner::BottomLeft => {
            let block = Rect::from_min_max(pos2(rect.min.x, rect.max.y - arm_h.max(outer)), pos2(rect.min.x + arm_w, rect.max.y));
            painter.rect_filled(block, CornerRadius { sw: r_outer, ..Default::default() }, color);
            painter.rect_filled(
                Rect::from_min_max(rect.min, pos2(rect.min.x + arm_w, block.min.y + 1.0)),
                0.0,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(rect.min.x + arm_w - 1.0, rect.max.y - arm_h), rect.max),
                0.0,
                color,
            );
            fillet(painter, pos2(rect.min.x + arm_w, rect.max.y - arm_h), inner, 1.0, -1.0, color);
        }
    }
}

/// Concave fillet in the inside corner at `at`, growing toward (+dx, +dy).
fn fillet(painter: &egui::Painter, at: Pos2, r: f32, dx: f32, dy: f32, color: Color32) {
    let center = pos2(at.x + r * dx, at.y + r * dy);
    let steps = 16;
    let arc: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32 * FRAC_PI_2;
            // From the point on the horizontal arm (t=0) to the vertical arm (t=π/2).
            pos2(center.x - r * dx * t.sin(), center.y - r * dy * t.cos())
        })
        .collect();
    let mut mesh = Mesh::default();
    mesh.colored_vertex(at, color);
    for p in &arc {
        mesh.colored_vertex(*p, color);
    }
    for i in 1..arc.len() as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(Shape::mesh(mesh));
    // Anti-aliased edge along the curve.
    painter.add(PathShape::line(arc, Stroke::new(1.0, color)));
}

/// A flat block button (sidebar style): label bottom-right, black text.
/// The active one is lit, outlined and points into the page; the rest are
/// slightly dimmed so the selection is unmistakable.
pub fn block_button(ui: &mut Ui, rect: Rect, label: &str, code: &str, color: Color32, active: bool) -> Response {
    block_button_pointing(ui, rect, label, code, color, active, false)
}

/// A [`block_button`] whose active arrow points up (`up`: a row of them
/// below the content, on a phone) instead of right.
pub fn block_button_pointing(ui: &mut Ui, rect: Rect, label: &str, code: &str, color: Color32, active: bool, up: bool) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with(label), Sense::click());
    let painter = ui.painter();
    let mut fill = if active { lighten(color, 0.15) } else { mix(color, Color32::BLACK, 0.28) };
    if resp.hovered() && !active {
        fill = color;
        glow(painter, rect, 2.0, color, 0.6);
    }
    if active {
        glow(painter, rect, 2.0, color, 1.4);
    }
    if resp.is_pointer_button_down_on() {
        fill = Color32::WHITE;
    }
    painter.rect_filled(rect, 2.0, fill);
    if active {
        painter.rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
        // Pointer into the content area.
        let (c, side) = if up { (rect.center().x, rect.top()) } else { (rect.center().y, rect.right()) };
        let at = |along: f32, out: f32| if up { pos2(along, side - out) } else { pos2(side + out, along) };
        let arrow = vec![at(c - 10.0, 2.0), at(c, 14.0), at(c + 10.0, 2.0)];
        painter.add(Shape::convex_polygon(arrow, lighten(color, 0.15), Stroke::NONE));
    }
    painter.text(
        rect.right_bottom() + vec2(-8.0, -4.0),
        Align2::RIGHT_BOTTOM,
        label.to_uppercase(),
        if active { display_bold(17.0) } else { display(17.0) },
        Color32::BLACK,
    );
    if !code.is_empty() {
        painter.text(rect.left_top() + vec2(8.0, 4.0), Align2::LEFT_TOP, code, mono(11.0), Color32::BLACK.gamma_multiply(0.6));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A console progress bar: a rounded dark track that fills with `color`.
/// The text reads black over the filled part and light over the rest, so
/// it stays legible whatever the fill colour and however far along.
pub fn progress(ui: &mut Ui, fraction: f32, width: f32, color: Color32, text: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(60.0), 22.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = rect.height() / 2.0;
        p.rect_filled(rect, r, Color32::from_rgb(16, 20, 34));
        p.rect_stroke(rect, r, Stroke::new(1.0, color.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        let split = rect.left() + rect.width() * fraction.clamp(0.0, 1.0);
        let filled = Rect::from_min_max(rect.min, pos2(split.max(rect.left() + 2.0 * r).min(rect.right()), rect.bottom()));
        if fraction > 0.0 {
            p.rect_filled(filled, r, color);
        }
        let font = body(14.0);
        let at = rect.center();
        let edge = if fraction > 0.0 { filled.right() } else { rect.left() };
        let light = p.with_clip_rect(Rect::from_min_max(pos2(edge, rect.top()), rect.max).intersect(p.clip_rect()));
        light.text(at, Align2::CENTER_CENTER, text, font.clone(), pal::TEXT);
        if fraction > 0.0 {
            let dark = p.with_clip_rect(filled.intersect(p.clip_rect()));
            dark.text(at, Align2::CENTER_CENTER, text, font, Color32::BLACK);
        }
    }
    resp
}

/// A pill button sized to its label.
pub fn pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    pill_sized(ui, label, color, vec2(0.0, 32.0))
}

pub fn pill_sized(ui: &mut Ui, label: &str, color: Color32, min: Vec2) -> Response {
    pill_with(ui, label, color, min, None)
}

/// A pill with the logo delta in front, pointing back (the way to the
/// Bridge).
pub fn back_pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    pill_with(ui, label, color, vec2(0.0, 32.0), Some(back_delta))
}

/// Draws the logo pointing left into `rect`, in `ink`, with the slash cut
/// out in `bg`.
fn back_delta(painter: &egui::Painter, rect: Rect, ink: Color32, bg: Color32) {
    // Logo space (pointing up) → pointing left: (x, y) ↦ (y, 1 − x).
    let at = |(x, y): (f32, f32)| pos2(rect.left() + y * rect.width(), rect.top() + (1.0 - x) * rect.height());
    for half in [&LOGO_LEFT, &LOGO_RIGHT] {
        painter.add(egui::Shape::convex_polygon(half.iter().copied().map(at).collect(), ink, Stroke::NONE));
    }
    let ((x0, y0), (x1, y1), half_width) = LOGO_SLASH;
    painter.line_segment([at((x0, y0)), at((x1, y1))], Stroke::new(half_width * 2.0 * rect.width(), bg));
}

type PillIcon = fn(&egui::Painter, Rect, Color32, Color32);

fn pill_with(ui: &mut Ui, label: &str, color: Color32, min: Vec2, icon: Option<PillIcon>) -> Response {
    let text = label.to_uppercase();
    let galley = ui.painter().layout_no_wrap(text, display(17.0), Color32::BLACK);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    // Padding follows the style's button padding (14 → 20 px a side).
    let pad = 12.0 + 2.0 * ui.spacing().button_padding.x;
    let size = vec2((galley.size().x + pad + icon_w).max(min.x), min.y.max(28.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let enabled = ui.is_enabled();
        let mut fill = if enabled { color } else { color.gamma_multiply(0.35) };
        if enabled && resp.hovered() {
            fill = lighten(color, 0.3);
            glow(painter, rect, rect.height() / 2.0, color, 1.0);
        }
        if enabled && resp.is_pointer_button_down_on() {
            fill = Color32::WHITE;
        }
        let r = (rect.height() / 2.0) as u8;
        painter.rect_filled(rect, CornerRadius::same(r), fill);
        // Black seams near both ends, like segmented console buttons —
        // only when the label clears them.
        let cap = rect.height() * 0.55;
        if rect.width() - 2.0 * cap > galley.size().x + icon_w + 12.0 {
            // With an icon in front, only the far seam (the icon takes the near one's place).
            let seams = if icon.is_some() { &[rect.right() - cap][..] } else { &[rect.left() + cap, rect.right() - cap][..] };
            for &x in seams {
                painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0, Color32::BLACK));
            }
        }
        let text_x = rect.center().x + icon_w / 2.0 - galley.size().x / 2.0;
        if let Some(draw) = icon {
            let s = rect.height() * 0.62;
            // Beside the label, or centred when there's none.
            let cx = if galley.size().x > 0.0 { text_x - 4.0 - s / 2.0 } else { rect.center().x };
            let r = Rect::from_center_size(pos2(cx, rect.center().y), vec2(s, s));
            draw(painter, r, Color32::BLACK, fill);
        }
        painter.galley(pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, Color32::BLACK);
    }
    if ui.is_enabled() { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// A panel header: rounded cap, title, and a rule running to the edge.
pub fn header(ui: &mut Ui, title: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let painter = ui.painter();
    let cap = Rect::from_min_size(rect.min, vec2(26.0, 26.0));
    painter.rect_filled(cap, CornerRadius { nw: 13, sw: 13, ..Default::default() }, color);
    let title_pos = pos2(cap.right() + 10.0, rect.center().y);
    let galley = painter.layout_no_wrap(title.to_uppercase(), display_bold(20.0), color);
    let tw = galley.size().x;
    painter.galley(pos2(title_pos.x, rect.center().y - galley.size().y / 2.0), galley, color);
    let bar = Rect::from_min_max(pos2(title_pos.x + tw + 12.0, rect.top() + 7.0), pos2(rect.right(), rect.bottom() - 7.0));
    if bar.width() > 8.0 {
        painter.rect_filled(bar, CornerRadius { ne: 6, se: 6, ..Default::default() }, color.gamma_multiply(0.85));
    }
}

// A floating window's console frame: a header band with the title and a
// Close segment, a rail down the left joined to it by an elbow, and a
// footer band joined by another (see `window_header`, `WindowChrome::footer`).
const RAIL: f32 = 20.0;
const RAIL_GAP: f32 = 14.0;
const BAND: f32 = 30.0;
const FOOT: f32 = 12.0;
const ELBOW: f32 = 12.0;

/// Height [`WindowChrome::footer`] takes: what a window's scrolling body
/// must leave free below it, or the window grows by that much every frame.
pub const FOOTER_SPACE: f32 = FOOT + 4.0 + 8.0 + 2.0;

/// A floating window in the console style: no grey title bar; start its
/// contents with [`window_header`] and end them with
/// [`WindowChrome::footer`]. It's dragged by its background.
pub fn window(id: &str, color: Color32) -> egui::Window<'static> {
    egui::Window::new(id.to_string()).title_bar(false).frame(
        egui::Frame::new()
            .fill(pal::PANEL)
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
            .corner_radius(CornerRadius::same(20))
            .inner_margin(egui::Margin { left: (12.0 + RAIL + RAIL_GAP) as i8, right: 14, top: 12, bottom: 12 })
            .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(170) }),
    )
}

/// Where a window's frame goes, from its header to its footer.
pub struct WindowChrome {
    /// The Close segment was pressed.
    pub close: bool,
    top: f32,
    left: f32,
    right: f32,
    color: Color32,
}

/// A [`window`]'s header band: the title on the window's colour, and a
/// Close segment at its end.
pub fn window_header(ui: &mut Ui, title: &str, color: Color32) -> WindowChrome {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAND), Sense::hover());
    let x0 = row.left() - RAIL - RAIL_GAP;
    // The frame lies in the window's margin, outside the content's clip.
    let p = ui.ctx().layer_painter(ui.layer_id());
    let close_w = 86.0;
    let close_rect = Rect::from_min_max(pos2(row.right() - close_w, row.top()), row.max);
    let band = Rect::from_min_max(pos2(x0, row.top()), pos2(close_rect.left() - 4.0, row.bottom()));
    p.rect_filled(band, CornerRadius { nw: 22, ..Default::default() }, color);
    p.text(pos2(row.left() + 2.0, row.center().y), egui::Align2::LEFT_CENTER, title.to_uppercase(), display_bold(21.0), pal::BG);
    let resp = ui.interact(close_rect, ui.scope_id().with(("window-close", title)), Sense::click());
    let fill = if resp.hovered() { pal::TAN } else { color.gamma_multiply(0.8) };
    p.rect_filled(close_rect, CornerRadius { ne: 15, se: 15, ..Default::default() }, fill);
    p.text(close_rect.center(), egui::Align2::CENTER_CENTER, "CLOSE", display(17.0), pal::BG);
    let close = resp.on_hover_text("Close").on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    ui.add_space(10.0);
    WindowChrome { close, top: row.top(), left: row.left(), right: row.right(), color }
}

impl WindowChrome {
    /// The footer band, and the rail and elbows joining it to the header.
    /// Call it last, below the window's contents.
    pub fn footer(&self, ui: &mut Ui) {
        ui.add_space(4.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width().min(self.right - self.left).max(1.0), FOOT), Sense::hover());
        let p = ui.ctx().layer_painter(ui.layer_id());
        let (x0, c) = (self.left - RAIL - RAIL_GAP, self.color);
        let rail_r = x0 + RAIL;
        let (head_bottom, foot_top) = (self.top + BAND, row.top());
        // Rail, in two segments with a break, like the main frame's.
        let split = head_bottom + (foot_top - head_bottom) * 0.62;
        p.rect_filled(Rect::from_min_max(pos2(x0, head_bottom - 1.0), pos2(rail_r, split - 2.0)), 0.0, c);
        p.rect_filled(Rect::from_min_max(pos2(x0, split + 2.0), pos2(rail_r, foot_top + 1.0)), 0.0, c.gamma_multiply(0.7));
        // Footer band, broken near its end.
        let foot = Rect::from_min_max(pos2(x0, foot_top), pos2(self.right, row.bottom()));
        let brk = foot.left() + foot.width() * 0.78;
        p.rect_filled(Rect::from_min_max(foot.min, pos2(brk - 2.0, foot.bottom())), CornerRadius { sw: 14, ..Default::default() }, c.gamma_multiply(0.7));
        p.rect_filled(Rect::from_min_max(pos2(brk + 2.0, foot.top()), foot.max), CornerRadius { ne: 6, se: 6, ..Default::default() }, c);
        // The elbows' inner corners curve in.
        for (fill, corner, center) in [
            (c, Rect::from_min_size(pos2(rail_r, head_bottom), vec2(ELBOW, ELBOW)), pos2(rail_r + ELBOW, head_bottom + ELBOW)),
            (c.gamma_multiply(0.7), Rect::from_min_size(pos2(rail_r, foot_top - ELBOW), vec2(ELBOW, ELBOW)), pos2(rail_r + ELBOW, foot_top - ELBOW)),
        ] {
            p.rect_filled(corner, 0.0, fill);
            p.circle_filled(center, ELBOW, pal::PANEL);
        }
    }
}

/// A content panel with a header and HUD corner brackets.
pub fn panel<R>(ui: &mut Ui, title: &str, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = egui::Frame::new()
        .fill(pal::PANEL.gamma_multiply(0.85))
        .inner_margin(egui::Margin::same(16))
        .corner_radius(CornerRadius::same(4));
    let out = frame.show(ui, |ui| {
        header(ui, title, color);
        ui.add_space(6.0);
        add(ui)
    });
    brackets(ui.painter(), out.response.rect, color.gamma_multiply(0.7), 14.0);
    out.inner
}

/// HUD corner brackets around a rect.
pub fn brackets(painter: &egui::Painter, rect: Rect, color: Color32, len: f32) {
    let s = Stroke::new(1.5, color);
    let r = rect.expand(3.0);
    for (corner, dx, dy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.left_bottom(), 1.0, -1.0),
        (r.right_bottom(), -1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + vec2(len * dx, 0.0)], s);
        painter.line_segment([corner, corner + vec2(0.0, len * dy)], s);
    }
}

/// Faint moving grid + vignette for empty space.
pub fn backdrop(painter: &egui::Painter, rect: Rect, t: f64) {
    painter.rect_filled(rect, 0.0, pal::BG);
    let spacing = 48.0;
    let offset = ((t * 6.0) % spacing as f64) as f32;
    let stroke = Stroke::new(1.0, pal::GRID.gamma_multiply(0.55));
    let mut x = rect.left() - offset;
    while x < rect.right() {
        if x > rect.left() {
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], stroke);
        }
        x += spacing;
    }
    let mut y = rect.top() + offset;
    while y < rect.bottom() {
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], stroke);
        y += spacing;
    }
    // Slow scan line.
    let sy = rect.top() + ((t * 0.12).fract() as f32) * rect.height();
    for (i, a) in [(0.0, 0.10), (2.0, 0.05), (5.0, 0.025)] {
        painter.line_segment(
            [pos2(rect.left(), sy - i), pos2(rect.right(), sy - i)],
            Stroke::new(1.0, pal::CYAN.gamma_multiply(a)),
        );
    }
}

/// Pulsing status lamp.
pub fn lamp(painter: &egui::Painter, center: Pos2, color: Color32, t: f64) {
    let pulse = 0.6 + 0.4 * ((t * 3.0).sin() as f32 * 0.5 + 0.5);
    painter.circle_filled(center, 9.0, color.gamma_multiply(0.15 * pulse));
    painter.circle_filled(center, 5.5, color.gamma_multiply(pulse));
    painter.circle_stroke(center, 8.0, Stroke::new(1.0, color.gamma_multiply(0.5)));
}

/// Deterministic pseudo-random console digits that tick over slowly.
pub fn readout(seed: u64, t: f64, digits: usize) -> String {
    let tick = (t * 0.7) as u64;
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ tick.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (0..digits)
        .map(|i| {
            x ^= x >> 31;
            x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
            if i == 2 { '-' } else { char::from(b'0' + (x % 10) as u8) }
        })
        .collect()
}

/// A rounded "LCARS-like" text field frame color for inputs.
pub fn input_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(4, 6, 12))
        .stroke(Stroke::new(1.5, pal::ORANGE.gamma_multiply(0.7)))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin::symmetric(14, 6))
}

/// Draws an arc stroke (used for small radar/spinner flourishes).
/// Eases a displayed number towards `value` so readouts roll smoothly
/// instead of jumping.
pub fn glide(ui: &egui::Ui, name: &'static str, value: f32) -> f32 {
    ui.ctx().animate_value_with_time(ui.scope_id().with(name), value, 0.35)
}

/// A sensor sweep: a busy trace with a scan line running across it (the
/// handheld frame's header).
pub fn sweep(painter: &egui::Painter, rect: Rect, t: f64, color: Color32) {
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, color.gamma_multiply(0.35)), egui::StrokeKind::Inside);
    let n = 64;
    let scan = ((t * 0.45).fract() as f32) * rect.width();
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let f = i as f32 / n as f32;
            let x = rect.left() + f * rect.width();
            let k = t as f32;
            // A few drifting waves, louder near the scan line.
            let near = 1.0 - ((x - rect.left() - scan).abs() / (rect.width() * 0.35)).min(1.0);
            let wave = (f * 23.0 + k * 3.1).sin() * 0.5 + (f * 57.0 - k * 5.3).sin() * 0.3 + (f * 131.0 + k * 1.7).sin() * 0.2;
            pos2(x, rect.center().y - wave * rect.height() * (0.18 + 0.3 * near))
        })
        .collect();
    painter.add(Shape::line(pts, Stroke::new(1.2, color)));
    let x = rect.left() + scan;
    painter.line_segment([pos2(x, rect.top() + 1.0), pos2(x, rect.bottom() - 1.0)], Stroke::new(2.0, color.gamma_multiply(0.9)));
}

pub fn spinner(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32, t: f64) {
    let start = (t * 4.0) as f32;
    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + i as f32 / 24.0 * PI * 1.4;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(PathShape::line(pts, Stroke::new(3.0, color)));
}

/// The Slipstream mark, in 0..1 units: a faceted arrowhead (two convex
/// halves) split by a diagonal slash. Mirrors `assets/logo.svg`.
const LOGO_LEFT: [(f32, f32); 4] = [(0.50, 0.03), (0.50, 0.66), (0.34, 0.83), (0.12, 0.95)];
const LOGO_RIGHT: [(f32, f32); 4] = [(0.50, 0.03), (0.88, 0.95), (0.66, 0.83), (0.50, 0.66)];
/// Slash centre line and half-width.
const LOGO_SLASH: ((f32, f32), (f32, f32), f32) = ((0.10, 0.80), (0.90, 0.38), 0.035);

fn inside_convex(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut sign = 0.0f32;
    for i in 0..poly.len() {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % poly.len()];
        let c = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if c != 0.0 {
            if sign != 0.0 && c.signum() != sign {
                return false;
            }
            sign = c.signum();
        }
    }
    true
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Colour of the mark at (x, y) in 0..1 units, if any.
pub fn logo_shade(x: f32, y: f32) -> Option<Color32> {
    let ((x0, y0), (x1, y1), half) = LOGO_SLASH;
    let (dx, dy) = (x1 - x0, y1 - y0);
    let dist = ((x - x0) * dy - (y - y0) * dx).abs() / (dx * dx + dy * dy).sqrt();
    if dist < half {
        return None;
    }
    if inside_convex(&LOGO_LEFT, x, y) {
        Some(mix(Color32::from_rgb(0xff, 0xc6, 0x40), Color32::from_rgb(0xff, 0x8c, 0x00), y))
    } else if inside_convex(&LOGO_RIGHT, x, y) {
        Some(mix(Color32::from_rgb(0xf2, 0x7a, 0x00), Color32::from_rgb(0xb9, 0x45, 0x00), y))
    } else {
        None
    }
}

/// Renders the mark (4×4 supersampled). With `badge`, it sits on a dark
/// rounded square with a soft glow — the app icon; without, it's on
/// transparency for in-app use.
pub fn render_logo(size: u32, badge: bool) -> egui::IconData {
    let shade = |fx: f32, fy: f32| -> Option<Color32> {
        if !badge {
            return logo_shade(fx, fy);
        }
        let corner = 0.2;
        let cx = fx.clamp(corner, 1.0 - corner);
        let cy = fy.clamp(corner, 1.0 - corner);
        if ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() > corner {
            return None;
        }
        // Mark scaled into the badge, over a dark field with an orange glow.
        let (mx, my) = ((fx - 0.14) / 0.72, (fy - 0.13) / 0.74);
        if let Some(c) = logo_shade(mx, my) {
            return Some(c);
        }
        let glow = (1.0 - ((fx - 0.5).powi(2) + (fy - 0.55).powi(2)).sqrt() / 0.55).clamp(0.0, 1.0);
        Some(mix(Color32::from_rgb(4, 6, 14), Color32::from_rgb(70, 30, 6), glow * glow))
    };
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let ss = 4;
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = (x as f32 + (sx as f32 + 0.5) / ss as f32) / size as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / ss as f32) / size as f32;
                    if let Some(c) = shade(fx, fy) {
                        r += c.r() as u32;
                        g += c.g() as u32;
                        b += c.b() as u32;
                        n += 1;
                    }
                }
            }
            // Average colour of covered samples; coverage becomes alpha.
            if let (Some(r), Some(g), Some(b)) = (r.checked_div(n), g.checked_div(n), b.checked_div(n)) {
                let o = ((y * size + x) * 4) as usize;
                let total = (ss * ss) as u32;
                rgba[o..o + 4].copy_from_slice(&[r as u8, g as u8, b as u8, (n * 255 / total) as u8]);
            }
        }
    }
    egui::IconData { rgba, width: size, height: size }
}

/// The app icon (window, tray, desktop entry, Windows .exe).
pub fn icon(size: u32) -> egui::IconData {
    render_logo(size, true)
}
