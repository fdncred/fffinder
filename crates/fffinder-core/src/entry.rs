//! A file or folder as shown in the browser and in search results.

use crate::kind::{self, FileKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Size in bytes; `None` for folders.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub kind: FileKind,
    pub hidden: bool,
    pub is_symlink: bool,
    /// When the item was made, where the file system records it.
    #[serde(default)]
    pub created: Option<SystemTime>,
    /// Unix permission bits, like `0o644`.
    #[serde(default)]
    pub mode: Option<u32>,
    /// Read-only, the only permission Windows keeps for files.
    #[serde(default)]
    pub readonly: bool,
    /// When the item was last opened. Only filled in when a view shows it;
    /// see [`FileEntry::read_last_opened`].
    #[serde(default)]
    pub last_opened: Option<SystemTime>,
}

impl FileEntry {
    /// Build from a path, reading metadata (follows symlinks, falls back to the link itself).
    pub fn from_path(path: impl Into<PathBuf>) -> Option<FileEntry> {
        let path = path.into();
        let link_meta = std::fs::symlink_metadata(&path).ok()?;
        let is_symlink = link_meta.file_type().is_symlink();
        let meta = if is_symlink {
            std::fs::metadata(&path).unwrap_or(link_meta)
        } else {
            link_meta
        };
        Some(Self::from_metadata(path, &meta, is_symlink))
    }

    pub fn from_metadata(path: PathBuf, meta: &std::fs::Metadata, is_symlink: bool) -> FileEntry {
        let is_dir = meta.is_dir();
        let name = display_name(&path);
        let hidden = is_hidden(&path, meta);
        FileEntry {
            kind: kind::classify(&path, is_dir),
            size: (!is_dir).then_some(meta.len()),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            mode: unix_mode(meta),
            last_opened: None,
            readonly: meta.permissions().readonly(),
            name,
            is_dir,
            hidden,
            is_symlink,
            path,
        }
    }

    /// Fill in [`FileEntry::last_opened`], which takes another read.
    pub fn read_last_opened(&mut self) {
        self.last_opened = crate::info::last_opened(&self.path);
    }

    /// Kind description for the list view.
    pub fn kind_label(&self) -> String {
        kind::kind_label(&self.path, self.is_dir)
    }

    pub fn badge(&self) -> Option<String> {
        kind::badge(&self.path, self.is_dir)
    }

    /// Folders can be navigated into; app bundles open instead.
    pub fn is_navigable(&self) -> bool {
        self.is_dir && self.kind == FileKind::Folder
    }

    pub fn parent(&self) -> Option<&Path> {
        self.path.parent()
    }
}

#[cfg(unix)]
fn unix_mode(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn unix_mode(_: &std::fs::Metadata) -> Option<u32> {
    None
}

/// File name, or the full path for roots like `/` and `C:\`.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn is_hidden(path: &Path, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 || dotfile(path)
}

#[cfg(not(windows))]
fn is_hidden(path: &Path, _meta: &std::fs::Metadata) -> bool {
    dotfile(path)
}

fn dotfile(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}
