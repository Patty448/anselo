//! Folder scans in the app (see [`lightcraft_engine::folder_scan`]): the folders set to Always are
//! scanned on a worker thread at startup, as soon as the folders set to Always change, every
//! `scanRescanMinutes`, and again shortly while a file in them is still being written; what a scan
//! finds is added like an Auto Import. They are also watched for changes
//! ([`lightcraft_engine::folder_watch`], with [`crate::Services::folder_watcher`]): a folder in
//! which something changed is scanned on its own once things go quiet there, and a folder that
//! can't be watched is said once and left to the rescans. Also the Scan ▸ submenu of folder rows (Folders and Local),
//! which sets a folder's mode and asks, for Never, whether the photos already in the library go.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_catalog::ScanMode;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::widgets::register;

/// The background task's name ([`crate::tasks`]).
pub const LABEL: &str = "Scan Folders";
/// Its kind in the activity stack.
pub const KIND: &str = "scanFolders";

/// When folder scans run.
#[derive(Default)]
pub struct FolderScans {
    /// A scan is wanted as soon as nothing else imports.
    due: bool,
    /// When (egui time, s) the next rescan is due.
    next_at: f64,
    /// The folders set to Always, as last seen (a change makes a scan due).
    roots: Option<Vec<String>>,
    /// The catalog revision `roots` was read at.
    revision: Option<u64>,
    /// The running scan's row in the activity stack.
    guard: Option<lightcraft_engine::activity::TaskGuard>,
    /// Folders (keys) where something changed, to scan once nothing else imports.
    changed: Vec<String>,
    /// The folders said to be left to the rescans (said once each).
    told: HashSet<String>,
}

impl FolderScans {
    /// Scan as soon as possible (a scan setting changed).
    pub fn request(&mut self) {
        self.due = true;
    }
}

fn always_roots(app: &LightcraftApp) -> Vec<String> {
    app.session.catalog.folder_scan_records().into_iter().filter(|(_, m)| *m == ScanMode::Always).map(|(k, _)| k).collect()
}

fn busy(app: &LightcraftApp) -> bool {
    app.import.is_some() || app.sync_run.is_some() || app.lightroom.is_some() || app.tasks.is_running(LABEL)
}

/// Start a scan when one is due (every frame; desktop only).
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let revision = app.session.catalog.revision;
    if app.folder_scans.revision != Some(revision) {
        app.folder_scans.revision = Some(revision);
        let roots = always_roots(app);
        if app.folder_scans.roots.as_ref() != Some(&roots) {
            // (also the first time: the scan at startup)
            app.folder_scans.due |= !roots.is_empty();
            watch(app, ctx, &roots);
            app.folder_scans.roots = Some(roots);
        }
    }
    if app.folder_scans.roots.as_ref().is_none_or(Vec::is_empty) {
        app.folder_scans.due = false;
        app.folder_scans.changed.clear();
        return;
    }
    changes(app, ctx);
    if now >= app.folder_scans.next_at {
        app.folder_scans.due = true;
    }
    if !busy(app) {
        if app.folder_scans.due {
            app.folder_scans.due = false;
            // (a full scan covers the folders that changed)
            app.folder_scans.changed.clear();
            let every = f64::from(app.session.import_defaults.scan_rescan_minutes()) * 60.0;
            app.folder_scans.next_at = now + every;
            start(app, None);
        } else if !app.folder_scans.changed.is_empty() {
            let changed = std::mem::take(&mut app.folder_scans.changed);
            start(app, Some(changed));
        }
    }
    let mut wait = (app.folder_scans.next_at - now).clamp(1.0, 60.0);
    if let Some(w) = app.session.folder_watch.wait(web_time::Instant::now()) {
        wait = wait.min(w.as_secs_f64().max(0.05));
    }
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
}

/// Watch the folders set to Always, `roots`, for changes (starting the watcher the first time
/// there are some).
fn watch(app: &mut LightcraftApp, ctx: &egui::Context, roots: &[String]) {
    if !roots.is_empty()
        && !app.session.folder_watch.started()
        && let Some(make) = app.services.folder_watcher.clone()
    {
        let repaint = ctx.clone();
        if let Err(e) = app.session.folder_watch.start(&make, Box::new(move || repaint.request_repaint())) {
            log::warn!("folder watch: {e}; the folders set to Always are only rescanned");
        }
    }
    app.session.sync_folder_watch(roots);
    // a folder no longer set to Always is said again if it is set again and can't be watched
    app.folder_scans.told.retain(|t| roots.contains(t));
}

/// What the watches saw: folders that changed (scanned once things are quiet there), and folders
/// that can't be watched (said once).
fn changes(app: &mut LightcraftApp, ctx: &egui::Context) {
    let taken = app.session.folder_watch.take(web_time::Instant::now());
    for (root, e) in taken.failed {
        if !app.folder_scans.told.insert(root.clone()) {
            continue;
        }
        let folder = lightcraft_catalog::folders::folder_label(&root);
        let minutes = app.session.import_defaults.scan_rescan_minutes();
        let text = if e.limit {
            crate::i18n::tr_format!(
                "Can't watch “{folder}” for changes (system watch limit reached); it is checked every {minutes} min instead.",
                folder = folder,
                minutes = minutes
            )
        } else {
            crate::i18n::tr_format!(
                "Can't watch “{folder}” for changes; it is checked every {minutes} min instead.",
                folder = folder,
                minutes = minutes
            )
        };
        app.toast_for(ctx, text, 8.0);
    }
    match taken.due {
        Some(lightcraft_engine::folder_watch::Due::All) => app.folder_scans.due = true,
        Some(lightcraft_engine::folder_watch::Due::Folders(dirs)) => {
            for d in dirs {
                if !app.folder_scans.changed.contains(&d) {
                    app.folder_scans.changed.push(d);
                }
            }
        }
        None => {}
    }
}

/// Scan the folders set to Always (all of them, or the ones `changed`).
fn start(app: &mut LightcraftApp, changed: Option<Vec<String>>) {
    let job = match changed {
        None => lightcraft_engine::folder_scan::ScanJob::new(&mut app.session, None, false),
        Some(c) => Ok(lightcraft_engine::folder_scan::ScanJob::of_changed(&mut app.session, c)),
    };
    let job = match job {
        Ok(j) if !j.roots.is_empty() => j,
        Ok(_) => return,
        Err(e) => return log::warn!("folder scan: {e}"),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let work = move || job.run(&flag);
    let done = |app: &mut LightcraftApp, ctx: &egui::Context, scanned: lightcraft_engine::folder_scan::Scanned| {
        app.folder_scans.guard = None;
        finish(app, ctx, scanned);
    };
    match crate::tasks::spawn(app, LABEL, None, work, done) {
        Ok(()) => {
            let guard = app.session.activity.start(KIND, "Scanning folders", lightcraft_engine::activity::Cancel::Flag(cancel));
            app.folder_scans.guard = Some(guard);
        }
        Err(e) => log::warn!("{e}"),
    }
}

/// A scan is over: keep what it saw and add what it found (an import that started meanwhile
/// leaves it for another scan, which lists again what this one saw).
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, scanned: lightcraft_engine::folder_scan::Scanned) {
    if app.import.is_some() {
        app.folder_scans.due = true;
        return;
    }
    let r = app.session.execute_fn("library.scanFolders", |s| lightcraft_engine::folder_scan::finish(s, scanned, false));
    let r = match r {
        Ok(r) => r,
        Err(e) => return log::warn!("folder scan: {e}"),
    };
    // a file still being written: looked at again once it has had time to settle
    if r["pending"].as_u64().is_some_and(|n| n > 0) {
        let soon = ctx.input(|i| i.time) + lightcraft_engine::folder_scan::SETTLE_SECS as f64 + 1.0;
        app.folder_scans.next_at = app.folder_scans.next_at.min(soon);
    }
    if let Some(params) = r.get("import") {
        start_import(app, params.clone(), true);
    }
}

/// Run `library.import` params of a folder scan as an import task (quiet like an Auto Import when
/// `background`).
fn start_import(app: &mut LightcraftApp, mut params: Value, background: bool) {
    let paths: Vec<String> = params["paths"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
    if let Some(o) = params.as_object_mut() {
        o.remove("paths");
    }
    let undo0 = app.session.undo.len();
    let task = crate::import::ImportTask::new(paths, params, undo0, false);
    app.import = Some(if background { task.auto().folder_scan() } else { task });
}

/// Scan ▸ of a folder row (Folders and Local; widget `folderScanMenu`, its items
/// `folderScan:never|once|always|inherit`): the folder's own mode is checked; Inherit says what it
/// would be without one.
pub fn scan_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, path: &str, name: &str) {
    let defaults = app.session.scan_defaults();
    let own = app.session.catalog.folder_scan_record(path);
    let inherited = app.session.catalog.scan_rules(&defaults).without(path).mode(path);
    let inherit_label = match (inherited.mode, inherited.default, inherited.from.as_deref()) {
        (None, ..) => crate::i18n::tr("Inherit (not scanned)").to_string(),
        (Some(_), true, _) => crate::i18n::tr("Inherit (default: Never)").to_string(),
        (Some(m), false, from) => crate::i18n::tr_format!(
            "Inherit (from {from}: {mode})",
            from = lightcraft_catalog::folders::folder_label(from.unwrap_or(path)),
            mode = crate::i18n::tr(mode_name(m))
        ),
    };
    let menu = crate::widgets::menu_button(ui, crate::i18n::tr("Scan"), |ui| {
        let choices = [
            (
                Some(ScanMode::Never),
                crate::i18n::tr("Never").to_string(),
                "Never: photos in this folder and the folders inside it are not added by scans",
            ),
            (Some(ScanMode::Once), crate::i18n::tr("Once").to_string(), "Once: add the photos in this folder now; later ones are not looked for"),
            (Some(ScanMode::Always), crate::i18n::tr("Always").to_string(), "Always: add new photos in this folder whenever the folders are scanned"),
            (None, inherit_label.clone(), "Inherit: as the folder above says"),
        ];
        for (mode, label, tip) in choices {
            let id = mode.map_or("inherit", ScanMode::as_str);
            // (Once imports now: not while another import runs)
            let enabled = mode != Some(ScanMode::Once) || app.import.is_none();
            let r = ui.add_enabled(enabled, egui::RadioButton::new(own == mode, label)).on_hover_text(crate::i18n::tr(tip));
            register(ui.ctx(), format!("folderScan:{id}"), r.rect);
            if r.clicked() {
                choose(app, ui.ctx(), path, name, mode);
                ui.close();
            }
        }
    });
    register(ui.ctx(), "folderScanMenu", menu.response.rect);
}

fn mode_name(m: ScanMode) -> &'static str {
    match m {
        ScanMode::Never => "Never",
        ScanMode::Once => "Once",
        ScanMode::Always => "Always",
    }
}

/// Set folder `path`'s scan mode from its menu: Once imports at once (in the background), Always
/// scans, and Never asks whether the photos already in the library from it go too.
pub fn choose(app: &mut LightcraftApp, ctx: &egui::Context, path: &str, name: &str, mode: Option<ScanMode>) {
    let id = mode.map_or("inherit", ScanMode::as_str);
    let r = match app.run("folder.scanMode", json!({"path": path, "mode": id, "start": false})) {
        Ok(r) => r,
        Err(e) => return app.toast(ctx, e),
    };
    if let Some(params) = r.get("import") {
        start_import(app, params.clone(), false);
    }
    let in_library = r["inLibrary"].as_u64().map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    if mode == Some(ScanMode::Never) && in_library > 0 {
        app.ui.dialog = Some(crate::state::Dialog::NeverScan { path: path.to_string(), name: name.to_string(), count: in_library });
    }
    if mode == Some(ScanMode::Always) {
        app.folder_scans.request();
    }
}
