//! The list view's extras: folders that open in place (disclosure
//! triangles), the column chooser, resizing and reordering columns, Keep
//! Folders on Top, and folder sizes and git statuses from the index. Dragging
//! an edge also resizes the column view's panes and the sidebar.

use super::{Finder, Row};
use crate::actions::ToggleFoldersFirst;
use crate::model::Location;
use crate::theme::ui;
use fffinder_core::config::{
    COLUMN_VIEW_WIDTH_RANGE, DEFAULT_PARENT_COLUMN_WIDTH, DEFAULT_PREVIEW_PANE_WIDTH,
    DEFAULT_SIDEBAR_WIDTH, MIN_COLUMN_WIDTH, MIN_NAME_WIDTH, PREVIEW_PANE_WIDTH_RANGE,
    SIDEBAR_WIDTH_RANGE, default_list_columns,
};
use fffinder_core::{ColumnSetting, FileEntry, ListColumn, SortKey, ViewMode, listing};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// The widest double-clicking an edge makes a column, in points.
const MAX_FIT_WIDTH: f32 = 800.;

/// What a list row adds around each text: its cell's padding, in points.
const CELL_PADDING: f32 = 12.;

/// An edge the mouse drags to make something wider or narrower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResizeEdge {
    /// The list's Name column.
    Name,
    /// The list's Folder column, shown for search results.
    Parent,
    /// One of the list's other columns, by its place in `list_columns`.
    Column(usize),
    /// A pane of the column view, by its place in `columns`.
    Pane(usize),
    /// The sidebar.
    Sidebar,
    /// The preview pane, whose edge to drag is on its left.
    PreviewPane,
}

/// Dragged to resize `.0`.
#[derive(Clone)]
pub(super) struct ResizeDrag(pub ResizeEdge);

/// Dragged to move list column `.0` to where it's dropped.
#[derive(Clone)]
pub(super) struct ColumnMove(pub usize);

/// What a dragged column header or sidebar row looks like: its name.
pub(super) struct DragLabel(pub SharedString);

impl Render for DragLabel {
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

    /// Whether the list's Folder column shows the matching line of a contents
    /// search instead of a folder.
    pub(super) fn parent_column_shows_match(&self) -> bool {
        self.search_contents && self.is_searching()
    }

    /// What the list's Folder column is called here.
    pub(super) fn parent_column_title(&self) -> &'static str {
        if self.parent_column_shows_match() {
            "Match"
        } else if self.in_trash() {
            "Original Folder"
        } else {
            "Folder"
        }
    }

    /// What clicking the Folder column's header sorts by. A matching line has
    /// no useful order, and everything in the Trash is in the one folder.
    pub(super) fn parent_column_sort(&self) -> Option<SortKey> {
        (!self.parent_column_shows_match() && !self.in_trash()).then_some(SortKey::Folder)
    }

    /// How wide `edge` is in points; `None` for Name while it fills the room
    /// the other columns leave, and for a column that's gone.
    pub(super) fn edge_width(&self, edge: ResizeEdge) -> Option<f32> {
        match edge {
            ResizeEdge::Name => self.config.name_column_width,
            ResizeEdge::Parent => Some(self.config.parent_column_width),
            ResizeEdge::Column(ix) => self.config.list_columns.get(ix).map(|c| c.width),
            ResizeEdge::Pane(col) => self.columns.get(col).map(|pane| pane.width),
            ResizeEdge::Sidebar => Some(self.config.sidebar_width()),
            ResizeEdge::PreviewPane => Some(self.config.preview_pane_width()),
        }
    }

    /// Make `edge` `width` points wide, as near as it can be.
    fn set_edge_width(&mut self, edge: ResizeEdge, width: f32) {
        match edge {
            ResizeEdge::Name => self.config.name_column_width = Some(width.max(MIN_NAME_WIDTH)),
            ResizeEdge::Parent => self.config.parent_column_width = width.max(MIN_COLUMN_WIDTH),
            ResizeEdge::Column(ix) => {
                if let Some(column) = self.config.list_columns.get_mut(ix) {
                    column.width = width.max(MIN_COLUMN_WIDTH);
                }
            }
            ResizeEdge::Pane(col) => {
                let (min, max) = COLUMN_VIEW_WIDTH_RANGE;
                let width = width.clamp(min, max);
                if let Some(pane) = self.columns.get_mut(col) {
                    pane.width = width;
                }
                // The next pane to open starts at the width last chosen.
                self.config.column_view_width = width;
            }
            ResizeEdge::Sidebar => {
                let (min, max) = SIDEBAR_WIDTH_RANGE;
                self.config.sidebar_width = width.clamp(min, max);
            }
            ResizeEdge::PreviewPane => {
                let (min, max) = PREVIEW_PANE_WIDTH_RANGE;
                self.config.preview_pane_width = width.clamp(min, max);
            }
        }
    }

    /// Start resizing `edge` from the mouse at `x`.
    pub(super) fn start_resize(&mut self, edge: ResizeEdge, x: Pixels) {
        // Name has no width of its own until it's dragged, so it starts from
        // what it was drawn at.
        let shown = f32::from(self.name_column_shown) / crate::theme::ui_scale();
        let width = self.edge_width(edge).unwrap_or(shown);
        self.resizing = Some((edge, x, width));
    }

    /// The mouse moved to `x` while resizing `edge`.
    pub(super) fn resize_edge(&mut self, edge: ResizeEdge, x: Pixels, cx: &mut Context<Self>) {
        let Some((resizing, start, width)) = self.resizing.filter(|(e, ..)| *e == edge) else {
            return;
        };
        let moved = f32::from(x - start) / crate::theme::ui_scale();
        // Its edge is on the left, so moving right makes it narrower.
        let moved = match resizing {
            ResizeEdge::PreviewPane => -moved,
            _ => moved,
        };
        self.set_edge_width(resizing, width + moved);
        self.widths_changed(cx);
    }

    /// Double-clicking an edge: make a column just wide enough for its
    /// longest text. The sidebar goes back to its usual width.
    pub(super) fn fit_edge(
        &mut self,
        edge: ResizeEdge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let width = match edge {
            ResizeEdge::Sidebar => DEFAULT_SIDEBAR_WIDTH,
            ResizeEdge::PreviewPane => DEFAULT_PREVIEW_PANE_WIDTH,
            _ => self.fitting_width(edge, window, cx),
        };
        self.set_edge_width(edge, width);
        self.widths_changed(cx);
    }

    /// The room `edge`'s longest text needs, and its header's, in points.
    fn fitting_width(&self, edge: ResizeEdge, window: &Window, cx: &App) -> f32 {
        let theme = cx.theme();
        // The header's title and the arrow that shows it's sorted by it.
        let header = |title: &str| vec![(SharedString::from(title.to_string()), 34.)];
        let (texts, family) = match edge {
            ResizeEdge::Name => {
                let tree = if self.tree_mode() { 20. } else { 0. };
                let rows = self.rows.iter().map(|row| {
                    let indent = 16. * row.depth as f32;
                    (row.name.clone(), CELL_PADDING + 22. + tree + indent)
                });
                (
                    rows.chain(header("Name")).collect(),
                    theme.font_family.clone(),
                )
            }
            ResizeEdge::Parent => {
                let rows = self
                    .rows
                    .iter()
                    .map(|row| (row.detail.clone(), CELL_PADDING));
                (
                    rows.chain(header(self.parent_column_title())).collect(),
                    theme.font_family.clone(),
                )
            }
            ResizeEdge::Column(ix) => {
                let Some(column) = self.config.list_columns.get(ix).map(|c| c.column) else {
                    return MIN_COLUMN_WIDTH;
                };
                let rows = self
                    .rows
                    .iter()
                    .map(|row| (row.column_text(column), CELL_PADDING));
                let family = match column {
                    ListColumn::Permissions => theme.mono_font_family.clone(),
                    _ => theme.font_family.clone(),
                };
                (rows.chain(header(column.title())).collect(), family)
            }
            ResizeEdge::Pane(col) => {
                let Some(pane) = self.columns.get(col) else {
                    return COLUMN_VIEW_WIDTH_RANGE.0;
                };
                // Its row's margins, icon, and arrow, and the edge to drag.
                let extra = CELL_PADDING * 2. + 22. + 18. + super::views::PANE_EDGE;
                let rows = pane
                    .entries
                    .iter()
                    .map(|entry| (SharedString::from(entry.name.clone()), extra));
                (rows.collect(), theme.font_family.clone())
            }
            ResizeEdge::Sidebar | ResizeEdge::PreviewPane => {
                (Vec::new(), theme.font_family.clone())
            }
        };
        let font = font(family);
        super::fit::widest_points(texts, &font, ui(12.), window).min(MAX_FIT_WIDTH)
    }

    /// A strip over an edge that drags it, and fits it on a double-click. The
    /// caller says where it goes. Dragging is watched from the window, so it
    /// keeps working when the mouse leaves the strip.
    pub(super) fn resize_handle(
        &self,
        id: impl Into<ElementId>,
        edge: ResizeEdge,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let highlight = cx.theme().primary.opacity(0.35);
        div()
            .id(id)
            .cursor(CursorStyle::ResizeLeftRight)
            .occlude()
            .hover(move |style| style.bg(highlight))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if event.click_count >= 2 {
                        this.fit_edge(edge, window, cx);
                    } else {
                        this.start_resize(edge, event.position.x);
                    }
                }),
            )
            .on_drag(ResizeDrag(edge), |_, _, _, cx| cx.new(|_| EmptyView))
    }

    /// Widths or columns changed: publish them and redraw.
    fn widths_changed(&mut self, cx: &mut Context<Self>) {
        self.settings_changed(cx);
        cx.notify();
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
        self.widths_changed(cx);
    }

    fn list_columns_changed(&mut self, cx: &mut Context<Self>) {
        self.widths_changed(cx);
        // Last-opened dates and git statuses are only read when shown.
        self.load_listing(Vec::new(), true, cx);
        self.reload_children(cx);
        self.reload_folder_stats(cx);
    }

    /// Reset Columns: the usual columns at their usual widths, with Name
    /// filling the room again.
    pub(super) fn reset_list_columns(&mut self, cx: &mut Context<Self>) {
        self.config.list_columns = default_list_columns();
        self.config.name_column_width = None;
        self.config.parent_column_width = DEFAULT_PARENT_COLUMN_WIDTH;
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
        self.settings_changed(cx);
        self.folders_first_changed(cx);
    }

    /// Sort everything again with folders on top, or not.
    pub(super) fn folders_first_changed(&mut self, cx: &mut Context<Self>) {
        self.rebuild_rows(cx);
        let dirs: Vec<PathBuf> = self.columns.iter().map(|c| c.dir.clone()).collect();
        for (ix, dir) in dirs.into_iter().enumerate().skip(1) {
            self.load_column(ix, dir, cx);
        }
    }

    pub(super) fn set_folder_sizes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.folder_sizes = on;
        self.settings_changed(cx);
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
