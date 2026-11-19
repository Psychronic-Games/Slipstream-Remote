//! Touch control of the remote screen (phones, tablets, touch laptops).
//!
//! egui turns the first finger into mouse events at once, so the first
//! finger of a pinch would already have clicked on the remote computer. The
//! session view reads the touches itself instead. Two ways to control:
//!
//! - **Trackpad** (the default on phones): a pointer of its own that a
//!   finger slides anywhere on the picture, like a laptop's trackpad. A tap
//!   clicks where the pointer is, tapping again and sliding drags, a long
//!   press or a two-finger tap right-clicks. Zoomed in, the view follows the
//!   pointer, so two fingers are free to scroll.
//! - **Direct touch**: a tap clicks where it lands, a long press
//!   right-clicks, one finger dragging drags (selects text, moves windows).
//!
//! In both, two fingers pinch to zoom (anywhere on the session, black bars
//! included) and drag to scroll the remote screen. Zoomed in with direct
//! touch, two fingers move the view instead.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use eframe::egui::{self, Pos2, Rect, Vec2, pos2};
use slipstream_core::proto::MouseButton;

/// How far a finger may wander and still be a tap or a long press.
const SLOP: f32 = 12.0;
const LONG_PRESS: Duration = Duration::from_millis(550);
/// The ring that fills up towards a long press appears after this.
const PRESS_SHOWN: Duration = Duration::from_millis(150);
/// Trackpad: a finger down this soon after a tap drags (tap, then slide).
const TAP_DRAG: Duration = Duration::from_millis(350);
/// Two fingers down and up this quickly, without moving, right-click.
const TWO_FINGER_TAP: Duration = Duration::from_millis(300);
/// Two fingers pinching this much (as a factor) zoom; moving this far scroll.
const PINCH: f32 = 0.08;
const SWIPE: f32 = 14.0;
const MAX_ZOOM: f32 = 8.0;

/// How fingers control the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Trackpad,
    Direct,
}

/// What a touch does on the remote computer (screen positions; the caller
/// maps them onto the remote screen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Move(Pos2),
    Button(MouseButton, bool),
    /// Scroll, in points (egui's sign: positive moves the content down).
    Scroll(Vec2),
}

/// A two-finger gesture this frame: where it is, how much it pinched
/// (factor) and moved.
#[derive(Debug, Clone, Copy)]
pub struct Gesture {
    pub center: Pos2,
    pub zoom: f32,
    pub pan: Vec2,
}

impl Gesture {
    pub fn from_egui(m: &egui::MultiTouchInfo) -> Self {
        Self { center: m.center_pos, zoom: m.zoom_delta, pan: m.translation_delta }
    }
}

/// How the picture is zoomed: `zoom` times the size that fits, its corner
/// `offset` from the fitted picture's, in fitted-picture sizes (so the view
/// survives the window or the remote screen changing size).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub offset: Vec2,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, offset: Vec2::ZERO }
    }
}

impl View {
    /// Where the picture goes: `fit` is the whole picture fitted into
    /// `area`. Zoomed in, it covers the area as far as it can; smaller than
    /// the area, it's centred.
    pub fn rect(&self, fit: Rect, area: Rect) -> Rect {
        let size = fit.size() * self.zoom;
        let mut min = fit.min + self.offset * fit.size();
        for axis in 0..2 {
            min[axis] = if size[axis] <= area.size()[axis] {
                area.center()[axis] - size[axis] / 2.0
            } else {
                min[axis].clamp(area.max[axis] - size[axis], area.min[axis])
            };
        }
        Rect::from_min_size(min, size)
    }

    fn set(&mut self, rect: Rect, fit: Rect, area: Rect) {
        self.zoom = rect.width() / fit.width();
        self.offset = (rect.min - fit.min) / fit.size();
        // Settle into the area's bounds.
        let settled = self.rect(fit, area);
        self.offset = (settled.min - fit.min) / fit.size();
    }

    /// Zooms by `factor`, keeping the point under `center` where it is.
    pub fn zoom_about(&mut self, fit: Rect, area: Rect, center: Pos2, factor: f32) {
        let r = self.rect(fit, area);
        let zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        let f = zoom / self.zoom;
        let min = center - (center - r.min) * f;
        self.set(Rect::from_min_size(min, r.size() * f), fit, area);
    }

    pub fn pan(&mut self, fit: Rect, area: Rect, delta: Vec2) {
        self.set(self.rect(fit, area).translate(delta), fit, area);
    }

    pub fn zoomed(&self) -> bool {
        self.zoom > 1.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Lock {
    Zoom,
    Scroll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Idle,
    /// One finger down, not yet a tap, a drag or a long press. `drag`: on
    /// the trackpad, it came right after a tap, so sliding drags.
    Pending { at: Pos2, since: Instant, on_picture: bool, drag: bool },
    /// The left button is down: following the finger (direct touch) or
    /// the pointer it slides (trackpad).
    Dragging,
    /// Trackpad: the finger slides the pointer.
    Moving,
    /// Two or more fingers: zoom or scroll, once it's clear which.
    Gesture { since: Instant, center: Pos2, lock: Option<Lock>, zoom: f32, pan: Vec2 },
    /// Finished (a tap, a long press, a gesture with fingers still down), or
    /// begun off the session: nothing more until every finger is up.
    Spent,
}

#[derive(Debug)]
pub struct Touch {
    state: State,
    /// Where each finger is (by `TouchId`).
    fingers: HashMap<u64, Pos2>,
    /// The trackpad pointer, as a fraction of the picture's size.
    pointer: Pos2,
    /// A finger has touched the picture (the trackpad pointer shows).
    used: bool,
    /// When the trackpad was last tapped (tap, then slide to drag).
    last_tap: Option<Instant>,
}

impl Default for Touch {
    fn default() -> Self {
        Self { state: State::Idle, fingers: HashMap::new(), pointer: pos2(0.5, 0.5), used: false, last_tap: None }
    }
}

impl Touch {
    /// Handles this frame's touch events. `ours` says whether a position is
    /// on the session (a touch that starts elsewhere is left to the rest of
    /// the window). The view zooms the picture fitted into the area (`fit`,
    /// `area`). Returns the actions for the remote computer and whether
    /// touch is in use, in which case the pointer events egui made from it
    /// are to be ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn handle(
        &mut self,
        events: &[egui::Event],
        gesture: Option<Gesture>,
        now: Instant,
        mode: Mode,
        ours: impl Fn(Pos2) -> bool,
        view: &mut View,
        (fit, area): (Rect, Rect),
    ) -> (Vec<Action>, bool) {
        let mut out = Vec::new();
        let mut touched = false;
        let trackpad = mode == Mode::Trackpad;
        for ev in events {
            let egui::Event::Touch { id, phase, pos, .. } = *ev else { continue };
            touched = true;
            match phase {
                egui::TouchPhase::Start => {
                    self.fingers.insert(id.0, pos);
                    self.state = match self.state {
                        State::Idle if self.fingers.len() == 1 => {
                            if ours(pos) {
                                self.used = true;
                                let on_picture = view.rect(fit, area).contains(pos);
                                let drag = trackpad && self.last_tap.is_some_and(|t| now.duration_since(t) < TAP_DRAG);
                                State::Pending { at: pos, since: now, on_picture, drag }
                            } else {
                                State::Spent
                            }
                        }
                        State::Pending { .. } | State::Moving => self.gesture(now),
                        State::Dragging => {
                            out.push(Action::Button(MouseButton::Left, false));
                            self.gesture(now)
                        }
                        other => other,
                    };
                }
                egui::TouchPhase::Move => {
                    let prev = self.fingers.insert(id.0, pos).unwrap_or(pos);
                    match self.state {
                        State::Pending { at, on_picture, drag, .. } if self.fingers.len() == 1 && at.distance(pos) > SLOP => {
                            if trackpad {
                                if drag {
                                    out.extend([self.pointer_move(view, fit, area), Action::Button(MouseButton::Left, true)]);
                                }
                                self.state = if drag { State::Dragging } else { State::Moving };
                                out.push(self.slide(pos - at, view, fit, area));
                            } else if on_picture {
                                out.extend([Action::Move(at), Action::Button(MouseButton::Left, true), Action::Move(pos)]);
                                self.state = State::Dragging;
                            } else {
                                self.state = State::Spent;
                            }
                        }
                        State::Moving | State::Dragging if trackpad && self.fingers.len() == 1 => {
                            out.push(self.slide(pos - prev, view, fit, area));
                        }
                        State::Dragging => out.push(Action::Move(pos)),
                        _ => {}
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    let fingers = self.fingers.len();
                    self.fingers.remove(&id.0);
                    let ended = phase == egui::TouchPhase::End;
                    match self.state {
                        State::Pending { at, on_picture, .. } if ended => {
                            if trackpad {
                                out.push(self.pointer_move(view, fit, area));
                                self.last_tap = Some(now);
                            } else if on_picture {
                                out.push(Action::Move(at));
                            }
                            if trackpad || on_picture {
                                out.extend([Action::Button(MouseButton::Left, true), Action::Button(MouseButton::Left, false)]);
                            }
                            self.state = State::Spent;
                        }
                        State::Dragging => {
                            out.push(Action::Button(MouseButton::Left, false));
                            self.state = State::Spent;
                        }
                        // Two fingers tapped together: a right click.
                        State::Gesture { since, center, lock: None, .. }
                            if ended && fingers == 2 && now.duration_since(since) < TWO_FINGER_TAP =>
                        {
                            let at = if trackpad {
                                Some(self.pointer_move(view, fit, area))
                            } else {
                                view.rect(fit, area).contains(center).then_some(Action::Move(center))
                            };
                            if let Some(at) = at {
                                out.extend([at, Action::Button(MouseButton::Right, true), Action::Button(MouseButton::Right, false)]);
                            }
                            self.state = State::Spent;
                        }
                        State::Pending { .. } | State::Moving | State::Gesture { .. } => self.state = State::Spent,
                        _ => {}
                    }
                    if self.fingers.is_empty() {
                        self.state = State::Idle;
                    }
                }
            }
        }
        if let State::Pending { at, since, on_picture, .. } = self.state
            && now.duration_since(since) >= LONG_PRESS
        {
            let at = if trackpad { Some(self.pointer_move(view, fit, area)) } else { on_picture.then_some(Action::Move(at)) };
            if let Some(at) = at {
                out.extend([at, Action::Button(MouseButton::Right, true), Action::Button(MouseButton::Right, false)]);
            }
            self.state = State::Spent;
        }
        if let State::Gesture { lock, zoom, pan, .. } = &mut self.state
            && let Some(g) = gesture
        {
            // Pinching or moving: decided once it's clear, and kept.
            *zoom *= g.zoom;
            *pan += g.pan;
            if lock.is_none() {
                if zoom.ln().abs() > PINCH {
                    *lock = Some(Lock::Zoom);
                } else if pan.length() > SWIPE {
                    *lock = Some(Lock::Scroll);
                }
            }
            match *lock {
                Some(Lock::Zoom) => {
                    view.zoom_about(fit, area, g.center, g.zoom);
                    if g.pan != Vec2::ZERO {
                        view.pan(fit, area, g.pan);
                    }
                }
                Some(Lock::Scroll) if g.pan != Vec2::ZERO => {
                    if !trackpad && view.zoomed() {
                        view.pan(fit, area, g.pan);
                    } else {
                        out.push(Action::Scroll(g.pan));
                    }
                }
                _ => {}
            }
        }
        (out, touched || self.state != State::Idle)
    }

    fn gesture(&self, now: Instant) -> State {
        let n = self.fingers.len().max(1) as f32;
        let center = self.fingers.values().fold(Pos2::ZERO, |sum, p| sum + p.to_vec2() / n);
        State::Gesture { since: now, center, lock: None, zoom: 1.0, pan: Vec2::ZERO }
    }

    /// Slides the trackpad pointer by a finger's movement: slow movements
    /// finely, fast ones further. Zoomed in, the same movement covers less
    /// of the remote screen, and the view follows the pointer.
    fn slide(&mut self, delta: Vec2, view: &mut View, fit: Rect, area: Rect) -> Action {
        let gain = (0.7 + delta.length() * 0.08).clamp(0.7, 2.4);
        let size = view.rect(fit, area).size();
        let p = self.pointer + delta * gain / size;
        self.pointer = pos2(p.x.clamp(0.0, 1.0), p.y.clamp(0.0, 1.0));
        if view.zoomed() {
            let rect = view.rect(fit, area);
            let at = rect.min + self.pointer.to_vec2() * rect.size();
            let inner = area.shrink((area.width().min(area.height()) * 0.12).min(48.0));
            let shift = Vec2::new(
                (inner.min.x - at.x).max(0.0) + (inner.max.x - at.x).min(0.0),
                (inner.min.y - at.y).max(0.0) + (inner.max.y - at.y).min(0.0),
            );
            if shift != Vec2::ZERO {
                view.pan(fit, area, shift);
            }
        }
        self.pointer_move(view, fit, area)
    }

    fn pointer_move(&self, view: &View, fit: Rect, area: Rect) -> Action {
        Action::Move(self.pointer_pos(view.rect(fit, area)))
    }

    /// Where the trackpad pointer is on the picture `rect`.
    pub fn pointer_pos(&self, rect: Rect) -> Pos2 {
        rect.min + self.pointer.to_vec2() * rect.size()
    }

    /// A finger has touched the session (show the trackpad pointer).
    pub fn used(&self) -> bool {
        self.used
    }

    /// A finger is down and may yet become a long press (keep redrawing).
    pub fn waiting(&self) -> bool {
        matches!(self.state, State::Pending { .. })
    }

    /// A finger held still: where, and how far towards a long press (0–1).
    pub fn pressing(&self, now: Instant) -> Option<(Pos2, f32)> {
        let State::Pending { at, since, .. } = self.state else { return None };
        let held = now.duration_since(since).checked_sub(PRESS_SHOWN)?;
        Some((at, held.as_secs_f32() / (LONG_PRESS - PRESS_SHOWN).as_secs_f32()))
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::pos2;

    use super::*;

    fn ev(id: u64, phase: egui::TouchPhase, x: f32, y: f32) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos: pos2(x, y), force: None }
    }

    const AREA: Rect = Rect { min: pos2(0.0, 0.0), max: pos2(400.0, 800.0) };
    /// A 16:9 picture fitted into the portrait area.
    const FIT: Rect = Rect { min: pos2(0.0, 287.5), max: pos2(400.0, 512.5) };

    fn run_mode(t: &mut Touch, view: &mut View, events: &[egui::Event], gesture: Option<Gesture>, now: Instant, mode: Mode) -> Vec<Action> {
        t.handle(events, gesture, now, mode, |p| AREA.contains(p), view, (FIT, AREA)).0
    }

    fn run(t: &mut Touch, view: &mut View, events: &[egui::Event], gesture: Option<Gesture>, now: Instant) -> Vec<Action> {
        run_mode(t, view, events, gesture, now, Mode::Direct)
    }

    fn pad(t: &mut Touch, view: &mut View, events: &[egui::Event], gesture: Option<Gesture>, now: Instant) -> Vec<Action> {
        run_mode(t, view, events, gesture, now, Mode::Trackpad)
    }

    use egui::TouchPhase::{End, Move, Start};
    use MouseButton::{Left, Right};

    fn pinch(zoom: f32) -> Option<Gesture> {
        Some(Gesture { center: pos2(200.0, 400.0), zoom, pan: Vec2::ZERO })
    }

    #[test]
    fn tap_clicks_where_it_landed() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        assert!(run(&mut t, &mut v, &[ev(1, Start, 100.0, 400.0)], None, now).is_empty());
        let out = run(&mut t, &mut v, &[ev(1, Move, 103.0, 402.0), ev(1, End, 103.0, 402.0)], None, now);
        assert_eq!(out, [Action::Move(pos2(100.0, 400.0)), Action::Button(Left, true), Action::Button(Left, false)]);
    }

    #[test]
    fn long_press_right_clicks_once_with_a_filling_ring() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 100.0, 400.0)], None, now);
        assert!(t.waiting());
        assert!(t.pressing(now).is_none(), "not straight away");
        let (at, half) = t.pressing(now + PRESS_SHOWN + (LONG_PRESS - PRESS_SHOWN) / 2).unwrap();
        assert_eq!(at, pos2(100.0, 400.0));
        assert!((half - 0.5).abs() < 0.01);
        let out = run(&mut t, &mut v, &[], None, now + LONG_PRESS);
        assert_eq!(out, [Action::Move(pos2(100.0, 400.0)), Action::Button(Right, true), Action::Button(Right, false)]);
        assert!(run(&mut t, &mut v, &[ev(1, End, 100.0, 400.0)], None, now + LONG_PRESS * 2).is_empty());
    }

    #[test]
    fn one_finger_drags_with_the_button_down() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 100.0, 400.0)], None, now);
        let out = run(&mut t, &mut v, &[ev(1, Move, 150.0, 400.0)], None, now);
        assert_eq!(out, [Action::Move(pos2(100.0, 400.0)), Action::Button(Left, true), Action::Move(pos2(150.0, 400.0))]);
        let out = run(&mut t, &mut v, &[ev(1, End, 150.0, 400.0)], None, now);
        assert_eq!(out, [Action::Button(Left, false)]);
    }

    #[test]
    fn pinch_zooms_without_clicking() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 150.0, 400.0)], None, now);
        let out = run(&mut t, &mut v, &[ev(2, Start, 250.0, 400.0)], None, now);
        assert!(out.is_empty());
        // Too little to tell yet, then clearly a pinch.
        run(&mut t, &mut v, &[], pinch(1.05), now);
        assert_eq!(v.zoom, 1.0);
        let out = run(&mut t, &mut v, &[ev(1, Move, 100.0, 400.0), ev(2, Move, 300.0, 400.0)], pinch(2.0), now);
        assert!(out.is_empty());
        assert!((v.zoom - 2.0).abs() < 1e-4);
        // The point under the fingers stays put: the picture's centre.
        assert!((v.rect(FIT, AREA).center() - pos2(200.0, 400.0)).length() < 0.01);
        let out = run(&mut t, &mut v, &[ev(1, End, 100.0, 400.0), ev(2, End, 300.0, 400.0)], None, now + LONG_PRESS);
        assert!(out.is_empty(), "lifting the fingers mustn't click: {out:?}");
    }

    #[test]
    fn a_pinch_can_start_on_the_black_bars() {
        // Portrait phone: most of the screen is above and below the picture.
        for mode in [Mode::Direct, Mode::Trackpad] {
            let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
            run_mode(&mut t, &mut v, &[ev(1, Start, 150.0, 100.0), ev(2, Start, 250.0, 700.0)], None, now, mode);
            run_mode(&mut t, &mut v, &[], pinch(3.0), now, mode);
            assert!(v.zoomed(), "{mode:?}");
        }
    }

    #[test]
    fn a_pinch_after_a_drag_began_lets_go_of_the_button() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 100.0, 400.0), ev(1, Move, 140.0, 400.0)], None, now);
        let out = run(&mut t, &mut v, &[ev(2, Start, 250.0, 400.0)], None, now);
        assert_eq!(out, [Action::Button(Left, false)]);
    }

    #[test]
    fn two_fingers_scroll_unless_zoomed_then_pan() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 150.0, 400.0), ev(2, Start, 250.0, 400.0)], None, now);
        let g = Gesture { center: pos2(200.0, 420.0), zoom: 1.0, pan: Vec2::new(0.0, 20.0) };
        assert_eq!(run(&mut t, &mut v, &[], Some(g), now), [Action::Scroll(Vec2::new(0.0, 20.0))]);
        v.zoom_about(FIT, AREA, pos2(200.0, 400.0), 3.0);
        let before = v.rect(FIT, AREA);
        let g = Gesture { center: pos2(200.0, 400.0), zoom: 1.0, pan: Vec2::new(30.0, 0.0) };
        assert!(run(&mut t, &mut v, &[], Some(g), now).is_empty());
        assert_eq!(v.rect(FIT, AREA).min.x, before.min.x + 30.0);
    }

    #[test]
    fn trackpad_scrolls_with_two_fingers_even_zoomed_in() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        v.zoom_about(FIT, AREA, pos2(200.0, 400.0), 3.0);
        pad(&mut t, &mut v, &[ev(1, Start, 150.0, 400.0), ev(2, Start, 250.0, 400.0)], None, now);
        let g = Gesture { center: pos2(200.0, 420.0), zoom: 1.0, pan: Vec2::new(0.0, 20.0) };
        assert_eq!(pad(&mut t, &mut v, &[], Some(g), now), [Action::Scroll(Vec2::new(0.0, 20.0))]);
    }

    #[test]
    fn two_finger_tap_right_clicks() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 150.0, 400.0), ev(2, Start, 250.0, 400.0)], None, now);
        let out = run(&mut t, &mut v, &[ev(1, End, 150.0, 400.0), ev(2, End, 250.0, 400.0)], None, now + Duration::from_millis(120));
        assert_eq!(out, [Action::Move(pos2(200.0, 400.0)), Action::Button(Right, true), Action::Button(Right, false)]);
        // A slow one is not a tap.
        run(&mut t, &mut v, &[ev(1, Start, 150.0, 400.0), ev(2, Start, 250.0, 400.0)], None, now);
        assert!(run(&mut t, &mut v, &[ev(1, End, 150.0, 400.0), ev(2, End, 250.0, 400.0)], None, now + TWO_FINGER_TAP * 2).is_empty());
    }

    #[test]
    fn trackpad_slides_the_pointer_and_taps_click_where_it_is() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        let centre = pos2(200.0, 400.0);
        assert_eq!(t.pointer_pos(v.rect(FIT, AREA)), centre);
        // A tap anywhere (here on the black bar) clicks at the pointer.
        pad(&mut t, &mut v, &[ev(1, Start, 30.0, 60.0)], None, now);
        let out = pad(&mut t, &mut v, &[ev(1, End, 30.0, 60.0)], None, now);
        assert_eq!(out, [Action::Move(centre), Action::Button(Left, true), Action::Button(Left, false)]);
        // Sliding moves the pointer by about the finger's movement, without clicking.
        let later = now + Duration::from_secs(1);
        pad(&mut t, &mut v, &[ev(2, Start, 100.0, 100.0)], None, later);
        let mut moved = Vec::new();
        for x in (104..=160).step_by(4) {
            moved.extend(pad(&mut t, &mut v, &[ev(2, Move, x as f32, 100.0)], None, later));
        }
        assert!(moved.iter().all(|a| matches!(a, Action::Move(_))), "{moved:?}");
        let Some(Action::Move(at)) = moved.last() else { panic!() };
        assert!(at.x > centre.x + 30.0 && at.x < centre.x + 80.0 && (at.y - centre.y).abs() < 0.01, "{at:?}");
        assert!(pad(&mut t, &mut v, &[ev(2, End, 160.0, 100.0)], None, later).is_empty());
    }

    #[test]
    fn trackpad_tap_then_slide_drags() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        pad(&mut t, &mut v, &[ev(1, Start, 100.0, 100.0), ev(1, End, 100.0, 100.0)], None, now);
        let soon = now + Duration::from_millis(150);
        pad(&mut t, &mut v, &[ev(2, Start, 100.0, 100.0)], None, soon);
        let out = pad(&mut t, &mut v, &[ev(2, Move, 130.0, 100.0)], None, soon);
        assert!(matches!(out[..], [Action::Move(_), Action::Button(Left, true), Action::Move(_)]), "{out:?}");
        assert_eq!(pad(&mut t, &mut v, &[ev(2, End, 130.0, 100.0)], None, soon), [Action::Button(Left, false)]);
    }

    #[test]
    fn trackpad_view_follows_the_pointer_when_zoomed() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        v.zoom_about(FIT, AREA, pos2(200.0, 400.0), 4.0);
        let before = v.rect(FIT, AREA);
        pad(&mut t, &mut v, &[ev(1, Start, 100.0, 100.0)], None, now);
        for x in (110..=390).step_by(10) {
            pad(&mut t, &mut v, &[ev(1, Move, x as f32, 100.0)], None, now);
        }
        let after = v.rect(FIT, AREA);
        assert!(after.min.x < before.min.x, "the view moved left to follow: {before:?} → {after:?}");
        assert!(AREA.contains(t.pointer_pos(after)));
    }

    #[test]
    fn the_view_stays_on_the_picture() {
        let mut v = View::default();
        assert_eq!(v.rect(FIT, AREA), FIT);
        v.zoom_about(FIT, AREA, pos2(10.0, 300.0), 4.0);
        // Wider than the area: no gap at the left edge.
        let r = v.rect(FIT, AREA);
        assert!(r.min.x <= AREA.min.x + 1e-3 && r.max.x >= AREA.max.x - 1e-3);
        v.pan(FIT, AREA, Vec2::new(5000.0, 0.0));
        assert_eq!(v.rect(FIT, AREA).min.x, AREA.min.x);
        // Zooming out past the fitted size stops there.
        v.zoom_about(FIT, AREA, pos2(200.0, 400.0), 0.01);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.rect(FIT, AREA), FIT);
    }

    #[test]
    fn touches_that_start_off_the_session_are_left_alone() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        let off = |p: Pos2| p.y > 50.0;
        let out = t.handle(&[ev(1, Start, 100.0, 20.0), ev(1, End, 100.0, 20.0)], None, now, Mode::Direct, off, &mut v, (FIT, AREA)).0;
        assert!(out.is_empty());
        // Direct touch: a tap on the black bars clicks nothing either.
        let out = run(&mut t, &mut v, &[ev(1, Start, 100.0, 100.0), ev(1, End, 100.0, 100.0)], None, now);
        assert!(out.is_empty());
    }
}
