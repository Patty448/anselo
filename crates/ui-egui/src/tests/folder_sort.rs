//! Folders ▸ Sort by Date: the Folders section as one flat list of the folders holding photos,
//! newest first under year headings, chosen from the section header's menu or `view.folderSort`.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::FolderSort;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// A headless app over file-backed photos (`path`, capture time) that exist only in the catalog.
fn app(photos: &[(&str, Option<&str>)]) -> Headless {
    use lightcraft_catalog::{Op, Photo, Source};
    let mut session = lightcraft_engine::Session::new();
    for (path, captured) in photos {
        let id = session.catalog.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: (*path).into() }, "x.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00");
        p.captured = captured.map(String::from);
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let mut h = Headless::new(LightcraftApp::new(session, Services { png: None, ..Default::default() }), [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "photoGrid", "leftPanel": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn widget(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click_at(h: &mut Headless, id: &str, button: &str) {
    let c = widget(h, id).center();
    let r = h.request("ui.click", json!({"x": c.x, "y": c.y, "button": button}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn top(h: &Headless, id: &str) -> f32 {
    widget(h, id).top()
}

const PHOTOS: &[(&str, Option<&str>)] = &[
    ("/pics/2019/beach/a.jpg", Some("2019-07-01T10:00:00")),
    ("/pics/2024/city/a.jpg", Some("2024-02-01T10:00:00")),
    ("/pics/2024/city/night/b.jpg", Some("2024-02-03T10:00:00")),
    ("/pics/scans/c.jpg", None),
];

/// Given folders shot in different years, when Sort by Date is chosen from the Folders header's
/// menu, then each folder holding photos is a row of its own, newest first under its year, the
/// undated one last under Unknown Date; choosing a row still shows its photos.
#[test]
fn sort_by_date_lists_folders_newest_first_under_year_headings() {
    let mut h = app(PHOTOS);
    assert_eq!(h.app.ui.folder_sort, FolderSort::Tree);
    assert!(!has(&h, "folderYear:2024"), "the tree has no year headings");
    click_at(&mut h, "sidebarSection:folders", "right");
    assert!(has(&h, "sidebarMenu:folderSort:date"), "the Folders header's menu offers the sort");
    click_at(&mut h, "sidebarMenu:folderSort:date", "left");
    assert_eq!(h.app.ui.folder_sort, FolderSort::Date);
    h.step();
    let rows = ["folderYear:2024", "source:libfolder:/pics/2024/city/night", "source:libfolder:/pics/2024/city"];
    let rows = rows.into_iter().chain(["folderYear:2019", "source:libfolder:/pics/2019/beach", "folderYear:none", "source:libfolder:/pics/scans"]);
    let tops: Vec<f32> = rows.map(|id| top(&h, id)).collect();
    assert!(tops.windows(2).all(|w| w[0] < w[1]), "newest first, undated last: {tops:?}");
    assert!(!has(&h, "source:libfolder:/pics"), "a folder with no photos of its own is no row");
    click_at(&mut h, "source:libfolder:/pics/2024/city", "left");
    assert_eq!(h.app.session.visible().len(), 2, "its subfolder's photo too, as the tree's row shows");
    // and back to the tree
    let r = h.request("engine.execute", json!({"command": "view.folderSort", "params": {"sort": "tree"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "source:libfolder:/pics") && !has(&h, "folderYear:2024"));
}

/// `view.folderSort` without a sort flips it, a wrong one is an error, and the choice is kept in
/// the saved UI state (a hand-edited value reads as the default).
#[test]
fn folder_sort_command_and_saved_state() {
    let mut h = app(PHOTOS);
    let run = |h: &mut Headless, params: serde_json::Value| h.request("engine.execute", json!({"command": "view.folderSort", "params": params}), T);
    let r = run(&mut h, json!({}));
    assert_eq!(r["result"]["sort"], "date", "{r}");
    let r = run(&mut h, json!({"sort": "size"}));
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(h.app.ui.folder_sort, FolderSort::Date, "a wrong sort changes nothing");
    let saved = serde_json::to_value(&h.app.ui).unwrap();
    assert_eq!(saved["folderSort"], "date");
    let back: crate::state::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.folder_sort, FolderSort::Date);
    let odd: crate::state::UiState = serde_json::from_value(json!({"folderSort": 7, "showCounts": false})).unwrap();
    assert_eq!((odd.folder_sort, odd.show_counts), (FolderSort::Tree, false), "a bad value costs no other setting");
}
