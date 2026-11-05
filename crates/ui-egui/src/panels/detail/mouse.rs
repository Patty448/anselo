//! The single-image view's mouse, as in Picasa 3.9 (`docs/picasa-view-plan.md`, Step 1): the wheel
//! goes from photo to photo, every photo opens at fit-to-window, a drag at Fit zooms in at the
//! point pressed and pans, a click while zoomed returns to Fit and a double-click goes back to the
//! grid. The mouse's Back and Forward buttons are `crate::mouse_nav`.

use egui::{Rect, Vec2};
use lightcraft_catalog::PhotoId;
use serde_json::json;

use super::geometry::{pan_image, wheel_notch, wheel_notches};
use crate::LightcraftApp;
use crate::state::Zoom;

/// At most this many photos per frame for a burst of wheel notches.
const MAX_WHEEL_STEPS: f32 = 64.0;

/// Every photo opens at fit-to-window: the zoom belongs to the photo it was set on (Picasa 3.9;
/// before, it carried over to the next photo as in Lightroom). Also remembers the photo and its
/// place in the photo list for the mouse's Forward button.
pub(super) fn reset_zoom_for(app: &mut LightcraftApp, id: PhotoId) {
    if app.ui.zoom_photo == Some(id.0) {
        return;
    }
    app.ui.zoom_photo = Some(id.0);
    app.ui.zoom = Zoom::Fit;
    app.ui.pan = (0.5, 0.5);
    app.ui.zoom_anim = false;
    let index = app.session.visible().iter().position(|v| *v == id).unwrap_or(0);
    app.ui.last_viewed = Some((id.0, index));
}

/// A notched mouse wheel over the photo goes to the previous photo (up) or the next one (down),
/// one notch a photo; the small notches of a high-resolution wheel add up. Ctrl/Cmd+wheel and a
/// pinch still zoom and a trackpad still pans (`navigate_gesture`), so the notches are taken out
/// of this frame's input, and egui's smoothed scroll of them is swallowed until it has run out.
pub(super) fn wheel_step(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response) {
    let acc_id = egui::Id::unique("loupe-wheel-step-notches");
    let eat_id = egui::Id::unique("loupe-wheel-step-eat");
    if resp.hovered() {
        let notches = ui.input(|i| wheel_notches(&i.events));
        if notches != 0.0 {
            let before = ui.data(|d| d.get_temp::<f32>(acc_id)).unwrap_or(0.0);
            // turning the wheel the other way starts afresh
            let acc = if before * notches < 0.0 { notches } else { before + notches };
            let steps = acc.trunc().clamp(-MAX_WHEEL_STEPS, MAX_WHEEL_STEPS);
            ui.data_mut(|d| {
                d.insert_temp(acc_id, acc - acc.trunc());
                d.insert_temp(eat_id, true);
            });
            let command = if steps > 0.0 { "library.previous" } else { "library.next" };
            for _ in 0..steps.abs() as usize {
                let _ = app.run(command, json!({}));
            }
            ui.input_mut(|i| i.events.retain(|e| wheel_notch(e).is_none()));
            ui.ctx().request_repaint();
        }
    }
    if ui.data(|d| d.get_temp::<bool>(eat_id)).unwrap_or(false) {
        if ui.input(|i| i.smooth_scroll_delta) == Vec2::ZERO {
            ui.data_mut(|d| d.remove_temp::<bool>(eat_id));
        } else {
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
    }
}

/// The photo's own click and drag (no tool on the photo). At Fit a drag zooms to the click-zoom
/// ratio (`view.clickZoom`, 100 % by default) centred on the point pressed, then pans as the mouse
/// moves, and the zoom stays when the button is up; a click does nothing. Zoomed in, a drag pans
/// and a click returns to Fit. A double-click goes back to the grid (`view.back`). A movement under
/// egui's click distance (6 points) is a click, not a drag.
pub(super) fn click_and_drag(app: &mut LightcraftApp, resp: &egui::Response, img: Rect, canvas: Rect) {
    if resp.double_clicked() {
        let _ = app.run("view.back", json!({}));
        return;
    }
    let at_fit = matches!(app.ui.zoom, Zoom::Fit);
    if at_fit {
        if resp.drag_started_by(egui::PointerButton::Primary) {
            // where the button went down, not where the drag was recognised
            if let Some(q) = resp.ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()) {
                let u = ((q.x - img.left()) / img.width().max(1.0)).clamp(0.0, 1.0);
                let v = ((q.y - img.top()) / img.height().max(1.0)).clamp(0.0, 1.0);
                app.ui.pan = (u, v);
            }
            app.ui.zoom = Zoom::Percent(app.ui.click_zoom as f32);
            // straight there, so the drag pans the photo it shows
            app.ui.zoom_anim = false;
            resp.ctx.request_repaint();
        }
        return;
    }
    if resp.clicked() {
        app.ui.zoom = Zoom::Fit;
        app.ui.zoom_anim = true;
    } else if resp.dragged_by(egui::PointerButton::Primary) && super::zoomed(img, canvas) {
        pan_image(app, canvas, img, resp.drag_delta());
    }
}
