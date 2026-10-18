//! Android-only parts of the GUI:
//!
//! - the insets the activity reports (status and navigation bars, the
//!   camera cutout, the soft keyboard), which egui keeps content clear of
//!   (`Context::content_rect`); winit only reads them on iOS;
//! - typing on the soft keyboard: GameActivity hands the keyboard's text
//!   over as a whole (the IME edits a copy of it), which winit drops, so
//!   it's read here and passed on as typed text and backspaces. The field
//!   being typed into is kept in view above the keyboard.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU32, Ordering},
};

use android_activity::AndroidApp;
use eframe::egui;

/// Left, top, right, bottom, in physical pixels.
static INSETS: Mutex<[f32; 4]> = Mutex::new([0.0; 4]);
static APP: OnceLock<(AndroidApp, egui::Context)> = OnceLock::new();
/// The keyboard's Enter/Done presses not yet passed on.
static ENTERS: AtomicU32 = AtomicU32::new(0);
static TYPED: Mutex<Typed> = Mutex::new(Typed { sent: String::new(), focus: None });
/// What [`keep_focus_in_view`] last saw: the focused widget, the visible area.
static IN_VIEW: Mutex<Option<(Option<egui::Id>, egui::Rect)>> = Mutex::new(None);

/// The keyboard's text as passed on so far, for the field that has focus.
struct Typed {
    sent: String,
    focus: Option<egui::Id>,
}

pub fn install(app: AndroidApp, ctx: &egui::Context) {
    let _ = APP.set((app, ctx.clone()));
}

fn wake() {
    if let Some((_, ctx)) = APP.get() {
        ctx.request_repaint();
    }
}

/// `MainActivity.onInsets`: the screen area that's covered changed.
pub fn set_insets(px: [f32; 4]) {
    *INSETS.lock().unwrap_or_else(|e| e.into_inner()) = px;
    wake();
}

/// `MainActivity.onEditorAction`: Enter (or Done) on the soft keyboard.
pub fn editor_action() {
    ENTERS.fetch_add(1, Ordering::Relaxed);
    wake();
}

/// Adds this frame's insets and soft-keyboard typing to egui's input.
pub fn input(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let ppp = ctx.pixels_per_point();
    let [left, top, right, bottom] = INSETS.lock().unwrap_or_else(|e| e.into_inner()).map(|v| v / ppp);
    raw.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left, right, top, bottom }));

    let Some((app, _)) = APP.get() else { return };
    let mut typed = TYPED.lock().unwrap_or_else(|e| e.into_inner());
    let focus = ctx.memory(|m| m.focused());
    // winit asks for the keyboard "implicitly", which Android may ignore,
    // and only when a field first takes focus. Ask outright, and again when
    // the field is tapped after the keyboard was put away.
    let tapped_field = focus.and_then(|id| ctx.read_response(id)).is_some_and(|field| {
        raw.events.iter().any(|e| matches!(e, egui::Event::PointerButton { pos, pressed: false, .. } if field.rect.contains(*pos)))
    });
    if ctx.text_edit_focused() && (focus != typed.focus || tapped_field) {
        app.show_soft_input(false);
    }
    let text = app.text_input_state().text;
    if focus != typed.focus {
        // Another field: what the keyboard holds so far was typed elsewhere;
        // only what it adds from here on goes to this field.
        typed.focus = focus;
        typed.sent = text;
    } else if text != typed.sent {
        // Suggestions and autocorrect rewrite the end of the text: erase
        // back to where it still matches, then type the rest.
        let same = typed.sent.chars().zip(text.chars()).take_while(|(a, b)| a == b).count();
        for _ in same..typed.sent.chars().count() {
            key(raw, egui::Key::Backspace);
        }
        let new: String = text.chars().skip(same).collect();
        for (i, line) in new.split('\n').enumerate() {
            if i > 0 {
                key(raw, egui::Key::Enter);
            }
            if !line.is_empty() {
                raw.events.push(egui::Event::Text(line.to_string()));
            }
        }
        typed.sent = text;
    }
    for _ in 0..ENTERS.swap(0, Ordering::Relaxed) {
        key(raw, egui::Key::Enter);
    }
}

fn key(raw: &mut egui::RawInput, key: egui::Key) {
    for pressed in [true, false] {
        raw.events.push(egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: egui::Modifiers::NONE });
    }
}

/// When the keyboard opens or another field takes focus, scrolls the
/// focused field into view if it's covered. Call inside the scroll area,
/// which takes only requests made within it.
pub fn keep_focus_in_view(ui: &egui::Ui) {
    let ctx = ui.ctx();
    let now = (ctx.memory(|m| m.focused()), ui.clip_rect());
    let mut seen = IN_VIEW.lock().unwrap_or_else(|e| e.into_inner());
    if seen.replace(now) == Some(now) {
        return;
    }
    if let (Some(id), visible) = now
        && let Some(field) = ctx.read_response(id)
        && !visible.contains_rect(field.rect)
    {
        field.scroll_to_me(Some(egui::Align::Center));
    }
}
