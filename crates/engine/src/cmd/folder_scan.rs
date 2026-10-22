//! Folder scan commands: each folder's scan mode (Never / Once / Always), the scan settings and
//! the scan of the Always folders (see [`crate::folder_scan`]).

use lightcraft_catalog::{EffectiveScan, Filter, ScanMode, Sort};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::folder_scan::{ScanJob, finish, once_params};
use crate::import::SCAN_RESCAN_MAX_MINUTES;
use crate::{Result, Session};

/// The largest minimum long edge that can be set (pixels).
pub const MAX_MIN_LONG_EDGE: u32 = 100_000;

/// `{mode, from, default, inLibrary}` for folder `path`: its effective mode (`null`: not
/// scanned), the folder that says so, whether that is a built-in default, and how many library
/// photos the folder holds (the folders inside it included).
pub fn scan_mode_json(s: &Session, path: &str) -> Value {
    let EffectiveScan { mode, from, default } = s.catalog.folder_scan_mode(path, &s.scan_defaults());
    json!({"path": path, "mode": mode, "from": from, "default": default, "set": s.catalog.folder_scan_record(path), "inLibrary": in_library(s, path)})
}

fn in_library(s: &Session, path: &str) -> usize {
    let f = Filter { library_folder: Some(path.to_string()), ..Default::default() };
    s.catalog.query(&f, &Sort::default()).len()
}

fn settings_json(s: &Session) -> Value {
    let d = &s.import_defaults;
    json!({"minLongEdge": d.scan_min_long_edge(), "rescanMinutes": d.scan_rescan_minutes()})
}

fn u32_param(p: &Value, key: &str, c: &str, range: std::ops::RangeInclusive<u32>) -> Result<Option<u32>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let n = v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| range.contains(n));
    n.map(Some).ok_or_else(|| bad(c, format!("`{key}` must be a whole number from {} to {}", range.start(), range.end())))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "folder.scanMode",
            "Set Folder Scan Mode",
            [],
            None,
            "{path: folder on disk, mode: never|once|always|inherit, start?: bool} — whether the library looks for new photos in the folder and the folders inside it (unless one says otherwise): never (not even Auto Import; photos already in the library stay unless removed with library.removeFolder), once (add its photos now, where they are: Never folders inside it and images smaller than scanMinLongEdge left out; `start: false` returns the `library.import` params as `import` instead), always (added by library.scanFolders), inherit (as the folder above says; a built-in Never location — system folders, caches, the library — is Never again). One undo step; follows the folder when it is renamed or moved → {path, mode (in effect, null: not scanned), from (the folder that says so), default (a built-in Never location), set (this folder's own), inLibrary (library photos in it), imported?, import?}",
            always,
            |s, p| {
                const C: &str = "folder.scanMode";
                let path = str_param(p, "path").map(str::trim).filter(|d| !d.is_empty()).ok_or_else(|| bad(C, "missing `path`"))?;
                if lightcraft_catalog::folders::record_key_of(path).is_none() {
                    return Err(bad(C, format!("{path}: not a folder (give its whole path)")));
                }
                let mode = match str_param(p, "mode").ok_or_else(|| bad(C, "missing `mode` (never|once|always|inherit)"))? {
                    "inherit" => None,
                    m => Some(ScanMode::parse(m).ok_or_else(|| bad(C, format!("unknown mode `{m}` (never|once|always|inherit)")))?),
                };
                if mode == Some(ScanMode::Once) && !cfg!(target_arch = "wasm32") && !std::path::Path::new(path).is_dir() {
                    return Err(bad(C, format!("{path}: no such folder")));
                }
                if s.catalog.folder_scan_record(path) != mode {
                    let op = s.catalog.folder_scan_op(path, mode);
                    s.commit("Set Folder Scan Mode", op)?;
                }
                if mode == Some(ScanMode::Always) {
                    s.forget_scan_state(path);
                }
                let mut out = scan_mode_json(s, path);
                if mode == Some(ScanMode::Once) {
                    let params = once_params(s, path);
                    if !bool_or(p, "start", true) {
                        out["import"] = params;
                    } else {
                        let r = s.execute("library.import", &params)?;
                        out["imported"] = r["imported"].clone();
                        if let Some(sk) = r.get("skipped") {
                            out["skipped"] = sk.clone();
                        }
                        out["inLibrary"] = json!(in_library(s, path));
                    }
                }
                Ok(out)
            }
        ),
        cmd!(
            query "folder.scanModeOf",
            "Folder Scan Mode",
            [],
            None,
            "{path} → {path, mode (in effect, null: not scanned), from, default, set, inLibrary} (see folder.scanMode)",
            always,
            |s, p| {
                let path = str_param(p, "path").map(str::trim).filter(|d| !d.is_empty()).ok_or_else(|| bad("folder.scanModeOf", "missing `path`"))?;
                Ok(scan_mode_json(s, path))
            }
        ),
        cmd!(
            query "library.folderScanModes",
            "Folder Scan Modes",
            [],
            None,
            "{} → {folders: [{path, mode}] (the folders given a scan mode), defaults: [path] (the built-in Never locations on this machine), minLongEdge, rescanMinutes}",
            always,
            |s, _| {
                let folders: Vec<Value> = s.catalog.folder_scan_records().into_iter().map(|(path, mode)| json!({"path": path, "mode": mode})).collect();
                let mut out = settings_json(s);
                out["folders"] = json!(folders);
                out["defaults"] = json!(s.scan_defaults());
                Ok(out)
            }
        ),
        cmd!(
            "library.scanSettings",
            "Folder Scan Settings",
            [],
            None,
            "{minLongEdge?: pixels 0–100000 (folder scans leave out images whose long edge is shorter; default 300, 0: every size), rescanMinutes?: 1–1440 (minutes between full rescans of the Always folders; default 30)} → the settings",
            always,
            |s, p| {
                const C: &str = "library.scanSettings";
                let min = u32_param(p, "minLongEdge", C, 0..=MAX_MIN_LONG_EDGE)?;
                let every = u32_param(p, "rescanMinutes", C, 1..=SCAN_RESCAN_MAX_MINUTES)?;
                if min.is_some() || every.is_some() {
                    if let Some(m) = min {
                        s.import_defaults.scan_min_long_edge = Some(m);
                    }
                    if let Some(e) = every {
                        s.import_defaults.scan_rescan_minutes = Some(e);
                    }
                    s.save_prefs()?;
                }
                Ok(settings_json(s))
            }
        ),
        cmd!(
            query "library.folderWatch",
            "Folder Watch",
            [],
            None,
            "{} → {active: bool (changes are watched for: the desktop app), syncing: bool (watches being set up), folders: [{path, watched: bool, polling: bool (left to the rescans every rescanMinutes), reason?, limit?: bool (the system's watch limit)}]} — how the folders set to Always are watched for changes",
            always,
            |s, _| {
                let w = &s.folder_watch;
                let folders: Vec<Value> = w
                    .roots()
                    .into_iter()
                    .map(|(path, r)| {
                        let mut f = json!({"path": path, "watched": r.watched, "polling": r.error.is_some()});
                        if let Some(e) = r.error {
                            f["reason"] = json!(e.message);
                            f["limit"] = json!(e.limit);
                        }
                        f
                    })
                    .collect();
                Ok(json!({"active": w.active(), "syncing": w.syncing(), "folders": folders}))
            }
        ),
        cmd!(
            "library.scanFolders",
            "Scan Folders Now",
            [],
            None,
            "{roots?: [folder set to Always] (default: all of them), full?: bool (list unchanged folders too), start?: bool (false: return the `library.import` params as `import` instead of importing)} — add the new photos of the folders set to Always (folder.scanMode), where they are: folders set otherwise, hidden ones and the library are left out, folders unchanged since the last scan are not listed again, and a file still being written waits for the next scan → {roots, listed, unchanged, found, pending, imported, skipped?: {small}, import?, incomplete}",
            always,
            |s, p| {
                const C: &str = "library.scanFolders";
                let roots =
                    p.get("roots").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                let job = ScanJob::new(s, roots, bool_or(p, "full", false)).map_err(|e| bad(C, e))?;
                let scanned = job.run(&std::sync::atomic::AtomicBool::new(false));
                finish(s, scanned, bool_or(p, "start", true))
            }
        ),
    ]
}
