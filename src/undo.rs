//! Undo and Redo for file operations. Renames, moves, copies, new folders,
//! and moves to the Trash are recorded as they happen, in one log for every
//! window. Undo reverses the latest and keeps its opposite for Redo, which
//! reverses that in turn; any new operation clears what Redo had.

use crate::trash_bin::{Bin, Trashed};
use fffinder_core::entry::display_name;
use fffinder_core::ops;
use gpui_kit::{App, Global};
use std::path::PathBuf;

/// The most operations Undo remembers.
const LIMIT: usize = 100;

/// A change to files that Undo or Redo can reverse.
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

/// One recorded operation, named the way the notification after Undo or
/// Redo says it: "Undid Rename".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub name: &'static str,
    pub change: Change,
}

/// Which way through the log a step goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Undo,
    Redo,
}

impl Step {
    /// The notification's verb: "Undid Rename".
    pub fn past(self) -> &'static str {
        match self {
            Step::Undo => "Undid",
            Step::Redo => "Redid",
        }
    }
}

/// Operations Undo can reverse, and the opposites of undone ones, which
/// Redo can. The latest of each is last.
#[derive(Default)]
struct UndoLog {
    done: Vec<Operation>,
    undone: Vec<Operation>,
}

impl Global for UndoLog {}

fn push_capped(list: &mut Vec<Operation>, operation: Operation) {
    list.push(operation);
    if list.len() > LIMIT {
        list.remove(0);
    }
}

/// Remember an operation for Undo. Redo forgets what it had.
pub fn record(name: &'static str, change: Change, cx: &mut App) {
    if change.is_empty() {
        return;
    }
    let log = cx.default_global::<UndoLog>();
    log.undone.clear();
    push_capped(&mut log.done, Operation { name, change });
}

/// The latest operation `step` can reverse, taken off the log.
pub fn take_last(step: Step, cx: &mut App) -> Option<Operation> {
    let log = cx.default_global::<UndoLog>();
    match step {
        Step::Undo => log.done.pop(),
        Step::Redo => log.undone.pop(),
    }
}

/// `step` reversed an operation; `opposite` reverses it back, so it goes
/// where the other step finds it.
pub fn stepped(step: Step, opposite: Operation, cx: &mut App) {
    if opposite.change.is_empty() {
        return;
    }
    let log = cx.default_global::<UndoLog>();
    match step {
        Step::Undo => push_capped(&mut log.undone, opposite),
        Step::Redo => push_capped(&mut log.done, opposite),
    }
}

/// What reversing a change did: where the items it brought back are now,
/// and the change that would put things back as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reversed {
    pub back: Vec<PathBuf>,
    pub opposite: Change,
}

/// Reverse a change: move items back, trash what it made, or put back what
/// it trashed.
pub fn reverse(change: &Change, bin: &dyn Bin) -> Result<Reversed, String> {
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
            let opposite = pairs.iter().rev().map(|(f, t)| (t.clone(), f.clone()));
            Ok(Reversed {
                back,
                opposite: Change::Moved(opposite.collect()),
            })
        }
        Change::Created(paths) => {
            let there: Vec<PathBuf> = paths
                .iter()
                .filter(|p| p.symlink_metadata().is_ok())
                .cloned()
                .collect();
            let trashed = match there.is_empty() {
                true => Vec::new(),
                false => bin.trash(&there)?,
            };
            Ok(Reversed {
                back: Vec::new(),
                opposite: Change::Trashed(trashed),
            })
        }
        Change::Trashed(items) => {
            let back = bin.restore(items)?;
            Ok(Reversed {
                opposite: Change::Created(back.clone()),
                back,
            })
        }
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
        let undone = reverse(&moved, &bin).unwrap();
        assert_eq!(undone.back, std::slice::from_ref(&a));
        assert!(a.exists() && !b.exists());
        let redone = reverse(&undone.opposite, &bin).unwrap();
        assert!(
            b.exists() && !a.exists(),
            "reversing the opposite redoes it"
        );
        assert_eq!(redone.opposite, moved);
        reverse(&moved, &bin).unwrap();

        std::fs::copy(&a, &b).unwrap();
        std::fs::rename(&a, base.join("taken")).unwrap();
        std::fs::write(&a, "in the way").unwrap();
        let blocked = Change::Moved(vec![(a.clone(), base.join("taken"))]);
        assert!(reverse(&blocked, &bin).is_err(), "a taken name blocks undo");

        let undone = reverse(&Change::Created(vec![b.clone()]), &bin).unwrap();
        assert!(!b.exists(), "an undone copy goes to the Trash");
        reverse(&undone.opposite, &bin).unwrap();
        assert!(b.exists(), "a redone copy comes back out of the Trash");

        let trashed = bin.trash(std::slice::from_ref(&a)).unwrap();
        let undone = reverse(&Change::Trashed(trashed), &bin).unwrap();
        assert_eq!(undone.back, std::slice::from_ref(&a));
        assert!(a.exists());
        reverse(&undone.opposite, &bin).unwrap();
        assert!(!a.exists(), "a redone Move to Trash trashes it again");
        let _ = std::fs::remove_dir_all(&base);
    }
}
