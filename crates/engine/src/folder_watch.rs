//! Watching the folders set to Always for changes (see [`crate::folder_scan`]): instead of waiting
//! for the next rescan, a folder in which a name was added, removed or renamed, or a photo
//! written, is scanned on its own shortly after things go quiet there.
//!
//! The system's notifications come through a [`FolderWatcher`] (the native one, [`native`], uses
//! the `notify` crate: inotify, FSEvents, ReadDirectoryChangesW; tests give their own). Each change
//! reaches a [`Sink`], which marks the folder it happened in as changed, leaving out hidden
//! entries, the library, folders no longer watched and writes to files a scan doesn't look at.
//! [`FolderWatch::take`] hands the changed folders out once nothing has changed for [`QUIET`] (or
//! [`MAX_WAIT`] after the first change, while changes keep coming).
//!
//! A folder that can't be watched (the system's watch limit, e.g. inotify's `max_user_watches`, or
//! another error) is left to the regular rescans and reported once ([`FolderWatch::take`],
//! `library.folderWatch`). Watches are set up on a worker thread with a row in the activity stack:
//! on Linux, watching a large folder tree means one watch per folder in it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use lightcraft_catalog::query::{folder_key, key_within};
use web_time::Instant;

use crate::activity::{Activity, Cancel};

/// How long nothing has to change before the changed folders are scanned (a copy in progress
/// keeps changing).
pub const QUIET: Duration = Duration::from_secs(2);
/// The longest a change waits for things to go quiet.
pub const MAX_WAIT: Duration = Duration::from_secs(10);
/// Changed folders kept at most; past that, all the watched folders are rescanned.
pub const MAX_DIRTY: usize = 10_000;
/// The activity stack's kind for setting up watches.
pub const KIND: &str = "watchFolders";

/// Why a folder can't be watched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchError {
    /// The system's limit on watches was reached.
    pub limit: bool,
    pub message: String,
}

/// The system's change notifications for whole folder trees. `watch` reports through the [`Sink`]
/// it was made with.
pub trait FolderWatcher: Send {
    /// Report the changes in folder `root` and every folder inside it.
    fn watch(&mut self, root: &Path) -> Result<(), WatchError>;
    fn unwatch(&mut self, root: &Path);
}

/// Makes the watcher (the host decides: [`native`] on the desktop, none on the web).
pub type MakeWatcher = Arc<dyn Fn(Sink) -> Result<Box<dyn FolderWatcher>, String> + Send + Sync>;

/// Called (from any thread) when something arrives that the UI should look at.
pub type Wake = Box<dyn Fn() + Send + Sync>;

/// What a watcher saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// File or folder `path` was added, removed or renamed (`written`: only its contents or
    /// attributes changed).
    Path { path: PathBuf, written: bool },
    /// Notifications were lost (the system's queue overflowed): anything may have changed.
    Lost,
    /// A folder inside a watched one (`path`, made after the watch began) couldn't be watched.
    Failed { path: PathBuf, error: WatchError },
}

/// How a folder set to Always is watched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootWatch {
    /// Changes in it are reported (all of them, or some when `error` is set as well).
    pub watched: bool,
    /// Why it isn't (fully) watched: it is left to the regular rescans.
    pub error: Option<WatchError>,
}

/// Where a watcher's changes go (cheap to clone: one `Arc`).
#[derive(Clone)]
pub struct Sink {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    wake: Wake,
}

#[derive(Default)]
struct State {
    /// The folders that should be watched (keys), and the ones that are.
    want: Vec<String>,
    roots: BTreeMap<String, RootWatch>,
    /// Changes inside these (the library) don't count.
    skip: Vec<String>,
    /// The folders changed (keys), or all of them.
    dirty: BTreeSet<String>,
    all: bool,
    first: Option<Instant>,
    last: Option<Instant>,
    /// Folders newly left to the rescans, not yet handed out.
    failed: Vec<(String, WatchError)>,
    /// A worker is setting up watches.
    syncing: bool,
}

impl Sink {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Note what a watcher saw.
    pub fn push(&self, changes: impl IntoIterator<Item = Change>) {
        let now = Instant::now();
        let mut woke = false;
        {
            let mut st = self.lock();
            // (every change is noted: not stopping at the first that counts)
            for c in changes {
                woke |= st.note(c, now);
            }
        }
        if woke {
            (self.shared.wake)();
        }
    }
}

impl State {
    /// → whether it counts.
    fn note(&mut self, change: Change, now: Instant) -> bool {
        match change {
            Change::Path { path, written } => {
                let Some(dir) = self.changed_folder(&path, written) else { return false };
                if self.all {
                    // (every folder is scanned already)
                } else if self.dirty.len() >= MAX_DIRTY {
                    self.all = true;
                    self.dirty.clear();
                } else {
                    self.dirty.insert(dir);
                }
            }
            Change::Lost => {
                self.all = true;
                self.dirty.clear();
            }
            Change::Failed { path, error } => {
                let key = folder_key(&path.to_string_lossy());
                let Some((root, w)) = self.roots.iter_mut().find(|(r, w)| w.watched && key_within(&key, r)) else { return false };
                if w.error.is_some() {
                    return false;
                }
                w.error = Some(error.clone());
                self.failed.push((root.clone(), error));
                return true;
            }
        }
        self.first.get_or_insert(now);
        self.last = Some(now);
        true
    }

    /// The folder a change at `path` happened in (a key), when it is one a scan looks at.
    fn changed_folder(&self, path: &Path, written: bool) -> Option<String> {
        let key = folder_key(&path.to_string_lossy());
        let root = self.roots.iter().filter(|(r, w)| w.watched && key_within(&key, r)).map(|(r, _)| r).max_by_key(|r| r.len())?;
        if self.skip.iter().any(|s| key_within(&key, s)) {
            return None;
        }
        let rest = key.get(root.len()..).unwrap_or_default();
        if rest.split('/').any(|n| n.starts_with('.')) {
            return None;
        }
        // a file being written that no scan would pick up (a log, a sidecar) changes nothing
        if written && !crate::import::is_supported(path) {
            return None;
        }
        if rest.trim_matches('/').is_empty() {
            return Some(root.clone());
        }
        let parent = key.rsplit_once('/').map(|(p, _)| p)?;
        Some(if parent.len() < root.len() { root.clone() } else { parent.to_string() })
    }
}

/// What changed, once it is time to scan it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    /// Every watched folder (changes were lost, or too many folders changed).
    All,
    /// These folders (keys), each without the folders inside it unless they changed too.
    Folders(Vec<String>),
}

/// What [`FolderWatch::take`] hands out.
#[derive(Debug, Default)]
pub struct Taken {
    /// Folders set to Always newly left to the rescans, and why.
    pub failed: Vec<(String, WatchError)>,
    pub due: Option<Due>,
}

/// The watches on the folders set to Always (kept in the session; idle until
/// [`FolderWatch::start`]).
#[derive(Default)]
pub struct FolderWatch {
    watcher: Option<Arc<Mutex<Box<dyn FolderWatcher>>>>,
    sink: Option<Sink>,
    tried: bool,
}

impl FolderWatch {
    /// Start watching with the watcher `make` makes; `wake` is called when changes arrive.
    pub fn start(&mut self, make: &MakeWatcher, wake: Wake) -> Result<(), String> {
        self.tried = true;
        let sink = Sink { shared: Arc::new(Shared { state: Mutex::new(State::default()), wake }) };
        let watcher = make(sink.clone())?;
        self.watcher = Some(Arc::new(Mutex::new(watcher)));
        self.sink = Some(sink);
        Ok(())
    }

    /// [`FolderWatch::start`] was called (whether or not it worked).
    pub fn started(&self) -> bool {
        self.tried
    }

    /// Changes are watched for.
    pub fn active(&self) -> bool {
        self.watcher.is_some()
    }

    /// Watch the folders `roots` (keys of the folders set to Always; one inside another is
    /// covered by it), leaving out changes inside `skip` (the library). The watches are set up and
    /// taken down on a worker thread, with a row in `activity` while that lasts.
    pub fn sync(&mut self, roots: &[String], skip: Vec<String>, activity: &Activity) {
        let (Some(watcher), Some(sink)) = (&self.watcher, &self.sink) else { return };
        let mut want: Vec<String> = roots.iter().map(|r| folder_key(r)).filter(|r| !r.is_empty()).collect();
        want.sort();
        want.dedup();
        let outer: Vec<String> = want.iter().filter(|r| !want.iter().any(|o| o != *r && key_within(r, o))).cloned().collect();
        {
            let mut st = sink.lock();
            st.want = outer;
            st.skip = skip.iter().map(|s| folder_key(s)).collect();
            if st.syncing {
                // the running worker picks the change up
                return;
            }
            st.syncing = true;
        }
        let (watcher, sink, activity) = (watcher.clone(), sink.clone(), activity.clone());
        let work = move || {
            let _row = activity.start(KIND, "Watching folders for changes", Cancel::No);
            reconcile(&watcher, &sink);
        };
        if let Err(e) = std::thread::Builder::new().name("folder-watch".into()).spawn(work) {
            log::warn!("folder watch: {e}");
            if let Some(s) = &self.sink {
                s.lock().syncing = false;
            }
        }
    }

    /// The watches are still being set up or taken down.
    pub fn syncing(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| s.lock().syncing)
    }

    /// The folders newly left to the rescans, and the changed folders once they are due (nothing
    /// has changed for [`QUIET`], or the first change is [`MAX_WAIT`] old).
    pub fn take(&self, now: Instant) -> Taken {
        let Some(sink) = &self.sink else { return Taken::default() };
        let mut st = sink.lock();
        let failed = std::mem::take(&mut st.failed);
        let (Some(first), Some(last)) = (st.first, st.last) else { return Taken { failed, due: None } };
        if now.saturating_duration_since(last) < QUIET && now.saturating_duration_since(first) < MAX_WAIT {
            return Taken { failed, due: None };
        }
        let dirty = std::mem::take(&mut st.dirty);
        let due = if std::mem::take(&mut st.all) { Due::All } else { Due::Folders(dirty.into_iter().collect()) };
        st.first = None;
        st.last = None;
        Taken { failed, due: Some(due) }
    }

    /// How long until changes noted so far are due (`None`: none are waiting).
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let st = self.sink.as_ref()?.lock();
        let (first, last) = (st.first?, st.last?);
        let at = (last + QUIET).min(first + MAX_WAIT);
        Some(at.saturating_duration_since(now))
    }

    /// The folders set to Always and how each is watched.
    pub fn roots(&self) -> Vec<(String, RootWatch)> {
        self.sink.as_ref().map(|s| s.lock().roots.iter().map(|(k, w)| (k.clone(), w.clone())).collect()).unwrap_or_default()
    }
}

/// Bring the watches in line with what is wanted, until nothing more changes.
fn reconcile(watcher: &Mutex<Box<dyn FolderWatcher>>, sink: &Sink) {
    let mut w = watcher.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let (add, remove) = {
            let mut st = sink.lock();
            let remove: Vec<String> = st.roots.keys().filter(|r| !st.want.contains(r)).cloned().collect();
            let add: Vec<String> = st.want.iter().filter(|r| !st.roots.contains_key(*r)).cloned().collect();
            if add.is_empty() && remove.is_empty() {
                st.syncing = false;
                return;
            }
            (add, remove)
        };
        for root in remove {
            let watched = sink.lock().roots.remove(&root).is_some_and(|r| r.watched);
            if watched {
                w.unwatch(Path::new(&root));
            }
        }
        for root in add {
            let result = w.watch(Path::new(&root));
            let mut st = sink.lock();
            match result {
                Ok(()) => {
                    st.roots.insert(root, RootWatch { watched: true, error: None });
                }
                Err(e) => {
                    log::warn!("folder watch: {root}: {}", e.message);
                    st.failed.push((root.clone(), e.clone()));
                    st.roots.insert(root, RootWatch { watched: false, error: Some(e) });
                }
            }
        }
        (sink.shared.wake)();
    }
}

/// The system's watcher (`notify`): inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on
/// Windows, kqueue on the BSDs. Links to folders are not followed.
#[cfg(not(target_arch = "wasm32"))]
pub fn native(sink: Sink) -> Result<Box<dyn FolderWatcher>, String> {
    use notify::Watcher as _;
    let config = notify::Config::default().with_follow_symlinks(false);
    let handler = move |res: notify::Result<notify::Event>| match res {
        Ok(ev) => sink.push(changes(&ev)),
        Err(e) => match e.paths.first() {
            Some(p) => sink.push([Change::Failed { path: p.clone(), error: watch_error(&e) }]),
            None => log::warn!("folder watch: {e}"),
        },
    };
    let w = notify::RecommendedWatcher::new(handler, config).map_err(|e| e.to_string())?;
    Ok(Box::new(Native { w }))
}

#[cfg(not(target_arch = "wasm32"))]
struct Native {
    w: notify::RecommendedWatcher,
}

#[cfg(not(target_arch = "wasm32"))]
impl FolderWatcher for Native {
    fn watch(&mut self, root: &Path) -> Result<(), WatchError> {
        use notify::Watcher as _;
        self.w.watch(root, notify::RecursiveMode::Recursive).map_err(|e| {
            // (a recursive watch that failed part of the way keeps the watches it made)
            let _ = self.w.unwatch(root);
            watch_error(&e)
        })
    }

    fn unwatch(&mut self, root: &Path) {
        use notify::Watcher as _;
        if let Err(e) = self.w.unwatch(root) {
            log::debug!("folder watch: {}: {e}", root.display());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn watch_error(e: &notify::Error) -> WatchError {
    WatchError { limit: matches!(e.kind, notify::ErrorKind::MaxFilesWatch), message: e.to_string() }
}

/// What a `notify` event means for scans.
#[cfg(not(target_arch = "wasm32"))]
fn changes(ev: &notify::Event) -> Vec<Change> {
    use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind};
    if ev.need_rescan() {
        return vec![Change::Lost];
    }
    let written = match ev.kind {
        // a file closed after writing: done (the last word on a copy)
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => return Vec::new(),
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Any) => true,
        _ => false,
    };
    ev.paths.iter().map(|p| Change::Path { path: p.clone(), written }).collect()
}
