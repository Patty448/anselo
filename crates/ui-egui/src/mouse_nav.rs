//! The mouse's Back and Forward buttons (`docs/picasa-view-plan.md`, Step 1), anywhere in the
//! window: Back does what Escape does (`view.back`: a dialog, tool, Compare or the full-screen
//! preview closes first, then the single-image view goes back to the grid); in the grid, Forward
//! reopens the last photo the single-image view showed (`view.forward`).

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::ViewMode;

/// Run Back / Forward for this frame's button presses. Called with the shortcuts, so not while a
/// prompt or notice holds the input.
pub(crate) fn handle(app: &mut LightcraftApp, ctx: &egui::Context) {
    let presses: Vec<egui::PointerButton> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerButton { button: b @ (egui::PointerButton::Extra1 | egui::PointerButton::Extra2), pressed: true, .. } => Some(*b),
                _ => None,
            })
            .collect()
    });
    for button in presses {
        let command = if button == egui::PointerButton::Extra1 { "view.back" } else { "view.forward" };
        let _ = app.run(command, json!({}));
    }
}

/// Every frame: once the single-image view is left, the next photo shown there opens at Fit, the
/// same photo too (`mouse::reset_zoom_for`).
pub(crate) fn forget_zoom_outside_the_view(app: &mut LightcraftApp) {
    if app.ui.view != ViewMode::Detail && !app.ui.fullscreen {
        app.ui.zoom_photo = None;
    }
}

/// The `view.forward` command: in the grid (no dialog), open the last photo the single-image view
/// showed; if it is no longer in the photo list, the photo now at its place (the last one when the
/// list got shorter). `{"opened": false}` when there is nothing to go forward to.
pub(crate) fn forward_command(app: &mut LightcraftApp) -> Result<Value, String> {
    let in_grid = matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid);
    let Some((photo, index)) = app.ui.last_viewed.filter(|_| in_grid && !app.ui.fullscreen && app.ui.dialog.is_none()) else {
        return Ok(json!({"opened": false}));
    };
    let visible = app.session.visible();
    let target =
        if visible.contains(&PhotoId(photo)) { Some(PhotoId(photo)) } else { visible.get(index.min(visible.len().saturating_sub(1))).copied() };
    let Some(target) = target else { return Ok(json!({"opened": false})) };
    app.run("library.select", json!({"ids": [target.0]}))?;
    app.run("view.detail", json!({}))?;
    Ok(json!({"opened": true, "photo": target.0}))
}
