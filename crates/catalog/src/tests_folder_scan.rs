//! Folder scan modes (Never / Once / Always), kept in a folder's record (see
//! [`crate::folders::ScanMode`]).
//!
//! Scenarios, in the words of someone choosing where the library looks for new photos:
//!
//! * A folder nobody said anything about is not scanned.
//! * A folder set to Always is scanned, and so is every folder inside it, unless one of them says
//!   otherwise; the nearest folder that says something wins.
//! * The built-in Never locations (system folders, caches) are Never without the library keeping
//!   anything, also inside a folder set to Always. Setting a mode on one overrides it; taking the
//!   mode off brings the default back.
//! * Setting a mode keeps the folder's colour label, and the other way round.
//! * The mode follows the folder when it is renamed or moved, and survives reopening.
//! * Every change is one undo step.

use crate::*;

fn set(c: &mut Catalog, path: &str, mode: Option<ScanMode>) -> Op {
    let op = c.folder_scan_op(path, mode);
    c.apply(op).unwrap()
}

fn defaults() -> Vec<String> {
    vec!["/usr".into(), "/home/me/.cache".into()]
}

fn mode(c: &Catalog, path: &str) -> (Option<ScanMode>, Option<String>, bool) {
    let e = c.folder_scan_mode(path, &defaults());
    (e.mode, e.from, e.default)
}

#[test]
fn a_folder_nobody_set_is_not_scanned() {
    let c = Catalog::new();
    assert_eq!(mode(&c, "/home/me/Pictures"), (None, None, false));
    assert_eq!(mode(&c, "relative/path"), (None, None, false), "a path that names no folder");
}

#[test]
fn the_nearest_folder_that_says_something_wins() {
    let mut c = Catalog::new();
    set(&mut c, "/home/me/Pictures", Some(ScanMode::Always));
    set(&mut c, "/home/me/Pictures/exports", Some(ScanMode::Never));
    assert_eq!(mode(&c, "/home/me/Pictures/trip/day1"), (Some(ScanMode::Always), Some("/home/me/Pictures".into()), false));
    assert_eq!(mode(&c, "/home/me/Pictures/exports/web"), (Some(ScanMode::Never), Some("/home/me/Pictures/exports".into()), false));
    assert_eq!(mode(&c, "/home/me//Pictures/"), (Some(ScanMode::Always), Some("/home/me/Pictures".into()), false), "any spelling");
    assert_eq!(mode(&c, "/home/me/Pictures2"), (None, None, false), "a neighbour that only starts the same");
    assert_eq!(c.folder_scan_records(), vec![("/home/me/Pictures".into(), ScanMode::Always), ("/home/me/Pictures/exports".into(), ScanMode::Never)]);
}

#[test]
fn default_locations_are_never_until_a_mode_is_set_on_them() {
    let mut c = Catalog::new();
    assert_eq!(mode(&c, "/usr/share/icons"), (Some(ScanMode::Never), Some("/usr".into()), true));
    assert_eq!(mode(&c, "/home/me/.cache"), (Some(ScanMode::Never), Some("/home/me/.cache".into()), true));
    // inside a folder set to Always, a default still keeps its subtree out
    set(&mut c, "/home/me", Some(ScanMode::Always));
    assert_eq!(mode(&c, "/home/me/.cache/thumbs"), (Some(ScanMode::Never), Some("/home/me/.cache".into()), true));
    // a mode on the default itself overrides it…
    set(&mut c, "/usr", Some(ScanMode::Always));
    assert_eq!(mode(&c, "/usr/share/icons"), (Some(ScanMode::Always), Some("/usr".into()), false));
    // …and taking it off brings the default back
    set(&mut c, "/usr", None);
    assert_eq!(mode(&c, "/usr/share/icons"), (Some(ScanMode::Never), Some("/usr".into()), true));
    assert_eq!(c.folder_record("/usr"), None, "nothing left to keep");
}

#[test]
fn a_mode_and_a_label_live_side_by_side() {
    let mut c = Catalog::new();
    let op = c.folder_label_op("/pics/trip", Some(ColorLabel::Red));
    c.apply(op).unwrap();
    set(&mut c, "/pics/trip", Some(ScanMode::Once));
    assert_eq!(c.folder_color_label("/pics/trip"), Some(ColorLabel::Red));
    let op = c.folder_label_op("/pics/trip", None);
    c.apply(op).unwrap();
    assert_eq!(c.folder_scan_record("/pics/trip"), Some(ScanMode::Once));
    set(&mut c, "/pics/trip", None);
    assert_eq!(c.folder_record("/pics/trip"), None);
}

#[test]
fn a_mode_follows_a_renamed_folder_and_undo_brings_it_back() {
    let mut c = Catalog::new();
    set(&mut c, "/pics/trip", Some(ScanMode::Always));
    set(&mut c, "/pics/trip/raw", Some(ScanMode::Never));
    let before = c.to_snapshot();
    let ops = c.folder_records_follow("/pics/trip", "/pics/holiday");
    let undo = c.apply(Op::Batch { ops }).unwrap();
    assert_eq!(mode(&c, "/pics/holiday/day1"), (Some(ScanMode::Always), Some("/pics/holiday".into()), false));
    assert_eq!(mode(&c, "/pics/holiday/raw"), (Some(ScanMode::Never), Some("/pics/holiday/raw".into()), false));
    assert_eq!(mode(&c, "/pics/trip"), (None, None, false));
    c.apply(undo).unwrap();
    assert_eq!(c.to_snapshot(), before);
}

#[test]
fn setting_a_mode_is_one_undo_step_and_survives_reopening() {
    let mut c = Catalog::new();
    let before = c.to_snapshot();
    let undo = set(&mut c, "/pics/trip", Some(ScanMode::Always));
    let reopened = Catalog::from_snapshot(&c.to_snapshot()).unwrap();
    assert_eq!(reopened.folder_scan_record("/pics/trip"), Some(ScanMode::Always));
    assert!(c.to_snapshot().contains("\"scan\":\"always\""), "{}", c.to_snapshot());
    c.apply(undo).unwrap();
    assert_eq!(c.to_snapshot(), before);
}
