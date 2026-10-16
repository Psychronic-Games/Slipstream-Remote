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
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    };
    add(&mut fonts, "Antonio-Regular", include_bytes!("../../assets/fonts/Antonio-Regular.ttf"));
    add(&mut fonts, "Antonio-Bold", include_bytes!("../../assets/fonts/Antonio-Bold.ttf"));
    add(&mut fonts, "Barlow", include_bytes!("../../assets/fonts/BarlowSemiCondensed-Regular.ttf"));
    add(&mut fonts, "Barlow-SemiBold", include_bytes!("../../assets/fonts/BarlowSemiCondensed-SemiBold.ttf"));
    add(&mut fonts, "ShareTechMono", include_bytes!("../../assets/fonts/ShareTechMono-Regular.ttf"));

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

    ctx.global_style_mut(|style| {
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
pub fn block_button(ui: &mut Ui, rect: Rect, label: &str, code: &str, color: Color32, active: bool) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with(label), Sense::click());
    let painter = ui.painter();
    let mut fill = color;
    if resp.hovered() {
        fill = lighten(color, 0.25);
        glow(painter, rect, 2.0, color, 0.8);
    }
    if active {
        fill = lighten(color, 0.4);
        glow(painter, rect, 2.0, color, 1.2);
    }
    if resp.is_pointer_button_down_on() {
        fill = Color32::WHITE;
    }
    painter.rect_filled(rect, 2.0, fill);
    painter.text(
        rect.right_bottom() + vec2(-8.0, -4.0),
        Align2::RIGHT_BOTTOM,
        label.to_uppercase(),
        display(17.0),
        Color32::BLACK,
    );
    if !code.is_empty() {
        painter.text(rect.left_top() + vec2(8.0, 4.0), Align2::LEFT_TOP, code, mono(11.0), Color32::BLACK.gamma_multiply(0.6));
    }
    if active {
        // Active indicator notch.
        painter.rect_filled(Rect::from_min_size(rect.right_top() - vec2(6.0, 0.0), vec2(6.0, rect.height())), 0.0, Color32::BLACK);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A pill button sized to its label.
pub fn pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    pill_sized(ui, label, color, vec2(0.0, 32.0))
}

pub fn pill_sized(ui: &mut Ui, label: &str, color: Color32, min: Vec2) -> Response {
    let text = label.to_uppercase();
    let galley = ui.painter().layout_no_wrap(text, display(17.0), Color32::BLACK);
    let size = vec2((galley.size().x + 40.0).max(min.x), min.y.max(28.0));
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
        if rect.width() - 2.0 * cap > galley.size().x + 12.0 {
            for x in [rect.left() + cap, rect.right() - cap] {
                painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0, Color32::BLACK));
            }
        }
        painter.galley(
            pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
            galley,
            Color32::BLACK,
        );
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

/// App icon: an elbow and data bars on black, generated at startup
/// (4×4 supersampled for smooth edges).
pub fn icon(size: u32) -> egui::IconData {
    fn shade(fx: f32, fy: f32) -> Option<Color32> {
        // Rounded-square badge.
        let corner = 0.18;
        let cx = fx.clamp(corner, 1.0 - corner);
        let cy = fy.clamp(corner, 1.0 - corner);
        if ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() > corner {
            return None;
        }
        let dist = |x: f32, y: f32, px: f32, py: f32| ((x - px).powi(2) + (y - py).powi(2)).sqrt();
        // Elbow: vertical arm x∈[.14,.40], horizontal arm y∈[.14,.34],
        // outer radius .26 at (.40,.40), inner fillet radius .10 at (.50,.44).
        let in_corner = (0.14..0.40).contains(&fx) && (0.14..0.40).contains(&fy) && dist(fx, fy, 0.40, 0.40) <= 0.26;
        let in_v = (0.14..0.40).contains(&fx) && (0.40..0.86).contains(&fy);
        let in_h = (0.40..0.86).contains(&fx) && (0.14..0.34).contains(&fy);
        let in_fillet = (0.40..0.50).contains(&fx) && (0.34..0.44).contains(&fy) && dist(fx, fy, 0.50, 0.44) > 0.10;
        if in_corner || in_v || in_h || in_fillet {
            return Some(pal::ORANGE);
        }
        let bar = |x0: f32, y0: f32, y1: f32| (x0..0.86).contains(&fx) && (y0..y1).contains(&fy);
        if bar(0.50, 0.50, 0.58) {
            return Some(pal::LAVENDER);
        }
        if bar(0.58, 0.64, 0.72) {
            return Some(pal::BLUE);
        }
        if bar(0.66, 0.78, 0.84) {
            return Some(pal::CYAN);
        }
        Some(Color32::from_rgb(2, 3, 8))
    }
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let ss = 4;
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = (x as f32 + (sx as f32 + 0.5) / ss as f32) / size as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / ss as f32) / size as f32;
                    if let Some(c) = shade(fx, fy) {
                        r += c.r() as u32;
                        g += c.g() as u32;
                        b += c.b() as u32;
                        a += 255;
                    }
                }
            }
            let n = (ss * ss) as u32;
            let o = ((y * size + x) * 4) as usize;
            // Straight (unpremultiplied) colour of covered samples, coverage as alpha.
            let covered = (a / 255).max(1);
            rgba[o..o + 4].copy_from_slice(&[(r / covered) as u8, (g / covered) as u8, (b / covered) as u8, (a / n) as u8]);
        }
    }
    egui::IconData { rgba, width: size, height: size }
}
