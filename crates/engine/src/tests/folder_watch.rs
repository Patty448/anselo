//! Watching the folders set to Always for changes.
//!
//! Scenarios, in the words of someone whose photos arrive in folders the library watches:
//!
//! * A photo copied into a watched folder marks that folder as changed, and it is scanned once the
//!   copying has gone quiet; hidden files, the library's own files and a log file being written
//!   don't count, nor does anything outside the watched folders.
//! * A folder inside another watched one isn't watched twice; a folder no longer set to Always
//!   isn't watched any more.
//! * A folder that can't be watched (the system's watch limit) is said once and left to the
//!   rescans; so is a folder made later that can't be watched.
//! * When notifications were lost, or too many folders changed, every watched folder is scanned.
//! * A scan of the changed folders lists just those, not the rest of the watched folder.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use web_time::Instant;

use crate::Session;
use crate::activity::Activity;
use crate::folder_watch::{Change, Due, FolderWatch, FolderWatcher, MAX_DIRTY, MakeWatcher, QUIET, Sink, WatchError};

/// A watcher that refuses the folders in `fail` (as at the system's limit) and logs the rest.
struct Fake {
    fail: Vec<String>,
    log: Arc<Mutex<Vec<String>>>,
}

impl FolderWatcher for Fake {
    fn watch(&mut self, root: &Path) -> Result<(), WatchError> {
        let root = root.to_string_lossy().to_string();
        if self.fail.contains(&root) {
            return Err(WatchError { limit: true, message: "no more watches".into() });
        }
        self.log.lock().unwrap().push(format!("watch {root}"));
        Ok(())
    }

    fn unwatch(&mut self, root: &Path) {
        self.log.lock().unwrap().push(format!("unwatch {}", root.to_string_lossy()));
    }
}

/// A started watch over a [`Fake`], its sink (to send changes through) and its log.
fn fake(fail: &[&str]) -> (FolderWatch, Sink, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let slot: Arc<Mutex<Option<Sink>>> = Arc::new(Mutex::new(None));
    let (l, sl, fail) = (log.clone(), slot.clone(), fail.iter().map(|f| f.to_string()).collect::<Vec<_>>());
    let make: MakeWatcher = Arc::new(move |sink: Sink| {
        *sl.lock().unwrap() = Some(sink);
        Ok(Box::new(Fake { fail: fail.clone(), log: l.clone() }) as Box<dyn FolderWatcher>)
    });
    let mut w = FolderWatch::default();
    w.start(&make, Box::new(|| {})).unwrap();
    let sink = slot.lock().unwrap().clone().unwrap();
    (w, sink, log)
}

fn sync(w: &mut FolderWatch, roots: &[&str], skip: &[&str]) {
    let roots: Vec<String> = roots.iter().map(|r| r.to_string()).collect();
    w.sync(&roots, skip.iter().map(|s| s.to_string()).collect(), &Activity::default());
    let until = Instant::now() + Duration::from_secs(10);
    while w.syncing() {
        assert!(Instant::now() < until, "the watches were set up");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn made(path: &str) -> Change {
    Change::Path { path: PathBuf::from(path), written: false }
}

fn written(path: &str) -> Change {
    Change::Path { path: PathBuf::from(path), written: true }
}

/// The changes due once things have been quiet.
fn due(w: &FolderWatch) -> Option<Due> {
    w.take(Instant::now() + QUIET).due
}

#[test]
fn a_change_marks_its_folder_and_is_due_once_things_go_quiet() {
    let (mut w, sink, log) = fake(&[]);
    sync(&mut w, &["/pics", "/pics/2026", "/b"], &["/pics/library"]);
    assert_eq!(*log.lock().unwrap(), ["watch /b", "watch /pics"], "a folder inside a watched one isn't watched twice");

    sink.push([
        made("/pics/trip/a.jpg"),
        written("/pics/trip/a.jpg"),
        made("/pics/2026/may"),
        made("/pics/.thumbs/a.jpg"),
        made("/pics/trip/.a.jpg.part"),
        written("/pics/trip/notes.txt"),
        made("/pics/library/catalog.json"),
        made("/elsewhere/a.jpg"),
        made("/b"),
    ]);
    let now = Instant::now();
    assert!(w.take(now).due.is_none(), "not while things may still change");
    assert!(w.wait(now).is_some_and(|d| d <= QUIET));
    assert_eq!(due(&w), Some(Due::Folders(vec!["/b".into(), "/pics/2026".into(), "/pics/trip".into()])));
    assert_eq!(due(&w), None, "handed out once");
    assert_eq!(w.wait(Instant::now()), None);

    // a log being written, or a hidden file, changes nothing at all
    sink.push([written("/pics/trip/notes.txt"), made("/pics/trip/.DS_Store")]);
    assert_eq!(w.wait(Instant::now()), None);

    // no longer Always: no longer watched, and its changes don't count
    sync(&mut w, &["/pics"], &[]);
    assert_eq!(log.lock().unwrap().last().map(String::as_str), Some("unwatch /b"));
    sink.push([made("/b/c.jpg")]);
    assert_eq!(due(&w), None);
}

#[test]
fn a_folder_that_cant_be_watched_is_said_once_and_left_to_the_rescans() {
    let (mut w, sink, _) = fake(&["/full"]);
    sync(&mut w, &["/full", "/pics"], &[]);
    let t = w.take(Instant::now());
    assert_eq!(t.failed.len(), 1);
    assert_eq!((t.failed[0].0.as_str(), t.failed[0].1.limit), ("/full", true));
    assert!(w.take(Instant::now()).failed.is_empty(), "said once");
    let roots = w.roots();
    assert_eq!(
        roots.iter().map(|(k, r)| (k.as_str(), r.watched, r.error.is_some())).collect::<Vec<_>>(),
        [("/full", false, true), ("/pics", true, false)]
    );
    sink.push([made("/full/a.jpg")]);
    assert_eq!(due(&w), None, "its changes aren't reported");

    // a folder made later that can't be watched: its watched folder is left to the rescans too
    let limit = WatchError { limit: true, message: "no more watches".into() };
    sink.push([Change::Failed { path: "/pics/new/deep".into(), error: limit.clone() }]);
    sink.push([Change::Failed { path: "/pics/new/deeper".into(), error: limit }]);
    let t = w.take(Instant::now());
    assert_eq!(t.failed.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["/pics"], "said once");
    assert!(w.roots().iter().any(|(k, r)| k == "/pics" && r.watched && r.error.is_some()), "still watched where it could be");

    // the library's command says so
    let mut session = Session::new();
    session.folder_watch = w;
    let r = session.execute("library.folderWatch", &json!({})).unwrap();
    assert_eq!(r["active"], true, "{r}");
    assert_eq!(r["folders"][0], json!({"path": "/full", "watched": false, "polling": true, "reason": "no more watches", "limit": true}));
    assert_eq!(r["folders"][1]["polling"], true, "{r}");
    // without a watcher (the web, the CLI), nothing is watched
    let r = Session::new().execute("library.folderWatch", &json!({})).unwrap();
    assert_eq!(r, json!({"active": false, "syncing": false, "folders": []}));
}

#[test]
fn lost_notifications_or_too_many_changes_rescan_everything() {
    let (mut w, sink, _) = fake(&[]);
    sync(&mut w, &["/pics"], &[]);
    sink.push([made("/pics/a/x.jpg"), Change::Lost]);
    assert_eq!(due(&w), Some(Due::All));
    sink.push((0..=MAX_DIRTY).map(|i| made(&format!("/pics/{i}/x.jpg"))));
    assert_eq!(due(&w), Some(Due::All));
    sink.push([made("/pics/a/x.jpg")]);
    assert_eq!(due(&w), Some(Due::Folders(vec!["/pics/a".into()])), "back to folders after");
}

/// The system's watcher reports a photo copied into a folder inside a watched one.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn the_native_watcher_reports_a_new_photo() {
    let dir = std::env::temp_dir().join(format!("lc-watch-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("trip")).unwrap();
    // (macOS reports /private/var/… for a temporary folder in /var/…)
    let dir = dir.canonicalize().unwrap();
    let make: MakeWatcher = Arc::new(crate::folder_watch::native);
    let mut w = FolderWatch::default();
    w.start(&make, Box::new(|| {})).unwrap();
    let root = dir.to_string_lossy().to_string();
    w.sync(std::slice::from_ref(&root), Vec::new(), &Activity::default());
    let until = Instant::now() + Duration::from_secs(10);
    while w.syncing() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(w.roots().iter().any(|(_, r)| r.watched), "{:?}", w.roots());
    std::fs::write(dir.join("trip/a.jpg"), b"not really a jpeg").unwrap();
    let trip = lightcraft_catalog::query::folder_key(&dir.join("trip").to_string_lossy());
    let until = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while Instant::now() < until {
        if let Some(Due::Folders(f)) = w.take(Instant::now() + QUIET).due {
            seen.extend(f);
            if seen.contains(&trip) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(seen.contains(&trip), "{seen:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scan of the folders that changed lists those, not the rest of the folder set to Always.
#[test]
fn a_scan_of_the_changed_folders_lists_just_those() {
    let src = std::env::temp_dir().join(format!("lc-watch-changed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&src);
    for d in ["a", "b", "never"] {
        std::fs::create_dir_all(src.join(d)).unwrap();
    }
    let key = |p: &Path| lightcraft_catalog::query::folder_key(&p.to_string_lossy());
    let mut session = Session::new().with_fs();
    session.execute("folder.scanMode", &json!({"path": key(&src), "mode": "always"})).unwrap();
    session.execute("folder.scanMode", &json!({"path": key(&src.join("never")), "mode": "never"})).unwrap();
    let job = crate::folder_scan::ScanJob::of_changed(&mut session, vec![key(&src.join("a")), key(&src.join("never")), "/elsewhere".into()]);
    assert_eq!(job.roots, [key(&src.join("a"))], "only folders set to Always");
    let scanned = job.run(&std::sync::atomic::AtomicBool::new(false));
    assert_eq!(scanned.listed, 1);
    let _ = std::fs::remove_dir_all(&src);
}

/// CPU time this process has used, in seconds (Linux: `/proc/self/stat`, at the usual 100 ticks
/// a second; `None` elsewhere).
fn cpu_secs() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = stat.rsplit_once(')')?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

/// Measurements for the folder workflow plan (Phase 4): a generated tree of 10,000 files in about
/// 1,000 folders, all in the library already, set to Always. Prints the startup scan (cold: every
/// folder listed; warm: the mtime skip), setting up the watches, idle CPU with the watches on
/// (`LC_BENCH_IDLE_SECS`, default 60), and the time and CPU from copying in 100 photos to their
/// import, through the watch as the app runs it.
/// `cargo test -p lightcraft-engine --lib bench_folder_watch -- --ignored --nocapture`
#[test]
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn bench_folder_watch() {
    use crate::folder_scan::{ScanJob, finish};
    use lightcraft_catalog::{Op, Photo, Source};
    use std::sync::atomic::AtomicBool;
    let root = std::env::temp_dir().join(format!("lc-bench-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut session = Session::new().with_fs();
    let mut folders = 1;
    for y in 0..10 {
        for m in 0..100 {
            let dir = root.join(format!("y{y}/m{m:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..10 {
                let path = dir.join(format!("IMG_{i:04}.jpg"));
                std::fs::write(&path, b"x").unwrap();
                let id = session.catalog.alloc_photo_id();
                let p = Photo::new(id, Source::File { path: path.to_string_lossy().to_string() }, "x.jpg", "JPEG", 4000, 3000, "2026-01-01T10:00:00");
                session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            }
            folders += 1;
        }
        folders += 1;
    }
    // left alone for a while, as a photo collection is
    let old = std::time::SystemTime::now() - Duration::from_secs(600);
    let mut dirs = vec![root.clone()];
    dirs.extend((0..10).map(|y| root.join(format!("y{y}"))));
    dirs.extend((0..10).flat_map(|y| (0..100).map(move |m| (y, m))).map(|(y, m)| root.join(format!("y{y}/m{m:02}"))));
    for d in &dirs {
        std::fs::File::open(d).unwrap().set_modified(old).unwrap();
    }
    let key = lightcraft_catalog::query::folder_key(&root.to_string_lossy());
    session.execute("folder.scanMode", &json!({"path": key, "mode": "always"})).unwrap();
    let files = session.catalog.len();
    eprintln!("tree: {files} files in {folders} folders");

    let measure = |label: &str, session: &mut Session, full: bool| {
        let (t0, c0) = (Instant::now(), cpu_secs());
        let job = ScanJob::new(session, None, full).unwrap();
        let r = finish(session, job.run(&AtomicBool::new(false)), true).unwrap();
        let (t, c) = (t0.elapsed(), cpu_secs().zip(c0).map(|(a, b)| a - b));
        eprintln!("{label}: {:.0} ms wall, {c:?} s CPU, listed {} unchanged {}", t.as_secs_f64() * 1000.0, r["listed"], r["unchanged"]);
    };
    measure("startup scan, cold (every folder listed)", &mut session, true);
    measure("startup scan, mtime-warm", &mut session, false);

    let make: MakeWatcher = Arc::new(crate::folder_watch::native);
    let (t0, c0) = (Instant::now(), cpu_secs());
    session.folder_watch.start(&make, Box::new(|| {})).unwrap();
    session.sync_folder_watch(std::slice::from_ref(&key));
    while session.folder_watch.syncing() {
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!(
        "watch setup: {:.0} ms wall, {:?} s CPU, {:?}",
        t0.elapsed().as_secs_f64() * 1000.0,
        cpu_secs().zip(c0).map(|(a, b)| a - b),
        session.folder_watch.roots()
    );

    let idle: u64 = std::env::var("LC_BENCH_IDLE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let c0 = cpu_secs();
    std::thread::sleep(Duration::from_secs(idle));
    eprintln!("idle {idle} s with the watches on: {:?} s CPU", cpu_secs().zip(c0).map(|(a, b)| a - b));

    // 100 photos copied into 10 folders, then picked up the way the app does it: the changed
    // folders once quiet, and a rescan once a file still settling has had time to
    let (t0, c0) = (Instant::now(), cpu_secs());
    for m in 0..10 {
        for i in 0..10 {
            let w = 400;
            let data: Vec<[u8; 4]> = (0..w * 300).map(|p| [(p % w) as u8, (p / w) as u8, (m * 10 + i) as u8, 255]).collect();
            let img = lightcraft_raster::Rgba8 { width: w, height: 300, data };
            let bytes =
                lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
            std::fs::write(root.join(format!("y5/m{m:02}/new_{i}.png")), bytes).unwrap();
        }
    }
    let copied = t0.elapsed();
    let mut recheck: Option<Instant> = None;
    let deadline = Instant::now() + Duration::from_secs(120);
    let (mut changed_scans, mut rescans) = (0, 0);
    while session.catalog.len() < files + 100 && Instant::now() < deadline {
        let now = Instant::now();
        let mut r = None;
        if let Some(due) = session.folder_watch.take(now).due {
            changed_scans += 1;
            let job = match due {
                Due::Folders(f) => ScanJob::of_changed(&mut session, f),
                Due::All => ScanJob::new(&mut session, None, false).unwrap(),
            };
            r = Some(finish(&mut session, job.run(&AtomicBool::new(false)), true).unwrap());
        } else if recheck.is_some_and(|t| now >= t) {
            rescans += 1;
            recheck = None;
            let job = ScanJob::new(&mut session, None, false).unwrap();
            r = Some(finish(&mut session, job.run(&AtomicBool::new(false)), true).unwrap());
        }
        if r.is_some_and(|r| r["pending"].as_u64().is_some_and(|n| n > 0)) {
            recheck = Some(Instant::now() + Duration::from_secs(crate::folder_scan::SETTLE_SECS + 1));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!(
        "100 photos: copied in {:.0} ms; imported {} after {:.1} s ({changed_scans} changed-folder scans, {rescans} rescans), {:?} s CPU",
        copied.as_secs_f64() * 1000.0,
        session.catalog.len() - files,
        t0.elapsed().as_secs_f64(),
        cpu_secs().zip(c0).map(|(a, b)| a - b)
    );
    assert_eq!(session.catalog.len(), files + 100);
    let _ = std::fs::remove_dir_all(&root);
}
