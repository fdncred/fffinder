//! The Trash: moving items there, listing what's in it, putting items back,
//! and deleting them for good. On macOS, NSFileManager does the trashing in
//! this process, so macOS never asks to let fffinder control Finder, and it
//! says where each item went. Linux and Windows use the system trash through
//! the `trash` crate. UI tests install a [`FolderBin`] instead, so they never
//! touch the real Trash.

use fffinder_core::ops::{self, TransferError};
use gpui_kit::{App, Global};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Why the Trash couldn't do something.
#[derive(Debug, thiserror::Error)]
pub enum TrashError {
    /// The item's name; where it was before it was trashed isn't known.
    #[error("fffinder doesn’t know where “{0}” came from")]
    UnknownOrigin(String),
    /// The item's name; the folder it was in is gone.
    #[error("The folder “{0}” was in no longer exists")]
    FolderGone(String),
    #[error("An item named “{name}” is already in “{folder}”")]
    Taken { name: String, folder: String },
    #[error(
        "fffinder needs Full Disk Access to show the Trash. Turn it on in System Settings › Privacy & Security, then reopen fffinder."
    )]
    NeedsFullDiskAccess,
    #[error("There is no home folder")]
    NoHome,
    /// The system Trash (NSFileManager, or the `trash` crate) refused, with
    /// its own message.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Transfer(#[from] TransferError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A move to the Trash that stopped part way: the items that went, and why
/// the rest didn't.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct TrashStopped {
    pub trashed: Vec<Trashed>,
    #[source]
    pub error: TrashError,
}

impl From<TrashError> for TrashStopped {
    fn from(error: TrashError) -> TrashStopped {
        TrashStopped {
            trashed: Vec::new(),
            error,
        }
    }
}

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
    fn put_back_path(&self) -> Result<&Path, TrashError> {
        let original = self
            .original
            .as_deref()
            .ok_or_else(|| TrashError::UnknownOrigin(self.name()))?;
        let parent = original
            .parent()
            .filter(|p| p.is_dir())
            .ok_or_else(|| TrashError::FolderGone(self.name()))?;
        if original.symlink_metadata().is_ok() {
            return Err(TrashError::Taken {
                name: self.name(),
                folder: fffinder_core::entry::display_name(parent),
            });
        }
        Ok(original)
    }

    /// Whether it's still in the Trash: not put back or deleted since.
    pub fn in_trash(&self) -> bool {
        self.path.symlink_metadata().is_ok()
    }
}

/// Somewhere trashed items go.
pub trait Bin: Send + Sync {
    /// Move items to the Trash; returns where each one went.
    ///
    /// # Errors
    ///
    /// Stops at the first item that can't be trashed, and says which items
    /// went before it.
    fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, TrashStopped>;
    /// Everything in the Trash.
    ///
    /// # Errors
    ///
    /// Fails when the Trash can't be read.
    fn list(&self) -> Result<Vec<Trashed>, TrashError>;
    /// Put items back where they were, in order; returns where they are now.
    ///
    /// # Errors
    ///
    /// Stops at the first item that can't go back, as when its name is
    /// taken; the items before it are back. [`Trashed::in_trash`] tells them
    /// apart.
    fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, TrashError>;
    /// Delete items in the Trash for good.
    ///
    /// # Errors
    ///
    /// Stops at the first item that can't be deleted.
    fn delete(&self, items: &[Trashed]) -> Result<(), TrashError>;
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
    /// A Trash in `dir`, which is made if it doesn't exist.
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
    fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, TrashStopped> {
        let mut trashed = Vec::with_capacity(paths.len());
        let result = paths.iter().try_for_each(|path| {
            let name = fffinder_core::entry::display_name(path);
            let target = ops::free_name(&self.dir, |n| match n {
                1 => name.clone(),
                n => format!("{name} {n}"),
            });
            ops::move_item(path, &target)?;
            trashed.push(Trashed {
                path: target,
                original: Some(path.clone()),
                deleted: Some(SystemTime::now()),
                id: None,
            });
            Ok(())
        });
        self.items().extend(trashed.iter().cloned());
        match result {
            Ok(()) => Ok(trashed),
            Err(error) => Err(TrashStopped { trashed, error }),
        }
    }

    fn list(&self) -> Result<Vec<Trashed>, TrashError> {
        let mut items = self.items();
        items.retain(Trashed::in_trash);
        Ok(items.clone())
    }

    fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, TrashError> {
        let mut back = Vec::with_capacity(items.len());
        for item in items {
            let to = item.put_back_path()?;
            ops::move_item(&item.path, to)?;
            back.push(to.to_path_buf());
            self.items().retain(|t| t.path != item.path);
        }
        Ok(back)
    }

    fn delete(&self, items: &[Trashed]) -> Result<(), TrashError> {
        for item in items {
            ops::remove_any(&item.path)?;
            self.items().retain(|t| t.path != item.path);
        }
        Ok(())
    }
}

/// The system Trash.
struct SystemBin;

#[cfg(target_os = "macos")]
mod imp {
    //! NSFileManager trashes items and says where each one went, and a small
    //! journal in the data folder remembers where they came from, for Put
    //! Back and Undo. Finder's own Put Back doesn't know these items.

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

    fn trash_dir() -> Result<PathBuf, TrashError> {
        dirs::home_dir()
            .map(|home| home.join(".Trash"))
            .ok_or(TrashError::NoHome)
    }

    impl Bin for SystemBin {
        fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, TrashStopped> {
            let now = SystemTime::now();
            let mut trashed = Vec::with_capacity(paths.len());
            let result = paths.iter().try_for_each(|original| {
                let path = crate::macos::trash_item(original).map_err(|why| {
                    TrashError::Refused(format!(
                        "“{}”: {why}",
                        fffinder_core::entry::display_name(original)
                    ))
                })?;
                trashed.push(Trashed {
                    path,
                    original: Some(original.clone()),
                    deleted: Some(now),
                    id: None,
                });
                Ok(())
            });
            update(|entries| {
                entries.extend(trashed.iter().map(|t| Entry {
                    path: t.path.clone(),
                    original: t.original.clone().unwrap_or_default(),
                }))
            });
            match result {
                Ok(()) => Ok(trashed),
                Err(error) => Err(TrashStopped { trashed, error }),
            }
        }

        fn list(&self) -> Result<Vec<Trashed>, TrashError> {
            use std::os::unix::fs::MetadataExt as _;
            let dir = trash_dir()?;
            let read = std::fs::read_dir(&dir).map_err(|err| {
                if err.kind() == std::io::ErrorKind::PermissionDenied {
                    TrashError::NeedsFullDiskAccess
                } else {
                    err.into()
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

        fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, TrashError> {
            let mut back = Vec::with_capacity(items.len());
            let result = items.iter().try_for_each(|item| {
                let to = item.put_back_path()?;
                ops::move_item(&item.path, to)?;
                back.push(to.to_path_buf());
                Ok(())
            });
            update(|_| {});
            result.map(|()| back)
        }

        fn delete(&self, items: &[Trashed]) -> Result<(), TrashError> {
            let result = items
                .iter()
                .try_for_each(|item| ops::remove_any(&item.path));
            update(|_| {});
            Ok(result?)
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

    fn refused(err: trash::Error) -> TrashError {
        TrashError::Refused(err.to_string())
    }

    /// Where each of `paths` went in the Trash, for the ones that are there.
    fn trashed(paths: &[PathBuf]) -> Result<Vec<Trashed>, TrashError> {
        let listed = os_limited::list().map_err(refused)?;
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

    impl Bin for SystemBin {
        fn trash(&self, paths: &[PathBuf]) -> Result<Vec<Trashed>, TrashStopped> {
            match trash::delete_all(paths) {
                Ok(()) => Ok(trashed(paths)?),
                // Some may have gone before the one that failed.
                Err(err) => {
                    let gone: Vec<PathBuf> = paths
                        .iter()
                        .filter(|p| p.symlink_metadata().is_err())
                        .cloned()
                        .collect();
                    Err(TrashStopped {
                        trashed: trashed(&gone).unwrap_or_default(),
                        error: refused(err),
                    })
                }
            }
        }

        fn list(&self) -> Result<Vec<Trashed>, TrashError> {
            let listed = os_limited::list().map_err(refused)?;
            Ok(listed.into_iter().map(from_item).collect())
        }

        fn restore(&self, items: &[Trashed]) -> Result<Vec<PathBuf>, TrashError> {
            for item in items {
                item.put_back_path()?;
            }
            os_limited::restore_all(items.iter().filter_map(to_item)).map_err(refused)?;
            Ok(items.iter().filter_map(|t| t.original.clone()).collect())
        }

        fn delete(&self, items: &[Trashed]) -> Result<(), TrashError> {
            os_limited::purge_all(items.iter().filter_map(to_item)).map_err(refused)
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
        assert!(
            matches!(bin.restore(&trashed), Err(TrashError::Taken { .. })),
            "the name is taken again"
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(bin.restore(&trashed).unwrap(), std::slice::from_ref(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a");
        assert!(bin.list().unwrap().is_empty());

        let trashed = bin.trash(std::slice::from_ref(&file)).unwrap();
        bin.delete(&trashed).unwrap();
        assert!(!trashed[0].path.exists() && !file.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_failed_trash_says_what_went() {
        let base = std::env::temp_dir().join(format!("fffinder-bin-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (a, gone) = (base.join("a.txt"), base.join("gone.txt"));
        std::fs::write(&a, "a").unwrap();
        let bin = FolderBin::new(base.join("bin"));
        let stopped = bin.trash(&[a.clone(), gone]).unwrap_err();
        assert_eq!(stopped.trashed.len(), 1);
        assert_eq!(stopped.trashed[0].original.as_deref(), Some(a.as_path()));
        assert_eq!(bin.list().unwrap(), stopped.trashed);
        let _ = std::fs::remove_dir_all(&base);
    }
}
