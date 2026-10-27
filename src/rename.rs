//! Renaming many files at once: checking a list of new names before
//! anything changes, then renaming in two phases so swaps and case-only
//! changes work, rolling back if a step fails.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    let moves: Vec<&Planned> = planned
        .iter()
        .filter(|p| p.outcome == Outcome::Rename)
        .collect();
    // Phase 1: every source to a temporary name next to it.
    let stamp = format!("{}-{}", std::process::id(), nanos());
    let mut parked: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (i, p) in moves.iter().enumerate() {
        let temp = p.from.with_file_name(format!(".fff-rename-{stamp}-{i}"));
        if let Err(err) = std::fs::rename(&p.from, &temp) {
            undo(&parked);
            return Err(format!("Couldn’t rename “{}”: {err}", display(&p.from)));
        }
        parked.push((temp, p.from.clone()));
    }
    // Phase 2: temporary names to the new ones.
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (p, (temp, _)) in moves.iter().zip(&parked) {
        let to = p.to();
        // Checked in `plan`, but something may have appeared since.
        if to.exists() {
            undo(
                &done
                    .iter()
                    .map(|(t, f)| (t.clone(), f.clone()))
                    .collect::<Vec<_>>(),
            );
            undo(&parked[done.len()..]);
            return Err(format!("“{}” appeared while renaming.", p.new_name));
        }
        if let Err(err) = std::fs::rename(temp, &to) {
            undo(&done);
            undo(&parked[done.len()..]);
            return Err(format!("Couldn’t rename “{}”: {err}", display(&p.from)));
        }
        done.push((to, p.from.clone()));
    }
    Ok(done.into_iter().map(|(to, _)| to).collect())
}

/// Move each `(now, original)` back, newest first.
fn undo(moves: &[(PathBuf, PathBuf)]) {
    for (now, original) in moves.iter().rev() {
        let _ = std::fs::rename(now, original);
    }
}

fn display(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
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
