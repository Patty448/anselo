//! Folder scans: each folder's scan mode (Never / Once / Always), the built-in Never locations,
//! the minimum image size, and the scan of the Always folders that skips unchanged folders.
//!
//! Scenarios, in the words of someone letting the library find their photos:
//!
//! * System folders and caches are Never without me saying so; I can override one, and taking my
//!   mode off brings the default back. Each change is one undo step.
//! * Setting a folder to Once adds its photos now, but not the icons and thumbnails in it, nor the
//!   photos of a folder inside it I set to Never. Importing by hand still takes every size.
//! * Setting a folder to Always and scanning adds its photos, and those that arrive later; a
//!   folder that hasn't changed isn't listed again, and a file still being written waits.
//! * A folder set to Never is left alone by Auto Import too.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;
use crate::folder_scan::{Platform, default_never};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A procedural PNG `w` × `h` (distinct per seed), last written long ago (settled).
fn write_png(path: &Path, w: usize, h: usize, seed: u8) {
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w) as u8, (i / w) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_768_370_328);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

/// Date folder `dir`'s last change `secs_ago` seconds back, as if it had been left alone since.
fn age_dir(dir: &Path, secs_ago: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
    std::fs::File::open(dir).unwrap().set_modified(t).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn imported(v: &Value) -> usize {
    v["imported"].as_array().map_or(0, Vec::len)
}

fn names(session: &Session) -> Vec<String> {
    let mut v: Vec<String> = session
        .catalog
        .photos()
        .filter(|p| p.in_library())
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

#[test]
fn the_default_never_locations_cover_the_system_caches_and_lightcraft_itself() {
    let home = Path::new("/home/me");
    let d =
        default_never(Platform::Unix, Some(home), Some(Path::new("/home/me/Pictures/Lib")), Some(Path::new("/home/me/.config/lightcraft")), |_| None);
    for p in ["/usr", "/proc", "/var", "/snap", "/home/me/.cache", "/home/me/.local/share", "/home/me/.config", "/home/me/Pictures/Lib"] {
        assert!(d.iter().any(|x| x == p), "{p} in {d:?}");
    }
    assert!(!d.iter().any(|x| x == "/home/me"), "the home folder itself is not");
    let mac = default_never(Platform::Mac, Some(Path::new("/Users/me")), None, None, |_| None);
    assert!(mac.iter().any(|x| x == "/System") && mac.iter().any(|x| x == "/Users/me/Library"), "{mac:?}");
    let win = default_never(Platform::Windows, None, None, None, |n| (n == "APPDATA").then(|| r"C:\Users\me\AppData\Roaming".into()));
    for p in [r"C:\Windows", r"C:\Program Files", r"C:\Program Files (x86)", r"C:\Users\me\AppData\Roaming"] {
        assert!(win.iter().any(|x| x == p), "{p} in {win:?}");
    }
}

#[test]
fn a_default_location_is_never_until_overridden_and_comes_back_when_the_mode_is_taken_off() {
    let mut session = Session::new();
    let default = session.scan_defaults().first().cloned().expect("there are defaults");
    let inside = format!("{default}/something");
    let of = |session: &mut Session| session.execute("folder.scanModeOf", &json!({"path": inside})).unwrap();
    let r = of(&mut session);
    assert_eq!((r["mode"].as_str(), r["default"].as_bool()), (Some("never"), Some(true)), "{r}");
    let r = session.execute("folder.scanMode", &json!({"path": default, "mode": "always"})).unwrap();
    assert_eq!((r["mode"].as_str(), r["default"].as_bool(), r["set"].as_str()), (Some("always"), Some(false), Some("always")), "{r}");
    assert_eq!(of(&mut session)["mode"], "always");
    session.execute("folder.scanMode", &json!({"path": default, "mode": "inherit"})).unwrap();
    assert_eq!(of(&mut session)["default"], true);
    // each change was one undo step
    session.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(of(&mut session)["mode"], "always");
    session.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(of(&mut session)["default"], true);
    assert!(session.catalog.folder_scan_records().is_empty());
}

#[test]
fn bad_scan_mode_calls_are_refused() {
    let mut session = Session::new().with_fs();
    for p in [
        json!({"mode": "always"}),
        json!({"path": "relative/dir", "mode": "always"}),
        json!({"path": "/pics", "mode": "sometimes"}),
        json!({"path": "/pics"}),
    ] {
        assert!(session.execute("folder.scanMode", &p).is_err(), "{p}");
    }
    assert!(session.execute("folder.scanMode", &json!({"path": "/no/such/folder/here", "mode": "once"})).is_err(), "Once needs the folder");
    for p in [json!({"minLongEdge": -1}), json!({"minLongEdge": 100_001}), json!({"rescanMinutes": 0}), json!({"rescanMinutes": "5"})] {
        assert!(session.execute("library.scanSettings", &p).is_err(), "{p}");
    }
    assert!(session.execute("library.scanFolders", &json!({"roots": ["/pics"]})).is_err(), "not set to Always");
    assert!(session.catalog.folder_scan_records().is_empty());
}

#[test]
fn once_adds_the_folders_photos_but_not_small_images_nor_never_folders() {
    let src = temp_dir("once");
    write_png(&src.join("big.png"), 400, 300, 1);
    write_png(&src.join("icon.png"), 200, 150, 2);
    write_png(&src.join("skip/c.png"), 400, 300, 3);
    write_png(&src.join("trip/d.png"), 320, 400, 4);
    let mut session = Session::new().with_fs();
    session.execute("folder.scanMode", &json!({"path": s(&src.join("skip")), "mode": "never"})).unwrap();
    let r = session.execute("folder.scanMode", &json!({"path": s(&src), "mode": "once"})).unwrap();
    assert_eq!(imported(&r), 2, "{r}");
    assert_eq!(r["skipped"]["small"], 1, "{r}");
    assert_eq!((r["mode"].as_str(), r["inLibrary"].as_u64()), (Some("once"), Some(2)), "{r}");
    assert_eq!(names(&session), ["big.png", "d.png"]);
    // the minimum is a setting; importing by hand takes every size
    let st = session.execute("library.scanSettings", &json!({"minLongEdge": 100})).unwrap();
    assert_eq!(st["minLongEdge"], 100);
    assert_eq!(session.execute("folder.scanMode", &json!({"path": s(&src), "mode": "once", "start": false})).unwrap()["import"]["minLongEdge"], 100);
    let mut manual = Session::new().with_fs();
    let r = manual.execute("library.import", &json!({"paths": [s(&src)]})).unwrap();
    assert_eq!(imported(&r), 4, "{r}");
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn always_folders_are_scanned_and_unchanged_folders_are_not_listed_again() {
    let src = temp_dir("always");
    write_png(&src.join("a.png"), 400, 300, 1);
    write_png(&src.join("trip/b.png"), 400, 300, 2);
    write_png(&src.join("trip/day2/c.png"), 400, 300, 3);
    write_png(&src.join("raw/never.png"), 400, 300, 4);
    write_png(&src.join(".hidden/h.png"), 400, 300, 5);
    write_png(&src.join("thumb.png"), 64, 64, 6);
    for d in ["trip/day2", "trip", "raw", ".hidden", ""] {
        age_dir(&src.join(d), 600);
    }
    let mut session = Session::new().with_fs();
    session.execute("folder.scanMode", &json!({"path": s(&src), "mode": "always"})).unwrap();
    session.execute("folder.scanMode", &json!({"path": s(&src.join("raw")), "mode": "never"})).unwrap();
    // nothing happens until a scan
    assert_eq!(session.catalog.len(), 0);
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!(imported(&r), 3, "{r}");
    assert_eq!(r["skipped"]["small"], 1, "{r}");
    assert_eq!((r["listed"].as_u64(), r["unchanged"].as_u64()), (Some(3), Some(0)), "the folder, trip, day2: {r}");
    assert_eq!(names(&session), ["a.png", "b.png", "c.png"]);
    assert!(session.selection.ids.is_empty(), "arrivals don't take the selection");

    // nothing changed: no folder is listed again
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!((r["listed"].as_u64(), r["unchanged"].as_u64(), imported(&r)), (Some(0), Some(3), 0), "{r}");

    // a new photo in one folder: only that folder is listed
    write_png(&src.join("trip/day2/e.png"), 400, 300, 7);
    age_dir(&src.join("trip/day2"), 300);
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!((r["listed"].as_u64(), r["unchanged"].as_u64(), imported(&r)), (Some(1), Some(2), 1), "{r}");

    // a full scan lists everything again, and finds nothing new
    let r = session.execute("library.scanFolders", &json!({"full": true})).unwrap();
    assert_eq!((r["listed"].as_u64(), imported(&r)), (Some(3), 0), "{r}");

    // set to Never: a scan has no roots
    session.execute("folder.scanMode", &json!({"path": s(&src), "mode": "never"})).unwrap();
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!(r["roots"], json!([]), "{r}");
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn a_file_still_being_written_waits_for_the_next_scan() {
    let src = temp_dir("pending");
    let mut session = Session::new().with_fs();
    session.execute("folder.scanMode", &json!({"path": s(&src), "mode": "always"})).unwrap();
    write_png(&src.join("arriving.png"), 400, 300, 1);
    // written just now
    std::fs::File::options().write(true).open(src.join("arriving.png")).unwrap().set_modified(std::time::SystemTime::now()).unwrap();
    age_dir(&src, 600);
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!((imported(&r), r["pending"].as_u64()), (0, Some(1)), "{r}");
    // its folder is listed again, and the size held: in it comes
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!((r["listed"].as_u64(), imported(&r)), (Some(1), 1), "{r}");
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn the_scan_state_survives_reopening_the_library() {
    let src = temp_dir("state-src");
    let lib = temp_dir("state-lib");
    write_png(&src.join("a.png"), 400, 300, 1);
    age_dir(&src, 600);
    {
        let mut session = Session::new().with_fs();
        session.open_library(&lib, false).unwrap();
        session.execute("folder.scanMode", &json!({"path": s(&src), "mode": "always"})).unwrap();
        assert_eq!(imported(&session.execute("library.scanFolders", &json!({})).unwrap()), 1);
    }
    assert!(lib.join(crate::folder_scan::STATE_FILE).is_file());
    let mut session = Session::new().with_fs();
    session.open_library(&lib, false).unwrap();
    let r = session.execute("library.scanFolders", &json!({})).unwrap();
    assert_eq!((r["listed"].as_u64(), r["unchanged"].as_u64()), (Some(0), Some(1)), "{r}");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn auto_import_leaves_a_never_folder_alone() {
    let dir = temp_dir("auto-never");
    let mut session = Session::new().with_fs();
    session.execute("library.autoImport", &json!({"folder": s(&dir)})).unwrap();
    session.execute("folder.scanMode", &json!({"path": s(&dir), "mode": "never"})).unwrap();
    write_png(&dir.join("one.png"), 400, 300, 1);
    for _ in 0..2 {
        let r = session.execute("library.autoImportScan", &json!({})).unwrap();
        assert_eq!((imported(&r), r["never"].as_bool()), (0, Some(true)), "{r}");
    }
    session.execute("folder.scanMode", &json!({"path": s(&dir), "mode": "inherit"})).unwrap();
    session.execute("library.autoImportScan", &json!({})).unwrap();
    assert_eq!(imported(&session.execute("library.autoImportScan", &json!({})).unwrap()), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
