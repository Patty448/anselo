//! The Folder Manager: the whole computer's folders as a tree, each with its scan mode.
//!
//! Scenarios, in the words of someone choosing where the library looks for photos:
//!
//! * Opening it lists nothing but where the tree starts; opening a folder lists just the folders
//!   in it, a big folder up to a point (“N more…” lists the rest).
//! * System folders show as Never without my saying so; I can set one myself and take it off again.
//! * Hidden folders show only when I ask for them.
//! * Setting a folder with library photos to Never asks, in the dialog, whether they go too.
//! * A folder set to Always that can't be watched for changes says so.
//! * It opens from File ▸ Folder Manager… and from Settings ▸ Import.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-manager-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

/// A headless app (with library photos at `photos`, which need not exist) whose Folder Manager
/// starts at `roots` and counts its listings.
fn manager(roots: Vec<(String, String)>, photos: &[String], services: Services) -> (Headless, Arc<AtomicUsize>) {
    use lightcraft_catalog::{Op, Photo, Source};
    let mut session = lightcraft_engine::Session::new().with_fs();
    for path in photos {
        let id = session.catalog.alloc_photo_id();
        let p = Photo::new(id, Source::File { path: path.clone() }, "x.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00");
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let mut h = Headless::new(LightcraftApp::new(session, services), [1400.0, 900.0], 1.0);
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    h.app.folder_manager.lister = Arc::new(move |path, hidden, cap| {
        c.fetch_add(1, Ordering::SeqCst);
        lightcraft_engine::folder_list::list_folders(Path::new(path), hidden, cap)
    });
    h.app.folder_manager.places = Arc::new(move || roots.clone());
    h.settle(SETTLE);
    (h, count)
}

fn open_dialog(h: &mut Headless) {
    let r = h.request("engine.execute", json!({"command": "dialog.folderManager"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.dialog, Some(Dialog::FolderManager));
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

fn rows(h: &Headless) -> Vec<String> {
    h.app.widgets.iter().filter_map(|(w, _)| w.strip_prefix("folderManager:row:").map(str::to_string)).collect()
}

/// What the row of folder `path` says: (mode, whose).
fn effective(h: &Headless, path: &str) -> Option<(String, String)> {
    let prefix = format!("folderManager:effective:{path}:");
    h.app.widgets.iter().find_map(|(w, _)| {
        let rest = w.strip_prefix(&prefix)?;
        let (mode, whose) = rest.split_once(':')?;
        Some((mode.to_string(), whose.to_string()))
    })
}

fn choose(h: &mut Headless, path: &str, choice: &str) {
    click(h, &format!("folderManager:mode:{path}"));
    let item = format!("folderManager:choice:{choice}");
    assert!(has(h, &item), "the menu offers {choice}");
    click(h, &item);
}

#[test]
fn folders_are_listed_only_when_opened_and_a_big_folder_up_to_a_point() {
    let root = temp_dir("lazy");
    for y in 0..5 {
        for m in 0..100 {
            std::fs::create_dir_all(root.join(format!("y{y}/m{m:03}"))).unwrap();
        }
    }
    for b in 0..(lightcraft_engine::folder_list::MAX_LISTED + 20) {
        std::fs::create_dir_all(root.join(format!("big/b{b:04}"))).unwrap();
    }
    let (mut h, listed) = manager(vec![("Tree".into(), s(&root))], &[], Services { png: None, ..Default::default() });
    open_dialog(&mut h);
    assert_eq!(rows(&h), [s(&root)], "only where the tree starts");
    assert_eq!(listed.load(Ordering::SeqCst), 0, "nothing listed yet");

    click(&mut h, &format!("folderManager:toggle:{}", s(&root)));
    assert_eq!(listed.load(Ordering::SeqCst), 1);
    assert_eq!(rows(&h).len(), 1 + 6, "the tree and its six folders: {:?}", rows(&h));

    let y3 = s(&root.join("y3"));
    click(&mut h, &format!("folderManager:toggle:{y3}"));
    assert_eq!(listed.load(Ordering::SeqCst), 2, "just y3");
    assert_eq!(rows(&h).iter().filter(|r| r.starts_with(&format!("{y3}/"))).count(), 100);

    // a big folder: the first ones, and the rest on asking
    let big = s(&root.join("big"));
    click(&mut h, &format!("folderManager:toggle:{big}"));
    let in_big = |h: &Headless| rows(h).iter().filter(|r| r.starts_with(&format!("{big}/"))).count();
    assert_eq!(in_big(&h), lightcraft_engine::folder_list::MAX_LISTED);
    // (the link is below the 500 folders: scrolled to)
    let more = format!("folderManager:more:{big}");
    let r = h.request("ui.hoverWidget", json!({"id": format!("folderManager:row:{big}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    let shown = |h: &Headless| h.app.widgets.iter().any(|(w, r)| *w == more && r.top() > 0.0 && r.bottom() < 900.0);
    for _ in 0..100 {
        if shown(&h) {
            break;
        }
        let r = h.request("ui.scroll", json!({"dy": -2000.0}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    }
    assert!(shown(&h), "scrolled to “20 more…”");
    click(&mut h, &format!("folderManager:more:{big}"));
    assert_eq!(in_big(&h), lightcraft_engine::folder_list::MAX_LISTED + 20);
    assert!(!has(&h, &format!("folderManager:more:{big}")));

    // closed and opened again: kept, not listed again; Refresh lists again
    let n = listed.load(Ordering::SeqCst);
    click(&mut h, &format!("folderManager:toggle:{y3}"));
    click(&mut h, &format!("folderManager:toggle:{y3}"));
    assert_eq!(listed.load(Ordering::SeqCst), n);
    click(&mut h, "button:folderManagerRefresh");
    assert!(listed.load(Ordering::SeqCst) > n);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_default_never_folder_shows_as_never_and_can_be_set() {
    let (mut h, _) = manager(Vec::new(), &[], Services { png: None, ..Default::default() });
    // a system folder this machine has (e.g. /usr, C:\Windows)
    let Some(system) = h.app.session.scan_defaults().into_iter().find(|d| Path::new(d).is_dir()) else {
        return lightcraft_testkit::skip!(System, "no built-in Never folder exists on this machine");
    };
    let key = lightcraft_catalog::query::folder_key(&system);
    h.app.folder_manager.places = Arc::new(move || vec![("System".into(), key.clone())]);
    let system = lightcraft_catalog::query::folder_key(&system);
    open_dialog(&mut h);
    assert_eq!(effective(&h, &system), Some(("never".into(), "default".into())));
    choose(&mut h, &system, "never");
    assert_eq!(h.app.session.catalog.folder_scan_record(&system), Some(lightcraft_catalog::ScanMode::Never));
    assert_eq!(effective(&h, &system), Some(("never".into(), "own".into())));
    choose(&mut h, &system, "inherit");
    assert_eq!(h.app.session.catalog.folder_scan_record(&system), None);
    assert_eq!(effective(&h, &system), Some(("never".into(), "default".into())), "the default comes back");
}

#[test]
fn hidden_folders_show_on_asking() {
    let root = temp_dir("hidden");
    std::fs::create_dir_all(root.join("shown")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    let (mut h, _) = manager(vec![("Tree".into(), s(&root))], &[], Services { png: None, ..Default::default() });
    open_dialog(&mut h);
    click(&mut h, &format!("folderManager:toggle:{}", s(&root)));
    let hidden = s(&root.join(".hidden"));
    assert!(has(&h, &format!("folderManager:row:{}", s(&root.join("shown")))));
    assert!(!has(&h, &format!("folderManager:row:{hidden}")));
    click(&mut h, "check:folderManagerHidden");
    assert!(has(&h, &format!("folderManager:row:{hidden}")));
    click(&mut h, "check:folderManagerHidden");
    assert!(!has(&h, &format!("folderManager:row:{hidden}")));
    let _ = std::fs::remove_dir_all(&root);
}

/// Never on a folder with library photos asks in the dialog, which stays open; Inherit shows the
/// mode of the folder above.
#[test]
fn never_asks_in_the_manager() {
    let root = temp_dir("never");
    std::fs::create_dir_all(root.join("trip")).unwrap();
    let trip = lightcraft_catalog::query::folder_key(&s(&root.join("trip")));
    let photos = [format!("{trip}/a.jpg"), format!("{trip}/b.jpg")];
    let (mut h, _) =
        manager(vec![("Tree".into(), lightcraft_catalog::query::folder_key(&s(&root)))], &photos, Services { png: None, ..Default::default() });
    open_dialog(&mut h);
    click(&mut h, &format!("folderManager:toggle:{}", lightcraft_catalog::query::folder_key(&s(&root))));
    assert_eq!(effective(&h, &trip), Some(("none".into(), "inherited".into())), "not scanned");
    choose(&mut h, &trip, "never");
    assert_eq!(h.app.ui.dialog, Some(Dialog::FolderManager), "the manager stays");
    assert_eq!(h.app.folder_manager.question.as_ref().map(|q| q.count), Some(2));
    // Keep Them keeps them
    click(&mut h, "button:folderManagerKeep");
    assert!(h.app.folder_manager.question.is_none());
    assert_eq!(h.app.session.catalog.photos().filter(|p| p.deleted).count(), 0);
    // asked again, and removed this time (to Recently Deleted)
    choose(&mut h, &trip, "inherit");
    choose(&mut h, &trip, "never");
    click(&mut h, "button:folderManagerRemove");
    assert_eq!(h.app.session.catalog.photos().filter(|p| p.deleted).count(), 2);
    assert_eq!(h.app.ui.dialog, Some(Dialog::FolderManager));
    assert_eq!(effective(&h, &trip), Some(("never".into(), "own".into())));
    let _ = std::fs::remove_dir_all(&root);
}

/// A watcher that can't watch anything (as at the system's watch limit).
struct Full;

impl lightcraft_engine::folder_watch::FolderWatcher for Full {
    fn watch(&mut self, _root: &Path) -> Result<(), lightcraft_engine::folder_watch::WatchError> {
        Err(lightcraft_engine::folder_watch::WatchError { limit: true, message: "no more watches".into() })
    }

    fn unwatch(&mut self, _root: &Path) {}
}

#[test]
fn a_folder_that_cant_be_watched_shows_polling() {
    let root = temp_dir("polling");
    let key = lightcraft_catalog::query::folder_key(&s(&root));
    let make: lightcraft_engine::folder_watch::MakeWatcher =
        Arc::new(|_| Ok(Box::new(Full) as Box<dyn lightcraft_engine::folder_watch::FolderWatcher>));
    let (mut h, _) = manager(vec![("Tree".into(), key.clone())], &[], Services { png: None, folder_watcher: Some(make), ..Default::default() });
    open_dialog(&mut h);
    choose(&mut h, &key, "always");
    assert!(h.step_until(T, |h| has(h, &format!("folderManager:polling:{key}"))), "the row says it is polled");
    assert_eq!(effective(&h, &key), Some(("always".into(), "own".into())));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn it_opens_from_the_file_menu_and_settings() {
    let (mut h, _) = manager(Vec::new(), &[], Services { png: None, ..Default::default() });
    let r = h.request("ui.menu.invoke", json!({"id": "dialog.folderManager"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.dialog, Some(Dialog::FolderManager));
    h.app.ui.dialog = None;
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "import"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    click(&mut h, "button:settingsFolderManager");
    assert_eq!(h.app.ui.dialog, Some(Dialog::FolderManager));
}
