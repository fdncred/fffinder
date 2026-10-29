//! Renaming many files at once: checking a list of new names before
//! anything changes, then renaming in two phases so swaps and case-only
//! changes work, rolling back if a step fails. Undo moves items back the
//! same way.

use fffinder_core::ops::{self, TransferError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Why [`move_all`] stopped. Everything it had moved is back where it was.
#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    /// Something already has the name an item was moving to.
    #[error("An item named “{}” is already in “{}”", display(.0), folder(.0))]
    Taken(PathBuf),
    /// An item couldn't be moved.
    #[error("Couldn’t move “{}”: {source}", display(.path))]
    Failed {
        path: PathBuf,
        #[source]
        source: TransferError,
    },
}

/// What would happen to one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rename,
    Unchanged,
    /// Why the new name can't be used.
    Problem(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub from: PathBuf,
    pub new_name: String,
    pub outcome: Outcome,
}

impl Planned {
    pub fn to(&self) -> PathBuf {
        self.from.with_file_name(&self.new_name)
    }
}

/// File systems that ignore case (macOS and Windows by default) treat
/// `a.txt` and `A.txt` as one name.
fn key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(any(target_os = "macos", windows)) {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

/// Why `name` can't be a file name, if it can't.
pub fn name_problem(name: &str) -> Option<String> {
    let bad_chars: &[char] = if cfg!(windows) {
        &['/', '\\', ':', '*', '?', '"', '<', '>', '|']
    } else {
        &['/', '\0']
    };
    if name.trim().is_empty() {
        Some("The name is empty.".into())
    } else if name == "." || name == ".." {
        Some(format!("“{name}” isn't a file name."))
    } else if let Some(c) = name.chars().find(|c| bad_chars.contains(c)) {
        Some(format!("A name can’t contain “{c}”."))
    } else if name.len() > 255 {
        Some("The name is too long.".into())
    } else {
        None
    }
}

/// Check every new name against the others and the folder contents.
pub fn plan(items: &[(PathBuf, String)]) -> Vec<Planned> {
    let old_name = |from: &Path| {
        from.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    // Names on their own: empty, bad characters, or wanted by two items.
    let mut wanted: HashMap<String, usize> = HashMap::new();
    for (from, name) in items {
        if *name != old_name(from) {
            *wanted.entry(key(&from.with_file_name(name))).or_default() += 1;
        }
    }
    let mut outcomes: Vec<Outcome> = items
        .iter()
        .map(|(from, name)| {
            if *name == old_name(from) {
                Outcome::Unchanged
            } else if let Some(problem) = name_problem(name) {
                Outcome::Problem(problem)
            } else if wanted[&key(&from.with_file_name(name))] > 1 {
                Outcome::Problem("Another item gets this name too.".into())
            } else {
                Outcome::Rename
            }
        })
        .collect();
    // A name already taken is fine only when its owner is moving away (a
    // swap or chain) or is this same item with different case. Settle which
    // items really move, since one blocked rename can block another.
    let sources: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .map(|(i, (from, _))| (key(from), i))
        .collect();
    loop {
        let mut changed = false;
        for (i, (from, name)) in items.iter().enumerate() {
            if outcomes[i] != Outcome::Rename {
                continue;
            }
            let to = from.with_file_name(name);
            let free = match sources.get(&key(&to)) {
                Some(&owner) => owner == i || outcomes[owner] == Outcome::Rename,
                None => !to.exists(),
            };
            if !free {
                outcomes[i] = Outcome::Problem("An item with this name already exists.".into());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    items
        .iter()
        .zip(outcomes)
        .map(|((from, name), outcome)| Planned {
            from: from.clone(),
            new_name: name.clone(),
            outcome,
        })
        .collect()
}

/// Rename every [`Outcome::Rename`] item. Nothing runs if any item has a
/// problem. Returns the new paths; on failure everything moved so far is put
/// back and the error says which item failed.
pub fn apply(planned: &[Planned]) -> Result<Vec<PathBuf>, String> {
    if let Some(p) = planned
        .iter()
        .find(|p| matches!(p.outcome, Outcome::Problem(_)))
    {
        return Err(format!("“{}” can’t be renamed yet.", p.new_name));
    }
    let moves: Vec<(PathBuf, PathBuf)> = planned
        .iter()
        .filter(|p| p.outcome == Outcome::Rename)
        .map(|p| (p.from.clone(), p.to()))
        .collect();
    move_all(&moves).map_err(|err| err.to_string())?;
    Ok(moves.into_iter().map(|(_, to)| to).collect())
}

/// Move each `(from, to)` pair, all or nothing. Every item first moves to a
/// hidden name beside itself, then to where it goes, so items can swap
/// names, take names others leave, or change only the case of theirs. A
/// name that's taken by anything else stops it, and whatever had moved goes
/// back.
///
/// # Errors
///
/// [`MoveError::Taken`] when something else has a name an item is moving
/// to, and [`MoveError::Failed`] when an item can't be moved.
pub fn move_all(moves: &[(PathBuf, PathBuf)]) -> Result<(), MoveError> {
    // Phase 1: every item to a hidden name beside it.
    let stamp = format!("{}-{}", std::process::id(), nanos());
    let mut parked: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(moves.len());
    for (i, (from, _)) in moves.iter().enumerate() {
        let temp = from.with_file_name(format!(".fff-rename-{stamp}-{i}"));
        if let Err(err) = std::fs::rename(from, &temp) {
            put_back(&[], &parked);
            return Err(MoveError::Failed {
                path: from.clone(),
                source: ops::io_error(from)(err),
            });
        }
        parked.push((temp, from.clone()));
    }
    // Phase 2: hidden names to where the items go.
    for (done, ((_, to), (temp, from))) in moves.iter().zip(&parked).enumerate() {
        // Every item has left its own name, so anything there now is in the
        // way, even one whose name differs only in case.
        let failed = if to.symlink_metadata().is_ok() {
            Some(MoveError::Taken(to.clone()))
        } else {
            ops::move_item(temp, to)
                .err()
                .map(|source| MoveError::Failed {
                    path: from.clone(),
                    source,
                })
        };
        if let Some(err) = failed {
            put_back(&moves[..done], &parked);
            return Err(err);
        }
    }
    Ok(())
}

/// Undo a [`move_all`] that stopped: `moved` items go back to their hidden
/// names, newest first, then every parked item to its own name.
fn put_back(moved: &[(PathBuf, PathBuf)], parked: &[(PathBuf, PathBuf)]) {
    for ((_, to), (temp, _)) in moved.iter().zip(parked).rev() {
        let _ = ops::move_item(to, temp);
    }
    for (temp, from) in parked.iter().rev() {
        let _ = std::fs::rename(temp, from);
    }
}

fn display(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The name of the folder `path` is in.
fn folder(path: &Path) -> String {
    path.parent().map(display).unwrap_or_default()
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str, files: &[&str]) -> Dir {
            let dir =
                std::env::temp_dir().join(format!("fff-rename-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for f in files {
                std::fs::write(dir.join(f), f).unwrap();
            }
            Dir(dir)
        }

        fn items(&self, pairs: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
            pairs
                .iter()
                .map(|(from, to)| (self.0.join(from), to.to_string()))
                .collect()
        }

        fn contents(&self, name: &str) -> String {
            std::fs::read_to_string(self.0.join(name)).unwrap()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn outcomes(planned: &[Planned]) -> Vec<Outcome> {
        planned.iter().map(|p| p.outcome.clone()).collect()
    }

    #[test]
    fn renames_and_swaps() {
        let dir = Dir::new("swap", &["a.txt", "b.txt", "c.txt"]);
        let planned =
            plan(&dir.items(&[("a.txt", "b.txt"), ("b.txt", "a.txt"), ("c.txt", "c.txt")]));
        assert_eq!(
            outcomes(&planned),
            [Outcome::Rename, Outcome::Rename, Outcome::Unchanged]
        );
        let moved = apply(&planned).unwrap();
        assert_eq!(moved.len(), 2);
        assert_eq!(dir.contents("a.txt"), "b.txt");
        assert_eq!(dir.contents("b.txt"), "a.txt");
    }

    #[test]
    fn flags_problems_before_anything_moves() {
        let dir = Dir::new("problems", &["a.txt", "b.txt", "c.txt", "keep.txt"]);
        let planned = plan(&dir.items(&[
            ("a.txt", "x.txt"),
            ("b.txt", "x.txt"),
            ("c.txt", "keep.txt"),
            ("keep.txt", "sub/dir.txt"),
        ]));
        let problems: Vec<bool> = planned
            .iter()
            .map(|p| matches!(p.outcome, Outcome::Problem(_)))
            .collect();
        assert_eq!(problems, [true, true, true, true]);
        assert!(apply(&planned).is_err());
        assert_eq!(dir.contents("a.txt"), "a.txt", "nothing moved");
        assert!(matches!(
            &plan(&dir.items(&[("a.txt", "  ")]))[0].outcome,
            Outcome::Problem(_)
        ));
        // A name another item frees is fine.
        let chain = plan(&dir.items(&[("a.txt", "b.txt"), ("b.txt", "d.txt")]));
        assert_eq!(outcomes(&chain), [Outcome::Rename, Outcome::Rename]);
        apply(&chain).unwrap();
        assert_eq!(dir.contents("b.txt"), "a.txt");
        assert_eq!(dir.contents("d.txt"), "b.txt");
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn case_only_renames_work() {
        let dir = Dir::new("case", &["photo.JPG"]);
        let planned = plan(&dir.items(&[("photo.JPG", "photo.jpg")]));
        assert_eq!(outcomes(&planned), [Outcome::Rename]);
        apply(&planned).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["photo.jpg"]);
    }

    #[test]
    fn moves_all_or_nothing() {
        let dir = Dir::new("move-all", &["a.txt", "b.txt", "c.txt"]);
        let path = |name: &str| dir.0.join(name);
        // A swap, then the same swap again.
        let swap = [
            (path("a.txt"), path("b.txt")),
            (path("b.txt"), path("a.txt")),
        ];
        move_all(&swap).unwrap();
        assert_eq!(dir.contents("a.txt"), "b.txt");
        move_all(&swap).unwrap();
        assert_eq!(dir.contents("a.txt"), "a.txt");
        // A name something else has stops it, and the swap before it in the
        // list is put back too.
        let blocked = [
            (path("a.txt"), path("b.txt")),
            (path("b.txt"), path("a.txt")),
            (path("c.txt"), path("a.txt.bak")),
            (path("a.txt.bak"), path("x.txt")),
        ];
        std::fs::write(path("a.txt.bak"), "in the way").unwrap();
        std::fs::write(path("x.txt"), "taken").unwrap();
        assert!(matches!(move_all(&blocked), Err(MoveError::Taken(_))));
        assert_eq!(dir.contents("a.txt"), "a.txt");
        assert_eq!(dir.contents("b.txt"), "b.txt");
        assert_eq!(dir.contents("c.txt"), "c.txt");
        assert_eq!(dir.contents("a.txt.bak"), "in the way");
        assert_eq!(dir.contents("x.txt"), "taken");
        let hidden = std::fs::read_dir(&dir.0)
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().starts_with('.'));
        assert!(!hidden, "no parked names are left");
    }

    #[test]
    fn a_failed_step_puts_everything_back() {
        let dir = Dir::new("rollback", &["a.txt", "b.txt"]);
        let mut planned = plan(&dir.items(&[("a.txt", "one.txt"), ("b.txt", "two.txt")]));
        // b.txt disappears after planning, so phase 1 fails on it.
        std::fs::remove_file(dir.0.join("b.txt")).unwrap();
        planned[1].outcome = Outcome::Rename;
        assert!(apply(&planned).is_err());
        assert_eq!(dir.contents("a.txt"), "a.txt");
        assert!(!dir.0.join("one.txt").exists());
    }
}
