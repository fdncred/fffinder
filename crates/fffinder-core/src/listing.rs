//! Folder listing and sorting.

use crate::entry::FileEntry;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// When a folder last changed: what it holds changes its modification time.
///
/// # Arguments
///
/// * `dir` - The folder
///
/// # Returns
///
/// Its modification time, or `None` when it can't be read
pub fn dir_stamp(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

/// Read a directory. Unreadable children are skipped; an unreadable folder is an error.
pub fn list_dir(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for child in std::fs::read_dir(dir)? {
        let Ok(child) = child else { continue };
        let path = child.path();
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        let is_symlink = file_type.is_symlink();
        let meta = if is_symlink {
            std::fs::metadata(&path).or_else(|_| child.metadata())
        } else {
            child.metadata()
        };
        let Ok(meta) = meta else { continue };
        let entry = FileEntry::from_metadata(path, &meta, is_symlink);
        if !show_hidden && entry.hidden {
            continue;
        }
        entries.push(entry);
    }
    Ok(entries)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Search ranking order (only meaningful for search results).
    Relevance,
    #[default]
    Name,
    Modified,
    Size,
    Kind,
    Created,
    #[serde(rename = "last_opened")]
    LastOpened,
    /// The folder an item is in, for lists that span folders.
    Folder,
}

impl SortKey {
    pub const BROWSE: [SortKey; 5] = [
        SortKey::Name,
        SortKey::Modified,
        SortKey::Created,
        SortKey::Size,
        SortKey::Kind,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Relevance => "Relevance",
            SortKey::Name => "Name",
            SortKey::Modified => "Date Modified",
            SortKey::Size => "Size",
            SortKey::Kind => "Kind",
            SortKey::Created => "Date Created",
            SortKey::LastOpened => "Date Last Opened",
            SortKey::Folder => "Folder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SortOrder {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for SortOrder {
    fn default() -> Self {
        SortOrder {
            key: SortKey::Name,
            ascending: true,
        }
    }
}

/// Case-insensitive, number-aware comparison ("file2" < "file10"), like Finder.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let na = take_number(&mut ai);
                let nb = take_number(&mut bi);
                let ord = na
                    .trim_start_matches('0')
                    .len()
                    .cmp(&nb.trim_start_matches('0').len())
                    .then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                let ord = ca.to_lowercase().cmp(cb.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        s.push(c);
        it.next();
    }
    s
}

/// Sort entries in place, folders first when `folders_first`. `Relevance`
/// keeps the incoming order.
pub fn sort_entries(entries: &mut [FileEntry], order: SortOrder, folders_first: bool) {
    if order.key == SortKey::Relevance {
        return;
    }
    entries.sort_by(|a, b| folders_on_top(a, b, folders_first).then_with(|| compare(a, b, order)));
}

/// With `folders_first`, folders before everything else, whichever way the
/// rest is sorted ("Keep Folders on Top").
pub fn folders_on_top(a: &FileEntry, b: &FileEntry, folders_first: bool) -> Ordering {
    if folders_first {
        b.is_navigable().cmp(&a.is_navigable())
    } else {
        Ordering::Equal
    }
}

/// Compare two entries for `order`; `Relevance` compares equal.
pub fn compare(a: &FileEntry, b: &FileEntry, order: SortOrder) -> Ordering {
    {
        let ord = match order.key {
            SortKey::Relevance => Ordering::Equal,
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Size => a
                .size
                .unwrap_or(0)
                .cmp(&b.size.unwrap_or(0))
                .then(a.is_dir.cmp(&b.is_dir).reverse()),
            SortKey::Kind => a
                .kind_label()
                .cmp(&b.kind_label())
                .then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Created => a.created.cmp(&b.created),
            SortKey::LastOpened => a.last_opened.cmp(&b.last_opened),
            SortKey::Folder => a.parent().cmp(&b.parent()),
        };
        let ord = ord.then_with(|| natural_cmp(&a.name, &b.name));
        if order.ascending { ord } else { ord.reverse() }
    }
}

/// An item that finishes a path being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCompletion {
    /// What the path field holds with this item chosen.
    pub text: String,
    /// The item's name.
    pub name: String,
    pub is_dir: bool,
}

/// `~` and `~/rest` as the home folder, and a relative path from `base`.
///
/// # Arguments
///
/// * `text` - The path as typed
/// * `base` - The folder a relative path starts from
///
/// # Returns
///
/// The path it names, which may not exist
pub fn resolve_typed_path(text: &str, base: Option<&Path>) -> PathBuf {
    let text = text.trim();
    let expanded = if text == "~" {
        dirs::home_dir().unwrap_or_default()
    } else if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        PathBuf::from(text)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base.map(|b| b.join(&expanded)).unwrap_or(expanded)
    }
}

/// The items a typed path could go on to name: everything in the folder it
/// has reached whose name starts with what's after the last separator,
/// ignoring case, folders first. Hidden items only show once a `.` is typed.
/// A bare `~` is the home folder, so Tab finishes it to `~/`.
///
/// # Arguments
///
/// * `typed` - The path as typed
/// * `base` - The folder a relative path starts from
///
/// # Returns
///
/// Every match, or none if the folder can't be read
pub fn complete_path(typed: &str, base: Option<&Path>) -> Vec<PathCompletion> {
    let typed = match typed.trim_start() {
        "~" => "~/",
        typed => typed,
    };
    // `/` works everywhere, and on Windows `\\` does too.
    let is_separator = |c: char| c == '/' || (cfg!(windows) && c == '\\');
    let split = typed.rfind(is_separator).map_or(0, |at| at + 1);
    let (folder, start) = typed.split_at(split);
    let dir = if folder.is_empty() {
        resolve_typed_path(".", base)
    } else {
        resolve_typed_path(folder, base)
    };
    let Ok(items) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let start_lower = start.to_lowercase();
    let mut matches: Vec<PathCompletion> = items
        .flatten()
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            let wanted = name.to_lowercase().starts_with(&start_lower)
                && (start.starts_with('.') || !name.starts_with('.'));
            if !wanted {
                return None;
            }
            // Through a link, as a folder that's a link is still a folder.
            let is_dir = item.path().is_dir();
            let end = if is_dir {
                std::path::MAIN_SEPARATOR_STR
            } else {
                ""
            };
            Some(PathCompletion {
                text: format!("{folder}{name}{end}"),
                name,
                is_dir,
            })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| natural_cmp(&a.name, &b.name))
    });
    matches
}

/// What every match shares, which is how far Tab can finish the path. Letters
/// that differ only in case count as shared, and the first match's are kept.
///
/// # Arguments
///
/// * `matches` - The matches from [`complete_path`]
///
/// # Returns
///
/// The shared text, or `None` when there are no matches
pub fn common_completion(matches: &[PathCompletion]) -> Option<String> {
    let first = matches.first()?;
    let mut end = first.text.len();
    for other in &matches[1..] {
        let shared: usize = first
            .text
            .chars()
            .zip(other.text.chars())
            .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            .map(|(a, _)| a.len_utf8())
            .sum();
        end = end.min(shared);
    }
    Some(first.text[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder holding `Documents/`, `Downloads/`, `Desktop.txt`, `.hidden`.
    fn typed_path_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fffinder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Documents")).unwrap();
        std::fs::create_dir_all(dir.join("Downloads")).unwrap();
        std::fs::write(dir.join("Desktop.txt"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        dir
    }

    fn names(matches: &[PathCompletion]) -> Vec<&str> {
        matches.iter().map(|m| m.name.as_str()).collect()
    }

    #[test]
    fn completes_from_the_folder_reached_so_far() {
        let dir = typed_path_fixture("complete");
        let sep = std::path::MAIN_SEPARATOR_STR;
        let base = Some(dir.as_path());

        assert_eq!(
            names(&complete_path("do", base)),
            ["Documents", "Downloads"]
        );
        assert_eq!(
            names(&complete_path("DE", base)),
            ["Desktop.txt"],
            "ignores case"
        );
        assert_eq!(
            names(&complete_path("", base)),
            ["Documents", "Downloads", "Desktop.txt"],
            "folders first, and no hidden items"
        );
        assert_eq!(names(&complete_path(".", base)), [".hidden"]);
        assert_eq!(complete_path("nope", base), []);
        assert_eq!(complete_path("nowhere/x", base), []);

        let typed = format!("{}/doc", dir.display());
        let found = complete_path(&typed, None);
        assert_eq!(
            found[0].text,
            format!("{}/Documents{sep}", dir.display()),
            "keeps what was typed, with the item's own case"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finishes_as_much_as_every_match_shares() {
        let dir = typed_path_fixture("common");
        let base = Some(dir.as_path());
        let sep = std::path::MAIN_SEPARATOR_STR;

        let shared = |typed: &str| common_completion(&complete_path(typed, base));

        assert_eq!(shared("doc"), Some(format!("Documents{sep}")));
        assert_eq!(shared("do"), Some("Do".to_string()), "both start with Do");
        assert_eq!(shared("d").as_deref(), Some("D"));
        assert_eq!(shared("nope"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_bare_tilde_completes_from_the_home_folder() {
        let dir = typed_path_fixture("tilde");
        let base = Some(dir.as_path());

        let from_tilde = complete_path("~", base);

        assert_eq!(from_tilde, complete_path("~/", base));
        assert!(from_tilde.iter().all(|found| found.text.starts_with("~/")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn typed_paths_resolve_from_home_and_the_base() {
        let base = Path::new("/base");
        let home = dirs::home_dir().unwrap_or_default();

        assert_eq!(resolve_typed_path("~", Some(base)), home);
        assert_eq!(resolve_typed_path("~/x", Some(base)), home.join("x"));
        assert_eq!(resolve_typed_path("x", Some(base)), base.join("x"));
        assert_eq!(resolve_typed_path("  /abs ", Some(base)), Path::new("/abs"));
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "alpha"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["alpha", "file1", "File2", "file10"]);
    }

    #[test]
    fn lists_temp_dir() {
        let dir = std::env::temp_dir().join(format!("fffinder-list-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.txt"), "hello").unwrap();
        std::fs::write(dir.join(".hidden"), "x").unwrap();
        let mut entries = list_dir(&dir, false).unwrap();
        sort_entries(&mut entries, SortOrder::default(), false);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["b.txt", "sub"]);
        assert_eq!(entries[0].size, Some(5));
        assert!(entries[1].is_dir);
        assert_eq!(list_dir(&dir, true).unwrap().len(), 3);
        sort_entries(&mut entries, SortOrder::default(), true);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sub", "b.txt"], "folders on top");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
