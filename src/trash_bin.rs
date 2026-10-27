//! The Trash: moving items there, listing what's in it, putting items back,
//! and deleting them for good. On macOS, Finder does the trashing, so its
//! own Put Back still works, and it tells fffinder where each item went.
//! Linux and Windows use the system trash through the `trash` crate. UI
//! tests install a [`FolderBin`] instead, so they never touch the real Trash.

use fffinder_core::ops;
use gpui_kit::{App, Global};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// An item in the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    /// Where the item is now, inside the Trash.
    pub path: PathBuf,
    /// Where it was before it was trashed, when that's known.
    pub original: Option<PathBuf>,
    pub deleted: Option<SystemTime>,
    /// The `trash` crate's id for it (Linux, Windows).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    id: Option<OsString>,
}

impl Trashed {
    /// The item's name from before it was trashed, when known.
    pub fn name(&self) -> String {
        let named = self.original.as_deref().unwrap_or(&self.path);
        fffinder_core::entry::display_name(named)
    }

    /// Where Put Back puts it, if that place is free.
    fn put_back_path(&self) -> Result<&Path, String> {
        let original = self
            .original
            .as_deref()
            .ok_or_else(|| format!("fffinder doesn’t know where “{}” came from", self.name()))?;
        let parent = original
            .parent()
            .filter(|p| p.is_dir())
            .ok_or_else(|| format!("The folder “{}” was in no longer exists", self.name()))?;
        if original.symlink_metadata().is_ok() {
            return Err(format!(
                "An item named “{}” is already in “{}”",
                self.name(),
                fffinder_core::entry::display_name(parent)
            ));
        }
        Ok(original)
    }
}

/// Somewhere trashed items go.
pub trait Bin: Send + Sync {
    /// Move items to the Trash; returns where each one went.
    fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, String>;
    /// Everything in the Trash.
    fn list(&self) -> Result<Vec<Trashed>, String>;
    /// Put items back where they were; returns where they are now.
    fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, String>;
    /// Delete items in the Trash for good.
    fn delete(&self, items: &[Trashed]) -> Result<(), String>;
}

/// Use this instead of the system Trash (UI tests).
pub struct TrashOverride(pub Arc<dyn Bin>);

impl Global for TrashOverride {}

/// The Trash in use: the system's, unless a test replaced it.
pub fn bin(cx: &App) -> Arc<dyn Bin> {
    match cx.try_global::<TrashOverride>() {
        Some(over) => over.0.clone(),
        None => Arc::new(SystemBin),
    }
}

/// A Trash that is a plain folder, for tests.
pub struct FolderBin {
    dir: PathBuf,
    items: Mutex<Vec<Trashed>>,
}

impl FolderBin {
    pub fn new(dir: PathBuf) -> FolderBin {
        let _ = std::fs::create_dir_all(&dir);
        FolderBin {
            dir,
            items: Mutex::new(Vec::new()),
        }
    }

    fn items(&self) -> std::sync::MutexGuard<'_, Vec<Trashed>> {
        self.items
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Bin for FolderBin {
    fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, String> {
        let mut trashed = Vec::with_capacity(paths.len());
        for path in paths {
            let name = fffinder_core::entry::display_name(path);
            let target = ops::free_name(&self.dir, |n| match n {
                1 => name.clone(),
                n => format!("{name} {n}"),
            });
            ops::move_item(path, &target).map_err(|e| e.to_string())?;
            trashed.push(Trashed {
                path: target,
                original: Some(path.clone()),
                deleted: Some(SystemTime::now()),
                id: None,
            });
        }
        self.items().extend(trashed.iter().cloned());
        Ok(trashed)
    }

    fn list(&self) -> Result<Vec<Trashed>, String> {
        let mut items = self.items();
        items.retain(|t| t.path.symlink_metadata().is_ok());
        Ok(items.clone())
    }

    fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, String> {
        let mut back = Vec::with_capacity(items.len());
        for item in items {
            let to = item.put_back_path()?;
            ops::move_item(&item.path, to).map_err(|e| e.to_string())?;
            back.push(to.to_path_buf());
            self.items().retain(|t| t.path != item.path);
        }
        Ok(back)
    }

    fn delete(&self, items: &[Trashed]) -> Result<(), String> {
        for item in items {
            ops::remove_any(&item.path).map_err(|e| e.to_string())?;
            self.items().retain(|t| t.path != item.path);
        }
        Ok(())
    }
}

/// The system Trash.
struct SystemBin;

#[cfg(target_os = "macos")]
mod imp {
    //! Finder trashes items (an AppleScript `delete` returns where each one
    //! went), and a small journal in the data folder remembers where they
    //! came from, for Put Back and Undo.

    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        path: PathBuf,
        original: PathBuf,
    }

    fn journal_path() -> PathBuf {
        fffinder_core::config::data_dir().join("trash.json")
    }

    fn load() -> Vec<Entry> {
        std::fs::read(journal_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(entries: &[Entry]) {
        let path = journal_path();
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        if let Ok(json) = serde_json::to_vec(entries) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Change the journal, dropping items no longer in the Trash.
    fn update(edit: impl FnOnce(&mut Vec<Entry>)) {
        let mut entries = load();
        edit(&mut entries);
        entries.retain(|e| e.path.symlink_metadata().is_ok());
        save(&entries);
    }

    fn trash_dir() -> Result<PathBuf, String> {
        dirs::home_dir()
            .map(|home| home.join(".Trash"))
            .ok_or_else(|| "There is no home folder".to_string())
    }

    impl Bin for SystemBin {
        fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, String> {
            const SCRIPT: &[&str] = &[
                "on run argv",
                "set out to \"\"",
                "repeat with p in argv",
                "set f to (POSIX file (p as text)) as alias",
                "tell application \"Finder\" to set t to delete f",
                "set out to out & POSIX path of (t as alias) & linefeed",
                "end repeat",
                "return out",
                "end run",
            ];
            let mut command = std::process::Command::new("osascript");
            for line in SCRIPT {
                command.args(["-e", line]);
            }
            let out = command
                .args(paths)
                .output()
                .map_err(|e| format!("Couldn’t run osascript: {e}"))?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            let now = SystemTime::now();
            let trashed: Vec<Trashed> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .zip(paths)
                .map(|(line, original)| Trashed {
                    path: PathBuf::from(line.trim_end_matches('/')),
                    original: Some(original.clone()),
                    deleted: Some(now),
                    id: None,
                })
                .collect();
            update(|entries| {
                entries.extend(trashed.iter().map(|t| Entry {
                    path: t.path.clone(),
                    original: t.original.clone().unwrap_or_default(),
                }))
            });
            Ok(trashed)
        }

        fn list(&self) -> Result<Vec<Trashed>, String> {
            use std::os::unix::fs::MetadataExt as _;
            let dir = trash_dir()?;
            let read = std::fs::read_dir(&dir).map_err(|err| {
                if err.kind() == std::io::ErrorKind::PermissionDenied {
                    "fffinder needs Full Disk Access to show the Trash. Turn it on in System Settings › Privacy & Security, then reopen fffinder.".to_string()
                } else {
                    err.to_string()
                }
            })?;
            let journal = load();
            Ok(read
                .filter_map(Result::ok)
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| {
                    let path = e.path();
                    let original = journal
                        .iter()
                        .find(|j| j.path == path)
                        .map(|j| j.original.clone());
                    // Moving an item into the Trash changes its status time.
                    let deleted = e.metadata().ok().map(|m| {
                        SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(m.ctime().max(0) as u64)
                    });
                    Trashed {
                        path,
                        original,
                        deleted,
                        id: None,
                    }
                })
                .collect())
        }

        fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, String> {
            let mut back = Vec::with_capacity(items.len());
            let result = items.iter().try_for_each(|item| {
                let to = item.put_back_path()?;
                ops::move_item(&item.path, to).map_err(|e| e.to_string())?;
                back.push(to.to_path_buf());
                Ok(())
            });
            update(|_| {});
            result.map(|()| back)
        }

        fn delete(&self, items: &[Trashed]) -> Result<(), String> {
            let result = items
                .iter()
                .try_for_each(|item| ops::remove_any(&item.path).map_err(|e| e.to_string()));
            update(|_| {});
            result
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;
    use trash::TrashItem;
    use trash::os_limited;

    fn from_item(item: TrashItem) -> Trashed {
        let id = PathBuf::from(&item.id);
        // On Linux the id is the item's `.trashinfo` file, beside `files/`.
        let path = match (id.parent().and_then(Path::parent), id.file_stem()) {
            (Some(trash), Some(stem)) if !cfg!(windows) => trash.join("files").join(stem),
            _ => id.clone(),
        };
        Trashed {
            path,
            original: Some(item.original_path()),
            deleted: u64::try_from(item.time_deleted)
                .ok()
                .map(|secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
            id: Some(item.id),
        }
    }

    fn to_item(item: &Trashed) -> Option<TrashItem> {
        let original = item.original.as_deref()?;
        Some(TrashItem {
            id: item.id.clone()?,
            name: original.file_name()?.to_os_string(),
            original_parent: original.parent()?.to_path_buf(),
            time_deleted: item
                .deleted
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64),
        })
    }

    impl Bin for SystemBin {
        fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, String> {
            trash::delete_all(paths).map_err(|e| e.to_string())?;
            let listed = os_limited::list().map_err(|e| e.to_string())?;
            Ok(paths
                .iter()
                .filter_map(|path| {
                    listed
                        .iter()
                        .filter(|item| item.original_path() == *path)
                        .max_by_key(|item| item.time_deleted)
                        .cloned()
                        .map(from_item)
                })
                .collect())
        }

        fn list(&self) -> Result<Vec<Trashed>, String> {
            let listed = os_limited::list().map_err(|e| e.to_string())?;
            Ok(listed.into_iter().map(from_item).collect())
        }

        fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, String> {
            for item in items {
                item.put_back_path()?;
            }
            os_limited::restore_all(items.iter().filter_map(to_item)).map_err(|e| e.to_string())?;
            Ok(items.iter().filter_map(|t| t.original.clone()).collect())
        }

        fn delete(&self, items: &[Trashed]) -> Result<(), String> {
            os_limited::purge_all(items.iter().filter_map(to_item)).map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_bin_trashes_restores_and_deletes() {
        let base = std::env::temp_dir().join(format!("fffinder-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("files")).unwrap();
        let file = base.join("files/a.txt");
        std::fs::write(&file, "a").unwrap();
        let bin = FolderBin::new(base.join("bin"));

        let trashed = bin.trash(std::slice::from_ref(&file)).unwrap();
        assert!(!file.exists());
        assert_eq!(bin.list().unwrap(), trashed);
        assert_eq!(trashed[0].name(), "a.txt");

        std::fs::write(&file, "new").unwrap();
        assert!(bin.restore(&trashed).is_err(), "the name is taken again");
        std::fs::remove_file(&file).unwrap();
        assert_eq!(bin.restore(&trashed).unwrap(), std::slice::from_ref(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a");
        assert!(bin.list().unwrap().is_empty());

        let trashed = bin.trash(std::slice::from_ref(&file)).unwrap();
        bin.delete(&trashed).unwrap();
        assert!(!trashed[0].path.exists() && !file.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
