//! The folders inside a folder, for the sidebar's folder tree. This reads
//! less than a full listing: only names and kinds, which the system returns
//! with the listing, plus where a link leads.

use crate::entry;
use crate::kind::{self, FileKind};
use crate::listing::{dir_stamp, natural_cmp};
use rayon::prelude::*;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A folder inside another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subfolder {
    /// Where it is.
    pub path: PathBuf,
    /// The name it shows under.
    pub name: String,
    /// Whether it's hidden, by a leading dot or the system's hidden flag.
    pub hidden: bool,
}

/// Which kinds of folders a folder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inside {
    /// A folder that isn't hidden.
    pub shown: bool,
    /// A hidden folder.
    pub hidden: bool,
    /// The folder couldn't be read, so what it holds is unknown.
    pub unreadable: bool,
}

impl Inside {
    /// Whether the folder has folders to show.
    ///
    /// # Arguments
    ///
    /// * `show_hidden` - Whether hidden folders show
    ///
    /// # Returns
    ///
    /// `true` when a folder inside would show
    pub fn any(self, show_hidden: bool) -> bool {
        self.shown || (show_hidden && self.hidden)
    }
}

/// What looking at a folder found, and when the folder last changed. The
/// time is read before the look, so a change during it shows as a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped<T> {
    /// The folder looked at.
    pub dir: PathBuf,
    /// What was found in it.
    pub found: T,
    /// When it last changed, or `None` when that can't be read.
    pub stamp: Option<SystemTime>,
}

/// The folders in `dir`, sorted by name as Finder sorts them, hidden ones
/// included and marked. App bundles and other packages are left out, since
/// they open rather than show what's inside, and links to folders count as
/// folders. Items that can't be read are skipped.
///
/// # Arguments
///
/// * `dir` - The folder to read
///
/// # Returns
///
/// The folders inside it
///
/// # Errors
///
/// Returns the I/O error if `dir` itself can't be read.
pub fn list(dir: &Path) -> io::Result<Vec<Subfolder>> {
    let mut folders: Vec<Subfolder> = folders_in(dir)?
        .map(|(path, hidden)| Subfolder {
            name: entry::display_name(&path),
            path,
            hidden,
        })
        .collect();
    folders.sort_unstable_by(|a, b| natural_cmp(&a.name, &b.name));
    Ok(folders)
}

/// [`list`] for each of `dirs`, read in parallel and stamped.
///
/// # Arguments
///
/// * `dirs` - The folders to read
///
/// # Returns
///
/// The folders in each, in the same order, or `None` for one that can't be
/// read
pub fn list_each(dirs: Vec<PathBuf>) -> Vec<Stamped<Option<Vec<Subfolder>>>> {
    stamp_each(dirs, |dir| list(dir).ok())
}

/// What folders `dir` holds. Reading stops at the first one that isn't
/// hidden, so it usually takes one look at the folder's first entries.
///
/// # Arguments
///
/// * `dir` - The folder to look in
///
/// # Returns
///
/// Which kinds of folders it holds, or that it can't be read
pub fn inside(dir: &Path) -> Inside {
    let Ok(folders) = folders_in(dir) else {
        return Inside {
            unreadable: true,
            ..Inside::default()
        };
    };
    let mut found = Inside::default();
    for (_, hidden) in folders {
        if !hidden {
            found.shown = true;
            break;
        }
        found.hidden = true;
    }
    found
}

/// [`inside`] for each of `dirs`, looked at in parallel and stamped.
///
/// # Arguments
///
/// * `dirs` - The folders to look in
///
/// # Returns
///
/// What each holds, in the same order
pub fn inside_each(dirs: Vec<PathBuf>) -> Vec<Stamped<Inside>> {
    stamp_each(dirs, inside)
}

/// The folders that changed since they were stamped, checked in parallel.
///
/// # Arguments
///
/// * `stamped` - Each folder, with when it last changed as of the last look
///
/// # Returns
///
/// The folders whose time differs now, in the same order
pub fn changed(stamped: Vec<(PathBuf, Option<SystemTime>)>) -> Vec<PathBuf> {
    stamped
        .into_par_iter()
        .filter(|(dir, stamp)| dir_stamp(dir) != *stamp)
        .map(|(dir, _)| dir)
        .collect()
}

/// Look at each of `dirs` in parallel, stamping each before the look.
fn stamp_each<T: Send>(dirs: Vec<PathBuf>, look: impl Fn(&Path) -> T + Sync) -> Vec<Stamped<T>> {
    dirs.into_par_iter()
        .map(|dir| {
            let stamp = dir_stamp(&dir);
            let found = look(&dir);
            Stamped { dir, found, stamp }
        })
        .collect()
}

/// Each folder in `dir` and whether it's hidden, in the order the system
/// lists them. Files are passed over without making their paths.
fn folders_in(dir: &Path) -> io::Result<impl Iterator<Item = (PathBuf, bool)>> {
    Ok(std::fs::read_dir(dir)?.filter_map(|child| {
        let child = child.ok()?;
        let file_type = child.file_type().ok()?;
        if !file_type.is_dir() && !file_type.is_symlink() {
            return None;
        }
        let path = child.path();
        // A link counts when it leads to a folder, which takes reading it.
        if file_type.is_symlink() && !path.is_dir() {
            return None;
        }
        if kind::classify(&path, true) != FileKind::Folder {
            return None;
        }
        let hidden = entry::listed_item_hidden(&child, &path);
        Some((path, hidden))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder holding `a/`, `a10/`, `a2/`, `B/`, `.cache/`, `Tool.app/`,
    /// and `notes.txt`; `a/` holds only `.git/`, and `B/` holds `inner/`.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fffinder-sub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for folder in [
            "a/.git",
            "a10",
            "a2",
            "B/inner",
            ".cache",
            "Tool.app/Contents",
        ] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "notes").unwrap();
        dir
    }

    #[test]
    fn lists_only_folders_sorted_by_name() {
        let dir = fixture("list");
        let names: Vec<(String, bool)> = list(&dir)
            .unwrap()
            .into_iter()
            .map(|f| (f.name, f.hidden))
            .collect();
        assert_eq!(
            names,
            [
                (".cache".to_string(), true),
                ("a".to_string(), false),
                ("a2".to_string(), false),
                ("a10".to_string(), false),
                ("B".to_string(), false),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_folder_is_an_error() {
        let dir = std::env::temp_dir()
            .join(format!("fffinder-sub-missing-{}", std::process::id()))
            .join("not-there");
        assert!(list(&dir).is_err());
        assert!(inside(&dir).unreadable);
        assert!(!inside(&dir).any(true));
        let each = list_each(vec![dir.clone()]);
        assert_eq!(
            each,
            [Stamped {
                dir,
                found: None,
                stamp: None
            }]
        );
    }

    #[test]
    fn tells_hidden_folders_from_shown_ones() {
        let dir = fixture("inside");
        assert_eq!(
            inside(&dir.join("a")),
            Inside {
                shown: false,
                hidden: true,
                unreadable: false
            }
        );
        assert_eq!(
            inside(&dir.join("B")),
            Inside {
                shown: true,
                hidden: false,
                unreadable: false
            }
        );
        assert_eq!(inside(&dir.join("a10")), Inside::default());
        assert!(!inside(&dir.join("a")).any(false));
        assert!(inside(&dir.join("a")).any(true));
        let each: Vec<Inside> = inside_each(vec![dir.join("B"), dir.join("a10")])
            .into_iter()
            .map(|looked| looked.found)
            .collect();
        assert_eq!(each, [inside(&dir.join("B")), Inside::default()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_read_is_stamped_with_when_its_folder_changed() {
        let dir = fixture("stamped");
        let read = list_each(vec![dir.join("B"), dir.join("a10")]);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].dir, dir.join("B"));
        assert_eq!(read[0].stamp, dir_stamp(&dir.join("B")));
        let names: Vec<String> = read[0]
            .found
            .iter()
            .flatten()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["inner"]);
        assert_eq!(read[1].found, Some(Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_folders_whose_time_moved_have_changed() {
        let dir = fixture("changed");
        let (same, moved) = (dir.join("B"), dir.join("a10"));
        let stamps = vec![
            (same.clone(), dir_stamp(&same)),
            (moved.clone(), Some(SystemTime::UNIX_EPOCH)),
        ];
        assert_eq!(changed(stamps), [moved]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_folder_is_a_folder() {
        let dir = fixture("links");
        std::os::unix::fs::symlink(dir.join("B"), dir.join("to-b")).unwrap();
        std::os::unix::fs::symlink(dir.join("notes.txt"), dir.join("to-notes")).unwrap();
        let names: Vec<String> = list(&dir).unwrap().into_iter().map(|f| f.name).collect();
        assert!(names.contains(&"to-b".to_string()));
        assert!(!names.contains(&"to-notes".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
