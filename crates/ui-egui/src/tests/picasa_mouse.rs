//! The single-image view's mouse, as in Picasa 3.9 (`docs/picasa-view-plan.md`, Step 1;
//! `panels/detail/mouse.rs`, `mouse_nav.rs`): the wheel goes from photo to photo, every photo
//! opens at Fit, a drag at Fit zooms in at the point pressed, a click while zoomed returns to Fit,
//! a double-click goes back to the grid, and the mouse's Back / Forward buttons leave and reopen
//! the photo.

use std::time::Duration;

use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{ViewMode, Zoom};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The single-image view of the demo library on its second photo, no panels around the photo.
fn photo_view() -> (Headless, Vec<PhotoId>) {
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    app.ui.settings.gpu = false;
    let mut h = Headless::new(app, [1000.0, 700.0], 1.0);
    let ids = h.app.session.visible_cloned();
    assert!(ids.len() >= 4, "the demo library has photos to step through");
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[1].0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "leftPanel": false, "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    (h, ids)
}

fn run(h: &mut Headless, command: &str) {
    let r = h.request("engine.execute", json!({"command": command}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
}

/// A wheel turn of `y` notches (up is positive) with the pointer on the photo.
fn wheel(h: &mut Headless, y: f32, modifiers: egui::Modifiers) {
    let r = h.request("ui.hoverWidget", json!({"id": "canvas:image"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.synthetic.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, y),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers,
    });
    for _ in 0..10 {
        h.step();
    }
}

/// A drag of `dx` points from the point `(fx, fy)` of the photo pane (not of the photo, which
/// reaches past it when zoomed).
fn drag(h: &mut Headless, fx: f32, fy: f32, dx: f32) {
    let pane = h.app.canvas_rect.expect("the photo pane");
    let at = pane.min + egui::vec2(pane.width() * fx, pane.height() * fy);
    let r = h.request("ui.drag", json!({"x": at.x, "y": at.y, "toX": at.x + dx, "toY": at.y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

fn click(h: &mut Headless, count: u32) {
    let r = h.request("ui.clickWidget", json!({"id": "canvas:image", "count": count}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

/// One of the mouse's side buttons, over the middle of the window.
fn side_button(h: &mut Headless, button: &str) {
    let r = h.request("ui.click", json!({"x": 500.0, "y": 350.0, "button": button}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

#[test]
fn the_wheel_goes_from_photo_to_photo_and_each_opens_at_fit() {
    let (mut h, ids) = photo_view();
    run(&mut h, "view.zoom100");
    assert_eq!(h.app.ui.zoom, Zoom::Percent(100.0));
    // down: the next photo, at Fit
    wheel(&mut h, -1.0, egui::Modifiers::NONE);
    h.settle(SETTLE);
    assert_eq!(h.app.session.active(), Some(ids[2]));
    assert_eq!(h.app.ui.zoom, Zoom::Fit, "the next photo opens at Fit");
    // up: the previous one
    wheel(&mut h, 1.0, egui::Modifiers::NONE);
    assert_eq!(h.app.session.active(), Some(ids[1]));
    // a high-resolution wheel's half notches add up to one photo
    wheel(&mut h, -0.5, egui::Modifiers::NONE);
    assert_eq!(h.app.session.active(), Some(ids[1]), "half a notch is not a photo yet");
    wheel(&mut h, -0.5, egui::Modifiers::NONE);
    assert_eq!(h.app.session.active(), Some(ids[2]));
    // Ctrl+wheel still zooms and stays on the photo
    wheel(&mut h, 1.0, egui::Modifiers::CTRL);
    // (its smoothed zoom glides on for a few frames)
    h.settle(SETTLE);
    assert!(matches!(h.app.ui.zoom, Zoom::Percent(_)), "{:?}", h.app.ui.zoom);
    assert_eq!(h.app.session.active(), Some(ids[2]));
    // any other way to the next photo opens it at Fit too
    run(&mut h, "library.next");
    assert_eq!(h.app.session.active(), Some(ids[3]));
    assert_eq!(h.app.ui.zoom, Zoom::Fit);
    // and so does the same photo opened again from the grid
    run(&mut h, "view.zoom100");
    run(&mut h, "view.back");
    run(&mut h, "view.detail");
    assert_eq!(h.app.session.active(), Some(ids[3]));
    assert_eq!(h.app.ui.zoom, Zoom::Fit);
}

#[test]
fn a_drag_at_fit_zooms_in_at_the_point_pressed_and_a_click_returns_to_fit() {
    let (mut h, _) = photo_view();
    h.app.ui.click_zoom = 100;
    // a click at Fit does nothing
    click(&mut h, 1);
    assert_eq!(h.app.ui.zoom, Zoom::Fit);
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    // a drag: 100 % around the top-left quarter pressed, panned by the drag, and it stays
    let (img, pane) = (h.app.image_rect.expect("the photo"), h.app.canvas_rect.expect("the pane"));
    let quarter = img.min + img.size() / 4.0;
    drag(&mut h, (quarter.x - pane.left()) / pane.width(), (quarter.y - pane.top()) / pane.height(), 40.0);
    assert_eq!(h.app.ui.zoom, Zoom::Percent(100.0));
    let pan = h.app.ui.pan;
    assert!(pan.0 < 0.3 && pan.1 < 0.3, "centred near the point pressed, not the middle: {pan:?}");
    // another drag pans and stays zoomed
    drag(&mut h, 0.5, 0.5, -60.0);
    assert_eq!(h.app.ui.zoom, Zoom::Percent(100.0));
    assert!(h.app.ui.pan.0 > pan.0, "dragged left: the view moves right, {:?} vs {pan:?}", h.app.ui.pan);
    // a movement of 3 points is a click, not a drag: back to Fit
    drag(&mut h, 0.5, 0.5, 3.0);
    assert_eq!(h.app.ui.zoom, Zoom::Fit);
    // a movement of 3 points at Fit is a click too: nothing happens
    drag(&mut h, 0.5, 0.5, 3.0);
    assert_eq!(h.app.ui.zoom, Zoom::Fit);
}

#[test]
fn a_double_click_on_the_photo_goes_back_to_the_grid() {
    let (mut h, _) = photo_view();
    click(&mut h, 2);
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
}

#[test]
fn back_leaves_the_photo_and_forward_reopens_it_or_the_one_at_its_place() {
    let (mut h, ids) = photo_view();
    run(&mut h, "library.next");
    assert_eq!(h.app.session.active(), Some(ids[2]));
    // Forward in the single-image view: nothing to go forward to
    side_button(&mut h, "forward");
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    side_button(&mut h, "back");
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
    // another photo selected in the grid meanwhile: Forward still reopens the one viewed
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[0].0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    side_button(&mut h, "forward");
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert_eq!(h.app.session.active(), Some(ids[2]));
    // a photo gone from the list: the one now at its place
    side_button(&mut h, "back");
    h.app.ui.last_viewed = Some((u64::MAX, 1));
    side_button(&mut h, "forward");
    assert_eq!(h.app.session.active(), Some(ids[1]));
    // Back closes a dialog before it leaves the photo
    run(&mut h, "dialog.folderManager");
    assert!(h.app.ui.dialog.is_some());
    side_button(&mut h, "back");
    assert!(h.app.ui.dialog.is_none());
    assert_eq!(h.app.ui.view, ViewMode::Detail);
}
