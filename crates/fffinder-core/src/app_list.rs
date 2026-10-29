//! The apps in the application folders, read straight from those folders
//! instead of indexed. Finder shows an app as one item, so indexing the
//! files inside every bundle only costs memory and minutes of reading.

use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The folders macOS keeps apps in. Their plain subfolders, such as
/// `Utilities`, are read too.
pub fn mac_app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
    }
    dirs
}

/// The app folders to read on this system: [`mac_app_dirs`] on macOS,
/// none elsewhere.
pub fn app_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        mac_app_dirs()
    } else {
        Vec::new()
    }
}

/// Whether `path` names an app bundle.
pub fn is_app(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// The `.app` bundles in some folders, plus the bundles one level down in
/// their plain subfolders (`/Applications/Adobe Photoshop 2025/Adobe
/// Photoshop 2025.app`) and those subfolders themselves. Bundles are never
/// read into.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppList {
    /// The folders read, as given.
    dirs: Vec<PathBuf>,
    /// Bundles and plain subfolders, by folder and then by name, with a
    /// subfolder after the bundles in it.
    entries: Vec<PathBuf>,
    /// Every folder read, with its modification time then. Adding, removing,
    /// or renaming an item changes its folder's time.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

impl AppList {
    /// Read `dirs`. A folder that can't be read lists nothing, and is read
    /// again once it appears.
    ///
    /// # Arguments
    ///
    /// * `dirs` - Folders holding apps, such as [`app_dirs`]
    pub fn scan(dirs: &[PathBuf]) -> AppList {
        let mut list = AppList {
            dirs: dirs.to_vec(),
            ..AppList::default()
        };
        for dir in dirs {
            list.read(dir, true);
        }
        list
    }

    fn read(&mut self, dir: &Path, descend: bool) {
        self.stamps.push((dir.to_path_buf(), modified(dir)));
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut found: Vec<PathBuf> = entries.flatten().filter(is_dir).map(|e| e.path()).collect();
        found.sort();
        for path in found {
            if is_app(&path) {
                self.entries.push(path);
            } else if descend {
                self.read(&path, false);
                self.entries.push(path);
            }
        }
    }

    /// Whether no folder read has changed since; a few `stat` calls.
    pub fn is_current(&self) -> bool {
        self.stamps.iter().all(|(dir, time)| modified(dir) == *time)
    }

    /// The folders it reads, as given to [`Self::scan`].
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// Every bundle and plain subfolder found.
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    /// The bundles alone.
    pub fn apps(&self) -> impl Iterator<Item = &PathBuf> {
        self.entries.iter().filter(|p| is_app(p))
    }

    /// The folder of [`Self::dirs`] that `path` was found in.
    pub fn dir_of(&self, path: &Path) -> Option<&Path> {
        self.dirs
            .iter()
            .find(|d| path.starts_with(d))
            .map(PathBuf::as_path)
    }
}

/// A folder, or a link to one (an app can be a link to its bundle).
fn is_dir(entry: &DirEntry) -> bool {
    // `file_type` comes with the listing and doesn't follow links.
    entry
        .file_type()
        .is_ok_and(|t| t.is_dir() || (t.is_symlink() && entry.path().is_dir()))
}

fn modified(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Tmp {
            let dir =
                std::env::temp_dir().join(format!("fffinder-apps-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lists_apps_and_one_level_of_folders() {
        let tmp = Tmp::new("scan");
        let apps = tmp.dir("Applications");
        tmp.dir("Applications/A.app/Contents");
        tmp.dir("Applications/Sub/B.app");
        tmp.dir("Applications/Sub/Deeper/E.app");
        tmp.dir("Applications/C.app/Contents/D.app");
        tmp.dir("Applications/Plain");
        std::fs::write(apps.join("readme.txt"), "").unwrap();

        let list = AppList::scan(std::slice::from_ref(&apps));

        assert_eq!(
            list.entries(),
            [
                apps.join("A.app"),
                apps.join("C.app"),
                apps.join("Plain"),
                apps.join("Sub/B.app"),
                apps.join("Sub"),
            ]
        );
        let bundles: Vec<&PathBuf> = list.apps().collect();
        assert_eq!(
            bundles,
            [
                &apps.join("A.app"),
                &apps.join("C.app"),
                &apps.join("Sub/B.app")
            ]
        );
        assert_eq!(list.dir_of(&apps.join("Sub/B.app")), Some(apps.as_path()));
    }

    #[test]
    fn notices_new_apps_and_new_folders() {
        let tmp = Tmp::new("fresh");
        let apps = tmp.dir("Applications");
        tmp.dir("Applications/Sub");
        let missing = tmp.0.join("Missing");
        let list = AppList::scan(&[apps.clone(), missing.clone()]);
        assert!(list.is_current());

        // Folder times have a coarse resolution on some file systems.
        std::thread::sleep(std::time::Duration::from_millis(20));
        tmp.dir("Applications/Sub/New.app");
        assert!(!list.is_current());

        let list = AppList::scan(&[apps.clone(), missing.clone()]);
        assert!(list.entries().contains(&apps.join("Sub/New.app")));
        tmp.dir("Missing");
        assert!(!list.is_current());
    }
}
