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
            (Heading, display_bold(24.0)),
            (Body, body(16.0)),
            (Button, display(16.0)),
            (Monospace, mono(15.0)),
            (Small, body(13.0)),
        ]
        .into();
        style.spacing.item_spacing = if cfg!(target_os="android") {vec2(8.0,8.0)} else {vec2(6.0,5.0)};
        style.spacing.button_padding = if cfg!(target_os="android") {vec2(14.0,6.0)} else {vec2(10.0,4.0)};
        style.spacing.interact_size.y = if cfg!(target_os="android") {32.0} else {28.0};

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
        let r = CornerRadius::same(6);
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

/// A compact rounded action button sized to its label.
pub fn pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    pill_sized(ui, label, color, vec2(0.0, if cfg!(target_os="android") {32.0} else {29.0}))
}

pub fn pill_sized(ui: &mut Ui, label: &str, color: Color32, min: Vec2) -> Response {
    pill_with(ui, label, color, min, None)
}

/// A pill with the logo delta in front, pointing back (the way to the
/// Bridge).
pub fn back_pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    pill_with(ui, label, color, vec2(0.0, 32.0), Some(back_delta))
}

pub fn transfer_pill(ui: &mut Ui, label: &str, color: Color32, send: bool) -> Response {
    pill_with(ui, label, color, vec2(0.0, 30.0), Some(if send {forward_delta} else {back_delta}))
}

fn forward_delta(painter: &egui::Painter, rect: Rect, ink: Color32, bg: Color32) {
    delta(painter, rect, ink, bg, true);
}

/// Draws the logo pointing left into `rect`, in `ink`, with the slash cut
/// out in `bg`.
fn back_delta(painter: &egui::Painter, rect: Rect, ink: Color32, bg: Color32) {
    delta(painter, rect, ink, bg, false);
}

pub fn delta(painter: &egui::Painter, rect: Rect, ink: Color32, bg: Color32, right: bool) {
    // Logo space (pointing up) → pointing left: (x, y) ↦ (y, 1 − x).
    let at = |(x, y): (f32, f32)| {
        let (x,y) = if right {(1.0-y,x)} else {(y,1.0-x)};
        pos2(rect.left() + x * rect.width(), rect.top() + y * rect.height())
    };
    for half in [&LOGO_LEFT, &LOGO_RIGHT] {
        painter.add(egui::Shape::convex_polygon(half.iter().copied().map(at).collect(), ink, Stroke::NONE));
    }
    let ((x0, y0), (x1, y1), half_width) = LOGO_SLASH;
    painter.line_segment([at((x0, y0)), at((x1, y1))], Stroke::new(half_width * 2.0 * rect.width(), bg));
}

/// A font-independent save/favourite star, including on older Windows fonts.
pub fn star(painter: &egui::Painter, rect: Rect, color: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), color);
    for i in 0..10 {
        let angle = -FRAC_PI_2 + i as f32 * PI / 5.0;
        let radius = rect.width().min(rect.height()) * if i % 2 == 0 {0.5} else {0.23};
        mesh.colored_vertex(rect.center() + vec2(angle.cos(),angle.sin()) * radius, color);
    }
    for i in 0..10 { mesh.add_triangle(0, i+1, (i+1)%10+1); }
    painter.add(Shape::mesh(mesh));
}

type PillIcon = fn(&egui::Painter, Rect, Color32, Color32);

fn pill_with(ui: &mut Ui, label: &str, color: Color32, min: Vec2, icon: Option<PillIcon>) -> Response {
    let text = label.to_uppercase();
    let galley = ui.painter().layout_no_wrap(text, display(17.0), Color32::BLACK);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let navigation = icon.is_some();
    let pad = 2.0 * ui.spacing().button_padding.x + if navigation { 12.0 } else { 0.0 };
    let size = vec2((galley.size().x + pad + icon_w).max(min.x), min.y.max(if cfg!(target_os="android") {40.0} else {28.0}));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let enabled = ui.is_enabled();
        let mut fill = if enabled { color } else { color.gamma_multiply(0.35) };
        let radius = if navigation { rect.height() / 2.0 } else { 6.0 };
        if enabled && resp.hovered() {
            fill = lighten(color, 0.3);
            glow(painter, rect, radius, color, 1.0);
        }
        if enabled && resp.is_pointer_button_down_on() {
            fill = Color32::WHITE;
        }
        let r = radius as u8;
        painter.rect_filled(rect, CornerRadius::same(r), fill);
        // Black seams near both ends, like segmented console buttons —
        // only when the label clears them.
        let cap = rect.height() * 0.55;
        if navigation && rect.width() - 2.0 * cap > galley.size().x + icon_w + 12.0 {
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

/// Small code-native vector symbols: no font glyphs or raster scaling.
#[derive(Clone, Copy)]
pub enum ConsoleIcon {
    Back,
    Fullscreen,
    Window,
    Keys,
    Chat,
    Files,
    Sound,
    Muted,
    Tools,
    Performance,
    End,
    Console,
}
impl ConsoleIcon {
    fn label(self) -> &'static str {
        match self {
            Self::Back => "Bridge",
            Self::Fullscreen => "Full screen",
            Self::Window => "Window",
            Self::Keys => "Keys",
            Self::Chat => "Chat",
            Self::Files => "Files",
            Self::Sound => "Sound",
            Self::Muted => "Muted",
            Self::Tools => "Tools",
            Self::Performance => "Picture quality",
            Self::End => "End link",
            Self::Console => "Console",
        }
    }
}

pub fn console_button_width(ui: &Ui, label: &str, menu: bool) -> f32 {
    let text_w = ui.painter().layout_no_wrap(label.to_uppercase(), display(17.0), pal::BG).size().x;
    34.0 + if text_w > 0.0 { 6.0 + text_w } else { 0.0 } + if menu { 12.0 } else { 0.0 }
}

pub fn console_button(ui: &mut Ui, label: &str, color: Color32, icon: ConsoleIcon, menu: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_uppercase(), display(17.0), pal::BG);
    let text_w = galley.size().x;
    let width = 16.0 + 18.0 + if text_w > 0.0 { 6.0 + text_w } else { 0.0 } + if menu { 12.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
    let enabled = ui.is_enabled();
    let fill = if !enabled {
        color.gamma_multiply(0.35)
    } else if response.is_pointer_button_down_on() {
        Color32::WHITE
    } else if response.hovered() {
        lighten(color, 0.22)
    } else {
        color
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, fill);
    let symbol = Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), vec2(18.0, 18.0));
    console_icon(painter, symbol, icon, pal::BG, fill);
    painter.galley(pos2(symbol.right() + 6.0, rect.center().y - galley.size().y / 2.0), galley, pal::BG);
    if menu {
        let c = pos2(rect.right() - 8.0, rect.center().y);
        painter.add(Shape::line(
            vec![c + vec2(-3.0, -1.5), c + vec2(0.0, 1.5), c + vec2(3.0, -1.5)],
            Stroke::new(1.5, pal::BG),
        ));
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, if label.is_empty() { icon.label() } else { label }));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Anchor the menu's outer frame to its console segment, with the same
/// corner radius and a narrow seam. Submenus inherit the row styling.
pub fn console_menu(response: &Response) -> egui::Popup<'_> {
    egui::Popup::menu(response)
        .align(egui::emath::RectAlign::BOTTOM_START)
        .gap(4.0)
        .width(response.rect.width().max(180.0))
        .frame(egui::Frame::new().fill(pal::PANEL)
            .stroke(Stroke::new(1.0, pal::VIOLET))
            .corner_radius(6.0).inner_margin(6))
        .style(console_menu_style)
}

/// Shared selection and spacing for console menus and form choice lists.
pub fn console_menu_style(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = if cfg!(target_os="android") {vec2(10.0,5.0)} else {vec2(8.0,3.0)};
    style.spacing.item_spacing = vec2(4.0, 4.0);
    style.spacing.interact_size.y = if cfg!(target_os="android") {32.0} else {28.0};
    style.visuals.override_text_color = Some(pal::BLUE);
    style.visuals.selection.bg_fill = Color32::from_rgb(24, 44, 64);
    style.visuals.selection.stroke = Stroke::new(1.0, pal::BLUE);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(30, 54, 76);
    for widget in [&mut style.visuals.widgets.inactive, &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active, &mut style.visuals.widgets.open] {
        widget.corner_radius = CornerRadius::same(6);
    }
}

/// The host's independent communications window: a swept left rail, channel
/// band and blue footer around the log and composer.
pub fn chat_body(ui: &Ui, area: Rect) -> Rect {
    let p = ui.painter();
    let rail = 18.0;
    let elbow = 14.0;
    let band = area.with_max_y(area.top() + 30.0);
    p.rect_filled(band, CornerRadius { nw: 18, ne: 6, ..Default::default() }, pal::PEACH);
    let symbol = Rect::from_center_size(pos2(area.left() + 19.0, band.center().y), vec2(18.0, 18.0));
    console_icon(p, symbol, ConsoleIcon::Chat, pal::BG, pal::PEACH);
    p.with_clip_rect(band.shrink2(vec2(8.0, 0.0))).text(
        pos2(symbol.right() + 8.0, band.center().y), Align2::LEFT_CENTER,
        "SUBSPACE COMMS", display_bold(18.0), pal::BG);
    let top = band.bottom();
    let foot_top = area.bottom() - 12.0;
    let middle = top + (foot_top - top) * 0.55;
    for (y0, y1, color) in [(top, middle - 2.0, pal::PEACH), (middle + 2.0, foot_top, pal::BLUE)] {
        p.rect_filled(Rect::from_min_max(pos2(area.left(), y0), pos2(area.left() + rail, y1)), 0.0, color);
    }
    let x = area.left() + rail;
    p.rect_filled(Rect::from_min_size(pos2(x, top), vec2(elbow, elbow)), 0.0, pal::PEACH);
    p.circle_filled(pos2(x + elbow, top + elbow), elbow, pal::BG);
    p.rect_filled(Rect::from_min_size(pos2(x, foot_top - elbow), vec2(elbow, elbow)), 0.0, pal::BLUE);
    p.circle_filled(pos2(x + elbow, foot_top - elbow), elbow, pal::BG);
    p.rect_filled(area.with_min_y(foot_top), CornerRadius { sw: 12, se: 6, ..Default::default() }, pal::BLUE);
    Rect::from_min_max(pos2(x + elbow + 8.0, top + 10.0), area.max - vec2(10.0, 24.0))
}

fn console_icon(p: &egui::Painter, rect: Rect, icon: ConsoleIcon, ink: Color32, fill: Color32) {
    let at = |(x, y): (f32, f32)| rect.min + vec2(x * rect.width(), y * rect.height());
    let stroke = Stroke::new(1.5, ink);
    let line = |points: &[(f32, f32)]| p.add(Shape::line(points.iter().copied().map(at).collect(), stroke));
    let box_at = |a, b| Rect::from_min_max(at(a), at(b));
    match icon {
        ConsoleIcon::Back => back_delta(p, rect, ink, fill),
        ConsoleIcon::Chat => {
            p.rect_stroke(box_at((0.08, 0.12), (0.92, 0.74)), 3.0, stroke, egui::StrokeKind::Inside);
            p.add(Shape::convex_polygon(
                vec![at((0.25, 0.66)), at((0.25, 0.93)), at((0.52, 0.7))],
                ink,
                Stroke::NONE,
            ));
            for x in [0.3, 0.5, 0.7] {
                p.circle_filled(at((x, 0.43)), 0.9, ink);
            }
        }
        ConsoleIcon::Files => {
            line(&[(0.06, 0.82), (0.06, 0.22), (0.4, 0.22), (0.52, 0.35), (0.93, 0.35), (0.93, 0.82), (0.06, 0.82)]);
            line(&[(0.1, 0.43), (0.9, 0.43)]);
        }
        ConsoleIcon::Keys => {
            p.rect_stroke(box_at((0.04, 0.2), (0.96, 0.84)), 2.0, stroke, egui::StrokeKind::Inside);
            for x in [0.23, 0.42, 0.61, 0.8] {
                for y in [0.37, 0.53] {
                    p.circle_filled(at((x, y)), 0.8, ink);
                }
            }
            line(&[(0.3, 0.7), (0.7, 0.7)]);
        }
        ConsoleIcon::Fullscreen => {
            for pts in [
                &[(0.08, 0.38), (0.08, 0.08), (0.38, 0.08)][..],
                &[(0.62, 0.08), (0.92, 0.08), (0.92, 0.38)],
                &[(0.08, 0.62), (0.08, 0.92), (0.38, 0.92)],
                &[(0.62, 0.92), (0.92, 0.92), (0.92, 0.62)],
            ] {
                line(pts);
            }
        }
        ConsoleIcon::Window => {
            p.rect_stroke(box_at((0.32, 0.08), (0.92, 0.68)), 2.0, stroke, egui::StrokeKind::Inside);
            let front = box_at((0.08, 0.32), (0.68, 0.92));
            p.rect_filled(front, 2.0, fill);
            p.rect_stroke(front, 2.0, stroke, egui::StrokeKind::Inside);
        }
        ConsoleIcon::Sound | ConsoleIcon::Muted => {
            line(&[(0.38, 0.2), (0.18, 0.38), (0.04, 0.38), (0.04, 0.64), (0.18, 0.64), (0.38, 0.82), (0.38, 0.2)]);
            if matches!(icon, ConsoleIcon::Muted) {
                line(&[(0.61, 0.35), (0.91, 0.67)]);
                line(&[(0.91, 0.35), (0.61, 0.67)]);
            } else {
                line(&[(0.58, 0.32), (0.69, 0.5), (0.58, 0.69)]);
                line(&[(0.77, 0.19), (0.94, 0.5), (0.77, 0.82)]);
            }
        }
        ConsoleIcon::Tools => {
            line(&[
                (0.83, 0.08),
                (0.7, 0.32),
                (0.51, 0.34),
                (0.4, 0.19),
                (0.54, 0.04),
                (0.31, 0.13),
                (0.25, 0.37),
                (0.37, 0.53),
                (0.09, 0.81),
                (0.09, 0.91),
                (0.21, 0.93),
                (0.54, 0.6),
                (0.75, 0.6),
                (0.93, 0.4),
                (0.94, 0.23),
                (0.83, 0.08),
            ]);
        }
        ConsoleIcon::Performance => {
            let points = (0..=20)
                .map(|i| {
                    let a = PI + i as f32 * PI / 20.0;
                    at((0.5 + a.cos() * 0.43, 0.72 + a.sin() * 0.53))
                })
                .collect();
            p.add(Shape::line(points, stroke));
            line(&[(0.08, 0.76), (0.92, 0.76)]);
            line(&[(0.5, 0.68), (0.74, 0.3)]);
            p.circle_filled(at((0.5, 0.68)), 1.8, ink);
        }
        ConsoleIcon::End => {
            line(&[(0.5, 0.1), (0.12, 0.1), (0.12, 0.9), (0.5, 0.9)]);
            line(&[(0.4, 0.5), (0.94, 0.5)]);
            line(&[(0.73, 0.28), (0.95, 0.5), (0.73, 0.72)]);
        }
        ConsoleIcon::Console => {
            p.rect_stroke(box_at((0.04, 0.1), (0.96, 0.9)), 2.0, stroke, egui::StrokeKind::Inside);
            line(&[(0.21, 0.31), (0.39, 0.5), (0.21, 0.69)]);
            line(&[(0.5, 0.69), (0.77, 0.69)]);
        }
    }
}

/// Complete the horizontal rail between controls and connection figures.
pub fn session_rail_label(p: &egui::Painter, rect: Rect, label: &str) {
    if rect.width()<12.0 {return;}
    p.rect_filled(rect,6.0,pal::TAN);
    if rect.width()>80.0 {
        let text=rect.shrink2(vec2(10.0,0.0));
        p.with_clip_rect(text).text(text.left_center(),Align2::LEFT_CENTER,label,display_bold(17.0),pal::BG);
    }
}

/// Reserve the desktop frame independently of video aspect ratio and display
/// scaling. Both fitting and input use this body, so even a zoomed picture
/// cannot cover the rails or lose its bottom row beneath the footer.
pub fn session_body(full: Rect, toolbar_height: f32) -> Rect {
    Rect::from_min_max(
        full.min + vec2(80.0, toolbar_height),
        full.max - vec2(16.0, 16.0),
    )
}

/// A dark instrument panel with a quiet blue outline, including on narrow bars.
pub fn session_stats_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(pal::PANEL)
        .stroke(Stroke::new(1.0, pal::BLUE.gamma_multiply(0.45)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 0))
}

/// Paint outside the reserved body; the left rail stays visible on every
/// desktop platform, including while awaiting a frame or zooming the picture.
pub fn session_rails(p: &egui::Painter, full: Rect, body: Rect) -> f32 {
    let width = 44.0;
    let left = full.left();
    let right = left + width;
    let top = body.top();
    let bottom = full.bottom();
    let h = bottom - top;
    // The elbows extend into the reserved left gutter, never into the video.
    let side = Rect::from_min_max(pos2(left, top), pos2(body.left() - 4.0, bottom));
    let side_p = p.with_clip_rect(side.intersect(p.clip_rect()));
    for (start, end, color) in [
        (0.0, 0.28, pal::ORANGE),
        (0.28, 0.5, pal::PEACH),
        (0.5, 0.78, pal::LAVENDER),
        (0.78, 1.0, pal::BLUE),
    ] {
        let segment_top = top + h * start + if start > 0.0 { 4.0 } else { 0.0 };
        side_p.rect_filled(Rect::from_min_max(pos2(left, segment_top), pos2(right, top + h * end)), 0.0, color);
    }
    let elbow = 32.0;
    side_p.rect_filled(Rect::from_min_size(pos2(right, top), vec2(elbow, elbow)), 0.0, pal::ORANGE);
    side_p.circle_filled(pos2(right + elbow, top + elbow), elbow, Color32::BLACK);

    let foot_top = bottom - 12.0;
    side_p.rect_filled(Rect::from_min_size(pos2(right, foot_top - elbow), vec2(elbow, elbow)), 0.0, pal::BLUE);
    side_p.circle_filled(pos2(right + elbow, foot_top - elbow), elbow, Color32::BLACK);
    p.rect_filled(
        Rect::from_min_max(pos2(left, foot_top), full.max),
        CornerRadius { sw: 12, se: 6, ..Default::default() },
        pal::BLUE,
    );
    rail_outer_corner(p,pos2(left,bottom),18.0,pal::BLUE,Color32::BLACK);

    // A black break separates the right edge from the End Link control.
    let x = full.right() - 8.0;
    let split = top + h * 0.7;
    p.rect_filled(
        Rect::from_min_max(pos2(x, top + 12.0), pos2(full.right(), split - 2.0)),
        CornerRadius { nw: 4, ne: 4, ..Default::default() },
        pal::LAVENDER,
    );
    p.rect_filled(
        Rect::from_min_max(pos2(x, split + 2.0), pos2(full.right(), foot_top - 4.0)),
        CornerRadius { sw: 4, se: 4, ..Default::default() },
        pal::BLUE,
    );
    width
}

/// A panel header: rounded cap, title, and a rule running to the edge.
pub fn header(ui: &mut Ui, title: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let painter = ui.painter();
    let cap = Rect::from_min_size(rect.min, vec2(26.0, 26.0));
    painter.rect_filled(cap, CornerRadius { nw: 13, sw: 13, ..Default::default() }, color);
    let title_pos = pos2(cap.right() + 10.0, rect.center().y);
    let mut job=egui::text::LayoutJob::simple(title.to_uppercase(),display_bold(20.0),color,(rect.right()-title_pos.x).max(1.0));
    job.wrap.max_rows=1;
    let galley = painter.layout_job(job);
    let tw = galley.size().x;
    painter.with_clip_rect(rect).galley(pos2(title_pos.x, rect.center().y - galley.size().y / 2.0), galley, color);
    let bar = Rect::from_min_max(pos2(title_pos.x + tw + 12.0, rect.top() + 7.0), pos2(rect.right(), rect.bottom() - 7.0));
    if bar.width() > 8.0 {
        painter.rect_filled(bar, CornerRadius { ne: 6, se: 6, ..Default::default() }, color.gamma_multiply(0.85));
    }
}

// A floating window's console frame: a header band with the title and a
// Close segment, a rail down the left joined to it by an elbow, and a
// footer band joined by another (see `window_header`, `WindowChrome::footer`).
const RAIL: f32 = 16.0;
const RAIL_GAP: f32 = 10.0;
const BAND: f32 = 30.0;
const FOOT: f32 = 12.0;
const ELBOW: f32 = 12.0;

/// Close requests alone leave a deferred window alive until its parent stops
/// registering it. Hide it immediately, then wake even an occluded parent so
/// Windows cannot leave an empty swapchain rectangle behind.
pub fn dismiss_tool_viewport(ctx: &egui::Context, desktop: bool) {
    if desktop {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    repaint_console(ctx);
}

/// Paint the console once even when covered, so new tool viewports register.
pub fn repaint_console(ctx: &egui::Context) {
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
    ctx.request_repaint_of(egui::ViewportId::ROOT);
}

/// Repainting an unknown viewport creates a default native window in egui.
/// A video worker can outlive a floating window, or run while its session is
/// docked, so only target windows reported by the backend as currently alive.
pub fn repaint_existing_viewport(ctx: &egui::Context, id: egui::ViewportId) {
    if id == egui::ViewportId::ROOT || ctx.input(|i| i.raw.viewports.contains_key(&id)) {
        ctx.request_repaint_of(id);
    }
}

pub fn repaint_existing_after(ctx: &egui::Context, id: egui::ViewportId, after: std::time::Duration) {
    if id == egui::ViewportId::ROOT || ctx.input(|i| i.raw.viewports.contains_key(&id)) {
        ctx.request_repaint_after_for(after, id);
    }
}

/// Height [`WindowChrome::footer`] takes: what a window's scrolling body
/// must leave free below it, or the window grows by that much every frame.
pub const FOOTER_SPACE: f32 = FOOT + 4.0 + 8.0 + 2.0;

// A side rail and thin footer overlap at this corner. Round their union,
// rather than limiting the outer radius to the footer's height. Keep the
// cutout inside the frame gutter, clear of scrolling content and controls.
fn rail_outer_corner(p:&egui::Painter,bottom_left:Pos2,radius:f32,color:Color32,background:Color32) {
    let rect=Rect::from_min_size(bottom_left-vec2(0.0,radius),vec2(radius,radius));
    let p=p.with_clip_rect(rect.intersect(p.clip_rect()));
    p.rect_filled(rect,0.0,background);
    p.circle_filled(rect.right_top(),radius,color);
}

/// Desktop window chrome in the console palette. Returns the content area
/// below the draggable title strip; full screen and mobile use the whole UI.
pub fn native_title_bar(ui: &mut Ui, title: &str) -> Rect {
    native_title_bar_with_rail(ui,title,false)
}

pub fn native_title_bar_with_rail(ui: &mut Ui, title: &str, linked:bool) -> Rect {
    native_title_bar_inner(ui, title, linked, None)
}

pub fn native_session_title_bar(ui: &mut Ui, title: &str, workstation: Option<&str>) -> Rect {
    native_title_bar_inner(ui, title, workstation.is_some(), workstation)
}

fn native_title_bar_inner(ui: &mut Ui, title: &str, linked: bool, workstation: Option<&str>) -> Rect {
    super::window_shape::update();
    let area=ui.max_rect();
    let square=cfg!(target_os="android") || ui.input(|i|i.viewport().fullscreen==Some(true)||i.viewport().maximized==Some(true));
    ui.painter().rect_filled(area,if square{0.0}else{18.0},pal::BG);
    if cfg!(target_os="android") || ui.input(|i|i.viewport().fullscreen==Some(true)) {return area;}
    let maximized=ui.input(|i|i.viewport().maximized==Some(true));
    let bar=Rect::from_min_max(area.min+vec2(5.0,4.0),pos2(area.right()-5.0,area.top()+38.0));
    let p=ui.painter();
    if linked {
        p.rect_filled(bar,CornerRadius{nw:18,ne:16,..Default::default()},pal::LAVENDER);
    } else {
        p.rect_filled(bar,16.0,pal::PANEL);
        p.rect_stroke(bar,16.0,Stroke::new(1.0,pal::VIOLET),egui::StrokeKind::Inside);
        p.rect_filled(Rect::from_min_size(bar.min,vec2(14.0,bar.height())),CornerRadius {nw:16,sw:16,..Default::default()},pal::LAVENDER);
    }
    let (right,close)=native_window_controls(ui,bar,linked,maximized,true);
    if close {ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);}
    let title_rect=Rect::from_min_max(bar.min+vec2(22.0,0.0),pos2(right-4.0,bar.bottom()));
    if let Some(workstation) = workstation {
        let app_width = (title_rect.width() * 0.32).min(240.0);
        let app_rect = title_rect.with_max_x(title_rect.left() + app_width);
        p.with_clip_rect(app_rect).text(app_rect.left_center(), Align2::LEFT_CENTER, "SLIPSTREAM REMOTE", display(15.0), pal::BG);
        let host_rect = title_rect.with_min_x(app_rect.right() + 12.0);
        p.with_clip_rect(host_rect).text(host_rect.center(), Align2::CENTER_CENTER, workstation.to_uppercase(), display_bold(21.0), pal::BG);
        ui.interact(host_rect, ui.scope_id().with("workstation-title"), Sense::hover()).on_hover_text(workstation);
    } else {
        p.with_clip_rect(title_rect).text(title_rect.left_center(),Align2::LEFT_CENTER,title,display_bold(17.0),if linked{pal::BG}else{pal::LAVENDER});
    }
    native_window_interactions(ui,area,title_rect,maximized);
    Rect::from_min_max(pos2(area.left()+5.0,bar.bottom()+3.0),area.max-vec2(5.0,5.0))
}

// Shared symbols and hit targets keep detached tools consistent with the app.
fn native_window_controls(ui:&Ui,bar:Rect,linked:bool,maximized:bool,desktop:bool)->(f32,bool) {
    let p=ui.painter();
    let mut close=false;
    let mut right=bar.right()-9.0;
    for (symbol,hint,color,command) in [
        (0,"Close",pal::PEACH,egui::ViewportCommand::Close),
        (if maximized{3}else{1},if maximized{"Restore window"}else{"Maximize"},pal::LAVENDER,egui::ViewportCommand::Maximized(!maximized)),
        (2,"Minimize",pal::BLUE,egui::ViewportCommand::Minimized(true)),
    ] {
        if !desktop && symbol != 0 { continue; }
        let width=44.0;
        let rect=Rect::from_min_max(pos2(right-width,bar.top()+3.0),pos2(right,bar.bottom()-3.0));
        let response=ui.interact(rect,ui.scope_id().with(("native-window",hint)),Sense::click());
        let fill=if response.hovered(){pal::TAN}else{color};
        if linked {p.rect_filled(rect.expand(2.0),11.0,pal::BG);}
        p.rect_filled(rect,9.0,fill);
        let icon=Rect::from_center_size(rect.center(),vec2(18.0,19.0));
        let stroke=Stroke::new(2.0,pal::BG);
        match symbol {
            0=>{p.line_segment([icon.left_top()+vec2(2.0,2.0),icon.right_bottom()-vec2(2.0,2.0)],stroke);p.line_segment([icon.right_top()+vec2(-2.0,2.0),icon.left_bottom()+vec2(2.0,-2.0)],stroke);},
            1|2=>{
                let at=|(x,y):(f32,f32)|{let(x,y)=if symbol==2{(1.0-x,1.0-y)}else{(x,y)};icon.min+vec2(x*icon.width(),y*icon.height())};
                for half in [&LOGO_LEFT,&LOGO_RIGHT]{p.add(Shape::convex_polygon(half.iter().copied().map(at).collect(),pal::BG,Stroke::NONE));}
                let(a,b,half)=LOGO_SLASH;p.line_segment([at(a),at(b)],Stroke::new(half*2.0*icon.width(),fill));
            },
            _=>{
                p.rect_stroke(Rect::from_min_max(icon.min+vec2(5.0,0.0),icon.max-vec2(0.0,5.0)),2.0,stroke,egui::StrokeKind::Inside);
                let front=Rect::from_min_max(icon.min+vec2(0.0,5.0),icon.max-vec2(5.0,0.0));
                p.rect_filled(front,2.0,fill);p.rect_stroke(front,2.0,stroke,egui::StrokeKind::Inside);
            },
        }
        response.widget_info(||egui::WidgetInfo::labeled(egui::Role::Button,true,hint));
        if response.on_hover_text(hint).clicked(){
            if symbol==0 {close=true;} else {ui.ctx().send_viewport_cmd(command);}
        }
        right=rect.left()-8.0;
    }
    (right,close)
}

fn native_window_interactions(ui:&Ui,area:Rect,title_rect:Rect,maximized:bool) {
    let drag=ui.interact(title_rect,ui.scope_id().with("native-title-drag"),Sense::click_and_drag());
    if drag.double_clicked(){ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));}
    else if drag.drag_started(){ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);}
    if !maximized {
        use egui::{ResizeDirection as D,CursorIcon as C};
        let e=5.0;let corner=12.0;
        for (rect,direction,cursor) in [
            (Rect::from_min_size(area.min,vec2(corner,corner)),D::NorthWest,C::ResizeNwSe),
            (Rect::from_min_size(pos2(area.right()-corner,area.top()),vec2(corner,corner)),D::NorthEast,C::ResizeNeSw),
            (Rect::from_min_size(pos2(area.left(),area.bottom()-corner),vec2(corner,corner)),D::SouthWest,C::ResizeNeSw),
            (Rect::from_min_size(area.max-vec2(corner,corner),vec2(corner,corner)),D::SouthEast,C::ResizeNwSe),
            (Rect::from_min_max(pos2(area.left()+corner,area.top()),pos2(area.right()-corner,area.top()+e)),D::North,C::ResizeVertical),
            (Rect::from_min_max(pos2(area.left()+corner,area.bottom()-e),pos2(area.right()-corner,area.bottom())),D::South,C::ResizeVertical),
            (Rect::from_min_max(pos2(area.left(),area.top()+corner),pos2(area.left()+e,area.bottom()-corner)),D::West,C::ResizeHorizontal),
            (Rect::from_min_max(pos2(area.right()-e,area.top()+corner),pos2(area.right(),area.bottom()-corner)),D::East,C::ResizeHorizontal),
        ] {
            let response=ui.interact(rect,ui.scope_id().with(("native-resize",format!("{direction:?}"))),Sense::drag()).on_hover_cursor(cursor);
            if response.drag_started(){ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));}
        }
    }
}

/// A single title rail joined to a detached tool's left frame. The frame stays
/// fixed while its contents scroll, including at the minimum window size.
pub fn tool_frame(ui:&mut Ui,title:&str,desktop:bool,color:Color32)->(Rect,bool) {
    super::window_shape::update();
    let area=ui.max_rect();
    let maximized=ui.input(|i|i.viewport().maximized==Some(true));
    let square=!desktop || maximized || ui.input(|i|i.viewport().fullscreen==Some(true));
    let p=ui.painter();
    p.rect_filled(area,if square{0.0}else{18.0},pal::BG);
    let frame=area.shrink(5.0);
    p.rect_filled(frame,16.0,pal::PANEL);
    let bar=Rect::from_min_max(frame.min,pos2(frame.right(),frame.top()+34.0));
    let rail_right=frame.left()+14.0;
    let foot_top=frame.bottom()-12.0;
    let split=bar.bottom()+(foot_top-bar.bottom())*0.65;
    p.rect_filled(bar,CornerRadius{nw:18,ne:18,..Default::default()},color);
    p.rect_filled(Rect::from_min_max(pos2(frame.left(),bar.bottom()),pos2(rail_right,split-2.0)),0.0,color);
    p.rect_filled(Rect::from_min_max(pos2(frame.left(),split+2.0),pos2(rail_right,foot_top)),0.0,pal::BLUE);
    let foot=Rect::from_min_max(pos2(frame.left(),foot_top),frame.max);
    p.rect_filled(foot,CornerRadius{sw:12,se:6,..Default::default()},pal::BLUE);
    rail_outer_corner(p,frame.left_bottom(),18.0,pal::BLUE,pal::BG);
    // Restrict each concave cutout to the elbow, clear of all content.
    for (color,corner,center) in [
        (color,Rect::from_min_size(pos2(rail_right,bar.bottom()),vec2(22.0,22.0)),pos2(rail_right+22.0,bar.bottom()+22.0)),
        (pal::BLUE,Rect::from_min_size(pos2(rail_right,foot_top-18.0),vec2(18.0,18.0)),pos2(rail_right+18.0,foot_top-18.0)),
    ] {
        let elbow=p.with_clip_rect(corner.intersect(p.clip_rect()));
        elbow.rect_filled(corner,0.0,color);
        elbow.circle_filled(center,corner.width(),pal::PANEL);
    }
    let (right,close)=native_window_controls(ui,bar,true,maximized,desktop);
    let title_rect=Rect::from_min_max(bar.min+vec2(42.0,0.0),pos2(right-8.0,bar.bottom()));
    p.with_clip_rect(title_rect).text(title_rect.left_center(),Align2::LEFT_CENTER,title.to_uppercase(),display_bold(19.0),pal::BG);
    if desktop {native_window_interactions(ui,area,title_rect,maximized);}
    (Rect::from_min_max(pos2(rail_right+16.0,bar.bottom()+8.0),pos2(frame.right()-10.0,foot_top-8.0)),close)
}

/// A floating window in the console style: no grey title bar; start its
/// contents with [`window_header`] and end them with
/// [`WindowChrome::footer`]. It's dragged by its background.
pub fn window(id: &str, color: Color32) -> egui::Window<'static> {
    egui::Window::new(id.to_string()).title_bar(false).frame(console_window_frame(color))
}

fn console_window_frame(color: Color32) -> egui::Frame {
    egui::Frame::new()
            .fill(pal::PANEL)
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
            .corner_radius(CornerRadius::same(20))
            .inner_margin(egui::Margin { left: (12.0 + RAIL + RAIL_GAP) as i8, right: 14, top: 12, bottom: 12 })
            .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(170) })
}

/// Modal dialogs reserve the same rail gutter as floating LCARS panels.
pub fn modal(id: &str, color: Color32) -> egui::Modal {
    egui::Modal::new(egui::Id::unique(id)).frame(console_window_frame(color))
}

/// Content width, leaving room for the frame and space around the dialog.
pub fn dialog_width(ctx: &egui::Context, preferred: f32) -> f32 {
    (ctx.content_rect().width() - 76.0).clamp(160.0, preferred)
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
    window_header_with_close(ui, title, color, color.gamma_multiply(0.8))
}

/// A console header with a distinct Close colour when that action needs
/// more contrast than the surrounding frame.
pub fn window_header_with_close(ui: &mut Ui, title: &str, color: Color32, close_color: Color32) -> WindowChrome {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAND), Sense::hover());
    let x0 = row.left() - RAIL - RAIL_GAP;
    // The frame lies in the window's margin, outside the content's clip.
    let p = ui.ctx().layer_painter(ui.layer_id());
    let close_w = 86.0;
    let close_rect = Rect::from_min_max(pos2(row.right() - close_w, row.top()), row.max);
    let band = Rect::from_min_max(pos2(x0, row.top()), pos2(close_rect.left() - 4.0, row.bottom()));
    p.rect_filled(band, CornerRadius { nw: 22, ..Default::default() }, color);
    let text_rect = Rect::from_min_max(row.min+vec2(2.0,0.0),pos2(band.right()-8.0,row.bottom()));
    let mut job=egui::text::LayoutJob::simple(title.to_uppercase(),display_bold(21.0),pal::BG,text_rect.width().max(1.0));
    job.wrap.max_rows=1;
    let galley=p.layout_job(job);
    p.with_clip_rect(text_rect).galley(pos2(text_rect.left(),text_rect.center().y-galley.size().y/2.0),galley,pal::BG);
    ui.interact(text_rect,ui.scope_id().with(("window-title",title)),Sense::hover()).on_hover_text(title);
    let resp = ui.interact(close_rect, ui.scope_id().with(("window-close", title)), Sense::click());
    let fill = if resp.hovered() { pal::TAN } else { close_color };
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
        rail_outer_corner(&p,foot.left_bottom(),22.0,c.gamma_multiply(0.7),pal::PANEL);
        // The elbows' inner corners curve in.
        for (fill, corner, center) in [
            (c, Rect::from_min_size(pos2(rail_r, head_bottom), vec2(ELBOW, ELBOW)), pos2(rail_r + ELBOW, head_bottom + ELBOW)),
            (c.gamma_multiply(0.7), Rect::from_min_size(pos2(rail_r, foot_top - ELBOW), vec2(ELBOW, ELBOW)), pos2(rail_r + ELBOW, foot_top - ELBOW)),
        ] {
            // Carve only this elbow: the rest of the circle can reach into
            // the content and erase a button's lower-left corner.
            let p = p.with_clip_rect(corner.intersect(p.clip_rect()));
            p.rect_filled(corner, 0.0, fill);
            p.circle_filled(center, ELBOW, pal::PANEL);
        }
    }
}

/// A content panel with a header and HUD corner brackets.
pub fn panel<R>(ui: &mut Ui, title: &str, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = egui::Frame::new()
        .fill(pal::PANEL.gamma_multiply(0.85))
        .inner_margin(egui::Margin::same(12))
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

/// Shared input chrome. Apply directly to TextEdit so padding is counted once
/// and the whole field (including its inset) accepts focus.
pub fn input_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(4, 6, 12))
        .stroke(Stroke::new(1.5, pal::ORANGE.gamma_multiply(0.7)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(9, 4))
}

/// Single-line form field with compact insets and a centered text baseline.
/// Width includes the frame, so equal widths also mean equal visible bounds.
pub fn text_input(value: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(value)
        .align(Align2::LEFT_CENTER)
        .frame(input_frame())
        .min_size(vec2(0.0, if cfg!(target_os="android") {40.0} else {28.0}))
}

/// Matching multiline input, preserving top alignment and caller row counts.
pub fn text_area(value: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(value).frame(input_frame()).align(Align2::LEFT_TOP)
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


/// X11 and Windows use native window masks; Wayland uses alpha compositing.
pub fn transparent_window() -> bool {
    cfg!(target_os="linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}
