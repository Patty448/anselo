//! Folder scans: where the library looks for new photos by itself, folder by folder.
//!
//! Each folder on disk has a scan mode ([`ScanMode`], kept in the catalog as part of the folder's
//! record and inherited by the folders inside it):
//! - **Never**: its photos are never added by a scan (nor by Auto Import). A few places are Never
//!   without the library keeping anything ([`default_never`]: system folders, caches, the library
//!   itself and LightCraft's own settings); a mode set on one overrides it.
//! - **Once**: its photos were added when the mode was set (`folder.scanMode`); nothing more.
//! - **Always**: its new photos are added whenever the Always folders are scanned
//!   (`library.scanFolders`: at startup, after a scan setting changes and every
//!   [`crate::import::ImportDefaults::scan_rescan_minutes`]).
//!
//! A scan ([`ScanJob::run`], on a worker thread) walks the Always folders, leaving out folders
//! whose mode is not Always, hidden ones and the library. A folder whose modification time is the
//! one the last scan saw is not listed again: its files are as they were, and the folders inside
//! it are taken from the last scan ([`ScanState`], kept in the library as [`STATE_FILE`], so it
//! survives restarts). [`finish`] then hands the files the library doesn't know yet to an
//! add-in-place import, once each has settled (its size held between two scans, or it was last
//! written [`SETTLE_SECS`] ago), with the library's minimum image size
//! ([`crate::import::ImportDefaults::scan_min_long_edge`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::query::{folder_key, key_within};
use lightcraft_catalog::{ScanMode, ScanRules};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Session;

/// The library file that keeps [`ScanState`].
pub const STATE_FILE: &str = "scan-state.json";
/// A file last written this long ago is taken to be complete without waiting for a second scan.
pub const SETTLE_SECS: u64 = 10;
/// Folders a scan looks at at most (all its roots together): far more than a photo collection
/// spreads over, far fewer than a whole system drive.
pub const MAX_FOLDERS: usize = 200_000;
/// Folder levels below a root a scan goes at most.
pub const MAX_DEPTH: usize = crate::walk::MAX_DEPTH;

/// The kind of system [`default_never`] lists places for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Linux, the BSDs and other Unix-like systems.
    Unix,
    Mac,
    Windows,
}

impl Platform {
    /// This build's system.
    pub fn this() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }
}

/// The places that are Never without the library keeping anything: the system's own folders,
/// per-user caches and settings, the open library `library` and LightCraft's settings folder
/// `config`. `home` is the user's home folder; `env` reads environment variables (Windows'
/// `%APPDATA%` and the like).
pub fn default_never(
    platform: Platform,
    home: Option<&Path>,
    library: Option<&Path>,
    config: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let in_home = |rel: &str| home.map(|h| h.join(rel).to_string_lossy().to_string());
    match platform {
        Platform::Unix => {
            out.extend(["/usr", "/opt", "/proc", "/sys", "/dev", "/run", "/boot", "/etc", "/var", "/snap"].map(String::from));
            out.extend([".cache", ".local/share", ".config"].iter().filter_map(|r| in_home(r)));
        }
        Platform::Mac => {
            out.extend(["/System", "/Library", "/private", "/usr", "/opt", "/bin", "/sbin", "/dev", "/cores"].map(String::from));
            out.extend(in_home("Library"));
        }
        Platform::Windows => {
            let var = |name: &str, fallback: &str| {
                env(name).filter(|v| !v.is_empty()).map(|v| v.to_string_lossy().to_string()).unwrap_or(fallback.to_string())
            };
            out.push(var("SystemRoot", r"C:\Windows"));
            out.push(var("ProgramFiles", r"C:\Program Files"));
            out.push(var("ProgramFiles(x86)", r"C:\Program Files (x86)"));
            out.push(var("ProgramData", r"C:\ProgramData"));
            out.extend(["APPDATA", "LOCALAPPDATA"].iter().filter_map(|n| env(n).filter(|v| !v.is_empty()).map(|v| v.to_string_lossy().to_string())));
        }
    }
    out.extend([library, config].into_iter().flatten().map(|p| p.to_string_lossy().to_string()));
    let mut seen = HashSet::new();
    out.retain(|p| seen.insert(folder_key(p)));
    out
}

impl Session {
    /// The built-in Never locations on this machine, for this library ([`default_never`]).
    pub fn scan_defaults(&self) -> Vec<String> {
        let platform = Platform::this();
        let home = std::env::var_os(if platform == Platform::Windows { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(PathBuf::from);
        let library = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        let config = crate::config::config_dir();
        default_never(platform, home.as_deref(), library.as_deref(), config.as_deref(), |n| std::env::var_os(n))
    }

    /// Where folder scans look ([`lightcraft_catalog::Catalog::scan_rules`] with this machine's
    /// defaults).
    pub fn scan_rules(&self) -> ScanRules {
        self.catalog.scan_rules(&self.scan_defaults())
    }

    /// The scan state, read from the library the first time it is asked for.
    fn scan_state(&mut self) -> ScanState {
        if self.scan_state.is_none() {
            let read = self.library.as_mut().and_then(|l| l.read_file(STATE_FILE));
            let state = read
                .and_then(|b| serde_json::from_slice::<ScanState>(&b).map_err(|e| log::warn!("{STATE_FILE}: {e}; folders are listed afresh")).ok());
            self.scan_state = Some(state.unwrap_or_default());
        }
        self.scan_state.clone().unwrap_or_default()
    }

    fn set_scan_state(&mut self, state: ScanState) {
        if let Some(lib) = self.library.as_mut()
            && let Ok(b) = serde_json::to_vec(&state)
            && let Err(e) = lib.write_file(STATE_FILE, &b)
        {
            log::warn!("{STATE_FILE}: {e}");
        }
        self.scan_state = Some(state);
    }

    /// Watch the folders `roots` (the folders set to Always) for changes, leaving out the library
    /// ([`crate::folder_watch::FolderWatch::sync`]; nothing until the host has started watching).
    pub fn sync_folder_watch(&mut self, roots: &[String]) {
        let skip = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.to_string_lossy().to_string()).into_iter().collect();
        self.folder_watch.sync(roots, skip, &self.activity);
    }

    /// Forget what the last scans saw in folder `path` and the folders inside it: the next scan
    /// lists them all (a folder just set to Always may hold files the library let go of).
    pub fn forget_scan_state(&mut self, path: &str) {
        let key = folder_key(path);
        let mut state = self.scan_state();
        let before = state.dirs.len();
        state.dirs.retain(|k, _| !key_within(k, &key));
        if state.dirs.len() != before {
            self.set_scan_state(state);
        }
    }
}

/// What the last scans saw of each folder they listed, by folder key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanState {
    pub dirs: BTreeMap<String, DirState>,
}

/// A folder as a scan saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirState {
    /// Its modification time (nanoseconds since 1970): it changes when a name in it is added,
    /// removed or renamed.
    pub mtime: u64,
    /// The folders in it (names), followed without listing it again.
    pub subdirs: Vec<String>,
}

/// A file a scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    pub size: u64,
    /// Seconds since it was last written (`None`: not known).
    pub age: Option<u64>,
    /// The key of the folder it is in.
    pub dir: String,
}

/// A scan of the Always folders, detached from the session to run on a worker thread
/// ([`ScanJob::new`], then [`ScanJob::run`], then [`finish`]).
pub struct ScanJob {
    pub roots: Vec<String>,
    rules: ScanRules,
    lib_dir: Option<PathBuf>,
    state: ScanState,
    /// List every folder, also unchanged ones.
    pub full: bool,
}

/// What [`ScanJob::run`] found.
#[derive(Debug, Default)]
pub struct Scanned {
    pub roots: Vec<String>,
    pub files: Vec<Found>,
    /// The scan state with what this scan saw.
    pub state: ScanState,
    /// Folders listed, and folders passed over because they had not changed.
    pub listed: usize,
    pub unchanged: usize,
    /// The scan stopped early (cancelled, or at [`MAX_FOLDERS`] / [`MAX_DEPTH`]).
    pub incomplete: bool,
}

impl ScanJob {
    /// A scan of `roots` (default: every folder set to Always), listing unchanged folders too
    /// when `full`. A root whose mode is not Always is refused.
    pub fn new(s: &mut Session, roots: Option<Vec<String>>, full: bool) -> Result<ScanJob, String> {
        let rules = s.scan_rules();
        let roots = match roots {
            Some(r) => {
                for root in &r {
                    if rules.mode(root).mode != Some(ScanMode::Always) {
                        return Err(format!("{root} is not set to Always (folder.scanMode)"));
                    }
                }
                r
            }
            None => rules.set_to(ScanMode::Always).filter(|k| rules.mode(k).mode == Some(ScanMode::Always)).map(str::to_string).collect(),
        };
        let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        Ok(ScanJob { roots, rules, lib_dir, state: s.scan_state(), full })
    }

    /// A scan of just the folders `changed` (folders in the Always folders where something
    /// changed, [`crate::folder_watch`]): those not set to Always are left out, and one inside
    /// another is covered by it.
    pub fn of_changed(s: &mut Session, changed: Vec<String>) -> ScanJob {
        let rules = s.scan_rules();
        let mut keys: Vec<String> = changed.iter().map(|c| folder_key(c)).filter(|k| rules.mode(k).mode == Some(ScanMode::Always)).collect();
        keys.sort();
        keys.dedup();
        let roots = keys.iter().filter(|k| !keys.iter().any(|o| o != *k && key_within(k, o))).cloned().collect();
        let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        ScanJob { roots, rules, lib_dir, state: s.scan_state(), full: false }
    }

    /// Walk the roots (no catalog access; stops before the next folder once `cancel` is set).
    pub fn run(self, cancel: &AtomicBool) -> Scanned {
        let ScanJob { roots, rules, lib_dir, state, full } = self;
        // (the browser build has no folders to walk, nor a clock std can read)
        if cfg!(target_arch = "wasm32") {
            return Scanned { roots, state, ..Default::default() };
        }
        let lib_key = lib_dir.as_ref().map(|l| folder_key(&l.to_string_lossy()));
        let now = std::time::SystemTime::now();
        let mut out = Scanned { roots: roots.clone(), state: state.clone(), ..Default::default() };
        let mut visited: HashSet<String> = HashSet::new();
        let mut folders = 0usize;
        for root in &roots {
            let mut stack: Vec<(PathBuf, usize)> = vec![(PathBuf::from(root), 0)];
            while let Some((dir, depth)) = stack.pop() {
                if cancel.load(Ordering::Relaxed) || folders >= MAX_FOLDERS {
                    out.incomplete = true;
                    break;
                }
                let key = folder_key(&dir.to_string_lossy());
                if !visited.insert(key.clone()) {
                    continue;
                }
                if depth > 0 && (hidden(&dir) || rules.mode(&key).mode != Some(ScanMode::Always)) {
                    continue;
                }
                if lib_key.as_ref().is_some_and(|l| key_within(&key, l)) {
                    continue;
                }
                // a link to a folder is not followed (it may lead anywhere, or round in a loop)
                let meta = if depth == 0 { std::fs::metadata(&dir) } else { std::fs::symlink_metadata(&dir) };
                let Some(meta) = meta.ok().filter(|m| m.is_dir()) else { continue };
                folders += 1;
                if depth >= MAX_DEPTH {
                    out.incomplete = true;
                    continue;
                }
                let mtime = meta.modified().ok().map(nanos);
                let below = depth.saturating_add(1);
                if !full
                    && let Some(m) = mtime
                    && let Some(seen) = state.dirs.get(&key).filter(|d| d.mtime == m)
                {
                    out.unchanged += 1;
                    stack.extend(seen.subdirs.iter().rev().map(|n| (dir.join(n), below)));
                    continue;
                }
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    out.state.dirs.remove(&key);
                    continue;
                };
                out.listed += 1;
                let mut subdirs = Vec::new();
                for e in rd.flatten() {
                    let name = e.file_name();
                    if name.to_string_lossy().starts_with('.') {
                        continue;
                    }
                    let Ok(ft) = e.file_type() else { continue };
                    let path = e.path();
                    if ft.is_dir() {
                        subdirs.push(name.to_string_lossy().to_string());
                    } else if ft.is_file() && crate::import::is_supported(&path) {
                        let Ok(m) = e.metadata() else { continue };
                        let age = m.modified().ok().and_then(|t| now.duration_since(t).ok()).map(|d| d.as_secs());
                        out.files.push(Found { path: path.to_string_lossy().to_string(), size: m.len(), age, dir: key.clone() });
                    }
                }
                subdirs.sort();
                stack.extend(subdirs.iter().rev().map(|n| (dir.join(n), below)));
                // a folder changed in the second it was listed may change again unseen in the same
                // tick: listed again next time
                let settled = mtime.is_some_and(|m| nanos(now).saturating_sub(m) > 2_000_000_000);
                match mtime.filter(|_| settled) {
                    Some(mtime) => {
                        out.state.dirs.insert(key, DirState { mtime, subdirs });
                    }
                    None => {
                        out.state.dirs.remove(&key);
                    }
                }
            }
        }
        // what a complete scan didn't reach under its roots is gone (or no longer Always)
        if !out.incomplete {
            let roots: Vec<String> = roots.iter().map(|r| folder_key(r)).collect();
            out.state.dirs.retain(|k, _| visited.contains(k) || !roots.iter().any(|r| key_within(k, r)));
        }
        out
    }
}

fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn nanos(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)).unwrap_or(0)
}

/// After a scan: keep what it saw, and add the files the library doesn't have that have settled
/// (the rest wait, and their folders are listed again next time). With `start` false the import
/// is not run but returned as `import` (`library.import` params; the app runs it on a worker
/// thread). → `{roots, listed, unchanged, found, pending, imported, import?, incomplete}`
pub fn finish(s: &mut Session, scanned: Scanned, start: bool) -> crate::Result<Value> {
    let Scanned { roots, files, mut state, listed, unchanged, incomplete } = scanned;
    let known: HashSet<&str> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    let mut fresh: Vec<String> = Vec::new();
    let mut waiting: HashMap<String, usize> = HashMap::new();
    let found = files.len();
    for f in files {
        if known.contains(f.path.as_str()) || s.auto_import_seen.get(&f.path) == Some(&u64::MAX) {
            continue;
        }
        let before = s.auto_import_seen.insert(f.path.clone(), f.size);
        let settled = f.size > 0 && (before == Some(f.size) || f.age.is_some_and(|a| a >= SETTLE_SECS));
        if settled {
            fresh.push(f.path);
        } else {
            *waiting.entry(f.dir).or_default() += 1;
        }
    }
    drop(known);
    for dir in waiting.keys() {
        state.dirs.remove(dir);
    }
    s.set_scan_state(state);
    let pending: usize = waiting.values().sum();
    let mut result = json!({"roots": roots, "listed": listed, "unchanged": unchanged, "found": found, "pending": pending, "imported": [], "incomplete": incomplete});
    if fresh.is_empty() {
        return Ok(result);
    }
    for f in &fresh {
        s.auto_import_seen.insert(f.clone(), u64::MAX);
    }
    // a photo deleted from the library doesn't come back on its own because its file is still there
    let params = json!({"paths": fresh, "mode": "add", "onDeleted": "skip", "minLongEdge": s.import_defaults.scan_min_long_edge()});
    if !start {
        result["import"] = params;
        return Ok(result);
    }
    let sel = s.selection.clone();
    let r = s.execute("library.import", &params)?;
    // arriving photos don't take over the selection
    s.selection = sel;
    result["imported"] = r["imported"].clone();
    if let Some(sk) = r.get("skipped") {
        result["skipped"] = sk.clone();
    }
    Ok(result)
}

/// The `library.import` params of a Once scan of folder `path`: its photos, added where they
/// are, leaving out the Never folders inside it and small images.
pub fn once_params(s: &Session, path: &str) -> Value {
    let rules = s.scan_rules();
    let key = folder_key(path);
    let inside = |k: &str| k != key && key_within(k, &key);
    let defaults = s.scan_defaults();
    let candidates = rules.set_to(ScanMode::Never).map(str::to_string).chain(defaults.iter().map(|d| folder_key(d)));
    let mut exclude: Vec<String> = candidates.filter(|k| inside(k) && rules.mode(k).mode == Some(ScanMode::Never)).collect();
    exclude.sort();
    exclude.dedup();
    json!({"paths": [path], "mode": "add", "onDeleted": "skip", "minLongEdge": s.import_defaults.scan_min_long_edge(), "exclude": exclude})
}
