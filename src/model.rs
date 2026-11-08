//! Browser state that doesn't depend on GPUI: navigation history and selection.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What the main area shows when no search is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Folder(PathBuf),
    /// Recently used files across the search roots.
    Recents,
    /// Files sent from elsewhere, like `fff show` in Nushell, shown the way
    /// search results are.
    Results {
        title: String,
        paths: Arc<Vec<PathBuf>>,
    },
    /// What's in the Trash.
    Trash,
}

impl Location {
    pub fn folder(&self) -> Option<&Path> {
        match self {
            Location::Folder(path) => Some(path),
            Location::Recents | Location::Results { .. } | Location::Trash => None,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Location::Folder(path) => fffinder_core::entry::display_name(path),
            Location::Recents => "Recents".into(),
            Location::Results { title, .. } => title.clone(),
            Location::Trash => "Trash".into(),
        }
    }

    /// Go where the folder went, when it moved or was inside one that did.
    /// Returns whether it moved.
    fn follow(&mut self, moves: &Moves) -> bool {
        let Location::Folder(path) = self else {
            return false;
        };
        match moves.follow(path) {
            Some(to) => {
                *path = to;
                true
            }
            None => false,
        }
    }
}

/// `path` with `from` at its start swapped for `to`.
///
/// # Arguments
///
/// * `path` - The path to move
/// * `from` - Where a folder was
/// * `to` - Where it is now
///
/// # Returns
///
/// The path inside the moved folder, or `None` when `path` isn't in `from`
pub fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    // Joining an empty path would add a trailing separator.
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}

/// Items that were renamed or moved, looked up by where each was, to tell
/// where a path that was in one of them is now.
#[derive(Debug, Clone, Default)]
pub struct Moves<'a>(HashMap<&'a Path, &'a Path>);

impl<'a> Moves<'a> {
    /// Moves from each pair's first path to its second.
    ///
    /// # Arguments
    ///
    /// * `pairs` - Where each item was, and where it is now
    ///
    /// # Returns
    ///
    /// The moves, ready to look up
    pub fn new(pairs: &'a [(PathBuf, PathBuf)]) -> Moves<'a> {
        Moves(
            pairs
                .iter()
                .map(|(from, to)| (from.as_path(), to.as_path()))
                .collect(),
        )
    }

    /// Where `path` is now. Each folder above it is looked up, so this
    /// takes as many lookups as the path is deep, however many items moved.
    ///
    /// # Arguments
    ///
    /// * `path` - Where something was
    ///
    /// # Returns
    ///
    /// Where it is now, or `None` when neither it nor a folder holding it
    /// moved
    pub fn follow(&self, path: &Path) -> Option<PathBuf> {
        path.ancestors()
            .find_map(|dir| rebase(path, dir, self.0.get(dir)?))
    }
}

#[derive(Debug, Clone)]
pub struct History {
    back: Vec<Location>,
    forward: Vec<Location>,
    current: Location,
}

impl History {
    pub fn new(start: Location) -> History {
        History {
            back: Vec::new(),
            forward: Vec::new(),
            current: start,
        }
    }

    pub fn current(&self) -> &Location {
        &self.current
    }

    /// Go somewhere new; clears the forward stack. No-op for the current location.
    pub fn navigate(&mut self, to: Location) -> bool {
        if to == self.current {
            return false;
        }
        let from = std::mem::replace(&mut self.current, to);
        self.back.push(from);
        self.forward.clear();
        true
    }

    pub fn back(&mut self) -> bool {
        let Some(prev) = self.back.pop() else {
            return false;
        };
        let from = std::mem::replace(&mut self.current, prev);
        self.forward.push(from);
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        let from = std::mem::replace(&mut self.current, next);
        self.back.push(from);
        true
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Keep up with items that moved: every folder in the history that was
    /// one of them, or inside one, is now where it went.
    ///
    /// # Arguments
    ///
    /// * `moves` - What moved, and where
    ///
    /// # Returns
    ///
    /// Whether the current location moved
    pub fn follow(&mut self, moves: &Moves) -> bool {
        for location in self.back.iter_mut().chain(&mut self.forward) {
            location.follow(moves);
        }
        self.current.follow(moves)
    }
}

/// Multi-selection over a list of items, Finder style: click, Cmd-click
/// toggles, Shift-click and Shift-arrows extend from an anchor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    items: BTreeSet<usize>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    /// Counts each change to `items`.
    revision: u64,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Selection {
            revision: self.revision + 1,
            ..Selection::default()
        };
    }

    /// Tells one set of selected items from another without comparing them.
    ///
    /// # Returns
    ///
    /// A number that changes whenever the selected items may have changed
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn contains(&self, ix: usize) -> bool {
        self.items.contains(&ix)
    }

    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.items.iter().copied()
    }

    /// The focused item: last clicked or moved to.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    pub fn select_only(&mut self, ix: usize) {
        self.revision += 1;
        self.items.clear();
        self.items.insert(ix);
        self.anchor = Some(ix);
        self.cursor = Some(ix);
    }

    pub fn toggle(&mut self, ix: usize) {
        self.revision += 1;
        if !self.items.remove(&ix) {
            self.items.insert(ix);
        }
        self.anchor = Some(ix);
        self.cursor = Some(ix);
    }

    /// Select the contiguous range from the anchor to `ix`.
    pub fn extend_to(&mut self, ix: usize) {
        self.revision += 1;
        let anchor = self.anchor.unwrap_or(ix);
        self.items = (anchor.min(ix)..=anchor.max(ix)).collect();
        self.anchor = Some(anchor);
        self.cursor = Some(ix);
    }

    /// Select exactly `items`, leaving the cursor on the last one and the
    /// anchor on the first, as a drag across them would.
    ///
    /// # Arguments
    ///
    /// * `items` - The indices to select
    pub fn select_items(&mut self, items: BTreeSet<usize>) {
        self.revision += 1;
        self.anchor = items.first().copied();
        self.cursor = items.last().copied();
        self.items = items;
    }

    pub fn select_all(&mut self, len: usize) {
        self.revision += 1;
        self.items = (0..len).collect();
        if len > 0 {
            self.anchor.get_or_insert(0);
            self.cursor.get_or_insert(0);
        }
    }

    /// Arrow-key movement by `delta` items, clamped to the list.
    pub fn move_by(&mut self, delta: isize, extend: bool, len: usize) {
        if len == 0 {
            return;
        }
        let target = match self.cursor {
            Some(cur) => (cur as isize + delta).clamp(0, len as isize - 1) as usize,
            None if delta < 0 => len - 1,
            None => 0,
        };
        if extend {
            self.extend_to(target);
        } else {
            self.select_only(target);
        }
    }

    /// Move the cursor `step` selected items along, wrapping around, and keep
    /// the selection: Quick Look steps through several items this way. A
    /// cursor outside the selection counts as the first selected item.
    pub fn cycle_cursor(&mut self, step: isize) {
        let items: Vec<usize> = self.items.iter().copied().collect();
        if items.is_empty() {
            return;
        }
        let pos = self
            .cursor
            .and_then(|cur| items.iter().position(|&ix| ix == cur))
            .unwrap_or(0);
        let next = (pos as isize + step).rem_euclid(items.len() as isize) as usize;
        self.cursor = Some(items[next]);
    }

    /// Keep the same items selected after the list was re-sorted or refreshed.
    pub fn remap(&mut self, old: &[PathBuf], new: &[PathBuf]) {
        self.revision += 1;
        let index_of = |path: &PathBuf| new.iter().position(|p| p == path);
        let items: BTreeSet<usize> = self
            .items
            .iter()
            .filter_map(|&ix| old.get(ix).and_then(index_of))
            .collect();
        let map = |ix: Option<usize>| ix.and_then(|ix| old.get(ix)).and_then(index_of);
        self.anchor = map(self.anchor);
        self.cursor = map(self.cursor);
        self.items = items;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_a_set_of_items_runs_from_the_first_to_the_last() {
        let mut selection = Selection::default();

        selection.select_items([2, 5, 7].into_iter().collect());

        assert_eq!(selection.indices().collect::<Vec<_>>(), [2, 5, 7]);
        assert_eq!(selection.cursor(), Some(7));
        // Shift+click starts again from the first of them.
        selection.extend_to(3);
        assert_eq!(selection.indices().collect::<Vec<_>>(), [2, 3]);

        selection.select_items(Default::default());
        assert!(selection.is_empty());
        assert_eq!(selection.cursor(), None);
    }

    #[test]
    fn the_revision_changes_with_the_selected_items() {
        let mut s = Selection::default();
        let mut seen = vec![s.revision()];
        let mut changed = |s: &Selection| {
            assert!(!seen.contains(&s.revision()), "{seen:?}");
            seen.push(s.revision());
        };

        s.select_only(1);
        changed(&s);
        s.toggle(2);
        changed(&s);
        s.extend_to(4);
        changed(&s);
        s.select_items([3].into_iter().collect());
        changed(&s);
        s.select_all(9);
        changed(&s);
        s.move_by(1, false, 9);
        changed(&s);
        s.clear();
        changed(&s);
        s.select_only(1);
        changed(&s);
        let paths: Vec<PathBuf> = ["/a", "/b"].map(PathBuf::from).into();
        s.remap(&paths, &paths);
        changed(&s);
    }

    #[test]
    fn history_back_forward() {
        let a = Location::Folder("/a".into());
        let b = Location::Folder("/b".into());
        let c = Location::Folder("/c".into());
        let mut h = History::new(a.clone());
        assert!(h.navigate(b.clone()));
        assert!(!h.navigate(b.clone()));
        h.navigate(c.clone());
        assert!(h.back());
        assert_eq!(h.current(), &b);
        assert!(h.can_forward());
        h.navigate(Location::Recents);
        assert!(!h.can_forward());
        assert!(h.back() && h.back());
        assert_eq!(h.current(), &a);
        assert!(!h.back());
    }

    #[test]
    fn rebase_moves_paths_inside_the_folder() {
        let (from, to) = (Path::new("/a/old"), Path::new("/a/new"));
        assert_eq!(
            rebase(Path::new("/a/old"), from, to),
            Some(PathBuf::from("/a/new"))
        );
        assert_eq!(
            rebase(Path::new("/a/old/x/y"), from, to),
            Some(PathBuf::from("/a/new/x/y"))
        );
        assert_eq!(rebase(Path::new("/a/older"), from, to), None);
        assert_eq!(rebase(Path::new("/b"), from, to), None);
    }

    #[test]
    fn moves_follow_paths_inside_what_moved() {
        let pairs = [
            (PathBuf::from("/a/old"), PathBuf::from("/b/new")),
            (PathBuf::from("/c"), PathBuf::from("/d")),
        ];
        let moves = Moves::new(&pairs);
        assert_eq!(moves.follow(Path::new("/a/old")), Some("/b/new".into()));
        assert_eq!(moves.follow(Path::new("/a/old/x")), Some("/b/new/x".into()));
        assert_eq!(moves.follow(Path::new("/c/y/z")), Some("/d/y/z".into()));
        assert_eq!(moves.follow(Path::new("/a")), None);
        assert_eq!(moves.follow(Path::new("/a/older")), None);
    }

    #[test]
    fn history_follows_folders_that_moved() {
        let pairs = [(PathBuf::from("/p/old"), PathBuf::from("/p/new"))];
        let moves = Moves::new(&pairs);
        let mut h = History::new(Location::Folder("/p/old/a".into()));
        h.navigate(Location::Folder("/p".into()));
        h.navigate(Location::Folder("/p/old".into()));
        h.back();
        assert!(!h.follow(&moves), "the current folder stayed put");
        assert_eq!(h.current(), &Location::Folder("/p".into()));
        h.forward();
        assert_eq!(h.current(), &Location::Folder("/p/new".into()));
        h.back();
        h.back();
        assert_eq!(h.current(), &Location::Folder("/p/new/a".into()));

        let mut shown = History::new(Location::Folder("/p/old".into()));
        assert!(shown.follow(&moves));
        assert!(!shown.can_back(), "following isn't going somewhere");
        assert_eq!(shown.current(), &Location::Folder("/p/new".into()));
    }

    #[test]
    fn selection_ranges() {
        let mut s = Selection::default();
        s.select_only(2);
        s.extend_to(5);
        assert_eq!(s.indices().collect::<Vec<_>>(), [2, 3, 4, 5]);
        s.extend_to(0);
        assert_eq!(s.indices().collect::<Vec<_>>(), [0, 1, 2]);
        s.toggle(1);
        assert_eq!(s.indices().collect::<Vec<_>>(), [0, 2]);
        s.move_by(10, false, 4);
        assert_eq!(s.cursor(), Some(3));
        s.move_by(-1, true, 4);
        assert_eq!(s.indices().collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn cycle_cursor_wraps_within_the_selection() {
        let mut s = Selection::default();
        s.select_only(1);
        s.toggle(4);
        s.toggle(6);
        s.cycle_cursor(1);
        assert_eq!(s.cursor(), Some(1), "wraps from the last to the first");
        s.cycle_cursor(-1);
        assert_eq!(s.cursor(), Some(6), "and back");
        s.cycle_cursor(-1);
        assert_eq!(s.cursor(), Some(4));
        assert_eq!(s.indices().collect::<Vec<_>>(), [1, 4, 6]);
        s.toggle(4);
        s.cycle_cursor(1);
        assert_eq!(
            s.cursor(),
            Some(6),
            "a deselected cursor counts as the first"
        );
    }

    #[test]
    fn selection_survives_resort() {
        let old: Vec<PathBuf> = ["/a", "/b", "/c"].map(PathBuf::from).into();
        let new: Vec<PathBuf> = ["/c", "/b"].map(PathBuf::from).into();
        let mut s = Selection::default();
        s.select_only(0);
        s.toggle(2);
        s.remap(&old, &new);
        assert_eq!(s.indices().collect::<Vec<_>>(), [0]);
        assert_eq!(s.cursor(), Some(0));
    }
}
