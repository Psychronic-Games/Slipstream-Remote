//! Reactions to Subspace chat messages: colour Fluent Emoji (MIT, see
//! assets/reactions/LICENSE.txt), drawn once from SVG into textures. The
//! wire carries an index into this list (`slipstream_core::meeting::REACTIONS`
//! has the same length); the first six are offered straight away.
use crate::gui::pal;
use eframe::egui::{self, ColorImage, RichText, TextureHandle, TextureOptions, Ui, vec2};
use std::sync::Arc;

const SVG: [&[u8]; 40] = [
    include_bytes!("../../../assets/reactions/00-thumbs_up.svg"),
    include_bytes!("../../../assets/reactions/01-red_heart.svg"),
    include_bytes!("../../../assets/reactions/02-party_popper.svg"),
    include_bytes!("../../../assets/reactions/03-face_with_tears_of_joy.svg"),
    include_bytes!("../../../assets/reactions/04-face_with_open_mouth.svg"),
    include_bytes!("../../../assets/reactions/05-clapping_hands.svg"),
    include_bytes!("../../../assets/reactions/06-thumbs_down.svg"),
    include_bytes!("../../../assets/reactions/07-fire.svg"),
    include_bytes!("../../../assets/reactions/08-hundred_points.svg"),
    include_bytes!("../../../assets/reactions/09-rocket.svg"),
    include_bytes!("../../../assets/reactions/10-eyes.svg"),
    include_bytes!("../../../assets/reactions/11-thinking_face.svg"),
    include_bytes!("../../../assets/reactions/12-smiling_face_with_heart-eyes.svg"),
    include_bytes!("../../../assets/reactions/13-grinning_face_with_big_eyes.svg"),
    include_bytes!("../../../assets/reactions/14-rolling_on_the_floor_laughing.svg"),
    include_bytes!("../../../assets/reactions/15-winking_face.svg"),
    include_bytes!("../../../assets/reactions/16-smiling_face_with_sunglasses.svg"),
    include_bytes!("../../../assets/reactions/17-star-struck.svg"),
    include_bytes!("../../../assets/reactions/18-folded_hands.svg"),
    include_bytes!("../../../assets/reactions/19-raising_hands.svg"),
    include_bytes!("../../../assets/reactions/20-flexed_biceps.svg"),
    include_bytes!("../../../assets/reactions/21-ok_hand.svg"),
    include_bytes!("../../../assets/reactions/22-waving_hand.svg"),
    include_bytes!("../../../assets/reactions/23-crying_face.svg"),
    include_bytes!("../../../assets/reactions/24-loudly_crying_face.svg"),
    include_bytes!("../../../assets/reactions/25-angry_face.svg"),
    include_bytes!("../../../assets/reactions/26-exploding_head.svg"),
    include_bytes!("../../../assets/reactions/27-partying_face.svg"),
    include_bytes!("../../../assets/reactions/28-face_with_rolling_eyes.svg"),
    include_bytes!("../../../assets/reactions/29-check_mark_button.svg"),
    include_bytes!("../../../assets/reactions/30-cross_mark.svg"),
    include_bytes!("../../../assets/reactions/31-warning.svg"),
    include_bytes!("../../../assets/reactions/32-light_bulb.svg"),
    include_bytes!("../../../assets/reactions/33-star.svg"),
    include_bytes!("../../../assets/reactions/34-trophy.svg"),
    include_bytes!("../../../assets/reactions/35-hot_beverage.svg"),
    include_bytes!("../../../assets/reactions/36-vulcan_salute.svg"),
    include_bytes!("../../../assets/reactions/37-alien.svg"),
    include_bytes!("../../../assets/reactions/38-robot.svg"),
    include_bytes!("../../../assets/reactions/39-sparkles.svg"),
];
pub const NAMES: [&str; 40] = ["thumbs up", "red heart", "party popper", "face with tears of joy", "face with open mouth", "clapping hands", "thumbs down", "fire", "hundred points", "rocket", "eyes", "thinking face", "smiling face with heart-eyes", "grinning face with big eyes", "rolling on the floor laughing", "winking face", "smiling face with sunglasses", "star-struck", "folded hands", "raising hands", "flexed biceps", "ok hand", "waving hand", "crying face", "loudly crying face", "angry face", "exploding head", "partying face", "face with rolling eyes", "check mark button", "cross mark", "warning", "light bulb", "star", "trophy", "hot beverage", "vulcan salute", "alien", "robot", "sparkles"];
/// Shown first; the rest behind +.
pub const QUICK: usize = 6;
const PIXELS: u32 = 64;

/// Each reaction's texture (None if its SVG didn't draw), made on first use.
fn textures(ctx: &egui::Context) -> Arc<Vec<Option<TextureHandle>>> {
    let id = egui::Id::unique("meeting-reaction-textures");
    if let Some(done) = ctx.data(|d| d.get_temp::<Arc<Vec<Option<TextureHandle>>>>(id)) {
        return done;
    }
    let made: Arc<Vec<_>> = Arc::new(
        SVG.iter()
            .enumerate()
            .map(|(i, svg)| rasterize(svg).map(|image| ctx.load_texture(format!("reaction-{i}"), image, TextureOptions::LINEAR)))
            .collect(),
    );
    ctx.data_mut(|d| d.insert_temp(id, made.clone()));
    made
}

fn rasterize(svg: &[u8]) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(PIXELS, PIXELS)?;
    let size = tree.size();
    let scale = PIXELS as f32 / size.width().max(size.height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied([PIXELS as usize, PIXELS as usize], pixmap.data()))
}

/// The reaction's picture at `size` points.
pub fn image(ui: &mut Ui, index: u8, size: f32) -> egui::Response {
    match textures(ui.ctx()).get(index as usize).cloned().flatten() {
        Some(texture) => ui.add(egui::Image::new(&texture).fit_to_exact_size(vec2(size, size))),
        None => ui.label("?"),
    }
}

/// A clickable reaction picture.
fn choice(ui: &mut Ui, index: usize, size: f32) -> bool {
    let texture = textures(ui.ctx()).get(index).cloned().flatten();
    let button = match texture {
        Some(t) => egui::Button::image(egui::Image::new(&t).fit_to_exact_size(vec2(size, size))),
        None => egui::Button::new("?"),
    };
    ui.add(button.frame(false)).on_hover_text(NAMES[index]).clicked()
}

/// The picker: the quick six, and + for all of them. Returns the choice.
pub fn picker(ui: &mut Ui, expanded: &mut bool) -> Option<u8> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        for i in 0..QUICK {
            if choice(ui, i, 26.0) {
                chosen = Some(i as u8);
            }
        }
        if !*expanded && ui.add(egui::Button::new(RichText::new("+").size(20.0).color(pal::TAN)).frame(false)).on_hover_text("More reactions").clicked() {
            *expanded = true;
        }
    });
    if *expanded {
        ui.separator();
        egui::Grid::new("more-reactions").spacing(vec2(4.0, 4.0)).show(ui, |ui| {
            for i in QUICK..SVG.len() {
                if choice(ui, i, 26.0) {
                    chosen = Some(i as u8);
                }
                if (i - QUICK + 1).is_multiple_of(8) {
                    ui.end_row();
                }
            }
        });
    }
    chosen
}
