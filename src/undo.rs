//! Undo and Redo for file operations. Renames, moves, copies, new folders,
//! and moves to the Trash are recorded as they happen, in one log for every
//! window. Undo reverses the latest and keeps its opposite for Redo, which
//! reverses that in turn; any new operation clears what Redo had. While an
//! Undo or Redo runs, no other file operation may start.

use crate::rename::{self, MoveError};
use crate::trash_bin::{Bin, TrashError, Trashed};
use gpui_kit::{App, Global};
use std::path::PathBuf;
use std::sync::{Arc, Weak};

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

    /// The command's name: "Undo".
    pub fn name(self) -> &'static str {
        match self {
            Step::Undo => "Undo",
            Step::Redo => "Redo",
        }
    }
}

/// Held for as long as an Undo or Redo runs. It ends when this is dropped,
/// whether the step finished or its window closed partway, so file
/// operations can never be left waiting for good.
pub struct Running {
    _step: Arc<Step>,
}

/// Operations Undo can reverse, and the opposites of undone ones, which
/// Redo can. The latest of each is last.
#[derive(Default)]
struct UndoLog {
    done: Vec<Operation>,
    undone: Vec<Operation>,
    /// The step running now, if any. `Weak` is a reference that doesn't
    /// keep its value alive, so this empties when the `Running` is dropped.
    running: Weak<Step>,
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

/// The latest operation `step` can reverse, taken off the log, and what
/// marks the step as running until it's dropped.
pub fn take_last(step: Step, cx: &mut App) -> Option<(Operation, Running)> {
    let log = cx.default_global::<UndoLog>();
    let operation = match step {
        Step::Undo => log.done.pop(),
        Step::Redo => log.undone.pop(),
    }?;
    let running = Arc::new(step);
    log.running = Arc::downgrade(&running);
    Some((operation, Running { _step: running }))
}

/// The Undo or Redo running now, if any.
pub fn running(cx: &App) -> Option<Step> {
    let log = cx.try_global::<UndoLog>()?;
    log.running.upgrade().map(|step| *step)
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

/// `step` couldn't reverse `operation` (or the part of one that's left), so
/// it goes back where `step` found it, to be tried again.
pub fn put_back(step: Step, operation: Operation, cx: &mut App) {
    if operation.change.is_empty() {
        return;
    }
    let log = cx.default_global::<UndoLog>();
    match step {
        Step::Undo => push_capped(&mut log.done, operation),
        Step::Redo => push_capped(&mut log.undone, operation),
    }
}

/// What reversing a change did: where the items it brought back are now,
/// and the change that would put things back as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reversed {
    pub back: Vec<PathBuf>,
    pub opposite: Change,
}

/// Why reversing a change failed.
#[derive(Debug, thiserror::Error)]
pub enum ReverseError {
    #[error(transparent)]
    Move(#[from] MoveError),
    #[error(transparent)]
    Trash(#[from] TrashError),
}

/// A reversal that stopped part way: what it reversed, what's left of the
/// change, and why it stopped.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct Stopped {
    pub done: Reversed,
    pub left: Change,
    #[source]
    pub error: ReverseError,
}

/// Reverse a change: move items back, trash what it made, or put back what
/// it trashed.
///
/// # Errors
///
/// Moves are reversed all or nothing; trashing and putting back stop at
/// the first item that fails. Either way [`Stopped`] says what was reversed
/// and what's left.
pub fn reverse(change: &Change, bin: &dyn Bin) -> Result<Reversed, Box<Stopped>> {
    match change {
        Change::Moved(pairs) => {
            let back: Vec<(PathBuf, PathBuf)> =
                pairs.iter().map(|(f, t)| (t.clone(), f.clone())).collect();
            match rename::move_all(&back) {
                Ok(()) => Ok(Reversed {
                    back: pairs.iter().map(|(from, _)| from.clone()).collect(),
                    opposite: Change::Moved(back),
                }),
                Err(err) => Err(Box::new(Stopped {
                    done: Reversed {
                        back: Vec::new(),
                        opposite: Change::Moved(Vec::new()),
                    },
                    left: change.clone(),
                    error: err.into(),
                })),
            }
        }
        Change::Created(paths) => {
            let there = |p: &&PathBuf| p.symlink_metadata().is_ok();
            let made: Vec<PathBuf> = paths.iter().filter(there).cloned().collect();
            match made.is_empty() {
                true => Ok(Vec::new()),
                false => bin.trash(&made),
            }
            .map(|trashed| Reversed {
                back: Vec::new(),
                opposite: Change::Trashed(trashed),
            })
            .map_err(|stopped| {
                Box::new(Stopped {
                    done: Reversed {
                        back: Vec::new(),
                        opposite: Change::Trashed(stopped.trashed),
                    },
                    left: Change::Created(paths.iter().filter(there).cloned().collect()),
                    error: stopped.error.into(),
                })
            })
        }
        Change::Trashed(items) => match bin.restore(items) {
            Ok(back) => Ok(Reversed {
                opposite: Change::Created(back.clone()),
                back,
            }),
            Err(err) => {
                let (left, restored): (Vec<Trashed>, Vec<Trashed>) =
                    items.iter().cloned().partition(Trashed::in_trash);
                let back: Vec<PathBuf> = restored.into_iter().filter_map(|t| t.original).collect();
                Err(Box::new(Stopped {
                    done: Reversed {
                        opposite: Change::Created(back.clone()),
                        back,
                    },
                    left: Change::Trashed(left),
                    error: err.into(),
                }))
            }
        },
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
        let stopped = reverse(&blocked, &bin).unwrap_err();
        assert!(matches!(
            stopped.error,
            ReverseError::Move(MoveError::Taken(_))
        ));
        assert_eq!(stopped.left, blocked, "all of it is left to undo");
        std::fs::rename(base.join("taken"), &a).unwrap();

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

    #[test]
    fn reverses_swaps_and_chains() {
        let base = std::env::temp_dir().join(format!("fffinder-undo-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let path = |name: &str| base.join(name);
        for name in ["a", "b", "c"] {
            std::fs::write(path(name), name).unwrap();
        }
        let bin = FolderBin::new(base.join("bin"));
        let read = |name: &str| std::fs::read_to_string(path(name)).unwrap();

        // Batch rename swapped a and b, and moved c to d.
        let renamed = Change::Moved(vec![
            (path("a"), path("b")),
            (path("b"), path("a")),
            (path("c"), path("d")),
        ]);
        rename::move_all(&[
            (path("a"), path("b")),
            (path("b"), path("a")),
            (path("c"), path("d")),
        ])
        .unwrap();
        assert_eq!(
            (read("a"), read("b"), read("d")),
            ("b".into(), "a".into(), "c".into())
        );
        let undone = reverse(&renamed, &bin).unwrap();
        assert_eq!(
            (read("a"), read("b"), read("c")),
            ("a".into(), "b".into(), "c".into())
        );
        reverse(&undone.opposite, &bin).unwrap();
        assert_eq!(read("a"), "b", "and Redo swaps them again");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_rename_differing_in_case_only_undoes_without_overwriting() {
        let base = std::env::temp_dir().join(format!("fffinder-undo-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (upper, lower) = (base.join("Notes"), base.join("notes"));
        std::fs::write(&lower, "renamed").unwrap();
        let bin = FolderBin::new(base.join("bin"));
        let renamed = Change::Moved(vec![(upper.clone(), lower.clone())]);
        let ignores_case = upper.exists();
        if !ignores_case {
            // A new item took the old name: Undo mustn't replace it.
            std::fs::write(&upper, "new").unwrap();
            assert!(reverse(&renamed, &bin).is_err());
            assert_eq!(std::fs::read_to_string(&upper).unwrap(), "new");
            assert_eq!(std::fs::read_to_string(&lower).unwrap(), "renamed");
        } else {
            reverse(&renamed, &bin).unwrap();
            let names: Vec<String> = std::fs::read_dir(&base)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n != "bin")
                .collect();
            assert_eq!(names, ["Notes"]);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_partial_reversal_says_what_is_left() {
        let base = std::env::temp_dir().join(format!("fffinder-undo-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let bin = FolderBin::new(base.join("bin"));
        let trashed = bin.trash(&[a.clone(), b.clone()]).unwrap();
        // b's name is taken again, so only a goes back.
        std::fs::write(&b, "new b").unwrap();
        let stopped = reverse(&Change::Trashed(trashed.clone()), &bin).unwrap_err();
        assert_eq!(stopped.done.back, std::slice::from_ref(&a));
        assert_eq!(stopped.done.opposite, Change::Created(vec![a.clone()]));
        assert_eq!(stopped.left, Change::Trashed(vec![trashed[1].clone()]));
        // Once b's place is free, the rest goes back too.
        std::fs::remove_file(&b).unwrap();
        let rest = reverse(&stopped.left, &bin).unwrap();
        assert_eq!(rest.back, std::slice::from_ref(&b));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn running_ends_when_dropped() {
        let log = UndoLog::default();
        let running = Arc::new(Step::Redo);
        let log = UndoLog {
            running: Arc::downgrade(&running),
            ..log
        };
        assert_eq!(log.running.upgrade().as_deref(), Some(&Step::Redo));
        drop(Running { _step: running });
        assert!(log.running.upgrade().is_none());
    }
}
