//! Explore mode: Quick Look on a structured file as data, the way Nushell's
//! `explore` shows it. Nushell opens the file and it's shown one level at a
//! time: a record as its fields, a table as its rows, a list as its items. A
//! click on a nested value, or Return, goes into it; Escape, or a name in the
//! path above the grid, comes back out.

use super::{AppState, Finder};
use crate::actions::{QUICK_LOOK, QuickLook, QuickLookExplore};
use crate::nushell::{self, Opened, plural};
use crate::theme::{palette, ui};
use fffinder_core::FileEntry;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value as Json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// How Quick Look shows a structured file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuickLookMode {
    /// Its text, colored. Space opens Quick Look this way.
    #[default]
    Text,
    /// Its data, drilled into level by level. Cmd-Space opens it this way.
    Explore,
}

/// The most of a cell's text that is drawn; a long string is cut off anyway.
const CELL_CHARS: usize = 200;
/// Columns are as wide as their heading and this many rows' cells need...
const MEASURED_ROWS: usize = 200;
/// ...in characters, within these bounds.
const COLUMN_CHARS: (usize, usize) = (3, 40);
/// A monospaced character's width, as a share of the text size.
const CHAR_WIDTH: f32 = 0.6;

/// What the shown value is, which decides how its grid is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A row per field: its name, then its value.
    Record,
    /// A row per record, a column per field the records have.
    Table,
    /// A row per item, numbered.
    List,
    /// A single value, shown whole.
    Scalar,
}

/// One step from a value into a part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// A record's field.
    Key(String),
    /// A list's item, counting from 0.
    Nth(usize),
}

/// A cell in the grid; `col` counts the columns after the lead one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Cursor {
    row: usize,
    col: usize,
}

/// A level explore mode went into from the one above it.
#[derive(Debug, Clone, PartialEq)]
struct Crumb {
    /// From the value above to this one: a table cell takes two.
    steps: Vec<Step>,
    /// Its name in the path above the grid.
    label: SharedString,
    /// The level above, and where its cursor was, to go back to.
    parent: Arc<Level>,
    from: Cursor,
}

/// A grid column's heading and how many characters wide it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Column {
    title: SharedString,
    chars: usize,
}

impl Column {
    /// A column as wide as `title` and the widest of `cells`, in characters.
    fn fitted(title: SharedString, cells: impl IntoIterator<Item = usize>) -> Column {
        let widest = cells.into_iter().fold(title.chars().count(), usize::max);
        Column {
            title,
            chars: widest.clamp(COLUMN_CHARS.0, COLUMN_CHARS.1),
        }
    }
}

/// How the shown value is laid out, worked out once each time it changes.
#[derive(Debug, Clone, PartialEq)]
struct Level {
    shape: Shape,
    rows: usize,
    /// A record's field names, or the row numbers of a table or list.
    lead: Column,
    /// A table's columns, or a single `value` column.
    columns: Vec<Column>,
}

impl Level {
    fn of(value: &Json) -> Level {
        let numbers = |rows: usize| Column::fitted("#".into(), [digits(rows)]);
        let values = |cells: &mut dyn Iterator<Item = &Json>| {
            vec![Column::fitted(
                "value".into(),
                cells.take(MEASURED_ROWS).map(cell_chars),
            )]
        };
        match value {
            Json::Object(map) => Level {
                shape: Shape::Record,
                rows: map.len(),
                lead: Column::fitted(
                    "key".into(),
                    map.keys().take(MEASURED_ROWS).map(|k| k.chars().count()),
                ),
                columns: values(&mut map.values()),
            },
            Json::Array(items) if nushell::is_table(items) => {
                // Every field any row has, in the order they first appear.
                let mut seen = HashSet::new();
                let keys: Vec<&str> = items
                    .iter()
                    .filter_map(Json::as_object)
                    .flat_map(|row| row.keys())
                    .map(String::as_str)
                    .filter(|key| seen.insert(*key))
                    .collect();
                let columns = keys
                    .iter()
                    .map(|key| {
                        let cells = items.iter().take(MEASURED_ROWS);
                        Column::fitted(
                            key.to_string().into(),
                            cells.filter_map(|row| row.get(key)).map(cell_chars),
                        )
                    })
                    .collect();
                Level {
                    shape: Shape::Table,
                    rows: items.len(),
                    lead: numbers(items.len()),
                    columns,
                }
            }
            Json::Array(items) => Level {
                shape: Shape::List,
                rows: items.len(),
                lead: numbers(items.len()),
                columns: values(&mut items.iter()),
            },
            _ => Level {
                shape: Shape::Scalar,
                rows: 0,
                lead: Column::fitted(SharedString::default(), []),
                columns: Vec::new(),
            },
        }
    }

    /// The value in `row` and `col` of `shown`, the value this level is of.
    fn cell<'a>(&self, shown: &'a Json, row: usize, col: usize) -> Option<&'a Json> {
        match (self.shape, shown) {
            (Shape::Record, Json::Object(map)) => map.values().nth(row),
            (Shape::List, Json::Array(items)) => items.get(row),
            (Shape::Table, Json::Array(items)) => {
                items.get(row)?.get(self.columns.get(col)?.title.as_ref())
            }
            _ => None,
        }
    }

    /// The steps into `row` of `shown`, and their name in the path: the
    /// whole row for `None`, or else the cell in column `col`.
    fn steps_to(
        &self,
        shown: &Json,
        row: usize,
        col: Option<usize>,
    ) -> Option<(Vec<Step>, String)> {
        match (self.shape, shown, col) {
            (Shape::Record, Json::Object(map), _) => {
                let key = map.keys().nth(row)?;
                Some((vec![Step::Key(key.clone())], key.clone()))
            }
            (Shape::List, Json::Array(_), _) | (Shape::Table, Json::Array(_), None) => {
                Some((vec![Step::Nth(row)], row.to_string()))
            }
            (Shape::Table, Json::Array(_), Some(col)) => {
                let key = self.columns.get(col)?.title.to_string();
                let label = format!("{row}.{key}");
                Some((vec![Step::Nth(row), Step::Key(key)], label))
            }
            _ => None,
        }
    }
}

/// How many digits the last row number of `rows` rows has.
fn digits(rows: usize) -> usize {
    rows.saturating_sub(1)
        .checked_ilog10()
        .map_or(1, |log| log as usize + 1)
}

/// Whether there is more to see inside `value`: a record or list that
/// isn't empty.
fn is_nested(value: &Json) -> bool {
    match value {
        Json::Object(map) => !map.is_empty(),
        Json::Array(items) => !items.is_empty(),
        Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => false,
    }
}

/// Whether going into `value` shows anything: a nested value, or a single
/// one to see whole.
fn can_enter(value: &Json) -> bool {
    match value {
        Json::Object(_) | Json::Array(_) => is_nested(value),
        Json::Bool(_) | Json::Number(_) | Json::String(_) => true,
        Json::Null => false,
    }
}

/// A cell's text on one line, no longer than is drawn.
fn cell_text(value: &Json) -> SharedString {
    let one_line = |text: &str| -> SharedString {
        text.chars()
            .take(CELL_CHARS)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .into()
    };
    match value {
        Json::String(text) => one_line(text),
        other => one_line(&nushell::cell(other)),
    }
}

/// How many characters [`cell_text`] gives for `value`.
fn cell_chars(value: &Json) -> usize {
    match value {
        Json::String(text) => text.chars().take(CELL_CHARS).count(),
        // Numbers, booleans and summaries are ASCII.
        other => nushell::cell(other).len(),
    }
}

/// The value `crumbs` lead to from `root`.
fn resolve<'a>(root: &'a Json, crumbs: &[Crumb]) -> &'a Json {
    crumbs
        .iter()
        .flat_map(|crumb| &crumb.steps)
        .try_fold(root, |value, step| match (value, step) {
            (Json::Object(map), Step::Key(key)) => map.get(key),
            (Json::Array(items), Step::Nth(ix)) => items.get(*ix),
            _ => None,
        })
        .unwrap_or(root)
}

/// A structured file's data, and where in it explore mode is.
#[derive(Debug, Clone)]
pub(super) struct Explorer {
    root: Arc<Json>,
    /// How many rows the table or list at the top had before it was cut short.
    rows: Option<usize>,
    crumbs: Vec<Crumb>,
    level: Arc<Level>,
    cursor: Cursor,
}

impl Explorer {
    pub(super) fn new(opened: Opened) -> Explorer {
        Explorer {
            level: Arc::new(Level::of(&opened.data)),
            root: Arc::new(opened.data),
            rows: opened.rows,
            crumbs: Vec::new(),
            cursor: Cursor::default(),
        }
    }

    fn shown(&self) -> &Json {
        resolve(&self.root, &self.crumbs)
    }

    /// Put the cursor on `row` and `col`, within the grid.
    fn move_to(&mut self, row: usize, col: usize) {
        self.cursor = Cursor {
            row: row.min(self.level.rows.saturating_sub(1)),
            col: col.min(self.level.columns.len().saturating_sub(1)),
        };
    }

    /// Move the cursor `rows` down and `cols` right, within the grid.
    /// Returns whether it moved.
    fn move_by(&mut self, rows: isize, cols: isize) -> bool {
        let before = self.cursor;
        self.move_to(
            before.row.saturating_add_signed(rows),
            before.col.saturating_add_signed(cols),
        );
        self.cursor != before
    }

    /// Go into the value under the cursor, or with `whole_row` into the
    /// cursor's row of a table. Returns whether there was anything to see.
    fn enter(&mut self, whole_row: bool) -> bool {
        let Cursor { row, col } = self.cursor;
        let shown = self.shown();
        let target = if whole_row && self.level.shape == Shape::Table {
            shown.get(row)
        } else {
            self.level.cell(shown, row, col)
        };
        let Some(target) = target.filter(|value| can_enter(value)) else {
            return false;
        };
        let column = (!whole_row).then_some(col);
        let Some((steps, label)) = self.level.steps_to(shown, row, column) else {
            return false;
        };
        let level = Arc::new(Level::of(target));
        let parent = std::mem::replace(&mut self.level, level);
        self.crumbs.push(Crumb {
            steps,
            label: label.into(),
            parent,
            from: self.cursor,
        });
        self.cursor = Cursor::default();
        true
    }

    /// Go back out to `depth` levels in, 0 being the top, with the cursor
    /// where it was there. Returns whether that's out of this level.
    fn back_to(&mut self, depth: usize) -> bool {
        let Some(crumb) = self.crumbs.get(depth).cloned() else {
            return false;
        };
        self.crumbs.truncate(depth);
        self.level = crumb.parent;
        self.cursor = crumb.from;
        true
    }

    /// What the shown value is and how big, for the line above the grid.
    fn summary(&self) -> String {
        let n = self.level.rows;
        let cut = self.rows.filter(|&all| self.crumbs.is_empty() && all > n);
        let counted = |noun: &str| match cut {
            Some(all) => format!("first {n} of {all} {noun}s"),
            None => format!("{n} {noun}{}", plural(n)),
        };
        match self.level.shape {
            Shape::Record => format!("record · {}", counted("field")),
            Shape::Table => {
                let cols = self.level.columns.len();
                format!("table · {} · {cols} column{}", counted("row"), plural(cols))
            }
            Shape::List => format!("list · {}", counted("item")),
            Shape::Scalar => match self.shown() {
                Json::String(text) => {
                    let chars = text.chars().count();
                    format!("string · {chars} character{}", plural(chars))
                }
                Json::Number(_) => "number".into(),
                Json::Bool(_) => "bool".into(),
                Json::Null | Json::Array(_) | Json::Object(_) => "nothing".into(),
            },
        }
    }
}

/// Explore mode's state for the file Quick Look shows.
pub(super) struct Explore {
    path: PathBuf,
    state: ExploreState,
    rows_scroll: UniformListScrollHandle,
    sideways: ScrollHandle,
}

enum ExploreState {
    /// Nushell is opening the file; dropping this stops waiting for it.
    Loading {
        _task: Task<()>,
    },
    Ready(Explorer),
    Failed(SharedString),
}

/// A column's width at `size` points: its characters and the padding.
fn column_width(chars: usize, size: f32) -> Pixels {
    ui(chars as f32 * CHAR_WIDTH * size + 16.)
}

impl Explore {
    /// Scroll the cursor's cell into view, `size` being the text size.
    fn reveal_cursor(&self, size: f32, strategy: ScrollStrategy) {
        let ExploreState::Ready(explorer) = &self.state else {
            return;
        };
        let (level, cursor) = (&explorer.level, explorer.cursor);
        self.rows_scroll.scroll_to_item(cursor.row, strategy);
        let Some(column) = level.columns.get(cursor.col) else {
            return;
        };
        let start = level.columns[..cursor.col]
            .iter()
            .fold(column_width(level.lead.chars, size), |x, c| {
                x + column_width(c.chars, size)
            });
        let end = start + column_width(column.chars, size);
        let offset = self.sideways.offset();
        let (left, room) = (-offset.x, self.sideways.bounds().size.width);
        if start < left {
            self.sideways.set_offset(point(-start, offset.y));
        } else if end > left + room && room > Pixels::ZERO {
            self.sideways.set_offset(point(room - end, offset.y));
        }
    }

    /// Back to the top left, for a level just gone into.
    fn scroll_to_start(&self) {
        self.rows_scroll.scroll_to_item(0, ScrollStrategy::Top);
        let offset = self.sideways.offset();
        self.sideways.set_offset(point(Pixels::ZERO, offset.y));
    }
}

impl Finder {
    /// How Quick Look shows a structured file.
    pub fn quick_look_mode(&self) -> QuickLookMode {
        self.quick_look_mode
    }

    /// Show structured files in Quick Look in `mode`.
    pub(super) fn set_quick_look_mode(&mut self, mode: QuickLookMode, cx: &mut Context<Self>) {
        self.quick_look_mode = mode;
        self.ensure_explore(cx);
        cx.notify();
    }

    /// Whether `entry` is a file explore mode can open.
    pub(super) fn can_explore(entry: &FileEntry, cx: &App) -> bool {
        !entry.is_dir
            && entry.size.unwrap_or(0) < nushell::OPEN_LIMIT
            && nushell::is_structured(&entry.path)
            && cx.global::<AppState>().nu.is_some()
    }

    /// Whether Quick Look shows `entry` in explore mode.
    pub(super) fn explores(&self, entry: &FileEntry, cx: &App) -> bool {
        self.quick_look_mode == QuickLookMode::Explore && Finder::can_explore(entry, cx)
    }

    /// Start opening the selected file for explore mode, when Quick Look is
    /// open in that mode, unless it is already open.
    pub(super) fn ensure_explore(&mut self, cx: &mut Context<Self>) {
        if !self.quick_look_open || self.quick_look_mode != QuickLookMode::Explore {
            return;
        }
        let Some(entry) = self.primary_entry() else {
            return;
        };
        if self.explore.as_ref().is_some_and(|e| e.path == entry.path) {
            return;
        }
        let Some(nu) = cx.global::<AppState>().nu.clone() else {
            return;
        };
        if !Finder::can_explore(&entry, cx) {
            self.explore = None;
            return;
        }
        let path = entry.path;
        let task = cx.spawn({
            let path = path.clone();
            async move |this, cx| {
                let opened = cx
                    .background_spawn({
                        let path = path.clone();
                        async move { nushell::explore_file(&nu, &path).map(Explorer::new) }
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(explore) = this.explore.as_mut().filter(|e| e.path == path) {
                        explore.state = match opened {
                            Ok(explorer) => ExploreState::Ready(explorer),
                            Err(err) => ExploreState::Failed(err.into()),
                        };
                        cx.notify();
                    }
                })
                .ok();
                // Quick Look in `--term` mode is drawn by the window's root.
                cx.refresh();
            }
        });
        self.explore = Some(Explore {
            path,
            state: ExploreState::Loading { _task: task },
            rows_scroll: UniformListScrollHandle::new(),
            sideways: ScrollHandle::new(),
        });
    }

    /// Explore mode's state for the item Quick Look shows, while it shows
    /// it that way.
    fn shown_explore(&mut self) -> Option<&mut Explore> {
        if !self.quick_look_open || self.quick_look_mode != QuickLookMode::Explore {
            return None;
        }
        let path = self.primary_entry()?.path;
        self.explore.as_mut().filter(|e| e.path == path)
    }

    /// The arrow keys in explore mode: move the cursor by `dx` columns and
    /// `dy` rows. False when there's no grid, so they move through the items.
    pub(super) fn explore_move(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) -> bool {
        let size = self.config.preview_font_size();
        let Some(explore) = self.shown_explore() else {
            return false;
        };
        match &mut explore.state {
            ExploreState::Failed(_) => false,
            ExploreState::Loading { .. } => true,
            ExploreState::Ready(explorer) => {
                if explorer.move_by(dy, dx) {
                    explore.reveal_cursor(size, ScrollStrategy::Nearest);
                    cx.notify();
                }
                true
            }
        }
    }

    /// Return: go into the value under the cursor.
    pub(super) fn explore_into(&mut self, cx: &mut Context<Self>) {
        if let Some(explore) = self.shown_explore()
            && let ExploreState::Ready(explorer) = &mut explore.state
            && explorer.enter(false)
        {
            explore.scroll_to_start();
            cx.notify();
        }
    }

    /// Escape: back out of the value explore mode is in. False at the top,
    /// where Escape closes Quick Look instead.
    pub(super) fn explore_back(&mut self, cx: &mut Context<Self>) -> bool {
        let depth = match self.shown_explore().map(|e| &e.state) {
            Some(ExploreState::Ready(explorer)) if !explorer.crumbs.is_empty() => {
                explorer.crumbs.len() - 1
            }
            _ => return false,
        };
        self.explore_back_to(depth, cx);
        true
    }

    /// Back out to `depth` levels in, from the path above the grid.
    fn explore_back_to(&mut self, depth: usize, cx: &mut Context<Self>) {
        let size = self.config.preview_font_size();
        if let Some(explore) = self.shown_explore()
            && let ExploreState::Ready(explorer) = &mut explore.state
            && explorer.back_to(depth)
        {
            explore.reveal_cursor(size, ScrollStrategy::Center);
            cx.notify();
        }
    }

    /// A click in the grid: the cell in `row` and `col` (`None` for the
    /// lead column) gets the cursor. A nested value is gone into, and a
    /// double click goes into anything, a table's whole row from its number.
    fn explore_click(
        &mut self,
        row: usize,
        col: Option<usize>,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(explore) = self.shown_explore() else {
            return;
        };
        let ExploreState::Ready(explorer) = &mut explore.state else {
            return;
        };
        explorer.move_to(row, col.unwrap_or(explorer.cursor.col));
        let nested = col.is_some()
            && explorer
                .level
                .cell(explorer.shown(), explorer.cursor.row, explorer.cursor.col)
                .is_some_and(is_nested);
        let whole_row = col.is_none() && explorer.level.shape == Shape::Table;
        if (nested || clicks >= 2) && explorer.enter(whole_row) {
            explore.scroll_to_start();
        }
        cx.notify();
    }
}

/// Explore mode's view of `entry`: the path into its data, and the grid.
pub(super) fn render_explore(
    finder: &Finder,
    view: &Entity<Finder>,
    entry: &FileEntry,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let size = finder.config.preview_font_size();
    let explore = finder.explore.as_ref().filter(|e| e.path == entry.path);
    let explorer = match explore.map(|e| &e.state) {
        Some(ExploreState::Ready(explorer)) => Some(explorer),
        _ => None,
    };
    let body = match (explore, explorer) {
        (Some(explore), Some(explorer)) if explorer.level.shape == Shape::Scalar => {
            render_scalar(explore, explorer, size, cx)
        }
        (Some(explore), Some(explorer)) => render_grid(explore, explorer, view, size, cx),
        (
            Some(Explore {
                state: ExploreState::Failed(err),
                ..
            }),
            _,
        ) => v_flex()
            .id("explore-error")
            .size_full()
            .p(ui(16.))
            .gap(ui(10.))
            .overflow_y_scrollbar()
            .text_size(ui(size))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{} can't be explored", entry.name)),
            )
            .child(
                div()
                    .id("explore-failure")
                    .test_support()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(err.clone()),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Press Space to see it as text."),
            )
            .into_any_element(),
        _ => v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(Spinner::new().small())
            .into_any_element(),
    };
    v_flex()
        .size_full()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .rounded(ui(8.))
        .overflow_hidden()
        .child(render_path(explorer, view, entry, size, cx))
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// The file's name and the names of the levels gone into, each a way back,
/// then what the shown value is.
fn render_path(
    explorer: Option<&Explorer>,
    view: &Entity<Finder>,
    entry: &FileEntry,
    size: f32,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let crumbs = explorer.map_or(&[][..], |e| &e.crumbs[..]);
    let names = std::iter::once(SharedString::from(entry.name.clone()))
        .chain(crumbs.iter().map(|crumb| crumb.label.clone()));
    let depth = crumbs.len();
    h_flex()
        .flex_shrink_0()
        .gap(ui(10.))
        .px(ui(10.))
        .py(ui(6.))
        .border_b_1()
        .border_color(theme.border)
        .text_size(ui(size))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(ui(6.))
                .overflow_hidden()
                .children(names.enumerate().map(|(ix, name)| {
                    let view = view.clone();
                    let way_back = ix < depth;
                    h_flex()
                        .flex_shrink_0()
                        .gap(ui(6.))
                        .when(ix > 0, |this| {
                            this.child(div().text_color(theme.muted_foreground).child("›"))
                        })
                        .child(
                            div()
                                .id(("explore-crumb", ix))
                                .test_support()
                                .when(way_back, |this| {
                                    this.text_color(theme.link)
                                        .cursor_pointer()
                                        .hover(|style| style.underline())
                                        .on_click(move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.explore_back_to(ix, cx)
                                            });
                                            window.refresh();
                                        })
                                })
                                .when(!way_back, |this| this.font_weight(FontWeight::SEMIBOLD))
                                .child(name),
                        )
                })),
        )
        .when_some(explorer, |this, explorer| {
            this.child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.muted_foreground)
                    .child(explorer.summary()),
            )
        })
}

/// A single value, whole.
fn render_scalar(explore: &Explore, explorer: &Explorer, size: f32, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let text: SharedString = match explorer.shown() {
        Json::String(text) => text.clone().into(),
        other => nushell::cell(other).into(),
    };
    div()
        .id(("explore-scalar", super::views::path_id(&explore.path)))
        .test_support()
        .size_full()
        .p(ui(12.))
        .overflow_y_scrollbar()
        .font_family(theme.mono_font_family.clone())
        .text_size(ui(size))
        .child(text)
        .into_any_element()
}

/// The shown record, table or list as a grid with a heading row; only the
/// rows in view are drawn.
fn render_grid(
    explore: &Explore,
    explorer: &Explorer,
    view: &Entity<Finder>,
    size: f32,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let pal = palette(cx);
    let level = explorer.level.clone();
    let row_height = ui(size + 10.);
    let lead_width = column_width(level.lead.chars, size);
    let widths: Vec<Pixels> = level
        .columns
        .iter()
        .map(|column| column_width(column.chars, size))
        .collect();
    let total = widths.iter().fold(lead_width, |sum, w| sum + *w);
    let last = widths.len().saturating_sub(1);
    // The last column takes the room left over.
    let cell = move |width: Pixels, fill: bool| {
        div()
            .h_full()
            .line_height(row_height)
            .px(ui(8.))
            .flex_shrink_0()
            .truncate()
            .map(|this| {
                if fill {
                    this.flex_1().min_w(width)
                } else {
                    this.w(width)
                }
            })
    };
    let header = h_flex()
        .flex_shrink_0()
        .w_full()
        .h(row_height)
        .bg(theme.muted)
        .font_weight(FontWeight::SEMIBOLD)
        .child(
            cell(lead_width, false)
                .text_color(theme.muted_foreground)
                .child(level.lead.title.clone()),
        )
        .children(
            level
                .columns
                .iter()
                .zip(&widths)
                .enumerate()
                .map(|(ix, (column, width))| cell(*width, ix == last).child(column.title.clone())),
        );

    let root = explorer.root.clone();
    let crumbs = explorer.crumbs.clone();
    let cursor = explorer.cursor;
    let (link, muted) = (theme.link, theme.muted_foreground);
    let view = view.clone();
    let rows_level = level.clone();
    let mut rows = uniform_list(
        ("explore-rows", super::views::path_id(&explore.path)),
        level.rows,
        move |range, _, _| {
            let level = &rows_level;
            let shown = resolve(&root, &crumbs);
            // Each row's lead text and its cells.
            let lines: Vec<(SharedString, Vec<Option<&Json>>)> = match shown {
                Json::Object(map) => map
                    .iter()
                    .skip(range.start)
                    .take(range.len())
                    .map(|(key, value)| (key.clone().into(), vec![Some(value)]))
                    .collect(),
                Json::Array(items) => items
                    .iter()
                    .enumerate()
                    .skip(range.start)
                    .take(range.len())
                    .map(|(ix, item)| {
                        let cells = match level.shape {
                            Shape::Table => level
                                .columns
                                .iter()
                                .map(|column| item.get(column.title.as_ref()))
                                .collect(),
                            Shape::Record | Shape::List | Shape::Scalar => vec![Some(item)],
                        };
                        (ix.to_string().into(), cells)
                    })
                    .collect(),
                Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => Vec::new(),
            };
            let click = |row: usize, col: Option<usize>| {
                let view = view.clone();
                move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    view.update(cx, |this, cx| {
                        this.explore_click(row, col, event.click_count, cx)
                    });
                    window.refresh();
                }
            };
            lines
                .into_iter()
                .zip(range.start..)
                .map(|((lead, cells), row)| {
                    let on_row = row == cursor.row;
                    h_flex()
                        .h(row_height)
                        .w_full()
                        .when(row % 2 == 1, |this| this.bg(pal.stripe))
                        .when(on_row, |this| this.bg(pal.tile_selected))
                        .child(
                            cell(lead_width, false)
                                .text_color(muted)
                                .on_mouse_down(MouseButton::Left, click(row, None))
                                .child(lead),
                        )
                        .children(cells.into_iter().zip(&widths).enumerate().map(
                            |(col, (value, width))| {
                                let at_cursor = on_row && col == cursor.col;
                                cell(*width, col == last)
                                    .when(value.is_some_and(is_nested), |this| {
                                        this.text_color(link).cursor_pointer()
                                    })
                                    .when(at_cursor, |this| {
                                        this.bg(pal.selection).text_color(pal.selection_text)
                                    })
                                    .on_mouse_down(MouseButton::Left, click(row, Some(col)))
                                    .child(value.map(cell_text).unwrap_or_default())
                            },
                        ))
                })
                .collect::<Vec<_>>()
        },
    )
    .flex_1()
    .size_full()
    .track_scroll(&explore.rows_scroll);
    // A sideways swipe moves the scroller around this, not this.
    rows.style().restrict_scroll_to_axis = Some(true);

    // The heading and rows scroll sideways together when the columns are
    // wider than the view; the rows scroll up and down inside that.
    let columns = v_flex()
        .h_full()
        .min_w_full()
        .w(total)
        .font_family(theme.mono_font_family.clone())
        .text_size(ui(size))
        .child(header)
        .child(div().flex_1().min_h_0().child(rows));
    div()
        .size_full()
        .relative()
        .child(
            div()
                .id(("explore-columns", super::views::path_id(&explore.path)))
                .size_full()
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .track_scroll(&explore.sideways)
                .child(columns),
        )
        .child(
            div()
                .absolute()
                .top(row_height)
                .bottom_0()
                .left_0()
                .right_0()
                .vertical_scrollbar(&explore.rows_scroll),
        )
        .horizontal_scrollbar(&explore.sideways)
        .into_any_element()
}

/// Quick Look's Text and Explore buttons, for a file explore mode opens.
pub(super) fn mode_buttons(view: &Entity<Finder>, mode: QuickLookMode) -> impl IntoElement {
    let view = view.clone();
    ButtonGroup::new("ql-mode")
        .small()
        .outline()
        .child(
            Button::new("ql-text")
                .label("Text")
                .selected(mode == QuickLookMode::Text)
                .tooltip_with_action("Show as Text", &QuickLook, Some(QUICK_LOOK)),
        )
        .child(
            Button::new("ql-explore")
                .label("Explore")
                .selected(mode == QuickLookMode::Explore)
                .tooltip_with_action("Explore the Data", &QuickLookExplore, Some(QUICK_LOOK)),
        )
        .on_click(move |chosen: &Vec<usize>, window, cx| {
            let mode = match chosen.first() {
                Some(1) => QuickLookMode::Explore,
                _ => QuickLookMode::Text,
            };
            view.update(cx, |this, cx| this.set_quick_look_mode(mode, cx));
            window.refresh();
        })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would replace the standard one.
    use super::{Cursor, Explorer, Level, Shape, Step, cell_text, digits, resolve};
    use crate::nushell::Opened;
    use serde_json::json;

    fn explorer(data: serde_json::Value) -> Explorer {
        Explorer::new(Opened { data, rows: None })
    }

    #[test]
    fn shapes_follow_the_value() {
        let record = Level::of(&json!({"name": "fff", "tags": ["a"]}));
        assert_eq!((record.shape, record.rows), (Shape::Record, 2));
        assert_eq!(record.lead.title.as_ref(), "key");

        let table = Level::of(&json!([{"a": 1, "b": 2}, {"c": 3, "a": 4}]));
        assert_eq!(table.shape, Shape::Table);
        let titles: Vec<&str> = table.columns.iter().map(|c| c.title.as_ref()).collect();
        assert_eq!(titles, ["a", "b", "c"], "every field, in first-seen order");

        assert_eq!(Level::of(&json!([1, {"a": 1}])).shape, Shape::List);
        assert_eq!(Level::of(&json!([])).shape, Shape::List);
        assert_eq!(Level::of(&json!("text")).shape, Shape::Scalar);
    }

    #[test]
    fn columns_fit_their_text_within_bounds() {
        let level = Level::of(&json!([{"id": 1, "long": "x".repeat(500)}]));
        assert_eq!(level.columns[0].chars, 3, "at least three characters");
        assert_eq!(level.columns[1].chars, 40, "at most forty");
        assert_eq!(digits(1), 1);
        assert_eq!(digits(10), 1, "rows 0 to 9");
        assert_eq!(digits(11), 2);
    }

    #[test]
    fn cells_read_like_nushell() {
        assert_eq!(cell_text(&json!([1, 2])).as_ref(), "[list 2 items]");
        assert_eq!(cell_text(&json!([{"a": 1}])).as_ref(), "[table 1 row]");
        assert_eq!(cell_text(&json!({"a": 1})).as_ref(), "{record 1 field}");
        assert_eq!(cell_text(&json!("two\nlines")).as_ref(), "two lines");
        assert_eq!(cell_text(&json!(null)).as_ref(), "");
    }

    #[test]
    fn drills_into_records_tables_and_back() {
        let mut ex = explorer(json!({
            "package": {"name": "fff", "version": "1"},
            "bin": [{"name": "a", "path": "src/a.rs"}, {"name": "b", "path": "src/b.rs"}],
        }));
        assert!(ex.enter(false), "into the package record");
        assert_eq!(ex.crumbs[0].steps, [Step::Key("package".into())]);
        assert_eq!(ex.level.shape, Shape::Record);
        assert_eq!(ex.summary(), "record · 2 fields");

        assert!(ex.back_to(0));
        assert!(ex.move_by(1, 0), "down to bin");
        assert!(ex.enter(false), "into the bin table");
        assert_eq!(ex.level.shape, Shape::Table);
        assert_eq!(ex.summary(), "table · 2 rows · 2 columns");

        assert!(ex.move_by(1, 1), "row 1, path");
        assert!(ex.enter(false), "a scalar is shown whole");
        assert_eq!(ex.crumbs[1].label.as_ref(), "1.path");
        assert_eq!(ex.shown(), &json!("src/b.rs"));
        assert_eq!(ex.level.shape, Shape::Scalar);

        assert!(ex.back_to(1));
        assert_eq!(ex.cursor, Cursor { row: 1, col: 1 }, "the cursor is back");
        assert!(ex.enter(true), "a table's whole row");
        assert_eq!(ex.shown(), &json!({"name": "b", "path": "src/b.rs"}));

        assert!(ex.back_to(0));
        assert!(ex.crumbs.is_empty());
        assert_eq!(ex.cursor, Cursor { row: 1, col: 0 });
        assert!(!ex.back_to(0), "nothing above the top");
    }

    #[test]
    fn empty_and_null_values_are_not_entered() {
        let mut ex = explorer(json!({"none": null, "empty": [], "blank": {}}));
        assert!(!ex.enter(false));
        assert!(ex.move_by(1, 0));
        assert!(!ex.enter(false));
        assert!(ex.move_by(1, 0));
        assert!(!ex.enter(false));
        assert!(!ex.move_by(1, 0), "the cursor stays in the grid");
        assert!(!ex.move_by(0, 1), "a record has one value column");
    }

    #[test]
    fn a_cut_short_table_says_so() {
        let ex = Explorer::new(Opened {
            data: json!([{"a": 1}, {"a": 2}]),
            rows: Some(5),
        });
        assert_eq!(ex.summary(), "table · first 2 of 5 rows · 1 column");
    }

    #[test]
    fn a_broken_path_falls_back_to_the_top() {
        let mut ex = explorer(json!({"a": {"b": 1}}));
        assert!(ex.enter(false));
        ex.crumbs[0].steps = vec![Step::Key("missing".into())];
        assert_eq!(resolve(&ex.root, &ex.crumbs), &*ex.root);
    }
}
