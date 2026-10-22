//! Folder scans in the app: a folder row's Scan ▸ submenu (Never / Once / Always / Inherit), the
//! question Never asks about the photos already in the library, and the background scan of the
//! folders set to Always with its row in the activity stack.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// A headless app over file-backed photos that exist only in the catalog.
fn app(paths: &[&str]) -> Headless {
    use lightcraft_catalog::{Op, Photo, Source};
    let mut session = lightcraft_engine::Session::new().with_fs();
    for path in paths {
        let id = session.catalog.alloc_photo_id();
        let p = Photo::new(id, Source::File { path: (*path).into() }, "x.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00");
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let mut h = Headless::new(LightcraftApp::new(session, Services { png: None, ..Default::default() }), [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "photoGrid", "leftPanel": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A procedural PNG `w` × `h`, last written long ago (settled).
fn write_png(path: &Path, w: usize, h: usize, seed: u8) {
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w) as u8, (i / w) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_768_370_328);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn right_click(h: &mut Headless, id: &str) {
    let c = h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("no widget {id}"));
    let r = h.request("ui.click", json!({"x": c.x, "y": c.y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn choose(h: &mut Headless, row: &str, item: &str) {
    right_click(h, row);
    click(h, "folderScanMenu");
    assert!(has(h, item), "the Scan menu offers {item}");
    click(h, item);
}

fn deleted(h: &Headless) -> usize {
    h.app.session.catalog.photos().filter(|p| p.deleted).count()
}

/// Given a library folder, when Scan ▸ Never is chosen, then the folder is Never and a question
/// asks about its photos: Keep Them keeps them, Remove from Library sends them to Recently
/// Deleted. Inherit takes the mode off again.
#[test]
fn never_asks_whether_the_photos_already_in_the_library_go_too() {
    let mut h = app(&["/pics/trip/a.jpg", "/pics/trip/b.jpg", "/pics/home/c.jpg"]);
    click(&mut h, "libraryFolderToggle:/pics");
    choose(&mut h, "source:libfolder:/pics/trip", "folderScan:never");
    assert_eq!(h.app.session.catalog.folder_scan_record("/pics/trip"), Some(lightcraft_catalog::ScanMode::Never));
    assert_eq!(h.app.ui.dialog, Some(Dialog::NeverScan { path: "/pics/trip".into(), name: "pics/trip".into(), count: 2 }));
    let r = h.request("ui.dialog.cancel", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!((deleted(&h), h.app.ui.dialog.clone()), (0, None), "Keep Them keeps them");
    assert_eq!(h.app.session.catalog.folder_scan_record("/pics/trip"), Some(lightcraft_catalog::ScanMode::Never), "and the mode stays");

    // asked again (Inherit, then Never), and this time removed
    choose(&mut h, "source:libfolder:/pics/trip", "folderScan:inherit");
    assert_eq!(h.app.session.catalog.folder_scan_record("/pics/trip"), None);
    assert_eq!(h.app.ui.dialog, None, "only Never asks");
    choose(&mut h, "source:libfolder:/pics/trip", "folderScan:never");
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(deleted(&h), 2, "to Recently Deleted (no file touched)");
    assert_eq!(h.app.session.visible().len(), 1);
}

/// Scan ▸ Always on a folder adds its photos in the background (a row in the activity stack while
/// the scan runs), leaving out small images; a photo added later comes in with the next scan.
/// Once adds a folder's photos at once.
#[test]
fn always_scans_in_the_background_and_once_imports_now() {
    let src = temp_dir("always");
    write_png(&src.join("a.png"), 400, 300, 1);
    write_png(&src.join("icon.png"), 64, 64, 2);
    let photo = src.join("a.png").to_string_lossy().to_string();
    let mut h = app(&[]);
    let r = h.request("engine.execute", json!({"command": "folder.scanMode", "params": {"path": src.to_string_lossy(), "mode": "always"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    // the change starts a scan
    assert!(h.step_until(T, |h| h.app.session.catalog.len() == 1 && h.app.import.is_none()), "the scan added the photo");
    assert!(h.app.session.catalog.photos().any(|p| matches!(&p.source, lightcraft_catalog::Source::File { path } if *path == photo)));
    assert!(h.app.session.activity.list().iter().all(|r| r.kind != crate::folder_scan::KIND), "the row goes with the scan");

    // a later photo comes with the next scan (asked for, as a settings change asks)
    write_png(&src.join("b.png"), 400, 300, 3);
    h.app.folder_scans.request();
    assert!(h.step_until(T, |h| h.app.session.catalog.len() == 2 && h.app.import.is_none()));

    // Once, from a Folders row's menu, adds the rest of the folder's photos at once (an import task)
    let once = temp_dir("once");
    write_png(&once.join("c.png"), 400, 300, 4);
    write_png(&once.join("d.png"), 400, 300, 5);
    let r = h.request("engine.execute", json!({"command": "library.import", "params": {"paths": [once.join("c.png").to_string_lossy()]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let key = lightcraft_catalog::query::folder_key(&once.to_string_lossy());
    let row = format!("source:libfolder:{key}");
    // open the rows down to it
    assert!(h.step_until(T, |h| h.app.widgets.iter().any(|(w, _)| w.starts_with("libraryFolderToggle:"))));
    for _ in 0..12 {
        if has(&h, &row) {
            break;
        }
        let closed: Vec<String> = h
            .app
            .widgets
            .iter()
            .filter_map(|(w, _)| w.strip_prefix("libraryFolderToggle:").filter(|p| key.starts_with(&format!("{p}/"))).map(|p| p.to_string()))
            .collect();
        let Some(next) = closed.into_iter().max_by_key(String::len) else { break };
        click(&mut h, &format!("libraryFolderToggle:{next}"));
    }
    choose(&mut h, &row, "folderScan:once");
    assert!(h.step_until(T, |h| h.app.session.catalog.len() == 4 && h.app.import.is_none()), "{}", h.app.session.catalog.len());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&once);
}

/// The scan's row in the activity stack can stop it (✕).
#[test]
fn a_running_scan_has_a_cancellable_row() {
    let src = temp_dir("row");
    write_png(&src.join("a.png"), 400, 300, 1);
    let mut h = app(&[]);
    // run directly, not through a request (which runs frames): one frame starts the scan, and its
    // result is applied at the start of a later frame
    h.app.run("folder.scanMode", json!({"path": src.to_string_lossy(), "mode": "always"})).unwrap();
    h.step();
    let rows = h.app.session.activity.list();
    let row = rows.iter().find(|r| r.kind == crate::folder_scan::KIND).unwrap_or_else(|| panic!("no scan row in {rows:?}"));
    assert_eq!((row.label.as_str(), row.cancellable), ("Scanning folders", true));
    assert!(h.step_until(T, |h| h.app.session.catalog.len() == 1));
    let _ = std::fs::remove_dir_all(&src);
}
