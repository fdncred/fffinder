//! Folder listing and sorting.

use crate::entry::FileEntry;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;

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
        };
        let ord = ord.then_with(|| natural_cmp(&a.name, &b.name));
        if order.ascending { ord } else { ord.reverse() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
