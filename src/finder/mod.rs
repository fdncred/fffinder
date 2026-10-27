//! The browser window: state, navigation, search, selection, and actions.
//! Rendering lives in the submodules.

mod bars;
mod batch_rename;
mod dialogs;
mod editor;
mod filter;
mod fps;
mod menus;
mod nu_panel;
mod preview;
mod sidebar;
mod toolbar;
mod transfer;
mod trash;
mod views;

use crate::actions::*;
use crate::model::{History, Location, Selection};
use crate::nushell::{self, NuRun};
use crate::theme::ui;
use fffinder_core::places::{self, Place};
use fffinder_core::terminals::{self, CUSTOM, Terminal, TerminalSetting};
use fffinder_core::{
    Config, Engine, FileEntry, GrepRequest, KindFilter, RootStatus, SearchRequest, SortKey,
    SortOrder, ViewMode, format, listing,
};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, ElementExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub use fps::ShowFps;
pub use menus::reveal_label;
pub use preview::Preview;
pub use views::path_id;

/// Shared by every window.
pub struct AppState {
    pub engine: Arc<Engine>,
    pub nu: Option<PathBuf>,
    /// Use these terminals instead of detecting them (tests).
    pub terminals: Option<Vec<Terminal>>,
}

impl Global for AppState {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Computer,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickerOptions {
    pub multiple: bool,
}

/// What a terminal opened by fffinder runs.
#[derive(Debug, Clone)]
pub(super) enum InTerminal {
    Nu,
    Command { program: PathBuf, args: Vec<String> },
}

/// One displayed item with its labels precomputed for fast rendering.
#[derive(Clone)]
pub struct Row {
    pub entry: FileEntry,
    pub name: SharedString,
    pub date: SharedString,
    pub size: SharedString,
    pub kind: SharedString,
    /// Parent folder (search results) or the matching line (content search).
    pub detail: SharedString,
    pub highlights: Vec<(usize, usize)>,
    /// How `LS_COLORS` draws the name, when that option is on.
    pub name_style: Option<crate::nu_env::NameStyle>,
}

impl Row {
    fn new(entry: FileEntry, highlights: Vec<(usize, usize)>, detail: Option<String>) -> Row {
        let detail = detail.unwrap_or_else(|| entry.parent().map(tildify).unwrap_or_default());
        Row {
            name: entry.name.clone().into(),
            date: entry
                .modified
                .map(format::human_date)
                .unwrap_or_default()
                .into(),
            size: entry
                .size
                .map(format::human_size)
                .unwrap_or_else(|| "--".into())
                .into(),
            kind: entry.kind_label().into(),
            detail: detail.into(),
            highlights,
            name_style: None,
            entry,
        }
    }
}

/// `/Users/me/src` → `~/src`.
pub fn tildify(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
    }
    path.display().to_string()
}

/// One pane of the column browser.
pub struct ColumnPane {
    pub dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: Option<usize>,
    pub scroll: UniformListScrollHandle,
}

#[derive(Default)]
pub struct SearchStats {
    pub elapsed: Duration,
    pub total_matched: usize,
    pub indexing: bool,
    /// A content search hit the time limit before searching every file.
    pub stopped: bool,
}

pub struct Finder {
    focus: FocusHandle,
    files_focus: FocusHandle,
    pub(crate) engine: Arc<Engine>,
    pub(crate) config: Config,
    history: History,

    listing: Vec<FileEntry>,
    listing_error: Option<String>,
    listing_stamp: Option<SystemTime>,
    loading: bool,
    listing_task: Task<()>,

    search_input: Entity<InputState>,
    query: String,
    scope: SearchScope,
    search_contents: bool,
    results: Vec<Row>,
    search_stats: Option<SearchStats>,
    searching: bool,
    search_task: Task<()>,
    search_gen: u64,

    rows: Vec<Row>,
    selection: Selection,
    result_sort: SortOrder,
    grid_cols: usize,
    list_scroll: UniformListScrollHandle,
    grid_scroll: UniformListScrollHandle,
    strip_scroll: gpui_kit::component::VirtualListScrollHandle,
    columns_scroll: ScrollHandle,
    columns: Vec<ColumnPane>,
    active_column: usize,
    content_height: Pixels,

    previews: HashMap<PathBuf, Preview>,
    preview_tasks: HashMap<PathBuf, Task<()>>,
    quick_look_open: bool,
    quick_look_focus: FocusHandle,
    /// Type-to-select: the letters typed so far and when the last arrived.
    type_select: String,
    type_select_at: Option<std::time::Instant>,
    /// Where an item was last right-clicked. Context menus don't stop
    /// propagation, so the empty-space menu around the item checks this and
    /// stays closed for the same click.
    item_menu_at: Option<Point<Pixels>>,

    nu_open: bool,
    /// The open "Rename N Items" dialog's state.
    batch: Option<batch_rename::BatchRename>,
    /// The Nushell filter bar (see filter.rs).
    filter_input: Entity<InputState>,
    filter_open: bool,
    /// The filter in effect; empty for none.
    filter: String,
    filter_task: Task<()>,
    filter_debounce: Task<()>,
    /// Paths the filter kept, for the rows in `filter_source`.
    filter_keep: Option<HashSet<PathBuf>>,
    filter_source: Option<Vec<PathBuf>>,
    filter_error: Option<String>,
    filtering: bool,
    /// Rows before the filter, for the status bar.
    unfiltered_len: usize,
    /// The result cap the current search ran with.
    search_limit: usize,
    /// The user's Nushell environment, when "use Nushell's colors and
    /// editor" is on (see nu_env.rs).
    nu_env: Option<Arc<crate::nu_env::NuEnv>>,
    ls_colors: Option<Arc<lscolors::LsColors>>,
    nu_env_error: Option<String>,
    nu_env_loading: bool,
    nu_env_task: Task<()>,
    /// Commands from the user's actions.nu, for the "Nushell Actions" menus.
    user_actions: Vec<crate::user_actions::UserAction>,
    user_actions_error: Option<String>,
    /// When actions.nu was last read, to reload it after edits.
    user_actions_stamp: Option<SystemTime>,
    user_actions_task: Task<()>,
    /// Installed terminals for "Open Nushell Here"; `None` until detected.
    terminals: Option<Vec<Terminal>>,
    terminals_task: Task<()>,
    nu_input: Entity<InputState>,
    nu_result: Option<NuRun>,
    nu_running: bool,
    nu_task: Task<()>,

    favorites: Vec<Place>,
    locations: Vec<Place>,
    index_status: Vec<RootStatus>,
    status_task: Task<()>,
    /// Applies a change of search roots; a newer change replaces it.
    roots_task: Task<()>,
    /// Applies Settings › Search engine; a newer change replaces it.
    search_settings_task: Task<()>,
    watch_task: Task<()>,
    save_task: Task<()>,
    /// What's in the Trash, while the Trash is showing.
    trashed: Vec<crate::trash_bin::Trashed>,
    /// Copies and moves under way, and the task redrawing their progress.
    transfers: Vec<transfer::Job>,
    transfers_task: Task<()>,
    /// Frames per second for the status bar, with `--fps`.
    fps: Option<fps::FpsMeter>,
    fps_task: Task<()>,

    icon_slider: Entity<SliderState>,
    dialog_sub: Option<Subscription>,
    /// Write settings changes to disk (off for headless screenshots).
    persist: bool,
    picker: Option<PickerOptions>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for Finder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Finder {
    pub fn new(
        start: Location,
        query: Option<String>,
        picker: Option<PickerOptions>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let engine = cx.global::<AppState>().engine.clone();
        let config = Config::load();
        crate::theme::set_ui_scale(config.ui_scale(), cx);
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let nu_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(
                "Nushell pipeline over the selection, e.g.  where size > 1mb | sort-by modified",
            )
        });
        let filter_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("size > 10mb and modified > ((date now) - 1wk)")
                .clean_on_escape()
        });
        let icon_slider = cx.new(|_| {
            SliderState::new()
                .min(32.)
                .max(128.)
                .step(8.)
                .default_value(config.icon_size)
        });
        let subscriptions = vec![
            cx.subscribe(&icon_slider, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    this.set_icon_size(value.start(), cx);
                }
            }),
            cx.subscribe_in(&search_input, window, Self::on_search_event),
            cx.subscribe_in(&filter_input, window, Self::on_filter_event),
            cx.subscribe_in(&nu_input, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.run_nu_pipeline(window, cx);
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.config.appearance == fffinder_core::Appearance::System {
                    crate::theme::apply(this.config.appearance, Some(window), cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.locations = places::locations();
                    this.check_for_changes(cx);
                }
            }),
        ];
        let favorites = config
            .favorites
            .iter()
            .map(|p| places::favorite_for(p))
            .collect();
        let mut this = Finder {
            focus: cx.focus_handle(),
            files_focus: cx.focus_handle(),
            engine,
            history: History::new(start),
            listing: Vec::new(),
            listing_error: None,
            listing_stamp: None,
            loading: false,
            listing_task: Task::ready(()),
            search_input,
            query: String::new(),
            scope: SearchScope::Computer,
            search_contents: false,
            results: Vec::new(),
            search_stats: None,
            searching: false,
            search_task: Task::ready(()),
            search_gen: 0,
            rows: Vec::new(),
            selection: Selection::default(),
            result_sort: SortOrder {
                key: SortKey::Relevance,
                ascending: true,
            },
            grid_cols: 1,
            list_scroll: UniformListScrollHandle::new(),
            grid_scroll: UniformListScrollHandle::new(),
            strip_scroll: gpui_kit::component::VirtualListScrollHandle::new(),
            columns_scroll: ScrollHandle::new(),
            columns: Vec::new(),
            active_column: 0,
            content_height: ui(400.),
            previews: HashMap::new(),
            preview_tasks: HashMap::new(),
            quick_look_open: false,
            quick_look_focus: cx.focus_handle(),
            item_menu_at: None,
            type_select: String::new(),
            type_select_at: None,
            nu_open: false,
            batch: None,
            filter_input,
            filter_open: false,
            filter: String::new(),
            filter_task: Task::ready(()),
            filter_debounce: Task::ready(()),
            filter_keep: None,
            filter_source: None,
            filter_error: None,
            filtering: false,
            unfiltered_len: 0,
            search_limit: 0,
            terminals: None,
            terminals_task: Task::ready(()),
            nu_env: None,
            ls_colors: None,
            nu_env_error: None,
            nu_env_loading: false,
            nu_env_task: Task::ready(()),
            user_actions: Vec::new(),
            user_actions_error: None,
            user_actions_stamp: None,
            user_actions_task: Task::ready(()),
            nu_input,
            nu_result: None,
            nu_running: false,
            nu_task: Task::ready(()),
            favorites,
            locations: places::locations(),
            index_status: Vec::new(),
            status_task: Task::ready(()),
            roots_task: Task::ready(()),
            search_settings_task: Task::ready(()),
            watch_task: Task::ready(()),
            save_task: Task::ready(()),
            trashed: Vec::new(),
            transfers: Vec::new(),
            transfers_task: Task::ready(()),
            fps: None,
            fps_task: Task::ready(()),
            icon_slider,
            dialog_sub: None,
            persist: true,
            picker,
            _subscriptions: subscriptions,
            config,
        };
        this.load_listing(Vec::new(), false, cx);
        this.poll_index_status(cx);
        this.watch_folder(cx);
        this.detect_terminals(cx);
        this.load_user_actions(cx);
        if cx.has_global::<ShowFps>() {
            this.start_fps_meter(cx);
        }
        if this.config.use_nu_environment {
            this.load_nu_env(cx);
        }
        if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
            this.search_input
                .update(cx, |s, cx| s.set_value(query.clone(), window, cx));
            this.set_query(query, true, cx);
        }
        this
    }

    // ----- small accessors ---------------------------------------------------

    #[doc(hidden)]
    pub fn rows_for_test(&self) -> &[Row] {
        &self.rows
    }

    #[doc(hidden)]
    pub fn column_names_for_test(&self) -> Vec<Vec<String>> {
        self.columns
            .iter()
            .map(|c| c.entries.iter().map(|e| e.name.clone()).collect())
            .collect()
    }

    #[doc(hidden)]
    pub fn has_preview_for_test(&self, path: &Path) -> bool {
        self.previews.contains_key(path)
    }

    #[doc(hidden)]
    pub fn icon_size_for_test(&self) -> f32 {
        self.config.icon_size
    }

    pub fn search_scope(&self) -> SearchScope {
        self.scope
    }

    pub fn searches_contents(&self) -> bool {
        self.search_contents
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn nu_panel_open(&self) -> bool {
        self.nu_open
    }

    pub fn quick_look_is_open(&self) -> bool {
        self.quick_look_open
    }

    pub fn favorites(&self) -> &[Place] {
        &self.favorites
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_back()
    }

    /// The window's settings as they are now.
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn files_focus_handle(&self) -> FocusHandle {
        self.files_focus.clone()
    }

    pub fn location(&self) -> &Location {
        self.history.current()
    }

    pub fn current_folder(&self) -> Option<&Path> {
        self.history.current().folder()
    }

    pub fn is_searching(&self) -> bool {
        !self.query.is_empty()
    }

    /// Items come from many folders (a search, Recents, sent results), so
    /// the list shows where each one is.
    pub fn shows_parent_column(&self) -> bool {
        self.is_searching() || self.current_folder().is_none()
    }

    /// Show `paths` as a results view; returns how many exist and which
    /// don't.
    pub fn show_results(
        &mut self,
        title: String,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (usize, Vec<PathBuf>) {
        let (found, missing): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| p.exists());
        let shown = found.len();
        if shown > 0 {
            self.navigate(
                Location::Results {
                    title,
                    paths: Arc::new(found),
                },
                window,
                cx,
            );
        }
        (shown, missing)
    }

    /// Go to `path` (a folder, or a file's folder with the file selected)
    /// and search `query`, as `fff gui` and a second launch ask.
    pub fn open_request(
        &mut self,
        path: Option<&Path>,
        query: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = path {
            if path.is_dir() {
                self.navigate(Location::Folder(path.to_path_buf()), window, cx);
            } else if path.exists() {
                self.reveal_in_place(path, window, cx);
            }
        }
        if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
            let query = query.to_string();
            self.search_input
                .update(cx, |s, cx| s.set_value(query.clone(), window, cx));
            self.set_query(query, true, cx);
        }
    }

    /// What `/v1/selection` reports: where the window is and its selected
    /// (or, with `all`, shown) items.
    pub fn selection_json(&self, all: bool) -> serde_json::Value {
        let location = if self.is_searching() {
            "search"
        } else {
            match self.history.current() {
                Location::Folder(_) => "folder",
                Location::Recents => "recents",
                Location::Results { .. } => "results",
                Location::Trash => "trash",
            }
        };
        let items: Vec<FileEntry> = if all {
            self.rows.iter().map(|r| r.entry.clone()).collect()
        } else {
            self.selected_entries()
        };
        serde_json::json!({
            "location": location,
            "folder": self.current_folder(),
            "query": self.is_searching().then_some(&self.query),
            "items": items.iter().map(crate::server::item_json).collect::<Vec<_>>(),
        })
    }

    /// Search results always use the list in column mode, like Finder.
    pub fn effective_view(&self) -> ViewMode {
        match self.config.view_mode {
            ViewMode::Columns if self.is_searching() || self.current_folder().is_none() => {
                ViewMode::List
            }
            mode => mode,
        }
    }

    pub fn title(&self) -> String {
        if self.is_searching() {
            format!("Searching “{}”", self.query)
        } else {
            self.history.current().title()
        }
    }

    fn sort_order(&self) -> SortOrder {
        if self.is_searching() {
            self.result_sort
        } else {
            self.config.sort
        }
    }

    // ----- navigation --------------------------------------------------------

    pub fn navigate(&mut self, to: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(window, cx);
        if self.history.navigate(to) {
            self.location_changed(None, window, cx);
        }
    }

    fn location_changed(
        &mut self,
        select: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection.clear();
        self.columns.clear();
        self.active_column = 0;
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if let Some(folder) = self.current_folder() {
            self.config.last_folder = Some(folder.to_path_buf());
            self.schedule_save(cx);
        }
        window.set_window_title(&self.history.current().title());
        self.load_listing(select.into_iter().collect(), false, cx);
        cx.notify();
    }

    pub fn go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.current_folder().map(Path::to_path_buf);
        self.end_search(window, cx);
        if self.history.back() {
            self.location_changed(from, window, cx);
        }
    }

    pub fn go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(window, cx);
        if self.history.forward() {
            self.location_changed(None, window, cx);
        }
    }

    pub fn go_up(&mut self, _: &GoUp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        if let Some(parent) = folder.parent() {
            self.end_search(window, cx);
            if self
                .history
                .navigate(Location::Folder(parent.to_path_buf()))
            {
                self.location_changed(Some(folder), window, cx);
            }
        }
    }

    fn go_home(&mut self, _: &GoHome, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(home) = dirs::home_dir() {
            self.navigate(Location::Folder(home), window, cx);
        }
    }

    fn go_recents(&mut self, _: &GoRecents, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Location::Recents, window, cx);
    }

    fn go_trash(&mut self, _: &GoTrash, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Location::Trash, window, cx);
    }

    /// Open a folder and select `item` in it.
    pub fn reveal_in_place(&mut self, item: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = item.parent() else { return };
        self.end_search(window, cx);
        self.history
            .navigate(Location::Folder(parent.to_path_buf()));
        self.location_changed(Some(item.to_path_buf()), window, cx);
    }

    // ----- listing -----------------------------------------------------------

    /// List the current location, then select the items at `select`.
    fn load_listing(&mut self, select: Vec<PathBuf>, quiet: bool, cx: &mut Context<Self>) {
        let location = self.history.current().clone();
        let show_hidden = self.config.show_hidden;
        let engine = self.engine.clone();
        let bin = crate::trash_bin::bin(cx);
        if !quiet {
            self.loading = true;
        }
        self.listing_task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match &location {
                        Location::Folder(dir) => listing::list_dir(dir, show_hidden)
                            .map(|entries| (entries, dir_stamp(dir), Vec::new()))
                            .map_err(|e| e.to_string()),
                        Location::Recents => {
                            engine.wait_ready(Duration::from_secs(60));
                            Ok((engine.recents(500), None, Vec::new()))
                        }
                        Location::Results { paths, .. } => Ok((
                            paths.iter().filter_map(FileEntry::from_path).collect(),
                            None,
                            Vec::new(),
                        )),
                        Location::Trash => bin.list().map(|items| {
                            let entries = items
                                .iter()
                                .filter_map(|item| {
                                    let mut entry = FileEntry::from_path(&item.path)?;
                                    entry.name = item.name();
                                    Some(entry)
                                })
                                .collect();
                            (entries, None, items)
                        }),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((entries, stamp, trashed)) => {
                        this.listing = entries;
                        this.listing_error = None;
                        this.listing_stamp = stamp;
                        this.trashed = trashed;
                    }
                    Err(err) => {
                        this.listing.clear();
                        this.listing_error = Some(err);
                        this.listing_stamp = None;
                        this.trashed.clear();
                    }
                }
                this.rebuild_rows(cx);
                let found: Vec<usize> = select
                    .iter()
                    .filter_map(|path| this.rows.iter().position(|r| r.entry.path == *path))
                    .collect();
                if let Some((&first, rest)) = found.split_first() {
                    this.selection.select_only(first);
                    for &ix in rest {
                        this.selection.toggle(ix);
                    }
                    this.scroll_to(first);
                    this.selection_changed(cx);
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Reload quietly when the folder changed on disk.
    fn check_for_changes(&mut self, cx: &mut Context<Self>) {
        let actions = crate::user_actions::actions_path();
        if self.user_actions_stamp.is_some()
            && crate::user_actions::modified(&actions) != self.user_actions_stamp
        {
            self.load_user_actions(cx);
        }
        if self.loading {
            return;
        }
        if let Some(dir) = self.current_folder()
            && dir_stamp(dir) != self.listing_stamp
        {
            self.load_listing(Vec::new(), true, cx);
        }
    }

    fn watch_folder(&mut self, cx: &mut Context<Self>) {
        self.watch_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if this
                    .update(cx, |this, cx| this.check_for_changes(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.previews.clear();
        self.load_listing(Vec::new(), true, cx);
        if self.is_searching() {
            self.run_search(false, cx);
        }
        self.reload_columns(cx);
    }

    /// Reload the column view's panes after the first, which mirrors the rows.
    fn reload_columns(&mut self, cx: &mut Context<Self>) {
        let dirs: Vec<PathBuf> = self.columns.iter().map(|c| c.dir.clone()).collect();
        for (ix, dir) in dirs.into_iter().enumerate().skip(1) {
            self.load_column(ix, dir, cx);
        }
    }

    /// Rebuild the displayed rows from the listing or search results.
    pub fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let old: Vec<PathBuf> = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        let mut rows: Vec<Row> = if self.is_searching() {
            self.results.clone()
        } else {
            // Items in the Trash show the folder they came from.
            let came_from: HashMap<&Path, String> = self
                .trashed
                .iter()
                .map(|t| (t.path.as_path(), trash::original_folder(t)))
                .collect();
            self.listing
                .iter()
                .map(|e| {
                    let detail = came_from.get(e.path.as_path()).cloned();
                    Row::new(e.clone(), Vec::new(), detail)
                })
                .collect()
        };
        let order = self.sort_order();
        // Recents and sent results keep their own order until a sort is chosen.
        let keep_order = matches!(
            self.history.current(),
            Location::Recents | Location::Results { .. } | Location::Trash
        ) && order == SortOrder::default();
        if order.key != SortKey::Relevance && !keep_order {
            rows.sort_by(|a, b| listing::compare(&a.entry, &b.entry, order));
        }
        self.unfiltered_len = rows.len();
        self.apply_filter_to(&mut rows, cx);
        if let Some(colors) = &self.ls_colors {
            for row in &mut rows {
                row.name_style = crate::nu_env::name_style(colors, &row.entry);
            }
        }
        let new: Vec<PathBuf> = rows.iter().map(|r| r.entry.path.clone()).collect();
        self.selection.remap(&old, &new);
        self.rows = rows;
        if self.effective_view() == ViewMode::Columns {
            self.sync_root_column();
        }
        self.select_first_for_gallery(cx);
        cx.notify();
    }

    /// The gallery always shows an item, like Finder: pick the first one
    /// when nothing is selected.
    fn select_first_for_gallery(&mut self, cx: &mut Context<Self>) {
        if self.effective_view() == ViewMode::Gallery
            && self.selection.is_empty()
            && !self.rows.is_empty()
        {
            self.selection.select_only(0);
            self.scroll_to(0);
            self.selection_changed(cx);
        }
    }

    // ----- search ------------------------------------------------------------

    fn on_search_event(
        &mut self,
        state: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let value = state.read(cx).value().to_string();
                self.set_query(value, false, cx);
            }
            InputEvent::PressEnter { .. } => {
                // Return runs the search now and moves into the results.
                self.run_search(false, cx);
                if !self.rows.is_empty() {
                    self.focus_files(window, cx);
                    if self.selection.is_empty() {
                        self.selection.select_only(0);
                        self.selection_changed(cx);
                    }
                }
            }
            InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    pub fn set_query(&mut self, query: String, immediate: bool, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query == self.query {
            return;
        }
        let was_searching = self.is_searching();
        self.query = query;
        if self.query.is_empty() {
            self.search_gen += 1;
            self.search_task = Task::ready(());
            self.results.clear();
            self.search_stats = None;
            self.searching = false;
            self.selection.clear();
            self.rebuild_rows(cx);
            return;
        }
        if !was_searching {
            self.selection.clear();
            self.result_sort = SortOrder {
                key: SortKey::Relevance,
                ascending: true,
            };
        }
        self.run_search(!immediate, cx);
    }

    fn end_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_searching() {
            self.search_input
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.set_query(String::new(), true, cx);
        }
    }

    /// Run the current query on the background executor; `debounce` waits
    /// 50 ms of typing inactivity first. Stale responses are ignored.
    pub fn run_search(&mut self, debounce: bool, cx: &mut Context<Self>) {
        if self.query.is_empty() {
            return;
        }
        self.search_gen += 1;
        let generation = self.search_gen;
        let engine = self.engine.clone();
        let scope = match self.scope {
            SearchScope::Computer => None,
            SearchScope::Folder => self.current_folder().map(Path::to_path_buf),
        };
        let query = self.query.clone();
        let contents = self.search_contents;
        // A filter applies after the search, so fetch more for it to work on.
        self.search_limit = if self.filter.is_empty() {
            self.config.result_limit
        } else {
            filter::FILTER_SEARCH_LIMIT.max(self.config.result_limit)
        };
        let request = SearchRequest {
            query: query.clone(),
            scope: scope.clone(),
            kind: self.config.kind_filter,
            limit: self.search_limit,
            show_hidden: self.config.show_hidden,
            files_only: false,
        };
        self.searching = true;
        cx.notify();
        self.search_task = cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
            let (rows, stats) = cx
                .background_spawn(async move {
                    if contents {
                        content_search(&engine, &query, scope, request.limit)
                    } else {
                        let response = engine.search(&request);
                        let rows = response
                            .hits
                            .into_iter()
                            .map(|hit| Row::new(hit.entry, hit.highlights, None))
                            .collect();
                        let stats = SearchStats {
                            elapsed: response.elapsed,
                            total_matched: response.total_matched,
                            indexing: response.indexing,
                            stopped: false,
                        };
                        (rows, stats)
                    }
                })
                .await;
            let retry = this
                .update(cx, |this, cx| {
                    if this.search_gen != generation {
                        return false;
                    }
                    this.searching = false;
                    let retry = stats.indexing;
                    this.results = rows;
                    this.search_stats = Some(stats);
                    this.rebuild_rows(cx);
                    this.selection_changed(cx);
                    retry
                })
                .unwrap_or(false);
            if retry {
                // The index is still being built: try again shortly.
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                this.update(cx, |this, cx| {
                    if this.search_gen == generation {
                        this.run_search(false, cx);
                    }
                })
                .ok();
            }
        });
    }

    pub fn set_scope(&mut self, scope: SearchScope, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            self.run_search(false, cx);
            // The toolbar shows the scope even when nothing is searched.
            cx.notify();
        }
    }

    pub fn set_kind(&mut self, kind: KindFilter, cx: &mut Context<Self>) {
        if self.config.kind_filter != kind {
            self.config.kind_filter = kind;
            self.schedule_save(cx);
            self.run_search(false, cx);
        }
    }

    pub fn set_search_contents(&mut self, contents: bool, cx: &mut Context<Self>) {
        if self.search_contents != contents {
            self.search_contents = contents;
            self.run_search(false, cx);
        }
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }

    // ----- index status -------------------------------------------------------

    fn poll_index_status(&mut self, cx: &mut Context<Self>) {
        self.index_status = self.engine.status();
        self.status_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let was_scanning = this.index_status.iter().any(|s| s.scanning);
                        this.index_status = this.engine.status();
                        let scanning = this.index_status.iter().any(|s| s.scanning);
                        if was_scanning && !scanning {
                            // The index can now find terminals outside the app folders.
                            this.detect_terminals(cx);
                            if this.is_searching() {
                                this.run_search(false, cx);
                            }
                            if matches!(this.history.current(), Location::Recents) {
                                this.load_listing(Vec::new(), true, cx);
                            }
                        }
                        cx.notify();
                        scanning
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        });
    }

    pub fn indexing_progress(&self) -> Option<usize> {
        self.index_status
            .iter()
            .any(|s| s.scanning)
            .then(|| self.index_status.iter().map(|s| s.indexed_files).sum())
    }

    pub fn indexed_files(&self) -> usize {
        self.index_status.iter().map(|s| s.indexed_files).sum()
    }

    pub fn set_search_roots(&mut self, roots: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.config.search_roots = roots;
        self.schedule_save(cx);
        let engine = self.engine.clone();
        let roots = self.config.search_roots.clone();
        self.roots_task = cx.spawn(async move |this, cx| {
            cx.background_spawn(async move { engine.set_roots(&roots) })
                .await;
            this.update(cx, |this, cx| this.poll_index_status(cx)).ok();
        });
        cx.notify();
    }

    /// Change fff-search's options (Settings › Search Engine). Options that
    /// shape the index rebuild it, off the UI thread; the search reruns.
    pub fn update_search_settings(
        &mut self,
        edit: impl FnOnce(&mut fffinder_core::SearchSettings),
        cx: &mut Context<Self>,
    ) {
        edit(&mut self.config.search);
        self.schedule_save(cx);
        let engine = self.engine.clone();
        let settings = self.config.search.clone();
        self.search_settings_task = cx.spawn(async move |this, cx| {
            cx.background_spawn(async move { engine.set_search_settings(settings) })
                .await;
            this.update(cx, |this, cx| {
                this.poll_index_status(cx);
                if this.is_searching() {
                    this.run_search(false, cx);
                }
            })
            .ok();
        });
        cx.notify();
    }

    /// The most results a search shows.
    pub fn set_result_limit(&mut self, limit: usize, cx: &mut Context<Self>) {
        self.config.result_limit = limit.max(1);
        self.schedule_save(cx);
        if self.is_searching() {
            self.run_search(false, cx);
        }
        cx.notify();
    }

    pub(super) fn rescan(&mut self, _: &Rescan, window: &mut Window, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.background_spawn(async move { engine.rescan() }).detach();
        window.push_notification("Rescanning the search index…", cx);
        self.poll_index_status(cx);
    }

    // ----- selection ----------------------------------------------------------

    /// Close this window; a picker window cancels the pick.
    pub(super) fn close_window(&self, window: &mut Window) {
        if self.picker.is_some() {
            crate::picker::cancel();
        }
        window.remove_window();
    }

    pub fn focus_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files_focus.focus(window, cx);
    }

    pub fn files_focused(&self, window: &Window) -> bool {
        self.files_focus.is_focused(window)
    }

    /// Mouse selection on row `ix` with Finder modifier semantics.
    pub fn click_row(
        &mut self,
        ix: usize,
        modifiers: Modifiers,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_files(window, cx);
        if click_count >= 2 {
            self.selection.select_only(ix);
            self.open_rows(vec![ix], window, cx);
            return;
        }
        if modifiers.secondary() {
            self.selection.toggle(ix);
        } else if modifiers.shift {
            self.selection.extend_to(ix);
        } else {
            self.selection.select_only(ix);
        }
        self.selection_changed(cx);
        cx.notify();
    }

    /// Right-click selects the item unless it is already part of the selection.
    pub fn right_click_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.item_menu_at = Some(window.mouse_position());
        self.focus_files(window, cx);
        if !self.selection.contains(ix) {
            self.selection.select_only(ix);
            self.selection_changed(cx);
        }
        cx.notify();
    }

    /// True once for the right-click that opened an item's menu.
    pub fn take_item_menu_click(&mut self, position: Point<Pixels>) -> bool {
        self.item_menu_at.take() == Some(position)
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        self.selection_changed(cx);
        cx.notify();
    }

    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        let wants_preview = matches!(self.effective_view(), ViewMode::Gallery | ViewMode::Columns)
            || self.quick_look_open;
        if wants_preview && let Some(entry) = self.primary_entry() {
            self.ensure_preview(&entry, cx);
        }
    }

    /// Selected entries, in display order.
    pub fn selected_entries(&self) -> Vec<FileEntry> {
        if self.effective_view() == ViewMode::Columns {
            return self.column_selection().into_iter().collect();
        }
        self.selection
            .indices()
            .filter_map(|ix| self.rows.get(ix).map(|r| r.entry.clone()))
            .collect()
    }

    /// The focused item: the cursor, or the first selected item.
    pub fn primary_entry(&self) -> Option<FileEntry> {
        if self.effective_view() == ViewMode::Columns {
            return self.column_selection();
        }
        self.selection
            .cursor()
            .filter(|ix| self.selection.contains(*ix))
            .or_else(|| self.selection.indices().next())
            .and_then(|ix| self.rows.get(ix))
            .map(|r| r.entry.clone())
    }

    fn move_selection(
        &mut self,
        dx: isize,
        dy: isize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.effective_view();
        if view == ViewMode::Columns {
            self.move_in_columns(dx, dy, window, cx);
            return;
        }
        let delta = match view {
            ViewMode::Icons => dx + dy * self.grid_cols.max(1) as isize,
            ViewMode::Gallery => dx + dy,
            _ => dy,
        };
        if delta == 0 {
            return;
        }
        self.selection.move_by(delta, extend, self.rows.len());
        if let Some(ix) = self.selection.cursor() {
            self.scroll_to(ix);
        }
        self.selection_changed(cx);
        cx.notify();
    }

    pub fn scroll_to(&self, ix: usize) {
        match self.effective_view() {
            ViewMode::Icons => self
                .grid_scroll
                .scroll_to_item(ix / self.grid_cols.max(1), ScrollStrategy::Nearest),
            ViewMode::Gallery => self.strip_scroll.scroll_to_item(ix, ScrollStrategy::Center),
            _ => self.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest),
        }
    }

    /// Finder's type-to-select: letters typed within a second of each other
    /// form a prefix, and the first item starting with it is selected (or
    /// the next one after it, alphabetically, when none does).
    pub fn type_to_select(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        if self
            .type_select_at
            .is_none_or(|at| now.saturating_duration_since(at) > Duration::from_secs(1))
        {
            self.type_select.clear();
        }
        self.type_select.push_str(text);
        self.type_select_at = Some(now);
        let prefix = self.type_select.to_lowercase();
        let pick = |names: Vec<String>| -> Option<usize> {
            let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
            lower
                .iter()
                .position(|n| n.starts_with(&prefix))
                .or_else(|| {
                    lower
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| n.as_str() > prefix.as_str())
                        .min_by(|a, b| a.1.cmp(b.1))
                        .map(|(ix, _)| ix)
                })
                .or_else(|| names.len().checked_sub(1))
        };
        if self.effective_view() == ViewMode::Columns {
            let col = self.active_column;
            let Some(pane) = self.columns.get(col) else {
                return;
            };
            if let Some(ix) = pick(pane.entries.iter().map(|e| e.name.clone()).collect()) {
                self.select_in_column(col, ix, window, cx);
            }
            return;
        }
        if let Some(ix) = pick(self.rows.iter().map(|r| r.entry.name.clone()).collect()) {
            self.selection.select_only(ix);
            self.scroll_to(ix);
            self.selection_changed(cx);
            cx.notify();
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection.select_all(self.rows.len());
        cx.notify();
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            self.clear_selection(cx);
        } else if self.is_searching() {
            self.end_search(window, cx);
        } else if self.picker.is_some() {
            crate::picker::cancel();
        }
    }

    // ----- opening ------------------------------------------------------------

    fn open_selected(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            self.choose(&Choose, window, cx);
            return;
        }
        if self.effective_view() == ViewMode::Columns {
            if let Some(entry) = self.column_selection() {
                self.open_entries(vec![entry], window, cx);
            }
            return;
        }
        let rows: Vec<usize> = self.selection.indices().collect();
        self.open_rows(rows, window, cx);
    }

    pub fn open_rows(&mut self, rows: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let entries: Vec<FileEntry> = rows
            .into_iter()
            .filter_map(|ix| self.rows.get(ix).map(|r| r.entry.clone()))
            .collect();
        if self.picker.is_some() {
            // Double-click a folder to browse it; anything else is the choice.
            if let [only] = entries.as_slice()
                && only.is_navigable()
            {
                self.navigate(Location::Folder(only.path.clone()), window, cx);
                return;
            }
            self.choose(&Choose, window, cx);
            return;
        }
        self.open_entries(entries, window, cx);
    }

    /// Folders navigate; files and apps open with the system default (or an
    /// editor, for text files no app claims).
    pub fn open_entries(
        &mut self,
        entries: Vec<FileEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.is_searching().then(|| self.query.clone());
        if let Some(folder) = entries.iter().find(|e| e.is_navigable()) {
            self.navigate(Location::Folder(folder.path.clone()), window, cx);
            return;
        }
        self.open_files(entries.iter().map(|e| e.path.clone()).collect(), window, cx);
        let engine = self.engine.clone();
        let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
        cx.background_spawn(async move {
            for path in paths {
                engine.track_open(&path, query.as_deref());
            }
        })
        .detach();
    }

    pub(super) fn choose(&mut self, _: &Choose, _: &mut Window, _: &mut Context<Self>) {
        let Some(picker) = self.picker else { return };
        let mut paths: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            return;
        }
        if !picker.multiple {
            paths.truncate(1);
        }
        crate::picker::choose(&paths);
    }

    // ----- view state -----------------------------------------------------------

    pub fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.config.view_mode == mode {
            return;
        }
        self.config.view_mode = mode;
        if mode == ViewMode::Columns {
            self.columns.clear();
            self.sync_root_column();
        }
        if let Some(ix) = self.selection.cursor() {
            self.scroll_to(ix);
        }
        self.select_first_for_gallery(cx);
        self.selection_changed(cx);
        self.schedule_save(cx);
        cx.notify();
    }

    pub fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        let current = self.sort_order();
        let order = if current.key == key {
            SortOrder {
                key,
                ascending: !current.ascending,
            }
        } else {
            SortOrder {
                key,
                ascending: !matches!(key, SortKey::Modified | SortKey::Size),
            }
        };
        if self.is_searching() {
            self.result_sort = order;
        } else {
            self.config.sort = order;
            self.schedule_save(cx);
        }
        self.rebuild_rows(cx);
    }

    pub fn set_ascending(&mut self, ascending: bool, cx: &mut Context<Self>) {
        if self.is_searching() {
            self.result_sort.ascending = ascending;
        } else {
            self.config.sort.ascending = ascending;
            self.schedule_save(cx);
        }
        self.rebuild_rows(cx);
    }

    pub fn current_sort(&self) -> SortOrder {
        self.sort_order()
    }

    fn toggle_hidden(&mut self, _: &ToggleHidden, _: &mut Window, cx: &mut Context<Self>) {
        self.set_show_hidden(!self.config.show_hidden, cx);
    }

    pub fn set_show_hidden(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.config.show_hidden == show {
            return;
        }
        self.config.show_hidden = show;
        self.schedule_save(cx);
        self.load_listing(Vec::new(), true, cx);
        self.reload_columns(cx);
        if self.is_searching() {
            self.run_search(false, cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.show_sidebar = !self.config.show_sidebar;
        self.schedule_save(cx);
        cx.notify();
    }

    pub fn set_icon_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.config.icon_size = size.clamp(32., 128.);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Change the preview text size; `None` goes back to the default.
    pub fn set_preview_font_size(&mut self, size: Option<f32>, cx: &mut Context<Self>) {
        use fffinder_core::config::{DEFAULT_PREVIEW_FONT_SIZE, FONT_SIZE_RANGE, in_range};
        let size = size.unwrap_or(DEFAULT_PREVIEW_FONT_SIZE);
        self.config.preview_font_size = in_range(size, FONT_SIZE_RANGE, DEFAULT_PREVIEW_FONT_SIZE);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Change the Nushell panel's text size; `None` goes back to the default.
    pub fn set_nu_font_size(&mut self, size: Option<f32>, cx: &mut Context<Self>) {
        use fffinder_core::config::{DEFAULT_NU_FONT_SIZE, FONT_SIZE_RANGE, in_range};
        let size = size.unwrap_or(DEFAULT_NU_FONT_SIZE);
        self.config.nu_font_size = in_range(size, FONT_SIZE_RANGE, DEFAULT_NU_FONT_SIZE);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Scale all text and controls (Settings › Text size); `None` resets it.
    pub fn set_ui_scale(&mut self, scale: Option<f32>, cx: &mut Context<Self>) {
        use fffinder_core::config::{UI_SCALE_RANGE, in_range};
        // Whole tenths, so steps land on 90%, 100%, 110%, …
        let scale = (in_range(scale.unwrap_or(1.0), UI_SCALE_RANGE, 1.0) * 10.).round() / 10.;
        self.config.ui_scale = scale;
        crate::theme::set_ui_scale(scale, cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Scale everything 10% bigger (`steps` > 0) or smaller.
    pub fn step_ui_scale(&mut self, steps: f32, cx: &mut Context<Self>) {
        let scale = self.config.ui_scale() + steps * 0.1;
        self.set_ui_scale(Some(scale), cx);
    }

    /// Make the Nushell panel's text one point bigger (`steps` > 0) or smaller.
    pub fn step_nu_font_size(&mut self, steps: f32, cx: &mut Context<Self>) {
        let size = self.config.nu_font_size() + steps;
        self.set_nu_font_size(Some(size), cx);
    }

    /// Make the preview text one point bigger (`steps` > 0) or smaller.
    pub fn step_preview_font_size(&mut self, steps: f32, cx: &mut Context<Self>) {
        let size = self.config.preview_font_size() + steps;
        self.set_preview_font_size(Some(size), cx);
    }

    fn toggle_appearance(
        &mut self,
        _: &ToggleAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use fffinder_core::Appearance;
        self.config.appearance = if cx.theme().is_dark() {
            Appearance::Light
        } else {
            Appearance::Dark
        };
        crate::theme::apply(self.config.appearance, Some(window), cx);
        self.schedule_save(cx);
    }

    pub fn set_appearance(
        &mut self,
        appearance: fffinder_core::Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.appearance = appearance;
        crate::theme::apply(appearance, Some(window), cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Persist settings shortly after the last change.
    pub fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !self.persist {
            return;
        }
        self.save_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let config = this.read_with(cx, |this, _| this.config.clone()).ok();
            if let Some(config) = config {
                cx.background_spawn(async move {
                    if let Err(err) = config.save() {
                        log::warn!("saving settings: {err}");
                    }
                })
                .await;
            }
        });
    }

    // ----- favorites ------------------------------------------------------------

    pub fn add_favorite(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.config.favorites.contains(&path) {
            self.favorites.push(places::favorite_for(&path));
            self.config.favorites.push(path);
            self.schedule_save(cx);
            cx.notify();
        }
    }

    pub fn remove_favorite(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.config.favorites.retain(|p| p != path);
        self.favorites.retain(|p| p.path != path);
        self.schedule_save(cx);
        cx.notify();
    }

    // ----- clipboard, reveal, trash, nushell ------------------------------------

    fn copy_path(&mut self, _: &CopyPath, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .selected_entries()
            .iter()
            .map(|e| e.path.display().to_string())
            .collect();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
        window.push_notification(
            if count == 1 {
                "Copied path".to_string()
            } else {
                format!("Copied {count} paths")
            },
            cx,
        );
    }

    fn copy_as_nuon(&mut self, _: &CopyAsNuon, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(nushell::to_nuon(&paths)));
        window.push_notification("Copied as a Nushell value", cx);
    }

    fn reveal(&mut self, _: &RevealInFileManager, _: &mut Window, cx: &mut Context<Self>) {
        for entry in self.selected_entries() {
            crate::system::reveal(&entry.path, cx);
        }
    }

    fn open_in_nushell(&mut self, _: &OpenInNushell, window: &mut Window, cx: &mut Context<Self>) {
        let dir = match self.primary_entry() {
            Some(e) if e.is_dir => Some(e.path),
            Some(e) => e.parent().map(Path::to_path_buf),
            None => self.current_folder().map(Path::to_path_buf),
        }
        .or_else(dirs::home_dir);
        if let Some(dir) = dir {
            self.open_terminal_at(&dir, window, cx);
        }
    }

    pub fn open_terminal_at(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.open_in_terminal(dir, InTerminal::Nu, window, cx);
    }

    /// Open the chosen terminal in `dir`, running `nu` or another program.
    pub(super) fn open_in_terminal(
        &mut self,
        dir: &Path,
        job: InTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::notification::NotificationType;
        let nu = cx.global::<AppState>().nu.clone();
        if matches!(job, InTerminal::Nu) && nu.is_none() {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "Nushell (nu) was not found. Set its path in Settings.",
                ),
                cx,
            );
            return;
        }
        let Some(found) = self.terminals.clone() else {
            // Still detecting: finish that, then open.
            let engine = self.engine.clone();
            let dir = dir.to_path_buf();
            self.terminals_task = cx.spawn_in(window, async move |this, cx| {
                let found = cx
                    .background_spawn(async move { terminals::detect(Some(&engine)) })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    this.terminals = Some(found);
                    this.open_in_terminal(&dir, job, window, cx);
                })
                .ok();
            });
            return;
        };
        let setting = self.config.terminal_setting();
        let nu = nu.unwrap_or_else(|| PathBuf::from("nu"));
        let custom = setting
            .command
            .as_deref()
            .filter(|_| setting.app.as_deref() == Some(CUSTOM))
            .and_then(|command| match &job {
                InTerminal::Nu => terminals::custom_plan(command, &nu, dir),
                InTerminal::Command { program, args } => {
                    terminals::custom_plan_command(command, program, args, dir)
                }
            });
        let env_terminal = std::env::var("TERMINAL").ok();
        let (plan, warning) = match custom {
            Some(plan) => (plan, None),
            None => match terminals::resolve(&found, &setting, env_terminal.as_deref()) {
                Some((terminal, warning)) => (
                    match &job {
                        InTerminal::Nu => terminal.plan(&nu, dir),
                        InTerminal::Command { program, args } => {
                            terminal.plan_command(program, args, dir)
                        }
                    },
                    warning,
                ),
                None => {
                    window.push_notification(
                        (
                            NotificationType::Warning,
                            "No terminal was found. Choose a command in Settings.",
                        ),
                        cx,
                    );
                    return;
                }
            },
        };
        if let Some(warning) = warning {
            window.push_notification((NotificationType::Warning, warning), cx);
        }
        let launched = crate::system::launch(plan, cx);
        cx.spawn_in(window, async move |_, cx| {
            if let Err(err) = launched.await {
                cx.update(|window, cx| {
                    window.push_notification(
                        (
                            NotificationType::Error,
                            format!("Could not open a terminal: {err}"),
                        ),
                        cx,
                    )
                })
                .ok();
            }
        })
        .detach();
    }

    /// Look for installed terminals in the background.
    pub fn detect_terminals(&mut self, cx: &mut Context<Self>) {
        if let Some(fixed) = cx.global::<AppState>().terminals.clone() {
            self.terminals = Some(fixed);
            cx.notify();
            return;
        }
        let engine = self.engine.clone();
        self.terminals_task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { terminals::detect(Some(&engine)) })
                .await;
            this.update(cx, |this, cx| {
                this.terminals = Some(found);
                cx.notify();
            })
            .ok();
        });
    }

    pub fn terminals(&self) -> Option<&[Terminal]> {
        self.terminals.as_deref()
    }

    /// Choose the terminal by id, [`CUSTOM`], or `None` for automatic.
    pub fn set_terminal_app(&mut self, app: Option<String>, cx: &mut Context<Self>) {
        let mut setting = self.config.terminal_setting();
        setting.app = app;
        self.config.set_terminal_setting(setting);
        self.schedule_save(cx);
        cx.notify();
    }

    pub fn set_terminal_command(&mut self, command: String, cx: &mut Context<Self>) {
        let mut setting = self.config.terminal_setting();
        setting.command = (!command.trim().is_empty()).then_some(command);
        self.config.set_terminal_setting(setting);
        self.schedule_save(cx);
        cx.notify();
    }

    /// The terminal setting for this system.
    pub fn terminal_setting(&self) -> TerminalSetting {
        self.config.terminal_setting()
    }

    pub(super) fn toggle_nu_panel(
        &mut self,
        _: &ToggleNuPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nu_open = !self.nu_open;
        if self.nu_open {
            self.nu_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.focus_files(window, cx);
        }
        cx.notify();
    }

    /// Read actions.nu (writing the starters first if it's missing).
    pub fn load_user_actions(&mut self, cx: &mut Context<Self>) {
        let Some(nu) = cx.global::<AppState>().nu.clone() else {
            return;
        };
        let path = crate::user_actions::actions_path();
        self.user_actions_task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::user_actions::ensure_starters(&path).map_err(|e| e.to_string())?;
                    let stamp = crate::user_actions::modified(&path);
                    crate::user_actions::discover(&nu, &path).map(|found| (found, stamp))
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((found, stamp)) => {
                        this.user_actions = found;
                        this.user_actions_error = None;
                        this.user_actions_stamp = stamp;
                    }
                    Err(err) => {
                        this.user_actions_error = Some(err);
                        this.user_actions_stamp =
                            crate::user_actions::modified(&crate::user_actions::actions_path());
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    pub fn user_actions(&self) -> &[crate::user_actions::UserAction] {
        &self.user_actions
    }

    pub fn user_actions_error(&self) -> Option<&str> {
        self.user_actions_error.as_deref()
    }

    /// Run an action with the selection (or everything shown) as `$in`; its
    /// result shows in the Nushell panel.
    pub fn run_user_action(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.user_actions.get(ix).cloned() else {
            return;
        };
        let text = crate::user_actions::invocation(&crate::user_actions::actions_path(), &action);
        self.open_nu_with(&text, window, cx);
    }

    /// Open actions.nu to edit it, in the user's editor when that's on.
    pub fn edit_user_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = crate::user_actions::actions_path();
        let _ = crate::user_actions::ensure_starters(&path);
        self.open_in_editor(vec![path], window, cx);
    }

    /// Open the pipeline panel with `pipeline` filled in and run it.
    pub fn open_nu_with(&mut self, pipeline: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.nu_open = true;
        let pipeline = pipeline.to_string();
        self.nu_input
            .update(cx, |s, cx| s.set_value(pipeline, window, cx));
        self.run_nu_pipeline(window, cx);
    }

    /// Empty the pipeline and drop its result (or stop waiting for one), so
    /// the panel shows its examples again.
    pub fn clear_nu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.nu_task = Task::ready(());
        self.nu_running = false;
        self.nu_result = None;
        self.nu_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// The pipeline in the Nushell panel's field.
    pub fn nu_pipeline(&self, cx: &App) -> String {
        self.nu_input.read(cx).value().to_string()
    }

    pub fn nu_busy(&self) -> bool {
        self.nu_running
    }

    /// Stop writing settings to disk; used by the screenshot tool.
    pub fn set_persist(&mut self, persist: bool) {
        self.persist = persist;
    }

    pub fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.config.show_sidebar = visible;
        cx.notify();
    }

    pub fn run_nu_pipeline(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let pipeline = self.nu_input.read(cx).value().trim().to_string();
        if pipeline.is_empty() {
            return;
        }
        let Some(nu) = cx.global::<AppState>().nu.clone() else {
            self.nu_result = Some(NuRun {
                output: nushell::NuOutput::Empty,
                error: Some(
                    "Nushell (nu) was not found on PATH. Set its location in Settings.".into(),
                ),
                elapsed: Duration::ZERO,
            });
            cx.notify();
            return;
        };
        // The selection, or everything shown when nothing is selected.
        let mut input = self.selected_entries();
        if input.is_empty() {
            input = self.rows.iter().map(|r| r.entry.clone()).collect();
        }
        let cwd = self
            .current_folder()
            .map(Path::to_path_buf)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        self.nu_running = true;
        cx.notify();
        self.nu_task = cx.spawn(async move |this, cx| {
            let run = cx
                .background_spawn(
                    async move { nushell::run_pipeline(&nu, &pipeline, &input, &cwd) },
                )
                .await;
            this.update(cx, |this, cx| {
                this.nu_running = false;
                this.nu_result = Some(run);
                cx.notify();
            })
            .ok();
        });
    }

    fn new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let mut name = "untitled folder".to_string();
        let mut n = 2;
        while dir.join(&name).exists() {
            name = format!("untitled folder {n}");
            n += 1;
        }
        let path = dir.join(&name);
        match std::fs::create_dir(&path) {
            Ok(()) => {
                crate::undo::record(
                    "New Folder",
                    crate::undo::Change::Created(vec![path.clone()]),
                    cx,
                );
                self.end_search(window, cx);
                self.load_listing(vec![path.clone()], true, cx);
                self.open_rename_dialog(path, window, cx);
            }
            Err(err) => window.push_notification(
                (
                    gpui_kit::component::notification::NotificationType::Error,
                    format!("Could not create a folder: {err}"),
                ),
                cx,
            ),
        }
    }

    fn rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected_entries();
        if selected.len() > 1 {
            self.open_batch_rename(selected, window, cx);
        } else if let Some(entry) = self.primary_entry() {
            self.open_rename_dialog(entry.path, window, cx);
        }
    }

    // ----- columns ----------------------------------------------------------------

    /// Column 0 mirrors the current folder's rows.
    fn sync_root_column(&mut self) {
        let Some(dir) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let entries: Vec<FileEntry> = self.rows.iter().map(|r| r.entry.clone()).collect();
        match self.columns.first_mut() {
            Some(first) if first.dir == dir => {
                let selected_path = first
                    .selected
                    .and_then(|ix| first.entries.get(ix))
                    .map(|e| e.path.clone());
                first.selected =
                    selected_path.and_then(|p| entries.iter().position(|e| e.path == p));
                first.entries = entries;
            }
            _ => {
                self.columns = vec![ColumnPane {
                    dir,
                    entries,
                    selected: None,
                    scroll: UniformListScrollHandle::new(),
                }];
                self.active_column = 0;
            }
        }
    }

    pub fn column_selection(&self) -> Option<FileEntry> {
        let col = self.columns.get(self.active_column)?;
        col.selected.and_then(|ix| col.entries.get(ix)).cloned()
    }

    /// Select `ix` in column `col`; folders open a new column to the right.
    pub fn select_in_column(
        &mut self,
        col: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_files(window, cx);
        let Some(pane) = self.columns.get_mut(col) else {
            return;
        };
        let Some(entry) = pane.entries.get(ix).cloned() else {
            return;
        };
        pane.selected = Some(ix);
        pane.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        self.active_column = col;
        self.columns.truncate(col + 1);
        if col == 0 {
            self.selection.select_only(ix);
        }
        if entry.is_navigable() {
            self.columns.push(ColumnPane {
                dir: entry.path.clone(),
                entries: Vec::new(),
                selected: None,
                scroll: UniformListScrollHandle::new(),
            });
            self.load_column(col + 1, entry.path.clone(), cx);
        } else {
            self.ensure_preview(&entry, cx);
        }
        let columns = self.columns.len();
        self.columns_scroll
            .scroll_to_item(columns.saturating_sub(1));
        cx.notify();
    }

    fn load_column(&mut self, col: usize, dir: PathBuf, cx: &mut Context<Self>) {
        let show_hidden = self.config.show_hidden;
        let sort = self.config.sort;
        cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn({
                    let dir = dir.clone();
                    async move {
                        let mut entries = listing::list_dir(&dir, show_hidden).unwrap_or_default();
                        listing::sort_entries(&mut entries, sort);
                        entries
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(pane) = this.columns.get_mut(col)
                    && pane.dir == dir
                {
                    pane.entries = entries;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn move_in_columns(
        &mut self,
        dx: isize,
        dy: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let col = self.active_column;
        let Some(pane) = self.columns.get(col) else {
            return;
        };
        if dy != 0 {
            let len = pane.entries.len();
            if len == 0 {
                return;
            }
            let next = match pane.selected {
                Some(cur) => (cur as isize + dy).clamp(0, len as isize - 1) as usize,
                None => 0,
            };
            self.select_in_column(col, next, window, cx);
        } else if dx > 0 {
            if let Some(next) = self.columns.get(col + 1)
                && !next.entries.is_empty()
            {
                self.select_in_column(col + 1, 0, window, cx);
            }
        } else if dx < 0 && col > 0 {
            self.active_column = col - 1;
            self.columns.truncate(col + 1);
            if let Some(next) = self.columns.get_mut(col) {
                next.selected = None;
            }
            cx.notify();
        }
    }
}

fn dir_stamp(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

/// Content search: one row per matching file, showing the first matching line.
fn content_search(
    engine: &Engine,
    query: &str,
    scope: Option<PathBuf>,
    limit: usize,
) -> (Vec<Row>, SearchStats) {
    let response = engine.grep(&GrepRequest {
        patterns: vec![query.to_string()],
        scope,
        limit: limit.max(50) * 4,
        per_file: 20,
        trim: true,
        ..Default::default()
    });
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for hit in response.hits {
        if !seen.insert(hit.path.clone()) {
            continue;
        }
        if let Some(entry) = FileEntry::from_path(&hit.path) {
            let line = format!("{}: {}", hit.line, hit.text.trim());
            rows.push(Row::new(entry, Vec::new(), Some(line)));
        }
        if rows.len() >= limit {
            break;
        }
    }
    let stats = SearchStats {
        elapsed: response.elapsed,
        total_matched: response.files_with_matches,
        indexing: response.indexing,
        stopped: response.stopped,
    };
    (rows, stats)
}

impl Render for Finder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (background, foreground, font) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.foreground,
                theme.font_family.clone(),
            )
        };
        let show_sidebar = self.config.show_sidebar;
        div()
            .id("finder")
            .key_context(FINDER)
            .track_focus(&self.focus)
            .size_full()
            .bg(background)
            .text_color(foreground)
            .text_size(ui(13.))
            .font_family(font)
            // Open, Quick Look and Select All are bound to keys only inside the
            // file views, but menus can send them from anywhere in the window.
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::quick_look))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::go_up))
            .on_action(cx.listener(Self::go_home))
            .on_action(cx.listener(Self::go_recents))
            .on_action(cx.listener(Self::go_trash))
            .on_action(cx.listener(Self::undo_file_operation))
            .on_action(cx.listener(Self::put_back))
            .on_action(cx.listener(Self::delete_immediately))
            .on_action(cx.listener(Self::empty_trash))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::toggle_hidden))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_appearance))
            .on_action(cx.listener(Self::copy_path))
            .on_action(cx.listener(Self::copy_files))
            .on_action(cx.listener(Self::paste_files))
            .on_action(cx.listener(Self::move_items_here))
            .on_action(cx.listener(Self::copy_as_nuon))
            .on_action(cx.listener(Self::reveal))
            .on_action(cx.listener(Self::open_in_nushell))
            .on_action(cx.listener(Self::toggle_nu_panel))
            .on_action(cx.listener(Self::new_folder))
            .on_action(cx.listener(Self::rename))
            .on_action(cx.listener(Self::move_to_trash))
            .on_action(cx.listener(Self::rescan))
            .on_action(cx.listener(Self::choose))
            .on_action(cx.listener(Self::show_settings))
            .on_action(cx.listener(Self::go_to_folder))
            .on_action(cx.listener(Self::search_help))
            .on_action(cx.listener(Self::toggle_filter))
            .on_action(cx.listener(|this, _: &OpenInEditor, window, cx| {
                let paths = this
                    .selected_entries()
                    .into_iter()
                    .map(|e| e.path)
                    .collect();
                this.open_in_editor(paths, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, _| this.close_window(window)))
            .on_action(
                cx.listener(|this, _: &ViewIcons, _, cx| this.set_view_mode(ViewMode::Icons, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ViewList, _, cx| this.set_view_mode(ViewMode::List, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ViewColumns, _, cx| {
                    this.set_view_mode(ViewMode::Columns, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ViewGallery, _, cx| {
                    this.set_view_mode(ViewMode::Gallery, cx)
                }),
            )
            .child(
                h_flex()
                    .size_full()
                    .items_start()
                    .when(show_sidebar, |this| {
                        this.child(self.render_sidebar(window, cx))
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .child(self.render_toolbar(window, cx))
                            .when(self.in_trash(), |this| {
                                this.child(self.render_trash_bar(cx))
                            })
                            .when(self.is_searching(), |this| {
                                this.child(self.render_scope_bar(window, cx))
                            })
                            .when(self.filter_bar_open(), |this| {
                                this.child(self.render_filter_bar(cx))
                            })
                            .child(self.render_content(window, cx))
                            .when(self.nu_open, |this| {
                                this.child(self.render_nu_panel(window, cx))
                            })
                            .when(self.transfers_running(), |this| {
                                this.child(self.render_transfers(cx))
                            })
                            .child(self.render_path_bar(window, cx))
                            .child(self.render_status_bar(window, cx)),
                    ),
            )
    }
}

impl Finder {
    /// The file area: key context for arrows/Space/Return, plus the active view.
    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.effective_view();
        let body = if let Some(err) = self.listing_error.clone().filter(|_| !self.is_searching()) {
            let title = match self.history.current() {
                Location::Trash => "Can’t show the Trash",
                _ => "Can’t open this folder",
            };
            self.render_message(title, &err, cx).into_any_element()
        } else if self.rows.is_empty() && !self.loading && !self.searching {
            let (title, detail) = if self.is_searching() {
                match self.indexing_progress() {
                    Some(n) => (
                        "Indexing…".to_string(),
                        format!(
                            "{} files indexed so far. Results appear when indexing finishes.",
                            group_digits(n)
                        ),
                    ),
                    None => (
                        "No Results".to_string(),
                        "Try a different name, kind, or search location.".to_string(),
                    ),
                }
            } else if matches!(self.history.current(), Location::Recents) {
                (
                    "No Recent Files".into(),
                    "Files you change appear here.".into(),
                )
            } else if matches!(self.history.current(), Location::Trash) {
                (
                    "The Trash Is Empty".into(),
                    "Items you move to the Trash appear here.".into(),
                )
            } else {
                ("No Items".into(), "This folder is empty.".into())
            };
            self.render_message(&title, &detail, cx).into_any_element()
        } else if view == ViewMode::Columns {
            self.render_columns(window, cx).into_any_element()
        } else if self.rows.is_empty() {
            self.render_skeleton(view, cx).into_any_element()
        } else {
            match view {
                ViewMode::Icons => self.render_icons(window, cx).into_any_element(),
                ViewMode::List => self.render_list(window, cx).into_any_element(),
                ViewMode::Gallery => self.render_gallery(window, cx).into_any_element(),
                ViewMode::Columns => unreachable!(),
            }
        };
        let entity = cx.entity();
        div()
            .id("content")
            .test_support()
            .key_context(FILES)
            .track_focus(&self.files_focus)
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(|this, _: &MoveUp, window, cx| {
                this.move_selection(0, -1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveDown, window, cx| {
                this.move_selection(0, 1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLeft, window, cx| {
                this.move_selection(-1, 0, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveRight, window, cx| {
                this.move_selection(1, 0, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendUp, window, cx| {
                this.move_selection(0, -1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendDown, window, cx| {
                this.move_selection(0, 1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendLeft, window, cx| {
                this.move_selection(-1, 0, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendRight, window, cx| {
                this.move_selection(1, 0, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveHome, _, cx| {
                if !this.rows.is_empty() {
                    this.selection.select_only(0);
                    this.scroll_to(0);
                    this.selection_changed(cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &MoveEnd, _, cx| {
                if let Some(last) = this.rows.len().checked_sub(1) {
                    this.selection.select_only(last);
                    this.scroll_to(last);
                    this.selection_changed(cx);
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keys = &event.keystroke;
                let m = keys.modifiers;
                if m.control || m.platform || m.alt || m.function {
                    return;
                }
                if let Some(text) = keys.key_char.as_deref()
                    && !text.is_empty()
                    && !text.chars().any(char::is_control)
                    && (text != " " || !this.type_select.is_empty())
                {
                    this.type_to_select(text, window, cx);
                    cx.stop_propagation();
                }
            }))
            // Files from other apps are copied into the folder; with no
            // folder showing, a dropped folder opens instead.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(dest) = this.drop_folder() {
                    this.drop_files(paths.paths().to_vec(), dest, false, window, cx);
                } else if let Some(path) = paths.paths().first() {
                    if path.is_dir() {
                        this.navigate(Location::Folder(path.clone()), window, cx);
                    } else {
                        this.reveal_in_place(&path.clone(), window, cx);
                    }
                }
            }))
            .on_drop(
                cx.listener(|this, files: &views::DraggedFiles, window, cx| {
                    if let Some(dest) = this.drop_folder() {
                        this.drop_files(files.0.clone(), dest, true, window, cx);
                    }
                }),
            )
            .on_prepaint(move |bounds, _, cx| {
                entity.update(cx, |this, _| this.content_height = bounds.size.height);
            })
            .child(body)
    }

    /// The folder that files dropped on the view go into: the one shown,
    /// when it isn't search results.
    fn drop_folder(&self) -> Option<PathBuf> {
        self.current_folder()
            .filter(|_| !self.is_searching())
            .map(Path::to_path_buf)
    }

    fn render_message(&self, title: &str, detail: &str, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_size(ui(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .max_w(ui(420.))
                    .text_center()
                    .text_size(ui(12.))
                    .text_color(theme.muted_foreground)
                    .child(detail.to_string()),
            )
    }
}

/// 1234567 → "1,234,567".
pub fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
