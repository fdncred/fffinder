//! The sidebar's folder tree, for people used to Explorer's navigation pane.
//! With the sidebar in tree mode (View › Sidebar as Tree, or Settings ›
//! Sidebar), each place under Favorites and Locations opens in place to show
//! the folders inside it, and those open the same way. Clicking a folder shows it in the
//! window, and the tree opens down to whatever folder the window shows.
//! A folder is read only when it opens, and read again when it changes.

use super::Finder;
use super::sidebar::{FolderLine, Line};
use crate::actions::*;
use crate::disk_access::{self, Access};
use crate::model::{Location, Moves};
use fffinder_core::SidebarMode;
use fffinder_core::entry::display_name;
use fffinder_core::listing::natural_cmp;
use fffinder_core::subfolders::{self, Inside, Stamped, Subfolder};
use gpui_kit::component::input;
use gpui_kit::*;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Which list in the sidebar a tree hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Favorites,
    Locations,
}

/// A folder in the tree, under one sidebar place. The same folder can show
/// under more than one place, like the home folder under Favorites and under
/// the disk in Locations, and each opens and closes on its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeKey {
    /// The list the folder's place is in.
    pub section: Section,
    /// The sidebar place the folder is under, shared by every node there.
    pub root: Arc<Path>,
    /// The folder.
    pub path: PathBuf,
}

impl NodeKey {
    /// A sidebar place, the top of its own tree.
    ///
    /// # Arguments
    ///
    /// * `section` - Favorites or Locations
    /// * `path` - The place's folder
    ///
    /// # Returns
    ///
    /// The place's node
    pub fn place(section: Section, path: PathBuf) -> NodeKey {
        NodeKey {
            section,
            root: Arc::from(path.as_path()),
            path,
        }
    }

    /// A folder in the same tree as this node.
    ///
    /// # Arguments
    ///
    /// * `path` - The folder, inside this node's place
    ///
    /// # Returns
    ///
    /// The folder's node
    pub fn under(&self, path: PathBuf) -> NodeKey {
        NodeKey {
            section: self.section,
            root: self.root.clone(),
            path,
        }
    }

    /// Whether this is a sidebar place rather than a folder inside one.
    pub fn is_place(&self) -> bool {
        self.path == *self.root
    }

    /// The node holding this one.
    ///
    /// # Returns
    ///
    /// The parent folder's node, or `None` for a place
    pub fn parent(&self) -> Option<NodeKey> {
        if self.is_place() {
            return None;
        }
        self.path.parent().map(|dir| self.under(dir.to_path_buf()))
    }

    /// The id of the row of a folder inside a place. A place's own row
    /// keeps its sidebar id, `("fav", ix)` or `("loc", ix)`, which this
    /// isn't.
    pub fn row_id(&self) -> ElementId {
        ("tree", self.id_number()).into()
    }

    /// The id of the button that opens and closes the node.
    pub fn disclosure_id(&self) -> ElementId {
        ("disclose", self.id_number()).into()
    }

    /// The id of what holds the node's row and its menu.
    pub(super) fn menu_id(&self) -> ElementId {
        ("tree-menu", self.id_number()).into()
    }

    /// A number for the node's element ids, made from all of it.
    fn id_number(&self) -> usize {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish() as usize
    }
}

/// Whether a node can open, and whether it's open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disclosure {
    /// It holds no folders that show.
    Leaf,
    /// It holds folders, or hasn't been looked in yet, and is closed.
    Closed,
    /// It's open, showing its folders.
    Open,
    /// It can't be read, so what it holds is unknown.
    Unreadable,
}

/// A folder inside a folder that's open in the tree.
struct Child {
    path: PathBuf,
    name: SharedString,
    hidden: bool,
    /// What it holds, once looked at. Until then it shows as able to open.
    inside: Option<Inside>,
    /// When it last changed as of that look, to tell when to look again.
    stamp: Option<SystemTime>,
}

/// What's inside a folder that's open in the tree.
struct Contents {
    folders: Vec<Child>,
    /// When the folder last changed, to tell when to read it again.
    stamp: Option<SystemTime>,
    /// It couldn't be read, so it shows no folders.
    unreadable: bool,
}

/// The tree's state, per browser, kept as the window goes from folder to
/// folder.
#[derive(Default)]
pub(super) struct FolderTree {
    expanded: HashSet<NodeKey>,
    /// By folder, shared by every node showing it.
    contents: HashMap<PathBuf, Contents>,
    /// Folders being read now, each with whether a node opened during the
    /// read, which asks to look inside all of its folders again.
    reading: HashMap<PathBuf, bool>,
    /// The node of the folder the window shows.
    selected: Option<NodeKey>,
    /// A node to scroll into view once it shows.
    reveal: Option<NodeKey>,
    /// Counts the changes to what the tree shows.
    revision: u64,
    /// A look for changed folders is under way, and another is wanted after it.
    checking: bool,
    check_again: bool,
}

impl FolderTree {
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn selected(&self) -> Option<&NodeKey> {
        self.selected.as_ref()
    }

    fn changed(&mut self) {
        self.revision += 1;
    }

    /// Where the node waiting to be scrolled to is among `lines`, once it's
    /// there. It's waited for while folders are being read, since the one
    /// that holds it may be among them.
    ///
    /// # Arguments
    ///
    /// * `lines` - The sidebar's lines
    ///
    /// # Returns
    ///
    /// The node's line, the first time it shows
    pub(super) fn reveal_in(&mut self, lines: &[Line]) -> Option<usize> {
        let key = self.reveal.as_ref()?;
        let found = lines
            .iter()
            .position(|line| line.node().is_some_and(|(k, _)| k == key));
        if found.is_some() || self.reading.is_empty() {
            self.reveal = None;
        }
        found
    }

    /// Add a place's line, followed by its folders when it's open.
    ///
    /// # Arguments
    ///
    /// * `key` - The place's node
    /// * `ix` - Where the place is in its list
    /// * `show_hidden` - Whether hidden folders show
    /// * `current` - The folder the window shows
    /// * `out` - The lines
    pub(super) fn push_place(
        &self,
        key: NodeKey,
        ix: usize,
        show_hidden: bool,
        current: Option<&Path>,
        out: &mut Vec<Line>,
    ) {
        let open = self.expanded.contains(&key);
        let at = out.len();
        out.push(Line::Place {
            key: key.clone(),
            ix,
            disclosure: Some(if open {
                Disclosure::Open
            } else {
                Disclosure::Closed
            }),
        });
        if open
            && self.push_folders(&key, 0, show_hidden, current, out) == 0
            && let Some(empty) = self.empty_disclosure(&key.path)
            && let Line::Place { disclosure, .. } = &mut out[at]
        {
            *disclosure = Some(empty);
        }
    }

    /// How an open node that shows no folders looks once its folder is
    /// read: unable to open, or unreadable. Until then it stays open.
    fn empty_disclosure(&self, dir: &Path) -> Option<Disclosure> {
        self.contents
            .get(dir)
            .map(|contents| match contents.unreadable {
                true => Disclosure::Unreadable,
                false => Disclosure::Leaf,
            })
    }

    /// Add a line for each folder under `key` that shows, each followed by
    /// its own when it's open.
    ///
    /// # Arguments
    ///
    /// * `key` - The open node
    /// * `depth` - How deep its folders are, 0 right under a place
    /// * `show_hidden` - Whether hidden folders show
    /// * `current` - The folder the window shows, whose hidden folders on
    ///   the way down show anyway
    /// * `out` - The lines
    ///
    /// # Returns
    ///
    /// How many folders went in at this level
    pub(super) fn push_folders(
        &self,
        key: &NodeKey,
        depth: usize,
        show_hidden: bool,
        current: Option<&Path>,
        out: &mut Vec<Line>,
    ) -> usize {
        let Some(contents) = self.contents.get(&key.path) else {
            return 0;
        };
        let mut shown = 0;
        for child in &contents.folders {
            if child.hidden && !show_hidden && !current.is_some_and(|c| c.starts_with(&child.path))
            {
                continue;
            }
            shown += 1;
            let child_key = key.under(child.path.clone());
            let open = self.expanded.contains(&child_key);
            let disclosure = match (open, child.inside) {
                (true, _) => Disclosure::Open,
                (false, Some(inside)) if inside.unreadable => Disclosure::Unreadable,
                (false, Some(inside)) if !inside.any(show_hidden) => Disclosure::Leaf,
                (false, _) => Disclosure::Closed,
            };
            let at = out.len();
            out.push(Line::Folder(FolderLine {
                key: child_key.clone(),
                name: child.name.clone(),
                depth,
                disclosure,
            }));
            if open
                && self.push_folders(&child_key, depth + 1, show_hidden, current, out) == 0
                && let Some(empty) = self.empty_disclosure(&child.path)
                && let Line::Folder(line) = &mut out[at]
            {
                line.disclosure = empty;
            }
        }
        shown
    }

    /// Keep what was read of a folder, and what's known about its folders.
    /// Returns the folders to look inside: those not looked in before, or
    /// all of them with `look_again`.
    fn store(&mut self, read: Stamped<Option<Vec<Subfolder>>>, look_again: bool) -> Vec<PathBuf> {
        let Stamped { dir, found, stamp } = read;
        let known: HashMap<PathBuf, (Inside, Option<SystemTime>)> = self
            .contents
            .remove(&dir)
            .map(|old| {
                old.folders
                    .into_iter()
                    .filter_map(|f| Some((f.path, (f.inside?, f.stamp))))
                    .collect()
            })
            .unwrap_or_default();
        let unreadable = found.is_none();
        let folders: Vec<Child> = found
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                let (inside, stamp) = known.get(&f.path).copied().unzip();
                Child {
                    inside,
                    stamp: stamp.flatten(),
                    name: f.name.into(),
                    hidden: f.hidden,
                    path: f.path,
                }
            })
            .collect();
        let unknown = folders
            .iter()
            .filter(|f| look_again || f.inside.is_none())
            .map(|f| f.path.clone())
            .collect();
        let contents = Contents {
            folders,
            stamp,
            unreadable,
        };
        self.contents.insert(dir, contents);
        unknown
    }

    /// The folder at `path`, among what's known to be in its parent.
    fn child_mut(&mut self, path: &Path) -> Option<&mut Child> {
        let contents = self.contents.get_mut(path.parent()?)?;
        let name = display_name(path);
        let ix = contents
            .folders
            .binary_search_by(|f| natural_cmp(&f.name, &name))
            .ok()?;
        contents.folders.get_mut(ix)
    }

    /// Note what the folder at `path` holds, and when it last changed as
    /// of the look. Returns whether what it holds is news.
    fn set_inside(&mut self, path: &Path, inside: Inside, stamp: Option<SystemTime>) -> bool {
        let Some(child) = self.child_mut(path) else {
            return false;
        };
        child.stamp = stamp;
        child.inside.replace(inside) != Some(inside)
    }

    /// Show a folder that was just made in `dir` before `dir` is read again.
    fn add_child(&mut self, dir: &Path, path: PathBuf) {
        let contents = self
            .contents
            .entry(dir.to_path_buf())
            .or_insert_with(|| Contents {
                folders: Vec::new(),
                stamp: None,
                unreadable: false,
            });
        let name: SharedString = display_name(&path).into();
        if let Err(ix) = contents
            .folders
            .binary_search_by(|f| natural_cmp(&f.name, &name))
        {
            contents.folders.insert(
                ix,
                Child {
                    hidden: name.starts_with('.'),
                    inside: Some(Inside::default()),
                    stamp: None,
                    name,
                    path,
                },
            );
        }
        self.changed();
    }

    /// Folders moved: what was open or selected inside them is open or
    /// selected where they went, and what was read inside them is kept.
    fn moved(&mut self, moves: &Moves) {
        // A place stays where the sidebar says it is.
        let follow = |key: &NodeKey| moves.follow(&key.path).filter(|_| !key.is_place());
        self.expanded = self
            .expanded
            .drain()
            .map(|key| match follow(&key) {
                Some(path) => key.under(path),
                None => key,
            })
            .collect();
        if let Some(key) = self.selected.as_mut()
            && let Some(path) = follow(key)
        {
            key.path = path;
        }
        let inside: Vec<(PathBuf, PathBuf)> = self
            .contents
            .keys()
            .filter_map(|dir| Some((dir.clone(), moves.follow(dir)?)))
            .collect();
        for (from, to) in inside {
            if let Some(mut contents) = self.contents.remove(&from) {
                for child in &mut contents.folders {
                    if let Some(path) = moves.follow(&child.path) {
                        child.path = path;
                    }
                }
                self.contents.insert(to, contents);
            }
        }
        self.changed();
    }

    /// Close a node and every node open inside it, so what was read below
    /// it is let go rather than kept up to date unseen.
    ///
    /// # Returns
    ///
    /// Whether the node was open
    fn collapse(&mut self, key: &NodeKey) -> bool {
        if !self.expanded.contains(key) {
            return false;
        }
        self.expanded.retain(|open| {
            open.section != key.section
                || open.root != key.root
                || !open.path.starts_with(&key.path)
        });
        self.changed();
        true
    }

    /// The folders shown without a triangle because no folders were found
    /// in them, each with when it last changed as of that look, to look in
    /// again once it changes.
    fn leaves(&self, show_hidden: bool) -> Vec<(PathBuf, Option<SystemTime>)> {
        self.contents
            .values()
            .flat_map(|contents| &contents.folders)
            .filter(|f| {
                (show_hidden || !f.hidden)
                    && f.inside
                        .is_some_and(|inside| !inside.unreadable && !inside.any(show_hidden))
                    // An open folder is read again when it changes anyway.
                    && !self.contents.contains_key(&f.path)
            })
            .map(|f| (f.path.clone(), f.stamp))
            .collect()
    }

    /// Forget what's in folders no longer open anywhere.
    fn prune(&mut self) {
        let open: HashSet<&Path> = self.expanded.iter().map(|k| k.path.as_path()).collect();
        self.contents.retain(|dir, _| open.contains(dir.as_path()));
    }

    /// Whether `key` shows: every node above it is open, and its folder is
    /// among what's been read of its parent.
    fn shows(&self, key: &NodeKey) -> bool {
        let mut node = key.clone();
        while let Some(parent) = node.parent() {
            let listed = self
                .contents
                .get(&parent.path)
                .is_some_and(|c| c.folders.iter().any(|f| f.path == node.path));
            if !listed || !self.expanded.contains(&parent) {
                return false;
            }
            node = parent;
        }
        true
    }
}

impl Finder {
    /// Whether the sidebar shows the folder tree.
    pub(super) fn tree_on(&self) -> bool {
        self.config.sidebar_mode == SidebarMode::Tree
    }

    /// Show the sidebar as a list of places, as Finder does, or as a tree
    /// of folders, as Explorer does.
    ///
    /// # Arguments
    ///
    /// * `mode` - The sidebar's mode
    /// * `cx` - The browser's context
    pub fn set_sidebar_mode(&mut self, mode: SidebarMode, cx: &mut Context<Self>) {
        if self.config.sidebar_mode == mode {
            return;
        }
        self.config.sidebar_mode = mode;
        self.settings_changed(cx);
        self.sidebar_mode_changed(cx);
    }

    /// Choose whether the tree opens down to the folder the window shows.
    ///
    /// # Arguments
    ///
    /// * `on` - Whether it does
    /// * `cx` - The browser's context
    pub fn set_expand_to_open_folder(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.config.expand_to_open_folder == on {
            return;
        }
        self.config.expand_to_open_folder = on;
        self.settings_changed(cx);
        self.sidebar_mode_changed(cx);
    }

    /// The sidebar's mode changed, or whether the tree follows the window,
    /// here or in another browser.
    pub(super) fn sidebar_mode_changed(&mut self, cx: &mut Context<Self>) {
        if self.tree_on() {
            self.tree.selected = None;
            self.tree_follow_location(cx);
        } else {
            // Nothing of it shows, so nothing of it is kept.
            self.tree = FolderTree::default();
        }
        cx.notify();
    }

    /// The node of the folder the tree's keys act on, as long as it's a
    /// folder inside a place. A place is a favorite or a disk, and the keys
    /// that rename, cut, copy, or trash a folder leave those alone.
    fn tree_folder_node(&self) -> Option<NodeKey> {
        self.tree.selected.clone().filter(|key| !key.is_place())
    }

    /// The folder the tree's other keys act on: the selected node's.
    fn tree_folder(&self) -> Option<PathBuf> {
        self.tree.selected.as_ref().map(|key| key.path.clone())
    }

    /// Open a node that's closed, or close one that's open.
    pub(super) fn toggle_node(&mut self, key: NodeKey, cx: &mut Context<Self>) {
        if self.tree.collapse(&key) {
            cx.notify();
        } else {
            self.expand_node(key, cx);
        }
    }

    /// Open a node, showing what was read of it before while it's read
    /// again.
    pub(super) fn expand_node(&mut self, key: NodeKey, cx: &mut Context<Self>) {
        let dir = key.path.clone();
        if self.tree.expanded.insert(key) {
            self.tree.changed();
        }
        self.read_tree_folders(vec![dir], true, cx);
        cx.notify();
    }

    /// Whether the node is open.
    pub(super) fn node_is_open(&self, key: &NodeKey) -> bool {
        self.tree.expanded.contains(key)
    }

    /// Show the node's folder in the window, and select the node.
    pub(super) fn open_node(&mut self, key: NodeKey, window: &mut Window, cx: &mut Context<Self>) {
        let location = Location::Folder(key.path.clone());
        self.tree.selected = Some(key);
        self.navigate(location, window, cx);
        cx.notify();
    }

    /// Open the tree down to the folder the window shows and select it there,
    /// staying under the place the last folder was under when it can.
    pub(super) fn tree_follow_location(&mut self, cx: &mut Context<Self>) {
        if !self.tree_on() {
            return;
        }
        let Some(current) = self.current_folder().map(Path::to_path_buf) else {
            self.tree.selected = None;
            return;
        };
        // A folder that moved is selected where it went before the tree is
        // read there, so it may not show yet.
        let expand = self.config.expand_to_open_folder;
        if self
            .tree
            .selected
            .as_ref()
            .is_some_and(|k| k.path == current && (!expand || self.tree.shows(k)))
        {
            return;
        }
        let place = self
            .tree
            .selected
            .take()
            .filter(|key| current.starts_with(&key.root))
            .map(|key| key.under(key.root.to_path_buf()))
            .or_else(|| self.place_containing(&current));
        let Some(place) = place else {
            return;
        };
        let key = place.under(current.clone());
        if !expand {
            self.tree.selected = self.tree.shows(&key).then_some(key);
            return;
        }
        let mut unread = Vec::new();
        for dir in current
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&place.root))
        {
            self.tree.expanded.insert(place.under(dir.to_path_buf()));
            if !self.tree.contents.contains_key(dir) {
                unread.push(dir.to_path_buf());
            }
        }
        self.tree.selected = Some(key.clone());
        self.tree.reveal = Some(key);
        self.tree.changed();
        self.read_tree_folders(unread, false, cx);
    }

    /// The sidebar place closest around `folder`: the one whose folder holds
    /// it most deeply, Favorites first when two are as deep.
    fn place_containing(&self, folder: &Path) -> Option<NodeKey> {
        let favorites = self.favorites.iter().map(|p| (Section::Favorites, &p.path));
        let locations = self.locations.iter().map(|p| (Section::Locations, &p.path));
        favorites
            .chain(locations)
            .filter(|(_, path)| folder.starts_with(path))
            // The last of equals wins, so go backward for the first.
            .rev()
            .max_by_key(|(_, path)| path.components().count())
            .map(|(section, path)| NodeKey::place(section, path.clone()))
    }

    /// Read the folders inside each of `dirs` off the UI thread, then look
    /// inside those not looked in before to tell which can open, or inside
    /// all of them with `look_again`.
    pub(super) fn read_tree_folders(
        &mut self,
        dirs: Vec<PathBuf>,
        look_again: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.tree_on() {
            return;
        }
        // A folder being read already is read once; a node opened meanwhile
        // has all its folders looked in once that read is in.
        let dirs: Vec<PathBuf> = dirs
            .into_iter()
            .filter(|dir| match self.tree.reading.entry(dir.clone()) {
                Entry::Occupied(mut reading) => {
                    *reading.get_mut() |= look_again;
                    false
                }
                Entry::Vacant(reading) => {
                    reading.insert(false);
                    true
                }
            })
            .collect();
        if dirs.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_spawn(async move { subfolders::list_each(dirs) })
                .await;
            this.update(cx, |this, cx| {
                // Switched to the list meanwhile: nothing of the tree is kept.
                if !this.tree_on() {
                    return;
                }
                let mut found = Vec::new();
                for read in read {
                    let opened = this.tree.reading.remove(&read.dir).unwrap_or_default();
                    found.extend(this.tree.store(read, look_again || opened));
                }
                this.tree.changed();
                cx.notify();
                this.look_inside_tree_folders(found, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Look inside each of `dirs` off the UI thread, to tell which can open.
    fn look_inside_tree_folders(&mut self, dirs: Vec<PathBuf>, cx: &mut Context<Self>) {
        if dirs.is_empty() {
            return;
        }
        // Checked once at startup; the picker and --term don't check then.
        let access = cx
            .try_global::<disk_access::Checked>()
            .map(|checked| checked.0);
        cx.spawn(async move |this, cx| {
            // Without Full Disk Access, looking inside Desktop, Documents, or
            // Downloads makes macOS ask for each, so every folder shows as
            // able to open until it's opened.
            let insides = cx
                .background_spawn(async move {
                    match access.unwrap_or_else(disk_access::check) {
                        Access::Denied => Vec::new(),
                        Access::Granted | Access::Unknown => subfolders::inside_each(dirs),
                    }
                })
                .await;
            if insides.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                let mut news = false;
                for looked in insides {
                    news |= this
                        .tree
                        .set_inside(&looked.dir, looked.found, looked.stamp);
                }
                if news {
                    this.tree.changed();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Off the UI thread, read again every open folder that changed since
    /// it was read, and look again in every folder shown as holding none
    /// that changed since it was looked in.
    pub(super) fn check_tree_for_changes(&mut self, cx: &mut Context<Self>) {
        if !self.tree_on() || !self.config.show_sidebar {
            return;
        }
        if self.tree.checking {
            self.tree.check_again = true;
            return;
        }
        self.tree.prune();
        if self.tree.contents.is_empty() {
            return;
        }
        let dirs: Vec<(PathBuf, Option<SystemTime>)> = self
            .tree
            .contents
            .iter()
            .map(|(dir, contents)| (dir.clone(), contents.stamp))
            .collect();
        let leaves = self.tree.leaves(self.config.show_hidden);
        self.tree.checking = true;
        cx.spawn(async move |this, cx| {
            let (dirs, leaves) = cx
                .background_spawn(async move {
                    (subfolders::changed(dirs), subfolders::changed(leaves))
                })
                .await;
            this.update(cx, |this, cx| {
                this.tree.checking = false;
                this.read_tree_folders(dirs, false, cx);
                this.look_inside_tree_folders(leaves, cx);
                if std::mem::take(&mut this.tree.check_again) {
                    this.check_tree_for_changes(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The window listed the folder it shows: whether its node can open
    /// follows what's in it, and an open node that changed is read again.
    pub(super) fn tree_saw_listing(&mut self, cx: &mut Context<Self>) {
        if !self.tree_on() {
            return;
        }
        let Some(dir) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let folders = || self.listing.iter().filter(|e| e.is_navigable());
        let shown = folders().any(|e| !e.hidden);
        // Hidden items are listed only while they show.
        let hidden = match self.config.show_hidden {
            true => folders().any(|e| e.hidden),
            false => self
                .tree
                .child_mut(&dir)
                .and_then(|c| c.inside)
                .is_some_and(|i| i.hidden),
        };
        let inside = Inside {
            shown,
            hidden,
            unreadable: false,
        };
        if self.tree.set_inside(&dir, inside, self.listing_stamp) {
            self.tree.changed();
        }
        let stale = self
            .tree
            .contents
            .get(&dir)
            .is_some_and(|c| c.stamp != self.listing_stamp);
        if stale {
            self.read_tree_folders(vec![dir], false, cx);
        }
    }

    /// Items were renamed or moved: what was open or selected in the tree
    /// inside them stays so where they went, and the folders they left and
    /// went into are read again.
    ///
    /// # Arguments
    ///
    /// * `pairs` - Where each item was, and where it is now
    /// * `moves` - The same pairs, to look up
    /// * `cx` - The browser's context
    pub(super) fn tree_follow_moves(
        &mut self,
        pairs: &[(PathBuf, PathBuf)],
        moves: &Moves,
        cx: &mut Context<Self>,
    ) {
        if !self.tree_on() {
            return;
        }
        self.tree.moved(moves);
        // Only folders the tree has read are worth reading again.
        let parents: HashSet<&Path> = pairs
            .iter()
            .flat_map(|(from, to)| [from.parent(), to.parent()])
            .flatten()
            .filter(|dir| self.tree.contents.contains_key(*dir))
            .collect();
        let parents = parents.into_iter().map(Path::to_path_buf).collect();
        self.read_tree_folders(parents, false, cx);
    }

    /// Make a folder inside the node's folder, open the node, and name the
    /// new folder there.
    pub(super) fn new_folder_in_tree(
        &mut self,
        key: NodeKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.make_untitled_folder(&key.path, window, cx) else {
            return;
        };
        self.tree.add_child(&key.path, path.clone());
        self.tree.reveal = Some(key.under(path.clone()));
        self.expand_node(key, cx);
        self.begin_sidebar_rename(path, window, cx);
        self.files_changed(Vec::new(), cx);
    }

    /// Start renaming the node's folder in its row.
    pub(super) fn rename_in_tree(
        &mut self,
        key: NodeKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = key.path.clone();
        self.tree.reveal = Some(key);
        self.begin_sidebar_rename(path, window, cx);
    }

    // ----- keyboard -----------------------------------------------------------

    /// The sidebar's lines, and which is the one showing in the window.
    fn tree_cursor(&mut self) -> (std::rc::Rc<Vec<Line>>, Option<usize>) {
        let lines = self.sidebar_lines();
        let at = lines.iter().position(|line| self.line_is_active(line));
        (lines, at)
    }

    /// Show the line's place in the window, as clicking it would, and scroll
    /// the sidebar to it.
    fn activate_line(
        &mut self,
        lines: &[Line],
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(line) = lines.get(ix) else {
            return;
        };
        match line {
            Line::Heading(_) => return,
            Line::Recents => self.navigate(Location::Recents, window, cx),
            Line::Trash => self.navigate(Location::Trash, window, cx),
            Line::Smart(smart) => self.open_smart_folder(*smart, window, cx),
            Line::Place { key, .. } | Line::Folder(FolderLine { key, .. }) => {
                self.open_node(key.clone(), window, cx)
            }
        }
        self.sidebar_scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Up and Down, and Page Up and Page Down by `step` lines: go to the
    /// line `step` away, past headings. With no line active, Down starts
    /// at the first line and Up at the last.
    fn move_in_tree(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let (lines, at) = self.tree_cursor();
        let last = lines.len().saturating_sub(1) as isize;
        let start = match at {
            Some(at) => (at as isize + step).clamp(0, last),
            None if step > 0 => 0,
            None => last,
        };
        self.activate_near(&lines, start, step >= 0, window, cx);
    }

    /// Home and End: go to the first line, or the last, past headings.
    fn tree_to_end(&mut self, last: bool, window: &mut Window, cx: &mut Context<Self>) {
        let lines = self.sidebar_lines();
        let start = match last {
            true => lines.len().saturating_sub(1) as isize,
            false => 0,
        };
        self.activate_near(&lines, start, !last, window, cx);
    }

    /// Go to line `start`, or past headings from it, onward or back, and
    /// the other way at an end.
    fn activate_near(
        &mut self,
        lines: &[Line],
        start: isize,
        onward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = lines.len().saturating_sub(1) as isize;
        let selectable = |ix: &isize| !matches!(lines.get(*ix as usize), Some(Line::Heading(_)));
        let way = if onward { 1 } else { -1 };
        let found = std::iter::successors(Some(start), |ix| Some(ix + way))
            .take_while(|ix| (0..=last).contains(ix))
            .find(selectable)
            .or_else(|| {
                std::iter::successors(Some(start), |ix| Some(ix - way))
                    .take_while(|ix| (0..=last).contains(ix))
                    .find(selectable)
            });
        if let Some(ix) = found {
            self.activate_line(lines, ix as usize, window, cx);
        }
    }

    /// How many lines a page of the sidebar holds.
    fn tree_page(&self) -> isize {
        let height = f32::from(self.sidebar_scroll.bounds().size.height);
        let row = f32::from(crate::theme::ui(super::sidebar::ROW_HEIGHT));
        ((height / row).floor() as isize - 1).max(1)
    }

    /// Right: open the node, or go into its first folder when it's open.
    fn tree_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (lines, Some(at)) = self.tree_cursor() else {
            return;
        };
        let Some((key, disclosure)) = lines[at].node() else {
            return;
        };
        match disclosure {
            Disclosure::Closed => self.expand_node(key.clone(), cx),
            Disclosure::Open => {
                let first_child = lines
                    .get(at + 1)
                    .and_then(Line::node)
                    .is_some_and(|(child, _)| child.parent().as_ref() == Some(key));
                if first_child {
                    self.activate_line(&lines, at + 1, window, cx);
                }
            }
            Disclosure::Leaf | Disclosure::Unreadable => {}
        }
    }

    /// Left: close the node, or go to the folder holding it when it's closed.
    fn tree_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (lines, Some(at)) = self.tree_cursor() else {
            return;
        };
        let Some((key, disclosure)) = lines[at].node() else {
            return;
        };
        if disclosure == Disclosure::Open {
            self.toggle_node(key.clone(), cx);
            return;
        }
        let parent = key.parent();
        if let Some(ix) = lines[..at]
            .iter()
            .rposition(|line| line.node().map(|(k, _)| Some(k)) == Some(parent.as_ref()))
        {
            self.activate_line(&lines, ix, window, cx);
        }
    }

    /// Letters typed in the sidebar go to the next line whose name starts
    /// with them, as they do in Explorer's.
    fn tree_type_to_select(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let prefix = self.typed_prefix(text, cx);
        let (lines, at) = self.tree_cursor();
        // A first letter looks past the line already there; more letters
        // go on narrowing from it.
        let from = match (at, prefix.chars().count()) {
            (Some(at), 1) => at + 1,
            (Some(at), _) => at,
            (None, _) => 0,
        };
        let count = lines.len();
        let found = (0..count).map(|n| (from + n) % count).find(|&ix| {
            self.line_name(&lines[ix])
                .is_some_and(|name| name.to_lowercase().starts_with(&prefix))
        });
        if let Some(ix) = found {
            self.activate_line(&lines, ix, window, cx);
        }
    }

    /// The tree's own handling of keys and Edit menu commands while it has
    /// the keyboard. Each acts on the selected folder.
    pub(super) fn tree_keys(&self, el: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        el.on_action(cx.listener(|this, _: &MoveUp, window, cx| this.move_in_tree(-1, window, cx)))
            .on_action(
                cx.listener(|this, _: &MoveDown, window, cx| this.move_in_tree(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &PageUp, window, cx| {
                let page = this.tree_page();
                this.move_in_tree(-page, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, window, cx| {
                let page = this.tree_page();
                this.move_in_tree(page, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MoveHome, window, cx| this.tree_to_end(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveEnd, window, cx| this.tree_to_end(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveRight, window, cx| this.tree_right(window, cx)))
            .on_action(cx.listener(|this, _: &MoveLeft, window, cx| this.tree_left(window, cx)))
            // Return takes the keyboard to the folder's items.
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.focus_files(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Rename, window, cx| {
                if let Some(key) = this.tree_folder_node() {
                    this.rename_in_tree(key, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &MoveToTrash, window, cx| {
                if let Some(key) = this.tree_folder_node() {
                    this.trash_now(vec![key.path], window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &input::Cut, window, cx| {
                let folder = this.tree_folder_node().map(|key| key.path);
                this.cut_paths(folder.into_iter().collect(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &input::Copy, window, cx| {
                let folder = this.tree_folder_node().map(|key| key.path);
                this.copy_paths(folder.into_iter().collect(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &input::Paste, window, cx| {
                let folder = this.tree_folder();
                this.paste_into(folder, Self::paste_mode(cx), window, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveItemsHere, window, cx| {
                let folder = this.tree_folder();
                this.paste_into(folder, fffinder_core::ops::Mode::Move, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyPath, window, cx| {
                let folder: Vec<PathBuf> = this.tree_folder().into_iter().collect();
                this.copy_path_text(&folder, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GetInfo, window, cx| {
                let folder = this.tree_folder();
                this.show_info(folder.into_iter().collect(), window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keys = &event.keystroke;
                let m = keys.modifiers;
                if m.control || m.platform || m.alt || m.function || this.renaming.is_some() {
                    return;
                }
                if let Some(text) = keys.key_char.as_deref()
                    && !text.is_empty()
                    && !text.chars().any(char::is_control)
                    && text != " "
                {
                    this.tree_type_to_select(text, window, cx);
                    cx.stop_propagation();
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{Disclosure, FolderTree, NodeKey, Section};
    use crate::finder::sidebar::{FolderLine, Line};
    use crate::model::Moves;
    use fffinder_core::subfolders::{Inside, Stamped, Subfolder};
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    fn folder(path: &str) -> Subfolder {
        Subfolder {
            path: PathBuf::from(path),
            name: fffinder_core::entry::display_name(Path::new(path)),
            hidden: false,
        }
    }

    /// What reading `dir` found: `folders`, or `None` when it couldn't be read.
    fn read(dir: &str, folders: Option<&[&str]>) -> Stamped<Option<Vec<Subfolder>>> {
        Stamped {
            dir: PathBuf::from(dir),
            found: folders.map(|paths| paths.iter().map(|p| folder(p)).collect()),
            stamp: None,
        }
    }

    /// The folders' lines under `place`, by path, with how each opens.
    fn folder_lines(tree: &FolderTree, place: &NodeKey) -> Vec<(PathBuf, Disclosure)> {
        let mut lines = Vec::new();
        tree.push_place(place.clone(), 0, false, None, &mut lines);
        lines
            .iter()
            .filter_map(|line| match line {
                Line::Folder(FolderLine {
                    key, disclosure, ..
                }) => Some((key.path.clone(), *disclosure)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_moved_folder_keeps_what_was_open_and_read_inside_it() {
        let mut tree = FolderTree::default();
        let place = NodeKey::place(Section::Favorites, PathBuf::from("/p"));
        tree.expanded.insert(place.clone());
        tree.expanded.insert(place.under("/p/old".into()));
        tree.store(read("/p", Some(&["/p/old"])), false);
        tree.store(read("/p/old", Some(&["/p/old/a"])), false);
        tree.selected = Some(place.under("/p/old/a".into()));

        let pairs = [(PathBuf::from("/p/old"), PathBuf::from("/p/new"))];
        tree.moved(&Moves::new(&pairs));
        assert!(tree.expanded.contains(&place.under("/p/new".into())));
        assert!(tree.expanded.contains(&place), "a place stays put");
        assert_eq!(tree.selected, Some(place.under("/p/new/a".into())));
        // The parent is read again by the browser; here it's read by hand.
        tree.store(read("/p", Some(&["/p/new"])), false);
        let paths: Vec<PathBuf> = folder_lines(&tree, &place)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(paths, [PathBuf::from("/p/new"), PathBuf::from("/p/new/a")]);
    }

    #[test]
    fn only_folders_not_looked_in_are_looked_in_again() {
        let mut tree = FolderTree::default();
        let first = tree.store(read("/p", Some(&["/p/a"])), false);
        assert_eq!(first, [PathBuf::from("/p/a")]);
        let stamp = Some(SystemTime::UNIX_EPOCH);
        assert!(tree.set_inside(Path::new("/p/a"), Inside::default(), stamp));
        assert!(
            !tree.set_inside(Path::new("/p/a"), Inside::default(), stamp),
            "the same again is no news"
        );
        let again = tree.store(read("/p", Some(&["/p/a", "/p/b"])), false);
        assert_eq!(again, [PathBuf::from("/p/b")], "a is known already");
        assert_eq!(
            tree.leaves(false),
            [(PathBuf::from("/p/a"), stamp)],
            "what's known of a, and when, is kept"
        );
        let all = tree.store(read("/p", Some(&["/p/a", "/p/b"])), true);
        assert_eq!(all.len(), 2, "opening a node looks in everything again");
    }

    #[test]
    fn closing_a_node_closes_what_was_open_inside_it() {
        let mut tree = FolderTree::default();
        let place = NodeKey::place(Section::Favorites, PathBuf::from("/p"));
        let other = NodeKey::place(Section::Locations, PathBuf::from("/"));
        let (a, deep) = (place.under("/p/a".into()), place.under("/p/a/b".into()));
        let same_folder = other.under("/p/a".into());
        for key in [&place, &a, &deep, &other, &same_folder] {
            tree.expanded.insert(key.clone());
        }
        tree.store(read("/p/a/b", Some(&[])), false);

        assert!(tree.collapse(&a));
        assert!(!tree.collapse(&a), "it's closed already");
        assert!(!tree.expanded.contains(&deep));
        assert!(tree.expanded.contains(&place));
        assert!(
            tree.expanded.contains(&same_folder),
            "the same folder under another place is on its own"
        );
        tree.prune();
        assert!(
            tree.contents.is_empty(),
            "what was read inside is let go, not kept up to date"
        );
    }

    #[test]
    fn a_folder_that_cant_be_read_shows_so() {
        let mut tree = FolderTree::default();
        let place = NodeKey::place(Section::Favorites, PathBuf::from("/p"));
        let (locked, empty) = (
            place.under("/p/locked".into()),
            place.under("/p/empty".into()),
        );
        tree.expanded.insert(place.clone());
        tree.expanded.insert(locked.clone());
        tree.expanded.insert(empty.clone());
        tree.store(
            read("/p", Some(&["/p/empty", "/p/locked", "/p/shut"])),
            false,
        );
        tree.store(read("/p/locked", None), false);
        tree.store(read("/p/empty", Some(&[])), false);
        tree.set_inside(Path::new("/p/empty"), Inside::default(), None);
        let unreadable = Inside {
            unreadable: true,
            ..Inside::default()
        };
        tree.set_inside(Path::new("/p/shut"), unreadable, None);
        assert_eq!(
            folder_lines(&tree, &place),
            [
                (PathBuf::from("/p/empty"), Disclosure::Leaf),
                (PathBuf::from("/p/locked"), Disclosure::Unreadable),
                (PathBuf::from("/p/shut"), Disclosure::Unreadable),
            ]
        );
        assert!(
            tree.leaves(false).is_empty(),
            "neither an open folder nor an unreadable one is looked in again"
        );
    }

    #[test]
    fn a_node_goes_up_to_its_place_and_no_further() {
        let place = NodeKey::place(Section::Favorites, PathBuf::from("/home/me"));
        let deep = place.under(PathBuf::from("/home/me/src/app"));
        let up = deep.parent().expect("a folder inside a place has a parent");
        assert_eq!(up.path, PathBuf::from("/home/me/src"));
        assert_eq!(up.parent(), Some(place.clone()));
        assert!(place.is_place());
        assert_eq!(place.parent(), None);
    }

    #[test]
    fn the_same_folder_under_two_places_has_two_rows() {
        let home = NodeKey::place(Section::Favorites, PathBuf::from("/home/me"));
        let disk = NodeKey::place(Section::Locations, PathBuf::from("/"));
        let src = PathBuf::from("/home/me/src");
        assert_ne!(home.under(src.clone()).row_id(), disk.under(src).row_id());
    }
}
