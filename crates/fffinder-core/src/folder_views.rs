//! The view and sort last chosen in each folder, kept apart from the settings
//! because it grows with use. It only saves choices, so losing it is harmless.

use crate::config::{ViewMode, data_dir, write_file};
use crate::listing::SortOrder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How many folders to remember; the ones chosen longest ago go first.
const MAX_FOLDERS: usize = 2000;

/// What was chosen in a folder, and when compared with the other folders.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct FolderView {
    view: ViewMode,
    sort: SortOrder,
    chosen: u64,
}

/// The view and sort chosen in each folder.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderViews {
    // Paths as text, since JSON object keys have to be text.
    folders: HashMap<String, FolderView>,
    /// Counts up with each choice, to tell which folders were chosen longest ago.
    next_choice: u64,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn default_path() -> PathBuf {
    data_dir().join("folder_views.json")
}

impl FolderViews {
    /// Read what's saved.
    ///
    /// # Returns
    ///
    /// What's saved, or nothing if there's no file or it can't be read
    pub fn load() -> FolderViews {
        Self::load_from(&default_path())
    }

    fn load_from(path: &Path) -> FolderViews {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Save for next time.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the file can't be written.
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&default_path())
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        write_file(path, &text)
    }

    /// Look up what was chosen in a folder.
    ///
    /// # Arguments
    ///
    /// * `folder` - The folder to look up
    ///
    /// # Returns
    ///
    /// The view and sort last chosen there, or `None` if none was
    pub fn get(&self, folder: &Path) -> Option<(ViewMode, SortOrder)> {
        self.folders.get(&key(folder)).map(|f| (f.view, f.sort))
    }

    /// Remember what was chosen in a folder, forgetting the folder chosen
    /// longest ago once there are too many.
    ///
    /// # Arguments
    ///
    /// * `folder` - The folder it was chosen in
    /// * `view` - The view chosen
    /// * `sort` - The sort chosen
    pub fn set(&mut self, folder: &Path, view: ViewMode, sort: SortOrder) {
        let chosen = self.next_choice;
        self.next_choice += 1;
        self.folders
            .insert(key(folder), FolderView { view, sort, chosen });
        if self.folders.len() > MAX_FOLDERS
            && let Some(oldest) = self
                .folders
                .iter()
                .min_by_key(|(_, f)| f.chosen)
                .map(|(path, _)| path.clone())
        {
            self.folders.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::SortKey;

    fn by_size() -> SortOrder {
        SortOrder {
            key: SortKey::Size,
            ascending: false,
        }
    }

    #[test]
    fn remembers_each_folder_and_round_trips() {
        let path = std::env::temp_dir().join(format!("fffinder-views-{}.json", std::process::id()));
        let mut views = FolderViews::default();
        views.set(Path::new("/a"), ViewMode::List, by_size());
        views.set(Path::new("/b"), ViewMode::Gallery, SortOrder::default());
        views.set(Path::new("/a"), ViewMode::Columns, by_size());

        views.save_to(&path).unwrap();
        let loaded = FolderViews::load_from(&path);

        assert_eq!(loaded, views);
        assert_eq!(
            loaded.get(Path::new("/a")),
            Some((ViewMode::Columns, by_size()))
        );
        assert_eq!(loaded.get(Path::new("/nope")), None);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(FolderViews::load_from(&path), FolderViews::default());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn forgets_the_folders_chosen_longest_ago() {
        let mut views = FolderViews::default();
        for ix in 0..=MAX_FOLDERS {
            views.set(
                Path::new(&format!("/folder{ix}")),
                ViewMode::List,
                SortOrder::default(),
            );
        }

        assert_eq!(views.folders.len(), MAX_FOLDERS);
        assert_eq!(views.get(Path::new("/folder0")), None);
        assert!(views.get(Path::new("/folder1")).is_some());
    }
}
