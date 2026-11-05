# Picasa-style single-image view plan

The single-image view (Library loupe) should behave like Picasa 3.9's photo view. This plan lists what the maintainer
asked for, maps it to what LightCraft already has, and orders the work in steps. Existing behaviour that already works
is kept; anything here that conflicts with it is decided by the maintainer before it changes.

Status: ⬜ not started · 🟡 in progress · ✅ done.

## Step 1: mouse behaviour ✅

- Scroll wheel: go to the next or previous photo instead of zooming. If the photo is zoomed in, reset the zoom when
  the photo changes.
- Click and drag on a photo at normal fit-to-window size: zoom to 100%, centred on the point clicked, and pan as the
  mouse moves.
- Releasing the mouse: stay at 100%.
- Click and drag while at 100%: keep panning and stay at 100%.
- Single click while at 100%: return to fit-to-window.
- Treat mouse movement of less than about 5 pixels as a click, not a drag, so a small movement doesn't trigger the
  wrong action.
- Mouse back button: go back to the previous view.
- Mouse forward button: reopen the last photo viewed, at the same position in the photo list.

Decisions (2026-11-04):

- **Wheel:** a notched mouse wheel (line or page units, no modifier) goes to the next photo (down) or the previous
  one (up), one notch per photo; small notches of high-resolution wheels add up. Ctrl/Cmd+wheel and pinch still zoom,
  and a trackpad's two-finger scroll still pans a zoomed photo. Picasa 3.9 did the same (wheel = next / previous,
  Ctrl+wheel = zoom).
- **Zoom reset:** every photo change goes back to fit-to-window, as in Picasa 3.9: wheel, arrow keys, the filmstrip,
  and opening a photo from the grid. (Before, the zoom carried over to the next photo, as in Lightroom.)
- **Clicks at fit-to-window:** as in Picasa 3.9, a single click does nothing (only a drag zooms in) and a
  double-click on the photo goes back to the library grid (`view.back`). A click while zoomed still returns to fit.
- **Drag zoom level:** the click-zoom setting (`view.clickZoom`, 100 % by default), so the existing preference keeps
  working. Picasa always used 100 %.
- **Click or drag:** egui's own threshold (movement under 6 points is a click), close to "about 5 pixels".
- **Back / Forward buttons:** Back does what Esc does (`view.back`: closes a dialog, tool, Compare or full screen
  first, then leaves the photo for the grid). In the grid, Forward reopens the last photo viewed; if it has gone from
  the list, the photo now at its place. Both work anywhere in the window.
- **Already there before Step 1:** a click while zoomed returns to Fit, and a drag while zoomed pans and stays
  zoomed.
- **Scope:** the single-image view (Detail, including the Edit panel and the full-screen preview). Compare, Survey and
  Reference keep their wheel zoom. Space's hand over a tool (crop, masks…) keeps its click-to-zoom.

Where: `crates/ui-egui/src/panels/detail/mouse.rs` (wheel, zoom reset, click and drag), `crates/ui-egui/src/mouse_nav.rs`
(Back / Forward, `view.forward`), tests in `crates/ui-egui/src/tests/picasa_mouse.rs`. The control channel's
`ui.click` takes `button: "back" | "forward" | "middle"`.

## Step 2: layout ⬜

- Replace the library's left bar with an editing bar.
- Editing bar tabs include:
  - "Commonly needed fixes"
  - "Finely tuned lighting and colour fixes"
- Show a histogram and camera information at the bottom of the editing bar.
- Keep the bottom and right bars the same in library view and single-image view.
- Show a top bar containing:
  - Thumbnails of the photos before and after the current one
  - A slideshow button
  - View modes: one image, two different images, and the same image twice

Map tabs to existing LightCraft tools where possible. If a tab, the histogram, camera information, or a view mode
needs a new feature, say so here.

Mapping: (to be filled in before Step 2 starts)
