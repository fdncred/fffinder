//! Undo for file operations. Renames, moves, copies, new folders, and moves
//! to the Trash are recorded as they happen, in one log for every window,
//! and Undo reverses the latest.

use crate::trash_bin::{Bin, Trashed};
use fffinder_core::entry::display_name;
use fffinder_core::ops;
use gpui_kit::{App, Global};
use std::path::PathBuf;

/// The most operations Undo remembers.
const LIMIT: usize = 100;

/// A change to files that Undo can reverse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Items renamed or moved, from where to where.
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Items made by a copy or New Folder. Undo puts them in the Trash.
    Created(Vec<PathBuf>),
    /// Items moved to the Trash. Undo puts them back.
    Trashed(Vec<Trashed>),
}

impl Change {
    fn is_empty(&self) -> bool {
        match self {
            Change::Moved(pairs) => pairs.is_empty(),
            Change::Created(paths) => paths.is_empty(),
            Change::Trashed(items) => items.is_empty(),
        }
    }
}

/// One recorded operation, named the way the notification after Undo says
/// it: "Undid Rename".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub name: &'static str,
    pub change: Change,
}

#[derive(Default)]
struct UndoLog(Vec<Operation>);

impl Global for UndoLog {}

/// Remember an operation for Undo.
pub fn record(name: &'static str, change: Change, cx: &mut App) {
    if change.is_empty() {
        return;
    }
    let log = &mut cx.default_global::<UndoLog>().0;
    log.push(Operation { name, change });
    if log.len() > LIMIT {
        log.remove(0);
    }
}

/// The latest operation, taken off the log.
pub fn take_last(cx: &mut App) -> Option<Operation> {
    cx.default_global::<UndoLog>().0.pop()
}

/// Reverse a change. Returns where the items it brought back are now.
pub fn reverse(change: &Change, bin: &dyn Bin) -> Result<Vec<PathBuf>, String> {
    match change {
        Change::Moved(pairs) => {
            let mut back = Vec::with_capacity(pairs.len());
            for (from, to) in pairs.iter().rev() {
                // A case-only rename leaves `from` naming the same item.
                let same =
                    from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
                if from.symlink_metadata().is_ok() && !same {
                    return Err(format!(
                        "An item named “{}” is already where “{}” was",
                        display_name(from),
                        display_name(to)
                    ));
                }
                ops::move_item(to, from).map_err(|e| e.to_string())?;
                back.push(from.clone());
            }
            Ok(back)
        }
        Change::Created(paths) => {
            let there: Vec<PathBuf> = paths
                .iter()
                .filter(|p| p.symlink_metadata().is_ok())
                .cloned()
                .collect();
            if !there.is_empty() {
                bin.trash(&there)?;
            }
            Ok(Vec::new())
        }
        Change::Trashed(items) => bin.restore(items),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trash_bin::FolderBin;

    #[test]
    fn reverses_moves_copies_and_trashing() {
        let base = std::env::temp_dir().join(format!("fffinder-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("dir")).unwrap();
        let (a, b) = (base.join("a.txt"), base.join("dir/b.txt"));
        std::fs::write(&a, "a").unwrap();
        let bin = FolderBin::new(base.join("bin"));

        std::fs::rename(&a, &b).unwrap();
        let moved = Change::Moved(vec![(a.clone(), b.clone())]);
        assert_eq!(reverse(&moved, &bin).unwrap(), std::slice::from_ref(&a));
        assert!(a.exists() && !b.exists());

        std::fs::copy(&a, &b).unwrap();
        std::fs::rename(&a, base.join("taken")).unwrap();
        std::fs::write(&a, "in the way").unwrap();
        let blocked = Change::Moved(vec![(a.clone(), base.join("taken"))]);
        assert!(reverse(&blocked, &bin).is_err(), "a taken name blocks undo");

        reverse(&Change::Created(vec![b.clone()]), &bin).unwrap();
        assert!(!b.exists(), "an undone copy goes to the Trash");

        let trashed = bin.trash(std::slice::from_ref(&a)).unwrap();
        assert_eq!(
            reverse(&Change::Trashed(trashed), &bin).unwrap(),
            std::slice::from_ref(&a)
        );
        assert!(a.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
