//! Touch control of the remote screen (phones, tablets, touch laptops).
//!
//! egui turns the first finger into mouse events at once, so the first
//! finger of a pinch would already have clicked on the remote computer. The
//! session view reads the touches itself instead:
//!
//! - a tap clicks, a long press right-clicks, one finger dragging drags
//!   (selects text, moves windows);
//! - two fingers pinch to zoom the picture around them and drag to pan it;
//!   while it isn't zoomed in, dragging two fingers scrolls the remote
//!   screen.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use eframe::egui::{self, Pos2, Rect, Vec2};
use slipstream_core::proto::MouseButton;

/// How far a finger may wander and still be a tap or a long press.
const SLOP: f32 = 12.0;
const LONG_PRESS: Duration = Duration::from_millis(550);
const MAX_ZOOM: f32 = 8.0;

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
enum State {
    Idle,
    /// One finger down, not yet a tap, a drag or a long press.
    Pending { at: Pos2, since: Instant },
    /// One finger dragging with the left button down.
    Dragging,
    /// Two or more fingers: zoom, pan, scroll.
    Gesture,
    /// Finished (a long press, or a gesture with fingers still down), or
    /// begun outside the picture: nothing more until every finger is up.
    Spent,
}

#[derive(Debug)]
pub struct Touch {
    state: State,
    /// Where each finger is (by `TouchId`).
    fingers: HashMap<u64, Pos2>,
}

impl Default for Touch {
    fn default() -> Self {
        Self { state: State::Idle, fingers: HashMap::new() }
    }
}

impl Touch {
    /// Handles this frame's touch events. `on_picture` says whether a
    /// position is on the remote picture (a touch that starts elsewhere is
    /// left to the rest of the window). The view zooms the picture fitted
    /// into the area (`fit`, `area`). Returns the actions for the remote
    /// computer and whether touch is in use, in which case the pointer
    /// events egui made from it are to be ignored.
    pub fn handle(
        &mut self,
        events: &[egui::Event],
        gesture: Option<Gesture>,
        now: Instant,
        on_picture: impl Fn(Pos2) -> bool,
        view: &mut View,
        (fit, area): (Rect, Rect),
    ) -> (Vec<Action>, bool) {
        let mut out = Vec::new();
        let mut touched = false;
        for ev in events {
            let egui::Event::Touch { id, phase, pos, .. } = *ev else { continue };
            touched = true;
            match phase {
                egui::TouchPhase::Start => {
                    self.fingers.insert(id.0, pos);
                    self.state = match self.state {
                        State::Idle if self.fingers.len() == 1 => {
                            if on_picture(pos) {
                                State::Pending { at: pos, since: now }
                            } else {
                                State::Spent
                            }
                        }
                        State::Pending { .. } => State::Gesture,
                        State::Dragging => {
                            out.push(Action::Button(MouseButton::Left, false));
                            State::Gesture
                        }
                        other => other,
                    };
                }
                egui::TouchPhase::Move => {
                    self.fingers.insert(id.0, pos);
                    match self.state {
                        State::Pending { at, .. } if self.fingers.len() == 1 && at.distance(pos) > SLOP => {
                            out.extend([Action::Move(at), Action::Button(MouseButton::Left, true), Action::Move(pos)]);
                            self.state = State::Dragging;
                        }
                        State::Dragging => out.push(Action::Move(pos)),
                        _ => {}
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    self.fingers.remove(&id.0);
                    match self.state {
                        State::Pending { at, .. } if phase == egui::TouchPhase::End => {
                            out.extend([Action::Move(at), Action::Button(MouseButton::Left, true), Action::Button(MouseButton::Left, false)]);
                            self.state = State::Spent;
                        }
                        State::Pending { .. } => self.state = State::Spent,
                        State::Dragging => {
                            out.push(Action::Button(MouseButton::Left, false));
                            self.state = State::Spent;
                        }
                        _ => {}
                    }
                    if self.fingers.is_empty() {
                        self.state = State::Idle;
                    }
                }
            }
        }
        if let State::Pending { at, since } = self.state
            && now.duration_since(since) >= LONG_PRESS
        {
            out.extend([Action::Move(at), Action::Button(MouseButton::Right, true), Action::Button(MouseButton::Right, false)]);
            self.state = State::Spent;
        }
        if self.state == State::Gesture
            && let Some(g) = gesture
        {
            if g.zoom != 1.0 {
                view.zoom_about(fit, area, g.center, g.zoom);
            }
            if g.pan != Vec2::ZERO {
                if view.zoomed() {
                    view.pan(fit, area, g.pan);
                } else {
                    out.push(Action::Scroll(g.pan));
                }
            }
        }
        (out, touched || self.state != State::Idle)
    }

    /// A finger is down and may yet become a long press (keep redrawing).
    pub fn waiting(&self) -> bool {
        matches!(self.state, State::Pending { .. })
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

    fn run(t: &mut Touch, view: &mut View, events: &[egui::Event], gesture: Option<Gesture>, now: Instant) -> Vec<Action> {
        t.handle(events, gesture, now, |p| FIT.contains(p), view, (FIT, AREA)).0
    }

    use egui::TouchPhase::{End, Move, Start};
    use MouseButton::{Left, Right};

    #[test]
    fn tap_clicks_where_it_landed() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        assert!(run(&mut t, &mut v, &[ev(1, Start, 100.0, 400.0)], None, now).is_empty());
        let out = run(&mut t, &mut v, &[ev(1, Move, 103.0, 402.0), ev(1, End, 103.0, 402.0)], None, now);
        assert_eq!(out, [Action::Move(pos2(100.0, 400.0)), Action::Button(Left, true), Action::Button(Left, false)]);
    }

    #[test]
    fn long_press_right_clicks_once() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        run(&mut t, &mut v, &[ev(1, Start, 100.0, 400.0)], None, now);
        assert!(t.waiting());
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
        let g = Gesture { center: pos2(200.0, 400.0), zoom: 2.0, pan: Vec2::ZERO };
        let out = run(&mut t, &mut v, &[ev(1, Move, 100.0, 400.0), ev(2, Move, 300.0, 400.0)], Some(g), now);
        assert!(out.is_empty());
        assert!((v.zoom - 2.0).abs() < 1e-4);
        // The point under the fingers stays put: the picture's centre.
        assert!((v.rect(FIT, AREA).center() - pos2(200.0, 400.0)).length() < 0.01);
        let out = run(&mut t, &mut v, &[ev(1, End, 100.0, 400.0), ev(2, End, 300.0, 400.0)], None, now);
        assert!(out.is_empty(), "lifting the fingers mustn't click: {out:?}");
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
    fn touches_that_start_off_the_picture_are_left_alone() {
        let (mut t, mut v, now) = (Touch::default(), View::default(), Instant::now());
        let out = run(&mut t, &mut v, &[ev(1, Start, 100.0, 20.0), ev(1, End, 100.0, 20.0)], None, now);
        assert!(out.is_empty());
    }
}
