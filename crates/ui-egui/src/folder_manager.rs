//! The Folder Manager (File ▸ Folder Manager…, Settings ▸ Import; `dialog.folderManager`;
//! desktop only): the folders of this whole computer as a tree, each with the scan mode in effect
//! and a choice of Inherit, Never, Once or Always (see [`crate::folder_scan`]), and the scan
//! settings.
//!
//! The tree starts at the home folder, the computer (or its drives) and the mounted disks and
//! shares ([`lightcraft_engine::folder_list::places`]). A folder's folders are listed only when it
//! is opened, on a worker thread, at most [`MAX_LISTED`] until "N more…" is clicked, and kept until
//! Refresh. Hidden folders are left out unless Show hidden folders is on. Never asks about the
//! photos already in the library in the dialog itself, which stays open.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use egui::RichText;
use lightcraft_catalog::ScanMode;
use lightcraft_catalog::query::folder_key;
use lightcraft_engine::folder_list::{Listing, MAX_LISTED, MAX_LISTED_ALL};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// Lists the folders in a folder: `(path, hidden, cap)` ([`lightcraft_engine::folder_list::list_folders`]).
pub type Lister = Arc<dyn Fn(&str, bool, usize) -> Listing + Send + Sync>;
/// The places the tree starts from, as `(label, path)`.
pub type Places = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

/// Folder levels shown at most.
const MAX_DEPTH: usize = 64;
/// Indentation per level.
const INDENT: f32 = 16.0;

/// What a worker thread found.
enum Arrival {
    Places(Vec<(String, String)>),
    Listing { key: (String, bool), listing: Listing },
}

/// The Never question about the photos already in the library, asked in the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeverQuestion {
    pub path: String,
    pub name: String,
    pub count: usize,
}

/// The Folder Manager's tree (kept while the app runs, so it opens where it was).
pub struct FolderManager {
    pub lister: Lister,
    pub places: Places,
    roots: Option<Vec<(String, String)>>,
    roots_asked: bool,
    /// Listings by (path, hidden shown); `None` while one is being made.
    lists: HashMap<(String, bool), Option<Listing>>,
    /// Folders whose every folder is listed ("N more…" clicked).
    all: HashSet<String>,
    /// Open folders (by path).
    open: HashSet<String>,
    pub show_hidden: bool,
    pub question: Option<NeverQuestion>,
    inbox: Arc<Mutex<Vec<Arrival>>>,
}

impl Default for FolderManager {
    fn default() -> Self {
        let lister: Lister = Arc::new(|path, hidden, cap| lightcraft_engine::folder_list::list_folders(Path::new(path), hidden, cap));
        let places: Places = Arc::new(|| {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty());
            lightcraft_engine::folder_list::places(home.as_deref().map(Path::new))
        });
        FolderManager {
            lister,
            places,
            roots: None,
            roots_asked: false,
            lists: HashMap::new(),
            all: HashSet::new(),
            open: HashSet::new(),
            show_hidden: false,
            question: None,
            inbox: Arc::default(),
        }
    }
}

impl FolderManager {
    /// List everything again when next shown (Refresh).
    pub fn refresh(&mut self) {
        self.roots = None;
        self.roots_asked = false;
        self.lists.clear();
    }

    /// Whether folder `path` is open.
    pub fn is_open(&self, path: &str) -> bool {
        self.open.contains(path)
    }

    fn take_arrivals(&mut self) {
        let arrivals = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        for a in arrivals {
            match a {
                Arrival::Places(p) => self.roots = Some(p),
                Arrival::Listing { key, listing } => {
                    // (a Refresh meanwhile asked for it again: either answer will do)
                    self.lists.insert(key, Some(listing));
                }
            }
        }
    }

    /// Run `work` on a worker thread; its answer arrives at a later frame. Counted as pending work
    /// for the headless driver while it runs.
    fn spawn(&self, ctx: &egui::Context, work: impl FnOnce() -> Arrival + Send + 'static) {
        let running = crate::panels::left::fs_running_counter(ctx);
        running.fetch_add(1, Ordering::AcqRel);
        let (inbox, repaint, done) = (self.inbox.clone(), ctx.clone(), running.clone());
        let job = move || {
            let a = work();
            inbox.lock().unwrap_or_else(PoisonError::into_inner).push(a);
            done.fetch_sub(1, Ordering::AcqRel);
            repaint.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = std::thread::Builder::new().name("lc-folder-manager".into()).spawn(job) {
            log::warn!("folder manager: {e}");
            running.fetch_sub(1, Ordering::AcqRel);
        }
        #[cfg(target_arch = "wasm32")]
        job();
    }

    fn roots(&mut self, ctx: &egui::Context) -> Option<Vec<(String, String)>> {
        if self.roots.is_none() && !self.roots_asked {
            self.roots_asked = true;
            let places = self.places.clone();
            self.spawn(ctx, move || Arrival::Places(places()));
        }
        self.roots.clone()
    }

    /// The folders in `path` (`None` while they are being listed; asks for them the first time).
    fn listing(&mut self, ctx: &egui::Context, path: &str) -> Option<Listing> {
        let key = (path.to_string(), self.show_hidden);
        if let Some(l) = self.lists.get(&key) {
            return l.clone();
        }
        self.lists.insert(key.clone(), None);
        let (lister, cap) = (self.lister.clone(), if self.all.contains(path) { MAX_LISTED_ALL } else { MAX_LISTED });
        self.spawn(ctx, move || {
            let listing = lister(&key.0, key.1, cap);
            Arrival::Listing { key, listing }
        });
        None
    }
}

/// What the rows of one frame share.
struct Frame {
    rules: lightcraft_catalog::ScanRules,
    /// Watched folders (keys) left to the rescans, and why.
    polling: HashMap<String, String>,
    /// A row's choice this frame: (path, name, mode).
    chosen: Option<(String, String, Option<ScanMode>)>,
}

/// The dialog's body.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.set_min_width(620.0);
    app.folder_manager.take_arrivals();
    let ctx = ui.ctx().clone();
    question(app, ui, t);
    ui.label(
        RichText::new(crate::i18n::tr(
            "Choose where the library looks for new photos by itself. A folder without a choice of its own follows the folder it is in.",
        ))
        .color(t.text_dim),
    );
    ui.horizontal(|ui| {
        let mut hidden = app.folder_manager.show_hidden;
        let r = ui.checkbox(&mut hidden, crate::i18n::tr("Show hidden folders"));
        register(ui.ctx(), "check:folderManagerHidden", r.rect);
        app.folder_manager.show_hidden = hidden;
        let r = ui.button(crate::i18n::tr("Refresh"));
        register(ui.ctx(), "button:folderManagerRefresh", r.rect);
        if r.clicked() {
            app.folder_manager.refresh();
        }
    });
    let polling = app.session.folder_watch.roots().into_iter().filter_map(|(k, w)| w.error.map(|e| (k, e.message))).collect();
    let mut frame = Frame { rules: app.session.scan_rules(), polling, chosen: None };
    let height = (ui.ctx().content_rect().height() * 0.5).clamp(160.0, 460.0);
    egui::ScrollArea::vertical().id_salt("folderManagerTree").max_height(height).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        match app.folder_manager.roots(&ctx) {
            None => {
                ui.label(RichText::new(crate::i18n::tr("Loading…")).color(t.text_dim));
            }
            Some(roots) => {
                for (label, path) in roots {
                    tree(app, ui, t, &mut frame, crate::i18n::tr(&label), &path, 0);
                }
            }
        }
    });
    if let Some((path, name, mode)) = frame.chosen {
        choose(app, &ctx, &path, &name, mode);
    }
    ui.separator();
    settings(app, ui, t);
}

/// The Never question, above the tree.
fn question(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(q) = app.folder_manager.question.clone() else { return };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(crate::i18n::tr_format!(
            "Also remove the {count} photo{} from “{name}” that are already in the library?",
            if q.count == 1 { "" } else { "s" },
            name = q.name,
            count = q.count
        ));
        ui.label(RichText::new(crate::i18n::tr("They move to Recently Deleted and can be restored; no file on disk is touched.")).color(t.text_dim));
        ui.horizontal(|ui| {
            let keep = ui.button(crate::i18n::tr("Keep Them"));
            register(ui.ctx(), "button:folderManagerKeep", keep.rect);
            let remove = ui.button(crate::i18n::tr("Remove from Library"));
            register(ui.ctx(), "button:folderManagerRemove", remove.rect);
            if keep.clicked() {
                app.folder_manager.question = None;
            }
            if remove.clicked() {
                app.folder_manager.question = None;
                // (the question named the photos: a folder holding whole disks goes as asked)
                if let Err(e) = app.run("library.removeFolder", json!({"path": q.path, "disk": true})) {
                    app.toast(ui.ctx(), e);
                }
            }
        });
    });
}

/// Folder `path` and, when open, the folders in it.
fn tree(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, frame: &mut Frame, name: &str, path: &str, depth: usize) {
    row(app, ui, t, frame, name, path, depth);
    if !app.folder_manager.is_open(path) {
        return;
    }
    let inset = INDENT * (depth as f32 + 1.0) + 20.0;
    if depth + 1 >= MAX_DEPTH {
        return;
    }
    let Some(listing) = app.folder_manager.listing(ui.ctx(), path) else {
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.label(RichText::new(crate::i18n::tr("Loading…")).color(t.text_dim));
        });
        return;
    };
    if let Some(e) = &listing.error {
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.label(RichText::new(crate::i18n::tr_format!("Can't list this folder: {e}", e = e)).color(t.text_dim));
        });
    }
    for (child, child_path) in &listing.folders {
        tree(app, ui, t, frame, child, child_path, depth + 1);
    }
    if listing.more > 0 {
        ui.horizontal(|ui| {
            ui.add_space(inset);
            let r = ui.link(crate::i18n::tr_format!("{n} more…", n = listing.more));
            register(ui.ctx(), format!("folderManager:more:{path}"), r.rect);
            if r.clicked() {
                let fm = &mut app.folder_manager;
                fm.all.insert(path.to_string());
                fm.lists.remove(&(path.to_string(), fm.show_hidden));
            }
        });
    }
}

/// One folder: its triangle, name, mode in effect and the choice.
fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, frame: &mut Frame, name: &str, path: &str, depth: usize) {
    let own = app.session.catalog.folder_scan_record(path);
    let effective = frame.rules.mode(path);
    let polling = frame.polling.get(&folder_key(path)).cloned();
    ui.horizontal(|ui| {
        ui.add_space(INDENT * depth as f32);
        let open = app.folder_manager.is_open(path);
        let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
        paint(ui.painter(), rect, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_dim);
        register(ui.ctx(), format!("folderManager:toggle:{path}"), r.rect);
        if r.clicked() {
            let fm = &mut app.folder_manager;
            if open {
                fm.open.remove(path);
            } else {
                fm.open.insert(path.to_string());
            }
        }
        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        paint(ui.painter(), rect, Icon::Folder, t.text_dim);
        let r = ui.add(egui::Label::new(RichText::new(name).color(t.text)).truncate()).on_hover_text(path);
        register(ui.ctx(), format!("folderManager:row:{path}"), r.rect);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // (clear of the scroll bar)
            ui.add_space(14.0);
            let label = match own {
                None => crate::i18n::tr("Inherit").to_string(),
                Some(m) => crate::i18n::tr(mode_name(m)).to_string(),
            };
            let menu = crate::widgets::menu_button(ui, label, |ui| {
                let choices =
                    [(None, "Inherit"), (Some(ScanMode::Never), "Never"), (Some(ScanMode::Once), "Once"), (Some(ScanMode::Always), "Always")];
                for (mode, text) in choices {
                    let id = mode.map_or("inherit", ScanMode::as_str);
                    // (Once imports now: not while another import runs)
                    let enabled = mode != Some(ScanMode::Once) || app.import.is_none();
                    let r = ui.add_enabled(enabled, egui::RadioButton::new(own == mode, crate::i18n::tr(text)));
                    register(ui.ctx(), format!("folderManager:choice:{id}"), r.rect);
                    if r.clicked() {
                        frame.chosen = Some((path.to_string(), name.to_string(), mode));
                        ui.close();
                    }
                }
            });
            register(ui.ctx(), format!("folderManager:mode:{path}"), menu.response.rect);
            let badge = mode_badge(ui, t, &effective, own.is_some());
            // (what the row says, for the control channel and tests)
            let mode = effective.mode.map_or("none", ScanMode::as_str);
            let whose = if own.is_some() {
                "own"
            } else if effective.default {
                "default"
            } else {
                "inherited"
            };
            register(ui.ctx(), format!("folderManager:effective:{path}:{mode}:{whose}"), badge);
            if let Some(why) = polling {
                let r = ui.label(RichText::new(crate::i18n::tr("Polling")).size(11.0).color(t.text_dim));
                register(ui.ctx(), format!("folderManager:polling:{path}"), r.rect);
                r.on_hover_text(crate::i18n::tr_format!(
                    "Not watched for changes ({why}); checked every {minutes} min instead.",
                    why = why,
                    minutes = app.session.import_defaults.scan_rescan_minutes()
                ));
            }
        });
    });
}

/// The mode in effect: its icon and name, dimmed when it comes from a folder above or a default.
/// → where it is.
fn mode_badge(ui: &mut egui::Ui, t: &Tokens, effective: &lightcraft_catalog::EffectiveScan, own: bool) -> egui::Rect {
    let Some(mode) = effective.mode else {
        return ui.label(RichText::new(crate::i18n::tr("Not scanned")).size(11.0).color(t.text_dim)).rect;
    };
    let color = if own { t.text } else { t.text_dim };
    let text = ui.label(RichText::new(crate::i18n::tr(mode_name(mode))).size(11.5).color(color));
    let (rect, icon) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    paint(ui.painter(), rect, mode_icon(mode), color);
    let tip = if own {
        None
    } else if effective.default {
        Some(crate::i18n::tr("Default: system and app folders are never scanned").to_string())
    } else {
        effective.from.as_deref().map(|from| crate::i18n::tr_format!("Inherited from {from}", from = lightcraft_catalog::folders::folder_label(from)))
    };
    let rect = text.rect.union(icon.rect);
    if let Some(tip) = tip {
        text.on_hover_text(&tip);
        icon.on_hover_text(tip);
    }
    rect
}

pub fn mode_icon(m: ScanMode) -> Icon {
    match m {
        ScanMode::Never => Icon::ScanNever,
        ScanMode::Once => Icon::ScanOnce,
        ScanMode::Always => Icon::ScanAlways,
    }
}

fn mode_name(m: ScanMode) -> &'static str {
    match m {
        ScanMode::Never => "Never",
        ScanMode::Once => "Once",
        ScanMode::Always => "Always",
    }
}

/// Set a folder's mode from the tree; Never asks its question in the dialog.
fn choose(app: &mut LightcraftApp, ctx: &egui::Context, path: &str, name: &str, mode: Option<ScanMode>) {
    if let Some(count) = crate::folder_scan::set_mode(app, ctx, path, mode) {
        app.folder_manager.question = Some(NeverQuestion { path: path.to_string(), name: name.to_string(), count });
    }
}

/// The scan settings: minimum image size, rescan interval, Rescan Now.
fn settings(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let d = &app.session.import_defaults;
    let (mut min, mut every) = (d.scan_min_long_edge(), d.scan_rescan_minutes());
    let mut changed = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::tr("Skip images smaller than")).color(t.text_label));
        let r = ui.add(egui::DragValue::new(&mut min).range(0..=lightcraft_engine::import::MAX_MIN_LONG_EDGE).suffix(" px"));
        register(ui.ctx(), "field:folderManagerMinSize", r.rect);
        if r.changed() {
            changed = Some(json!({"minLongEdge": min}));
        }
        ui.add_space(12.0);
        ui.label(RichText::new(crate::i18n::tr("Rescan every")).color(t.text_label));
        let r = ui.add(egui::DragValue::new(&mut every).range(1..=lightcraft_engine::import::SCAN_RESCAN_MAX_MINUTES).suffix(" min"));
        register(ui.ctx(), "field:folderManagerRescan", r.rect);
        if r.changed() {
            changed = Some(json!({"rescanMinutes": every}));
        }
    });
    if let Some(p) = changed {
        match app.run("library.scanSettings", p) {
            Ok(_) => app.folder_scans.request(),
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    ui.horizontal(|ui| {
        let r = ui.button(crate::i18n::tr("Rescan Now"));
        register(ui.ctx(), "button:folderManagerRescan", r.rect);
        if r.clicked() {
            app.folder_scans.request();
        }
        ui.label(RichText::new(crate::i18n::tr("Looks for new photos in the folders set to Always.")).size(11.0).color(t.text_dim));
    });
}
