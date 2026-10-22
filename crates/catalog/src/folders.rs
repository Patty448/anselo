//! The library's folder tree: where on disk the imported photos live.
//!
//! A *folder* here is a folder holding at least one photo of the library (imported, not deleted,
//! not merely browsed in Local; demo scenes have no folder). The tree lists those folders and the
//! ones above them, never a folder that holds nothing of the library, with how many photos each
//! holds — its subfolders included, like a year in By Date counts its months.
//!
//! Every folder is a row of its own, the ones above the imported photos included
//! (`/Users` → `me` → `Pictures`), so a row is exactly the folder it names. A folder spelled two
//! ways (`/` or `\`, repeated or trailing separators, `.`) is one folder (see
//! [`crate::query::folder_key`]).
//!
//! Each disk is a *volume*, a top-level row of its own: a drive letter (`D:`), a Windows share
//! (`//server/share`), a mounted disk or share (macOS `/Volumes/tokyo`, Linux `/media/me/usb`,
//! `/mnt/disk`, `/run/user/1000/gvfs/smb-share:…`, `/net/host`) or, for everything else, the
//! startup disk (`/`). Only the spelling of the stored paths tells them apart: no disk is
//! touched, so a share that is asleep or gone lists and counts like one that is there, and the
//! sidebar never waits on the network.
//!
//! Limits of reading only the stored spelling of a path: file names are compared as written
//! (Windows folds case, other systems do not), so on a case-insensitive disk `/Volumes/tokyo` and
//! `/Volumes/TOKYO` are two volumes here; and one place reached by two spellings
//! (`/Volumes/Macintosh HD/Users/me` and `/Users/me`, `/media/me/usb` and `/run/media/me/usb`) is
//! two rows. Each row is still consistent: choosing it shows exactly the photos it counts.
//!
//! A folder is otherwise only where photos are, but the library can also keep a
//! [`FolderRecord`] about it (its colour label, and whether folder scans look in it: its
//! [`ScanMode`], also for a folder that holds no photo yet), stored under the folder's identity
//! ([`crate::query::folder_key`]). A record follows its folder when the folder is renamed or moved
//! here ([`Catalog::folder_records_follow`]); one about a folder that no longer holds a photo is
//! kept (undo can bring the photos back) and shows again when the folder does.
//!
//! This is the library's own view of its photos; browsing any folder on disk without importing
//! is Local's job (see [`crate::local`]).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::query::{folder_key, folder_rest, folder_within, key_within, split_path};
use crate::{Catalog, ColorLabel, Op, Source};

/// Folder levels kept per photo. No real path comes near it; a deeper one is counted at its
/// ancestor this many levels down, which bounds the tree's depth (and so every walk of it)
/// whatever a catalog holds.
pub const MAX_DEPTH: usize = 64;

/// One row of the library's folder tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FolderNode {
    /// What the row says: the folder's name; for a volume, its name (`nas`, `D:`,
    /// `//server/share`, `/` for the startup disk).
    pub name: String,
    /// The folder the row stands for, written out from the names of the photos' paths (forward
    /// slashes, no verbatim prefix); hand it back to [`crate::Filter::library_folder`] to see its
    /// photos.
    pub path: String,
    /// Photos in this folder or any folder inside it.
    pub count: usize,
    /// Photos directly in this folder.
    pub own: usize,
    /// A disk rather than a folder in one; its `path` is where it is mounted.
    pub volume: bool,
    /// Whether choosing the row (`Filter::library_folder`) shows exactly its `count` photos. Not
    /// so for the startup disk and for folders that hold other disks (`/Volumes`, `/mnt`…): their
    /// path covers those disks' photos too, so choosing them is not offered.
    pub selectable: bool,
    /// The folder's colour label (see [`FolderRecord`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<ColorLabel>,
    pub children: Vec<FolderNode>,
}

/// What the library keeps about one of its folders beyond the photos in it. A record with
/// nothing in it is not kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRecord {
    /// A colour label, as a photo has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ColorLabel>,
    /// Whether folder scans look in this folder and the folders inside it (`None`: as the folder
    /// above says, see [`Catalog::folder_scan_mode`]). Format version 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<ScanMode>,
}

impl FolderRecord {
    /// Whether the record says nothing (and so is not kept).
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.scan.is_none()
    }
}

/// Whether the library looks for new photos in a folder on disk (and the folders inside it,
/// unless one of them says otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    /// Never: its photos are not added by a folder scan (nor by Auto Import).
    Never,
    /// Its photos were added once, when the mode was set; later files are not looked for.
    Once,
    /// Its new photos are added whenever it is scanned.
    Always,
}

impl ScanMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "never" => Some(Self::Never),
            "once" => Some(Self::Once),
            "always" => Some(Self::Always),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Once => "once",
            Self::Always => "always",
        }
    }
}

/// Where folder scans look ([`Catalog::scan_rules`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanRules {
    /// (folder key, its mode).
    records: Vec<(String, ScanMode)>,
    /// (folder key, the default location as spelled).
    defaults: Vec<(String, String)>,
}

impl ScanRules {
    /// The scan mode in effect for folder `path`: the one set on the nearest of it and the
    /// folders above it, where a default location counts as set to Never unless the library keeps
    /// a mode for that very folder. So a mode set on a default location overrides it, taking that
    /// mode off brings the default back, and a default inside a folder set to Always still keeps
    /// its subtree out. Nothing set anywhere above: not scanned.
    pub fn mode(&self, path: &str) -> EffectiveScan {
        let Some(key) = record_key(path) else { return EffectiveScan::default() };
        // the nearest folder at or above `path` that says something (a longer key is nearer:
        // they all lead to `key`)
        let record = self.records.iter().filter(|(k, _)| key_within(&key, k)).max_by_key(|(k, _)| k.len());
        let default = self.defaults.iter().filter(|(k, _)| key_within(&key, k)).max_by_key(|(k, _)| k.len());
        match (record, default) {
            (Some((rk, mode)), Some((dk, _))) if rk.len() >= dk.len() => EffectiveScan { mode: Some(*mode), from: Some(rk.clone()), default: false },
            (_, Some((_, d))) => EffectiveScan { mode: Some(ScanMode::Never), from: Some(d.clone()), default: true },
            (Some((rk, mode)), None) => EffectiveScan { mode: Some(*mode), from: Some(rk.clone()), default: false },
            (None, None) => EffectiveScan::default(),
        }
    }

    /// The rules without the mode set on folder `path` itself (what it would inherit).
    pub fn without(&self, path: &str) -> ScanRules {
        let key = record_key(path);
        ScanRules { records: self.records.iter().filter(|(k, _)| Some(k) != key.as_ref()).cloned().collect(), defaults: self.defaults.clone() }
    }

    /// The folders set to `mode`, by identity key.
    pub fn set_to(&self, mode: ScanMode) -> impl Iterator<Item = &str> {
        self.records.iter().filter(move |(_, m)| *m == mode).map(|(k, _)| k.as_str())
    }
}

/// The scan mode in effect for a folder ([`Catalog::folder_scan_mode`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EffectiveScan {
    /// `None`: not scanned (no folder above it says anything).
    pub mode: Option<ScanMode>,
    /// The folder that says so (the folder itself or one above it), as its record or the default
    /// spells it; `None` with `mode`.
    pub from: Option<String>,
    /// It is one of the built-in Never locations, not something the library keeps.
    pub default: bool,
}

/// The identity a folder's record is kept under: its [`folder_key`]; `None` for a path that
/// names no folder (empty, relative, `.`).
pub(crate) fn record_key(path: &str) -> Option<String> {
    let key = folder_key(path);
    (split_path(path, true).absolute && !key.is_empty()).then_some(key)
}

/// Whether `path` names a folder a record can be kept about (absolute, not empty): its identity
/// key.
pub fn record_key_of(path: &str) -> Option<String> {
    record_key(path)
}

/// A folder while the tree is being built (children by identity key).
struct Level {
    name: String,
    path: String,
    own: usize,
    volume: bool,
    children: Vec<String>,
}

/// Where a stored folder path lies: its volume and the folder names below it.
struct Placed {
    mount: String,
    volume: String,
    names: Vec<String>,
}

/// A gvfs mount's name made readable: `smb-share:server=tokyo,share=photos` → `photos on tokyo`;
/// any other (`sftp:host=nas`) as it is.
fn mount_name(n: &str) -> String {
    let field = |key: &str| {
        let rest = n.strip_prefix("smb-share:")?;
        rest.split(',').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')).filter(|v| !v.is_empty())
    };
    match (field("share"), field("server")) {
        (Some(share), Some(server)) => format!("{share} on {server}"),
        _ => n.to_string(),
    }
}

/// The volume `dir` lies on and the folder names below it, read the way [`folder_key`] reads a
/// path (so a row and the filter that chooses it never disagree). `None` for a path that is not
/// absolute or names no share.
fn place(dir: &str) -> Option<Placed> {
    let sp = split_path(dir, false);
    let names: Vec<&str> = sp.parts.iter().map(String::as_str).collect();
    let named = |mount: String, volume: &str, used: usize| (mount, volume.to_string(), used);
    let (mount, volume, used) = if sp.prefix.starts_with("//") {
        // `//server/share`: a server alone (no share) is no place to keep photos
        if sp.prefix.ends_with('/') {
            return None;
        }
        named(sp.prefix.clone(), &sp.prefix, 0)
    } else if !sp.prefix.is_empty() {
        named(sp.prefix.clone(), &sp.prefix, 0)
    } else if sp.absolute {
        match names.as_slice() {
            ["Volumes", n, ..] => named(format!("/Volumes/{n}"), n, 2),
            ["run", "media", u, n, ..] => named(format!("/run/media/{u}/{n}"), n, 4),
            ["media", u, n, ..] => named(format!("/media/{u}/{n}"), n, 3),
            ["mnt", n, ..] => named(format!("/mnt/{n}"), n, 2),
            // Linux desktops mount network places per user, naming server and share
            ["run", "user", uid, "gvfs", n, ..] => named(format!("/run/user/{uid}/gvfs/{n}"), &mount_name(n), 5),
            // NFS automounts: Linux `/net/<host>`, macOS `/Network/Servers/<host>`
            ["net", host, ..] => named(format!("/net/{host}"), host, 2),
            ["Network", "Servers", host, ..] => named(format!("/Network/Servers/{host}"), host, 3),
            _ => named("/".to_string(), "/", 0),
        }
    } else {
        return None;
    };
    Some(Placed { mount, volume, names: names.iter().skip(used).take(MAX_DEPTH).map(|n| n.to_string()).collect() })
}

/// The disk or share (`/Volumes/nas`, `D:`, `/` for the startup disk) a file-backed photo is on,
/// from its stored path alone.
pub fn volume_of(p: &crate::Photo) -> Option<String> {
    let Source::File { path } = &p.source else { return None };
    let dir = path.rfind(['/', '\\']).and_then(|i| path.get(..i.max(1)))?;
    place(dir).map(|pl| folder_key(&pl.mount))
}

/// Whether `path` is a whole disk or share (`/`, `C:\`, `\\srv\share`, `/Volumes/nas`), not a
/// folder in one.
pub fn is_disk_root(path: &str) -> bool {
    place(path).is_some_and(|p| p.names.is_empty())
}

/// Whether `path` is the startup disk's root (`/`), whose path also covers every other disk.
pub fn is_startup_disk(path: &str) -> bool {
    place(path).is_some_and(|p| p.names.is_empty() && p.mount == "/")
}

/// A folder named by its last two names (`photos/travel`), so two folders called `travel` are
/// told apart; the path itself when it has no names (`/`).
pub fn folder_label(path: &str) -> String {
    let names: Vec<&str> = path.split(['/', '\\']).filter(|n| !n.is_empty()).collect();
    if names.is_empty() { path.to_string() } else { names.iter().skip(names.len().saturating_sub(2)).copied().collect::<Vec<_>>().join("/") }
}

impl Catalog {
    /// What the library keeps about folder `path`, however it is spelled.
    pub fn folder_record(&self, path: &str) -> Option<&FolderRecord> {
        record_key(path).and_then(|k| self.folder_records.get(&k))
    }

    /// The colour label of folder `path`, however it is spelled.
    pub fn folder_color_label(&self, path: &str) -> Option<ColorLabel> {
        self.folder_record(path).and_then(|r| r.label)
    }

    /// The op that gives folder `path` colour label `label` (`None`: takes it off), keeping the
    /// rest of its record. Applying it checks the path.
    pub fn folder_label_op(&self, path: &str, label: Option<ColorLabel>) -> Op {
        let mut record = self.folder_record(path).cloned().unwrap_or_default();
        record.label = label;
        Op::SetFolderRecord { folder: path.to_string(), record: Some(record) }
    }

    /// The scan mode of folder `path` as set on it, however it is spelled (`None`: inherited).
    pub fn folder_scan_record(&self, path: &str) -> Option<ScanMode> {
        self.folder_record(path).and_then(|r| r.scan)
    }

    /// The op that gives folder `path` scan mode `mode` (`None`: inherit from the folder above),
    /// keeping the rest of its record. Applying it checks the path.
    pub fn folder_scan_op(&self, path: &str, mode: Option<ScanMode>) -> Op {
        let mut record = self.folder_record(path).cloned().unwrap_or_default();
        record.scan = mode;
        Op::SetFolderRecord { folder: path.to_string(), record: Some(record) }
    }

    /// The folders given a scan mode, by identity key, in key order.
    pub fn folder_scan_records(&self) -> Vec<(String, ScanMode)> {
        self.folder_records.iter().filter_map(|(k, r)| Some((k.clone(), r.scan?))).collect()
    }

    /// The scan mode in effect for folder `path`: see [`ScanRules::mode`].
    pub fn folder_scan_mode(&self, path: &str, defaults: &[String]) -> EffectiveScan {
        self.scan_rules(defaults).mode(path)
    }

    /// The folders given a scan mode and the built-in Never locations `defaults` (any spelling),
    /// detached from the catalog (a folder scan resolves modes on a worker thread).
    pub fn scan_rules(&self, defaults: &[String]) -> ScanRules {
        ScanRules { records: self.folder_scan_records(), defaults: defaults.iter().filter_map(|d| Some((record_key(d)?, d.clone()))).collect() }
    }

    /// The ops that carry the records of folder `from` and the folders inside it over to `to`
    /// (the same place below it), for a folder renamed or moved on disk: commit them with the
    /// photos' relinks so undo puts both back. Records already at or below `to` are left from a
    /// folder that is no longer there and are dropped (undo restores them). Empty when there is
    /// nothing to carry or drop.
    pub fn folder_records_follow(&self, from: &str, to: &str) -> Vec<Op> {
        let (Some(root), Some(dest)) = (record_key(from), record_key(to)) else { return Vec::new() };
        if root == dest {
            return Vec::new();
        }
        let moved: Vec<(String, FolderRecord)> = self
            .folder_records
            .iter()
            .filter(|(k, _)| key_within(k, &root))
            .filter_map(|(k, r)| {
                let rest = folder_rest(k, &root)?;
                let base = dest.trim_end_matches('/');
                let path = if rest.is_empty() { dest.clone() } else { format!("{base}/{}", rest.join("/")) };
                Some((folder_key(&path), r.clone()))
            })
            .collect();
        // what was known at the old place goes, and so does what is left at the new one (also
        // when the moved folder carries nothing: the folder now there is not the one it was about)
        let stale = self.folder_records.keys().filter(|k| key_within(k, &root) || key_within(k, &dest));
        let mut ops: Vec<Op> = stale.map(|k| Op::SetFolderRecord { folder: k.clone(), record: None }).collect();
        ops.extend(moved.into_iter().map(|(k, r)| Op::SetFolderRecord { folder: k, record: Some(r) }));
        ops
    }

    /// The library's volumes with their folders and photo counts: volumes by name
    /// (case-insensitive), each folder with its subfolders the same way.
    pub fn folder_tree(&self) -> Vec<FolderNode> {
        // photos per containing folder, as spelled (sorted, so the spelling that names a folder
        // never depends on hash order)
        let mut dirs: BTreeMap<&str, usize> = BTreeMap::new();
        for p in self.photos().filter(|p| p.in_library()) {
            if let Source::File { path } = &p.source
                && let Some(i) = path.rfind(['/', '\\'])
                // a photo straight in the root has an empty folder part: the root itself
                && let Some(dir) = path.get(..i.max(1))
            {
                *dirs.entry(dir).or_default() += 1;
            }
        }
        let mut levels: HashMap<String, Level> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        for (dir, n) in dirs {
            let Some(placed) = place(dir) else { continue };
            let vkey = folder_key(&placed.mount);
            if !levels.contains_key(&vkey) {
                levels.insert(
                    vkey.clone(),
                    Level { name: placed.volume.clone(), path: placed.mount.clone(), own: 0, volume: true, children: Vec::new() },
                );
                roots.push(vkey.clone());
            }
            let mut prefix = if placed.mount == "/" { String::new() } else { placed.mount.clone() };
            let mut parent = vkey;
            for name in &placed.names {
                prefix.push('/');
                prefix.push_str(name);
                let key = folder_key(&prefix);
                if !levels.contains_key(&key) {
                    levels.insert(key.clone(), Level { name: name.clone(), path: prefix.clone(), own: 0, volume: false, children: Vec::new() });
                    if let Some(l) = levels.get_mut(&parent) {
                        l.children.push(key.clone());
                    }
                }
                parent = key;
            }
            if let Some(l) = levels.get_mut(&parent) {
                l.own += n;
            }
        }
        let mut tree: Vec<FolderNode> = roots.iter().filter_map(|k| build(k, &levels, &self.folder_records)).collect();
        sort(&mut tree);
        // a folder that holds another disk's mount covers that disk's photos too
        let mounts: Vec<String> = tree.iter().filter(|v| v.path != "/").map(|v| v.path.clone()).collect();
        mark_selectable(&mut tree, &mounts);
        tree
    }
    /// The folders that hold photos of their own, newest first (Folders ▸ Sort by Date). A
    /// folder's date is the latest capture time among the photos directly in it; folders none of
    /// whose photos has a usable capture time (none, or one that does not start with a year)
    /// come last, by name. Each row is its [`FolderNode`] from [`Self::folder_tree`] without its
    /// children: its count still includes the folders inside it, as choosing it shows them too.
    pub fn folders_by_date(&self) -> Vec<DatedFolder> {
        // latest capture per containing folder, by identity (the same split as `folder_tree`)
        let mut latest: HashMap<String, &str> = HashMap::new();
        for p in self.photos().filter(|p| p.in_library()) {
            if let Source::File { path } = &p.source
                && let Some(i) = path.rfind(['/', '\\'])
                && let Some(dir) = path.get(..i.max(1))
                && let Some(c) = p.captured.as_deref().filter(|c| dated(c))
            {
                let e = latest.entry(folder_key(dir)).or_insert(c);
                if c > *e {
                    *e = c;
                }
            }
        }
        let mut out = Vec::new();
        let mut stack: Vec<FolderNode> = self.folder_tree();
        // depth is bounded by `MAX_DEPTH` (the tree is built that way)
        while let Some(mut n) = stack.pop() {
            stack.append(&mut n.children);
            if n.own > 0 {
                let date = latest.get(&folder_key(&n.path)).map(|d| d.to_string());
                out.push(DatedFolder { folder: n, date });
            }
        }
        out.sort_by(|a, b| {
            let by_name = || a.folder.name.to_lowercase().cmp(&b.folder.name.to_lowercase()).then_with(|| a.folder.path.cmp(&b.folder.path));
            match (&a.date, &b.date) {
                (Some(x), Some(y)) => y.cmp(x).then_with(by_name),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => by_name(),
            }
        });
        out
    }
}

/// A row of Folders ▸ Sort by Date (see [`Catalog::folders_by_date`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DatedFolder {
    pub folder: FolderNode,
    /// The latest capture time (ISO 8601, as stored) of the photos directly in the folder; `None`
    /// when none has one ("No Date").
    pub date: Option<String>,
}

impl DatedFolder {
    /// The year heading the row goes under; `None` for "No Date".
    pub fn year(&self) -> Option<&str> {
        self.date.as_deref().and_then(|d| d.get(..4))
    }
}

/// Whether a stored capture time can date a folder: it starts with a four-digit year. A corrupt
/// or foreign one (a Lightroom catalog's captureTime is copied verbatim) counts as no date.
fn dated(captured: &str) -> bool {
    captured.get(..4).is_some_and(|y| y.bytes().all(|b| b.is_ascii_digit()))
}

/// The node for `key` with everything below it (at most [`MAX_DEPTH`] deep).
fn build(key: &str, levels: &HashMap<String, Level>, records: &BTreeMap<String, FolderRecord>) -> Option<FolderNode> {
    let l = levels.get(key)?;
    let mut children: Vec<FolderNode> = l.children.iter().filter_map(|k| build(k, levels, records)).collect();
    sort(&mut children);
    let count = l.own + children.iter().map(|c| c.count).sum::<usize>();
    Some(FolderNode {
        name: l.name.clone(),
        path: l.path.clone(),
        count,
        own: l.own,
        volume: l.volume,
        selectable: true,
        label: records.get(key).and_then(|r| r.label),
        children,
    })
}

fn mark_selectable(nodes: &mut [FolderNode], mounts: &[String]) {
    for n in nodes {
        n.selectable = !(n.volume && n.path == "/") && (n.volume || !mounts.iter().any(|m| folder_within(m, &n.path)));
        mark_selectable(&mut n.children, mounts);
    }
}

fn sort(v: &mut [FolderNode]) {
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
}
