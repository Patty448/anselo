# Folder workflow plan (Picasa-style)

> **Last updated:** 2026-10-22 · **Status:** Phases 1 (`b5d34059`), 2 (`4a3c7780`) and 3 (`228bd5f1`, `0694245c`) committed; Phase 4 waits for the user's go-ahead on `notify`; Phase 5 not started · **Branch:** `folder-workflow`

## Status / handoff

### Done
- **Phase 1, Copy Full Path: committed as `b5d34059`** (`MENU-PHOTO-COPYPATH: Copy Full Path in photo and folder menus (app.copyPath)`).
  - CI (2026-10-20, `RUST_TEST_THREADS=2 cargo xtask ci`): fmt, clippy, heif, test (whole workspace), parity, layers and assets passed.
  - wasm first failed because the `wasm32-unknown-unknown` target was missing. The user approved installing it (`rustup target add wasm32-unknown-unknown`), and `cargo xtask wasm` then passed on the unchanged tree.
- **Phase 2, Folders "Sort by Date": committed as `4a3c7780`** (`LRC-LIB-FOLDERS-BYDATE: Folders ▸ Sort by Date (view.folderSort)`). Full `cargo xtask ci` was green (all 8 steps, after a `cargo fmt` fix).
  - `Catalog::folders_by_date() -> Vec<DatedFolder {folder, date}>` (`catalog/src/folders.rs`), plus `DatedFolder::year()` and `dated()` (a time counts only when it starts with 4 ASCII digits). It reuses `folder_tree()`: nodes with `own > 0`, children emptied, count unchanged.
  - `state::FolderSort {Tree, Date}`, `UiState.folder_sort` (read leniently), `Caches::folders_by_date` (`READS_FILE_DATE`).
  - `view.folderSort {sort?: tree|date}` (flips when omitted). `left.rs`: `folder_date_rows` and `folder_year_heading` (`folderYear:<y>` / `folderYear:none`, "Unknown Date"), and radio items in the Folders header menu (`sidebarMenu:folderSort:tree|date`).
  - Tests: 4 in `catalog/src/tests_folders.rs`; `ui-egui/src/tests_folder_sort.rs` (headless: menu → date view order, row click, back to tree; command, error, saved state, lenient read).
  - Parity row `LRC-LIB-FOLDERS-BYDATE` plus a revision row. Locale strings: "Sort Folders", "Sort by Folder Tree", "Sort by Date" (13 languages).
  - Checked by eye with a headless snapshot (`lightcraft-cli snapshot --library <scratch>/folder-sort/lib`): years 2026 → 2019, Night above City, City counts 2.
- **Phase 3, scan modes: committed as `228bd5f1` (engine + catalog) and `0694245c` (UI)**, each after a green `RUST_TEST_THREADS=2 cargo xtask ci`. Parity row `LRC-LIB-FOLDERS-SCAN` (✅).
  - Catalog: `ScanMode`, `FolderRecord.scan`, `ScanRules` (`mode`, `without`, `set_to`), `EffectiveScan`, `Catalog::{folder_scan_mode, folder_scan_op, folder_scan_record(s), scan_rules}`, `folders::record_key_of`; `query::key_within` made pub. **Catalog format bumped 9 → 10** (the plan said no change was needed, but the journal rules require a bump for a serialized field older builds would drop).
  - Resolution rule chosen: the nearest folder that says something wins; a default location counts at its own level, and a record on that very folder beats it. So `~/.cache` stays Never inside a home set to Always.
  - Engine: `folder_scan.rs` (`default_never`, `Platform`, `ScanJob::{new, run}`, `Scanned`, `finish`, `once_params`, `ScanState` in the library's `scan-state.json`, `SETTLE_SECS` = 10: a file last written 10 s ago counts as settled without a second scan). Commands `folder.scanMode`, `folder.scanModeOf`, `library.folderScanModes`, `library.scanSettings`, `library.scanFolders`. `ImportOptions.{min_long_edge, exclude}`, `library.import` `minLongEdge` / `exclude`, report `skipped.small`; `ImportDefaults.scan_min_long_edge` / `scan_rescan_minutes` (Option, defaults 300 / 30). `walk::files_in_except`. Auto Import returns `never: true` for a Never folder.
  - The scanner does its own walk (not `walk::files_in`), since it needs per-folder mtimes; it doesn't follow links to folders and stops at 200,000 folders / depth 32. A folder changed within 2 s of being listed isn't recorded as clean.
  - UI: `crates/ui-egui/src/folder_scan.rs` (`FolderScans`, `tick`, `scan_menu`, `choose`), `Dialog::NeverScan`, `ImportTask::folder_scan()`. Tests: `catalog/src/tests_folder_scan.rs` (6), `engine/src/tests_folder_scan.rs` (8), `ui-egui/src/tests_folder_scan.rs` (3).
  - Checked by eye with headless snapshots (`lightcraft-cli snapshot --library … --script …`): the Scan submenu, and the Never dialog. Not tried in the windowed app.
  - Snapshot tip: a Folders row that leads to one folder opens by itself; clicking its toggle closes it.
- **A slip earlier:** Phase 2 was started while Phase 1's first CI run was going, which mixed it into that run. Phase 2 was then taken out, Phase 1 was checked on its own, and Phase 2 was put back after the commit.

### Decisions so far
- **Watcher crate: `notify` 8.2.** The licence check is **done, not pending**:
  - The repo has no `deny.toml` or cargo-deny step, and `AGENTS.md` / `CLAUDE.md` forbid only GPL/LGPL/AGPL code, so nothing rejects CC0-1.0.
  - The shared `craftrules` standards couldn't be read: there is no local checkout, and GitHub returns 404.
  - `notify` is not in `Cargo.lock` yet. Add it in Phase 4.
- **Phases 1 to 5 approved as written**, including Phase 4's watcher design and its measurements.
- **The Folder Manager is its own dialog** (Phase 5), not a section inside Settings.
- **Delete from Disk is skipped:** #1158 and #1851 aren't in this checkout.
- **Library panel on the right:** already possible by dragging its tab. No code change.
- **This file is committed** (since 2026-10-22, at the user's request; before that it was kept local). Keep it updated, and commit changes to it with the work they describe.

### Dev build, test library, test folder
- **None set up yet.** No isolated build directory, no app launch, no test library and no test folder exist.
  - Builds and tests used the default `target/`.
  - The Copy Full Path tests use an in-memory `Session::new()` with fake paths (`/photos/trip/a.jpg`, …) and touch no disk.
  - The Phase 4 test tree (about 10,000 files in 1,000 folders) has not been created.
- What `CLAUDE.md` prescribes when one is needed:
  - A separate `CARGO_TARGET_DIR=target/agent-<name>`.
  - A control port in the 18000–19999 range (e.g. `cargo run --release -p lightcraft -- --control 18421`), never 7980.
  - A scratch subfolder.
  - Headless checks through `lightcraft-cli snapshot --demo …` or `ui.screenshot {"headless": true}`.

### Next step
1. Before adding `notify` in Phase 4, **confirm with the user** that it's still OK. (Asked at the end of the 2026-10-22 session; no answer yet.)
2. Phase 4 plugs into `folder_scan::ScanJob` (`roots` restricted to dirty folders) and `ui-egui/src/folder_scan.rs` (`FolderScans::request`).
3. Never edit the working tree while a CI run is going. That is what mixed Phase 2 into Phase 1's first run.

### Unfinished or uncertain
- Phase 1 (Copy Full Path) hasn't been checked in the running app; the clipboard can't be seen in a headless snapshot. Phase 2 was checked by a headless snapshot only, not in the windowed app.
- On Windows, `native_path` turns a folder row's forward slashes into backslashes. That has only been tested through a `cfg!(windows)` branch in the test, never on Windows.
- The whole workspace test suite can run out of memory on machines with little free RAM; `RUST_TEST_THREADS=2` keeps it in bounds. `tests_masking::edit_in_photocraft_opens_a_psd_copy` alone peaks at about 3.5 GB.
- Phase 3 open points: the 300 px filter runs after the probe (a small file is read once, then skipped); a file that failed to import isn't retried until restart or until its folder changes; a photo emptied out of Recently Deleted returns only when its folder changes or with `library.scanFolders {full: true}`. The safety-interval rescans use the mtime skip (not `full`).

This plan adds a Picasa-style folder workflow on top of what LightCraft already has. Existing features are
reused and not reimplemented.

## What already exists (reused)

| Feature | Where |
|---|---|
| Folders sidebar (PR #307, LRC-LIB-LIBFOLDERS) | `crates/catalog/src/folders.rs` `folder_tree`, `crates/ui-egui/src/panels/left.rs` `folder_rows` |
| Folder context menu, platform reveal label | `left.rs` `folder_menu_for_library`, `menus.rs` `reveal_label()` |
| Photo reveal | `app.showInFinder` (`menus.rs`); on Linux, `xdg-open` on the parent folder |
| Info panel file path | `panels/right.rs` File Path row |
| Show in File Manager in the photo menus | the shared `panels/grid.rs` `context_menu`, used by the grid, loupe, full screen, filmstrip, compare and survey. Already complete. |
| Local folder browsing | `library.browse` (`engine/src/cmd/browse.rs`) and the Local sidebar section |
| Auto Import (one watched folder, top level, polled every 3 s) | `library.autoImport` / `library.autoImportScan` (`engine/src/cmd/library.rs`) and the poll in `ui-egui/src/lib.rs`. **Stays as it is.** |
| Add-in-place import with dedupe | `ImportMode::Add`, `library.import`, and `ImportTask` with an activity row |
| Remove from Library (to Recently Deleted, files untouched) | `library.removeFolder` |
| Per-folder records (undoable, follow renames and moves) | `FolderRecord` and `Op::SetFolderRecord` |
| Library panel docked on the right | Drag the Library tab to the right edge (`docking.rs`), or run `ui.dock {"operation":"move","panel":"library","target":"tools","zone":"right"}`. **No code change.** |

Not in this checkout: Delete from Disk / Move Files to Trash (#1158, #1851). It is skipped until those PRs land.

## Phases

Each phase is one or more commits with one task id each. `cargo xtask ci` is green before every commit.

### Phase 1: Copy Full Path (done, in CI)

- `app.copyPath` ("Copy Full Path") is a registered UI command in the Photo menu.
- It copies the selected originals, one path per line; demo scenes are skipped.
- It also appears in the photo right-click menu (every view that has one) and in the Folders and Local row menus, where it copies the folder.
- The text reaches the clipboard through `LightcraftApp::copy_request` at the end of the frame.
- Tests: `crates/ui-egui/src/tests_copy_path.rs`. Parity row: MENU-PHOTO-COPYPATH. Translated into all 13 languages.

### Phase 2: Folders "Sort by Date" (unchanged)

- `Catalog::folders_by_date()` lists folders with photos of their own, newest first. A folder's date is the latest `captured` among its own photos; folders with no capture date come last under "No Date".
- The result is cached per revision.
- The setting `UiState.folder_sort` (Tree by default, or Date) is saved in `ui.json`.
- `view.folderSort {sort}` sets it; the Folders section's right-click menu offers the same choice.
- The Date view shows year headings over rows drawn by the existing `folder_row`.
- Tests: catalog ordering, undated and malformed dates, and a headless sidebar test.

### Phase 3: Scan modes core (Never / Once / Always), without a watcher

**Model** (`crates/catalog/src/folders.rs`):
- `FolderRecord.scan: Option<ScanMode>`. It is undoable, follows renames and moves, and needs no journal format change.
- Built-in **default Never** locations are computed, not stored:
  - Linux and BSD: `/usr`, `/opt`, `/proc`, `/sys`, `/dev`, `/run`, `/boot`, `/etc`, `/var`, `/snap`.
  - Home: `~/.cache`, `~/.local/share`, `~/.config`.
  - macOS: `/System`, `/Library`, `~/Library`.
  - Windows: `C:\Windows`, `C:\Program Files*`, `%APPDATA%`, `%LOCALAPPDATA%`.
  - LightCraft itself: the open library folder and LightCraft's cache and config folders.
- `Catalog::folder_scan_mode(path, defaults)` resolves the effective mode. The nearest folder record with a mode wins, then the nearest default, else none (not scanned). It returns `{mode, from, default: bool}`.
- A record on a default location overrides the default; clearing the record brings the default back.

**Settings** (library `prefs.json`, through `ImportDefaults`):
- `scanMinLongEdge` (default 300 px): Once and Always scans skip images whose long edge is shorter. Manual imports and the existing Auto Import folder are not filtered.
  - It is applied inside the import pipeline as `ImportOptions.min_long_edge`, after the header read that import already does.
  - Skipped files are counted in the report as `skipped: {small: n}`.
- `scanRescanMinutes` (default 30): the safety interval between full rescans.

**Commands** (engine):
- `folder.scanMode {path, mode: never|once|always|inherit}` → `{mode, from, default, inLibrary}`.
  - `inLibrary` is how many library photos the folder holds. The UI uses it to ask the Never question.
  - Once returns import params; the UI runs them as an `ImportTask` with an activity row. Without the UI, the command imports directly.
- `library.folderScanModes` returns the explicit records plus the defaults, for the manager.
- `library.scanFolders {roots?, full?}` is the rescan:
  - It walks the Always roots, pruning Never and Once subtrees, hidden entries and the library itself.
  - It **skips directories whose mtime hasn't changed** since the last scan. Directory mtimes are kept in a per-library cache file (`scan-state.json` in the library's cache folder) and survive restarts.
  - It hands new files to the existing size-stable, `seen` and `onDeleted: skip` logic, then to an add-in-place import with `min_long_edge`.
  - It runs on a worker thread with an activity row ("Scanning folders"), cancellable.
  - Triggers: at startup, after any scan setting changes, and every `scanRescanMinutes`.
- The legacy Auto Import poll is untouched. A legacy folder whose effective mode is Never is still respected: Never files are dropped from its listing.

**Never removal question:**
- When Never is set (from the context menu or the manager) on a folder with `inLibrary > 0`, a dialog asks: "Also remove the N photos from “folder” that are already in the library?"
- The answers are **Keep Them** and **Remove from Library**. Removing runs the existing `library.removeFolder`, so the photos go to Recently Deleted and files on disk are never touched.

**UI:** the folder context menus (Folders and Local rows) get a **Scan ▸** submenu with Never, Once, Always, and "Inherit (from X: mode)".

**Tests:**
- Defaults: `/usr` and `~/.cache` resolve to Never as defaults, a record overrides them, and clearing the record restores the default.
- Inheritance, and following a rename.
- The size filter: a Once scan of 200 px and 400 px PNGs imports only the 400 px one, while a manual import takes both.
- The Never removal question opens, Remove sends the photos to Recently Deleted with the files still on disk, and Keep keeps them.
- The mtime skip: an unchanged directory is not listed again (the lister counts its calls).
- Undo of `folder.scanMode`.

### Phase 4: File-system notifications for Always roots (needs your crate approval)

- Always roots are watched recursively. An event marks its directory dirty. Events are debounced (about 2 s), then only dirty directories are rescanned through the Phase 3 scanner, so the size-stable check still holds back half-copied files.
- Full rescans still run at startup, after setting changes, and on the safety interval (Phase 3).
- **Fallback:** if a watch can't be added (an error, or the watch limit, e.g. inotify `max_user_watches`), the affected root falls back to the slow rescan.
  - The root is marked "polling" in the manager.
  - One notice (toast and Activity) says so: "Can't watch “X” for changes (system watch limit reached); it is checked every 30 min instead."
- The existing single Auto Import folder stays on its 3 s poll.
- The watcher is behind a small trait (`FolderWatcher`), so tests inject failures and events without touching the OS. It is compiled out on wasm.
- **Measurement:** a generated test tree of about 10,000 files in 1,000 folders, run as `cargo xtask bench-scan` or a `#[ignore]` bench test. It records:
  - the time and CPU (`getrusage` / `/usr/bin/time`) of the startup full scan, both cold and with an mtime-warm cache;
  - idle CPU over 60 s with the watchers active;
  - the time from adding 100 files to their import, and the CPU used.

  Results go to `plan/STATUS.md` → Metrics and this doc.

### Phase 5: Folder Manager

- A dialog of its own, since a whole-computer tree doesn't fit a Settings row. It opens from Settings ▸ Import ("Folder Manager…") and File ▸ Folder Manager… (`dialog.folderManager`). Desktop only.
- **Tree roots:**
  - Home, `/`, and mounted drives. Mounts are found the way the Local section's places and `folders::place` already detect `/media`, `/mnt`, `/Volumes`, gvfs and drive letters.
  - Children are listed **lazily** on expand, on a worker thread, directories only, capped per directory (with a "N more…" row). Listings are cached per path until refreshed.
  - A **Show hidden folders** toggle (off by default) shows dot folders and Windows hidden folders.
- **Each row:**
  - The folder name.
  - The effective mode, with an icon drawn in code (Never, Once, Always). Inherited and default modes are dimmed, with "inherited from X" or "default" in a tooltip.
  - A choice: Inherit, Never, Once, Always. Setting it runs `folder.scanMode`, including the Never question.
- Polling-fallback roots show a marker.
- The dialog also holds the scan settings: minimum long edge, rescan interval, and Rescan Now.
- **Tests:**
  - Lazy loading: with a 1,000-folder generated tree, opening the dialog lists only the roots, and expanding one lists only its direct children (a counting lister).
  - Defaults are shown as Never and can be changed.
  - The hidden toggle.
  - The Never question, run from the manager.

## Watcher crate: decision needed before Phase 4

| Option | Licence | Calls C? |
|---|---|---|
| **`notify` 8.2** (with `notify-types` 2.1; on Linux `inotify` 0.11 + `inotify-sys` 0.1, and `mio`) | notify CC0-1.0; notify-types MIT/Apache-2.0; inotify and inotify-sys ISC; mio MIT | No C is compiled or bundled. It does call OS C APIs through FFI: on Linux, glibc's `inotify_*` functions (`inotify-sys` declares them `extern`); on macOS the FSEvents framework (`fsevent-sys`, default) or kqueue; on Windows the Win32 API (`windows-sys`). It covers all three desktops. |
| **`rustix` 1.1** `fs::inotify` (already in `Cargo.lock` as a transitive dependency), with our own small watcher | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | On Linux, no: raw syscalls (`linux_raw` backend), no libc. **Linux only**: macOS and Windows would use the slow rescan only. |

**Decided (2026-10-20): `notify` 8.2.**

Licence check:
- The repo has no `deny.toml` or cargo-deny step.
- `AGENTS.md` / `CLAUDE.md` forbid only GPL/LGPL/AGPL code, and list CC0 / public domain as acceptable for assets.
- The `craftrules` standards weren't reachable (no local checkout; GitHub returns 404).
- `notify` is not in `Cargo.lock` yet. CC0-1.0 is already present there through `imgref` (`CC0-1.0 OR Apache-2.0`).
- One caveat: CC0 has no explicit patent grant. That's acceptable here, since the licence is public-domain-equivalent.

Earlier recommendation: **`notify`**. It is the standard choice and gives native watching on every desktop. Its FFI is to OS APIs only, the same kind of calls `std` makes, with no C build step. Neither option puts `unsafe` in our crates.

## Revision history

| Date | Change |
|---|---|
| 2026-10-19 | First plan: Copy Full Path, sort by date, scan modes on Auto Import polling, Settings folder list |
| 2026-10-20 | Always roots use change notifications plus mtime-skipping rescans with a polling fallback; a full lazy whole-computer Folder Manager; default Never locations; the Never removal question; small-image filter; scan measurements |
| 2026-10-22 | Phase 3 done (two commits); catalog format 10; resolution rule, settle time and open points recorded |
| 2026-10-22 | The file is committed instead of kept local |
| 2026-10-22 | Machine-specific memory notes trimmed, the memory reaper note dropped |
