//! Folders on this computer for the Folder Manager: the places its tree starts from ([`places`]:
//! the home folder, the computer's root or drives, mounted disks and shares) and the folders in a
//! folder ([`list_folders`], one level, capped). Both touch the disk, so the app calls them off
//! the UI thread.

use std::path::{Path, PathBuf};

/// Folders listed in one folder at most, unless more are asked for.
pub const MAX_LISTED: usize = 500;
/// The most ever listed in one folder.
pub const MAX_LISTED_ALL: usize = 20_000;

/// The folders in a folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Names and paths, by name (case ignored).
    pub folders: Vec<(String, String)>,
    /// Folders left out past the cap.
    pub more: usize,
    /// Why it couldn't be listed (no permission, gone).
    pub error: Option<String>,
}

/// The folders in `dir` (not links to folders, which scans don't follow), hidden ones too when
/// `hidden`; at most `cap`, the first by name.
pub fn list_folders(dir: &Path, hidden: bool, cap: usize) -> Listing {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => return Listing { error: Some(e.to_string()), ..Default::default() },
    };
    let mut folders: Vec<(String, String)> = Vec::new();
    // (a folder holding millions of entries isn't kept whole in memory)
    let mut seen = 0usize;
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if !hidden && is_hidden(&name, &e) {
            continue;
        }
        seen = seen.saturating_add(1);
        if folders.len() < MAX_LISTED_ALL {
            folders.push((name, e.path().to_string_lossy().to_string()));
        }
    }
    folders.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    folders.truncate(cap.min(MAX_LISTED_ALL));
    Listing { more: seen.saturating_sub(folders.len()), folders, error: None }
}

/// A dot folder, or (Windows) one with the hidden attribute.
fn is_hidden(name: &str, _entry: &std::fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if _entry.metadata().is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0) {
            return true;
        }
    }
    name.starts_with('.')
}

/// Where the Folder Manager's tree starts: the home folder `home`, the computer (`/`, or each
/// drive on Windows), then the disks and shares mounted where this system mounts them (by name).
/// The labels are English (the frontend translates "Home" and "Computer").
pub fn places(home: Option<&Path>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(h) = home.filter(|h| h.is_dir()) {
        out.push(("Home".into(), h.to_string_lossy().to_string()));
    }
    if cfg!(windows) {
        for letter in 'A'..='Z' {
            let root = format!("{letter}:\\");
            if Path::new(&root).is_dir() {
                out.push((format!("{letter}:"), root));
            }
        }
    } else {
        out.push(("Computer".into(), "/".into()));
    }
    let parents = crate::devices::mount_parents();
    let mut mounts: Vec<(String, String)> = Vec::new();
    let mut add = |dir: &Path| {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || parents.contains(&p) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            mounts.push((lightcraft_catalog::folders::mount_name(&name), p.to_string_lossy().to_string()));
        }
    };
    for p in &parents {
        add(p);
    }
    // GNOME's network shares and phones (gvfs)
    if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR").filter(|r| !r.is_empty()) {
        add(&PathBuf::from(run).join("gvfs"));
    }
    mounts.sort_by_key(|m| m.0.to_lowercase());
    mounts.dedup_by(|a, b| a.1 == b.1);
    out.extend(mounts);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_listed_by_name_capped_and_without_hidden_ones_or_files() {
        let d = std::env::temp_dir().join(format!("lc-folder-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for n in ["b", "A", "c", ".git"] {
            std::fs::create_dir_all(d.join(n)).unwrap();
        }
        std::fs::write(d.join("photo.jpg"), b"x").unwrap();
        let names = |l: &Listing| l.folders.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
        let l = list_folders(&d, false, 10);
        assert_eq!((names(&l), l.more), (vec!["A".to_string(), "b".into(), "c".into()], 0));
        assert_eq!(l.folders[0].1, d.join("A").to_string_lossy());
        assert_eq!(names(&list_folders(&d, true, 10)), [".git", "A", "b", "c"]);
        let l = list_folders(&d, false, 2);
        assert_eq!((names(&l), l.more), (vec!["A".to_string(), "b".into()], 1));
        let gone = list_folders(&d.join("gone"), false, 10);
        assert!(gone.error.is_some() && gone.folders.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn places_start_at_home_then_the_computer() {
        let home = std::env::temp_dir();
        let p = places(Some(&home));
        assert_eq!(p.first(), Some(&("Home".to_string(), home.to_string_lossy().to_string())));
        if !cfg!(windows) {
            assert_eq!(p.get(1), Some(&("Computer".to_string(), "/".to_string())));
        }
        assert!(!places(Some(&home.join("no-such-home-folder"))).iter().any(|(l, _)| l == "Home"));
    }
}
