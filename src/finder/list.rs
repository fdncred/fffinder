//! The list view's extras: folders that open in place (disclosure
//! triangles), the column chooser, resizing and reordering columns, Keep
//! Folders on Top, and folder sizes and git statuses from the index.

use super::{Finder, Row};
use crate::actions::ToggleFoldersFirst;
use crate::model::Location;
use crate::theme::ui;
use fffinder_core::config::{MIN_COLUMN_WIDTH, default_list_columns};
use fffinder_core::{ColumnSetting, FileEntry, ListColumn, SortKey, ViewMode, listing};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// Dragged to resize list column `.0`.
#[derive(Clone)]
pub(super) struct ColumnResize(pub usize);

/// Dragged to move list column `.0` to where it's dropped.
#[derive(Clone)]
pub(super) struct ColumnMove(pub usize);

/// What a dragged column header looks like.
pub(super) struct ColumnPreview(pub SharedString);

impl Render for ColumnPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        let theme = cx.theme();
        div()
            .px(ui(8.))
            .py(ui(3.))
            .rounded(ui(5.))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .text_size(ui(12.))
            .child(self.0.clone())
    }
}

impl Finder {
    /// Whether the list shows folders as a tree that opens in place: in a
    /// folder, not in search results or other lists.
    pub(super) fn tree_mode(&self) -> bool {
        self.effective_view() == ViewMode::List
            && !self.is_searching()
            && matches!(self.history.current(), Location::Folder(_))
    }

    /// Whether the list needs each item's last-opened date.
    pub(super) fn wants_last_opened(&self) -> bool {
        self.config.sort.key == SortKey::LastOpened
            || self.shows_column(ListColumn::LastOpened)
            || (self.config.use_groups
                && self.config.group_by == fffinder_core::GroupKey::LastOpened)
    }

    pub(super) fn shows_column(&self, column: ListColumn) -> bool {
        self.config.list_columns.iter().any(|c| c.column == column)
    }

    /// Whether folder sizes or git statuses are shown, which come from the index.
    fn wants_folder_stats(&self) -> bool {
        self.config.folder_sizes || self.shows_column(ListColumn::Git)
    }

    /// Read `dir`'s folder sizes and git statuses from the index, then redraw.
    /// Outside the indexed folders, app sizes are added up from the disk.
    pub(super) fn load_folder_stats(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.wants_folder_stats() {
            return;
        }
        let engine = self.engine.clone();
        let sizes = self.config.folder_sizes;
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_spawn({
                    let dir = dir.clone();
                    async move {
                        let mut stats = engine.folder_stats(&dir);
                        if sizes {
                            stats.sizes.extend(engine.bundle_sizes(&dir));
                        }
                        stats
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.folder_stats.insert(dir, stats);
                this.rebuild_rows(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reread folder stats for the folder shown and every open subfolder.
    pub(super) fn reload_folder_stats(&mut self, cx: &mut Context<Self>) {
        self.folder_stats.clear();
        let dirs: Vec<PathBuf> = self
            .current_folder()
            .map(Path::to_path_buf)
            .into_iter()
            .chain(self.expanded.iter().cloned())
            .collect();
        for dir in dirs {
            self.load_folder_stats(dir, cx);
        }
    }

    /// A folder's size from the index, and its or a file's git status.
    pub(super) fn decorate(&self, entry: &mut FileEntry) -> Option<String> {
        let stats = entry.parent().and_then(|dir| self.folder_stats.get(dir))?;
        if entry.is_dir && self.config.folder_sizes {
            entry.size = stats.sizes.get(&entry.path).copied();
        }
        stats.git.get(&entry.path).cloned()
    }

    /// Rows for `entries` and, under each open folder, its own items.
    pub(super) fn tree_rows(&self, mut entries: Vec<FileEntry>, depth: usize, out: &mut Vec<Row>) {
        let gits: Vec<Option<String>> = entries.iter_mut().map(|e| self.decorate(e)).collect();
        let mut decorated: Vec<(FileEntry, Option<String>)> =
            entries.into_iter().zip(gits).collect();
        decorated.sort_by(|(a, _), (b, _)| {
            listing::folders_on_top(a, b, self.config.folders_first)
                .then_with(|| listing::compare(a, b, self.config.sort))
        });
        for (entry, git) in decorated {
            let open = entry.is_navigable() && self.expanded.contains(&entry.path);
            let children = open.then(|| self.children.get(&entry.path)).flatten();
            let mut row = Row::new(entry, Vec::new(), None);
            row.depth = depth;
            row.git = git.unwrap_or_default().into();
            out.push(row);
            if let Some(children) = children {
                self.tree_rows(children.clone(), depth + 1, out);
            }
        }
    }

    /// Open or close a folder in the list.
    pub(super) fn toggle_expanded(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path.clone());
            self.load_children(path, cx);
        }
        self.rebuild_rows(cx);
    }

    /// List a folder that's open in the list, then redraw.
    fn load_children(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let show_hidden = self.config.show_hidden;
        let last_opened = self.wants_last_opened();
        self.load_folder_stats(dir.clone(), cx);
        cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn({
                    let dir = dir.clone();
                    async move {
                        let mut entries = listing::list_dir(&dir, show_hidden).unwrap_or_default();
                        if last_opened {
                            entries.iter_mut().for_each(FileEntry::read_last_opened);
                        }
                        entries
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.children.insert(dir, entries);
                this.rebuild_rows(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reread every folder that's open in the list.
    pub(super) fn reload_children(&mut self, cx: &mut Context<Self>) {
        let open: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        for dir in open {
            self.load_children(dir, cx);
        }
    }

    /// Right and Left in the list: open or close the selected folders; Left
    /// on an item inside an open folder selects that folder. Returns whether
    /// the key did anything.
    pub(super) fn tree_key(&mut self, dx: isize, cx: &mut Context<Self>) -> bool {
        if !self.tree_mode() || dx == 0 {
            return false;
        }
        let Some(ix) = self.selection.cursor() else {
            return false;
        };
        let Some(row) = self.rows.get(ix) else {
            return false;
        };
        let (path, depth) = (row.entry.path.clone(), row.depth);
        let open = self.expanded.contains(&path);
        let opens = dx > 0 && row.entry.is_navigable() && !open;
        if opens || (dx < 0 && open) {
            self.toggle_expanded(path, cx);
        } else if dx < 0 && depth > 0 {
            let parent = path.parent().map(Path::to_path_buf);
            if let Some(up) = self.rows[..ix]
                .iter()
                .rposition(|r| Some(&r.entry.path) == parent.as_ref())
            {
                self.selection.select_only(up);
                self.scroll_to(up);
                cx.notify();
            }
        }
        true
    }

    /// Show or hide a list column.
    pub(super) fn toggle_column(&mut self, column: ListColumn, cx: &mut Context<Self>) {
        let columns = &mut self.config.list_columns;
        match columns.iter().position(|c| c.column == column) {
            Some(ix) => {
                columns.remove(ix);
            }
            None => columns.push(ColumnSetting {
                column,
                width: column.default_width(),
            }),
        }
        self.list_columns_changed(cx);
    }

    /// Put the column at `from` where the one at `to` is.
    pub(super) fn move_column(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let columns = &mut self.config.list_columns;
        if from == to || from >= columns.len() || to >= columns.len() {
            return;
        }
        let moved = columns.remove(from);
        columns.insert(to, moved);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Start resizing column `ix` from the mouse at `x`.
    pub(super) fn start_column_resize(&mut self, ix: usize, x: Pixels) {
        if let Some(column) = self.config.list_columns.get(ix) {
            self.column_resize = Some((ix, x, column.width));
        }
    }

    /// The mouse moved to `x` while resizing a column.
    pub(super) fn resize_column(&mut self, ix: usize, x: Pixels, cx: &mut Context<Self>) {
        let Some((resizing, start, width)) = self.column_resize.filter(|(i, ..)| *i == ix) else {
            return;
        };
        let moved = f32::from(x - start) / crate::theme::ui_scale();
        if let Some(column) = self.config.list_columns.get_mut(resizing) {
            column.width = (width + moved).max(MIN_COLUMN_WIDTH);
            self.schedule_save(cx);
            cx.notify();
        }
    }

    fn list_columns_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_save(cx);
        // Last-opened dates and git statuses are only read when shown.
        self.load_listing(Vec::new(), true, cx);
        self.reload_children(cx);
        self.reload_folder_stats(cx);
        cx.notify();
    }

    pub(super) fn reset_list_columns(&mut self, cx: &mut Context<Self>) {
        self.config.list_columns = default_list_columns();
        self.list_columns_changed(cx);
    }

    /// Keep Folders on Top, in every sort.
    pub(super) fn toggle_folders_first(
        &mut self,
        _: &ToggleFoldersFirst,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.folders_first = !self.config.folders_first;
        self.schedule_save(cx);
        self.rebuild_rows(cx);
        let dirs: Vec<PathBuf> = self.columns.iter().map(|c| c.dir.clone()).collect();
        for (ix, dir) in dirs.into_iter().enumerate().skip(1) {
            self.load_column(ix, dir, cx);
        }
    }

    pub(super) fn set_folder_sizes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.folder_sizes = on;
        self.schedule_save(cx);
        self.load_listing(Vec::new(), true, cx);
        self.reload_children(cx);
        self.reload_folder_stats(cx);
    }
}

/// Right-click on the list's header: choose columns, and the list options.
pub(super) fn header_menu(
    view: &Entity<Finder>,
    mut menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let config = view.read(cx).config.clone();
    for column in ListColumn::ALL {
        let shown = config.list_columns.iter().any(|c| c.column == column);
        menu = menu.item(PopupMenuItem::new(column.title()).checked(shown).on_click(
            window.listener_for(view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                this.toggle_column(column, cx)
            }),
        ));
    }
    let folder_sizes = config.folder_sizes;
    menu.separator()
        .item(
            PopupMenuItem::new("Keep Folders on Top")
                .checked(config.folders_first)
                .action(Box::new(ToggleFoldersFirst)),
        )
        .item(
            PopupMenuItem::new("Calculate All Sizes")
                .checked(folder_sizes)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_folder_sizes(!folder_sizes, cx)
                    },
                )),
        )
        .separator()
        .item(PopupMenuItem::new("Reset Columns").on_click(
            window.listener_for(view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                this.reset_list_columns(cx)
            }),
        ))
        .min_w(ui(200.))
}
