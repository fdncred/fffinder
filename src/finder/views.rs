//! The four file views (icons, list, columns, gallery), loading skeletons,
//! file icons, and dragging files out to other apps.

use super::{Finder, Row};
use crate::macos::IconLookup;
use crate::theme::{Palette, palette, ui};
use fffinder_core::{FileEntry, FileKind, ListColumn, SortKey, ViewMode};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, h_flex, h_virtual_list,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

pub const LIST_ROW: f32 = 24.;
pub const COLUMN_ROW: f32 = 22.;
pub const COLUMN_WIDTH: f32 = 235.;
const GRID_PADDING: f32 = 18.;

/// Files dragged from a view: dropped on the sidebar or exported to other apps.
#[derive(Clone)]
pub struct DraggedFiles(pub Vec<PathBuf>);

pub struct DragPreview {
    label: SharedString,
    icon: &'static str,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap(ui(6.))
            .px(ui(8.))
            .py(ui(4.))
            .rounded(ui(6.))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .text_size(ui(12.))
            .child(img(self.icon).size(ui(18.)))
            .child(self.label.clone())
    }
}

pub fn path_id(path: &std::path::Path) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish() as usize
}

/// Image formats GPUI decodes, for thumbnails.
fn thumbnailable(entry: &FileEntry) -> bool {
    if entry.kind != FileKind::Image || entry.size.unwrap_or(0) > 40_000_000 {
        return false;
    }
    let ext = entry
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "svg"
    )
}

/// A file's colored icon, an image thumbnail, or a document icon with its
/// extension badge. With `thumbnails`, images show themselves, and on macOS
/// PDFs, videos, and documents show QuickLook's picture once it's ready.
pub fn file_icon(entry: &FileEntry, size: Pixels, thumbnails: bool, pal: &Palette) -> AnyElement {
    if thumbnails && thumbnailable(entry) {
        let fallback = entry.kind.icon_asset();
        return img(entry.path.clone())
            .size(size)
            .object_fit(ObjectFit::Contain)
            .with_fallback(move || img(fallback).size_full().into_any_element())
            .into_any_element();
    }
    if thumbnails && crate::thumbnails::wanted(entry) {
        // Device pixels, in a few sizes so neighboring icon sizes share them.
        let px = ((f32::from(size) * 2.) as u32)
            .next_power_of_two()
            .clamp(64, 512);
        let (shown, pal) = (entry.clone(), *pal);
        let icon = move || kind_icon(&shown, size, &pal);
        return img(crate::thumbnails::image(entry, px))
            .size(size)
            .flex_shrink_0()
            .object_fit(ObjectFit::Contain)
            .with_loading(icon.clone())
            .with_fallback(icon)
            .into_any_element();
    }
    kind_icon(entry, size, pal)
}

/// Finder's icon for the file's type, or the bundled one with a badge.
fn kind_icon(entry: &FileEntry, size: Pixels, pal: &Palette) -> AnyElement {
    // Finder's own icon for this type, on macOS.
    let later = match crate::macos::file_icon(entry, f32::from(size)) {
        IconLookup::Ready(icon) => {
            return img(icon)
                .size(size)
                .flex_shrink_0()
                .object_fit(ObjectFit::Contain)
                .into_any_element();
        }
        IconLookup::Later => true,
        IconLookup::Unavailable => false,
    };
    let badge = (size >= ui(40.)
        && !matches!(entry.kind, FileKind::Folder | FileKind::Application))
    .then(|| entry.badge())
    .flatten();
    div()
        .size(size)
        .relative()
        .flex_shrink_0()
        .child(img(entry.kind.icon_asset()).size_full())
        .when(later, |this| this.child(redraw_soon()))
        .when_some(badge, |this, badge| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(size * 0.13)
                    .flex()
                    .justify_center()
                    .text_size(size * 0.13)
                    .font_weight(FontWeight::BOLD)
                    .text_color(pal.badge_text)
                    .child(badge),
            )
        })
        .into_any_element()
}

/// An empty element that asks for another frame once painted, for an icon
/// that will be ready by then.
pub fn redraw_soon() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| window.request_animation_frame(),
    )
    .absolute()
    .size_0()
}

/// The file name with search matches emphasized.
fn name_text(row: &Row, selected: bool, pal: &Palette) -> AnyElement {
    let style = row.name_style.as_ref().filter(|_| !selected);
    let match_bg = (!selected).then_some(pal.highlight);
    div()
        .truncate()
        .child(styled_name(&row.name, style, &row.highlights, match_bg))
        .into_any_element()
}

/// A name drawn the way `ls` draws it (`LS_COLORS`: colors, a background
/// behind the letters, bold, underline), with `matches` in bold on
/// `match_bg`.
pub fn styled_name(
    name: &SharedString,
    style: Option<&crate::nu_env::NameStyle>,
    matches: &[(usize, usize)],
    match_bg: Option<Hsla>,
) -> StyledText {
    let base = style.map(|s| s.highlight()).unwrap_or_default();
    let mut highlights = Vec::new();
    let mut at = 0;
    for &(a, b) in matches {
        if a < at || a >= b || b > name.len() {
            continue;
        }
        if at < a {
            highlights.push((at..a, base));
        }
        let matched = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            background_color: match_bg.or(base.background_color),
            ..base
        };
        highlights.push((a..b, matched));
        at = b;
    }
    if at < name.len() {
        highlights.push((at..name.len(), base));
    }
    highlights.retain(|(_, style)| *style != HighlightStyle::default());
    StyledText::new(name.clone()).with_highlights(highlights)
}

impl Finder {
    /// Shared mouse handling for an item in the icon, list, or gallery views.
    /// `wrapper` hosts the context menu so the item itself stays observable
    /// in UI tests.
    fn item_interactions(
        &self,
        el: Stateful<Div>,
        ix: usize,
        wrapper: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.rows[ix];
        let selected = self.selection.contains(ix);
        let drag_paths: Vec<PathBuf> = if selected {
            self.selected_entries()
                .into_iter()
                .map(|e| e.path)
                .collect()
        } else {
            vec![row.entry.path.clone()]
        };
        let label: SharedString = if drag_paths.len() > 1 {
            format!("{} items", drag_paths.len()).into()
        } else {
            row.name.clone()
        };
        let icon = row.entry.kind.icon_asset();
        let view = cx.entity();
        let el = if row.entry.is_navigable() {
            super::transfer::folder_drop_target(el, row.entry.path.clone(), cx)
        } else {
            el
        };
        let item = el
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let already = this.selection.contains(ix);
                    if event.click_count >= 2
                        || event.modifiers.secondary()
                        || event.modifiers.shift
                        || !already
                    {
                        this.click_row(ix, event.modifiers, event.click_count, window, cx);
                    } else {
                        this.focus_files(window, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // A plain click on one item of a multi-selection narrows to it.
                let m = event.modifiers();
                if event.click_count() == 1
                    && !m.secondary()
                    && !m.shift
                    && this.selection.len() > 1
                {
                    this.click_row(ix, m, 1, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.right_click_row(ix, window, cx)
                }),
            )
            .on_drag(DraggedFiles(drag_paths), move |_, _, _, cx| {
                cx.new(|_| DragPreview {
                    label: label.clone(),
                    icon,
                })
            })
            .external_drag_payload(|files: &DraggedFiles, _, _| {
                Some(ExternalDragPayload::Files(FileDragPaths::new(
                    files.0.iter().map(|p| (p.clone(), p.is_dir())),
                )))
            })
            .test_support();
        wrapper
            .id(("item", path_id(&self.rows[ix].entry.path)))
            .child(item)
            .context_menu(move |menu, window, cx| super::menus::item_menu(&view, menu, window, cx))
            .into_any_element()
    }

    // ----- icons -------------------------------------------------------------

    pub(super) fn render_icons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon = self.config.icon_size;
        let tile_w = (icon + 52.).max(96.);
        let tile_h = icon + 58.;
        let sizes = self.icon_layout(tile_h, GRID_PADDING / 2.).sizes.clone();
        let view = cx.entity();
        div()
            .size_full()
            .relative()
            .on_prepaint(move |bounds, _, cx| {
                // Tiles are drawn at the UI scale; the bounds are real pixels.
                let scale = crate::theme::ui_scale();
                let width = f32::from(bounds.size.width) - GRID_PADDING * 2. * scale;
                let n = ((width / (tile_w * scale)).floor() as usize).max(1);
                view.update(cx, |this, cx| {
                    if this.grid_cols != n {
                        this.grid_cols = n;
                        cx.notify();
                    }
                });
            })
            .child(
                // Lines differ in height when group headers are among them.
                v_virtual_list(
                    cx.entity(),
                    "icon-grid",
                    sizes,
                    move |this: &mut Self, range: Range<usize>, _, cx| {
                        range
                            .map(|line| this.render_icon_line(line, tile_w, tile_h, cx))
                            .collect::<Vec<_>>()
                    },
                )
                .size_full()
                .track_scroll(&self.grid_scroll),
            )
            .vertical_scrollbar(&self.grid_scroll)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.focus_files(window, cx);
                    this.clear_selection(cx);
                }),
            )
            .context_menu({
                let view = cx.entity();
                move |menu, window, cx| super::menus::background_menu(&view, menu, window, cx)
            })
    }

    /// A line of the icon view: a group header, or a row of tiles. The
    /// first line has extra room above it.
    fn render_icon_line(
        &self,
        line: usize,
        tile_w: f32,
        tile_h: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pad = if line == 0 { GRID_PADDING / 2. } else { 0. };
        let layout = self
            .icon_layout
            .as_ref()
            .expect("the icon view makes its layout before drawing");
        match layout.lines.get(line).cloned() {
            Some(super::groups::IconLine::Header(g)) => div()
                .pt(ui(pad))
                .child(self.render_group_header(g, super::groups::ICON_HEADER, cx))
                .into_any_element(),
            Some(super::groups::IconLine::Tiles(tiles)) => {
                let mut row = h_flex()
                    .px(ui(GRID_PADDING))
                    .pt(ui(pad))
                    .h(ui(tile_h + pad))
                    .items_start();
                for ix in tiles {
                    row = row.child(self.render_tile(ix, tile_w, tile_h, cx));
                }
                row.into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    fn render_tile(
        &self,
        ix: usize,
        tile_w: f32,
        tile_h: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pal = palette(cx);
        let theme = cx.theme();
        let row = &self.rows[ix];
        let selected = self.selection.contains(ix);
        let focused_selection = selected;
        let icon = ui(self.config.icon_size);
        let el = v_flex()
            .id(("tile", super::views::path_id(&row.entry.path)))
            .w(ui(tile_w))
            .h(ui(tile_h))
            .items_center()
            .pt(ui(4.))
            .gap(ui(4.))
            .child(
                div()
                    .p(ui(4.))
                    .rounded(ui(6.))
                    .when(selected, |this| this.bg(pal.tile_selected))
                    .child(file_icon(&row.entry, icon, true, &pal)),
            )
            .child(
                div()
                    .max_w(ui(tile_w - 8.))
                    .px(ui(5.))
                    .py(ui(1.))
                    .rounded(ui(4.))
                    .text_size(ui(12.))
                    .text_center()
                    .line_clamp(2)
                    .when(focused_selection, |this| {
                        this.bg(pal.selection).text_color(pal.selection_text)
                    })
                    .when(!selected && row.entry.hidden, |this| {
                        this.text_color(theme.muted_foreground)
                    })
                    .child(styled_name(
                        &row.name,
                        row.name_style.as_ref().filter(|_| !selected),
                        &[],
                        None,
                    )),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        self.item_interactions(el, ix, div(), cx)
    }

    // ----- list --------------------------------------------------------------

    pub(super) fn render_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let searching = self.shows_parent_column();
        let sort = self.current_sort();
        let header_color = theme.muted_foreground;
        let border = theme.border;
        let len = self.list_line_count();
        let row = LIST_ROW * crate::theme::ui_scale();
        let visible = (f32::from(self.content_height) / row).ceil() as usize;
        let filler = visible.saturating_sub(len + 1);
        let active = self.files_focused(window);
        let parent_title = if self.search_contents && self.is_searching() {
            "Match"
        } else if self.in_trash() {
            "Original Folder"
        } else {
            "Folder"
        };

        let header = h_flex()
            .id("list-header")
            .h(ui(28.))
            .flex_shrink_0()
            .px(ui(10.))
            .border_b_1()
            .border_color(border)
            .text_size(ui(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(header_color)
            .child(
                div().flex_1().min_w(ui(160.)).h_full().child(
                    self.header_cell("Name", Some(SortKey::Name), false, sort, cx)
                        .test_support(),
                ),
            )
            .when(searching, |this| {
                this.child(div().w(ui(220.)).px(ui(6.)).child(parent_title))
            })
            .children(
                self.config
                    .list_columns
                    .clone()
                    .into_iter()
                    .enumerate()
                    .map(|(ix, setting)| self.column_header(ix, setting, sort, cx)),
            )
            .on_drag_move(cx.listener(
                |this, event: &DragMoveEvent<super::list::ColumnResize>, _, cx| {
                    let ix = event.drag(cx).0;
                    this.resize_column(ix, event.event.position.x, cx);
                },
            ))
            .context_menu({
                let view = cx.entity();
                move |menu, window, cx| super::list::header_menu(&view, menu, window, cx)
            });

        let body = uniform_list(
            "list",
            len + filler,
            cx.processor(move |this: &mut Self, range: Range<usize>, _, cx| {
                range
                    .map(|line| match this.list_line(line) {
                        Some(super::groups::ListLine::Header(g)) => {
                            this.render_group_header(g, LIST_ROW, cx)
                        }
                        Some(super::groups::ListLine::Row(ix, stripe)) => {
                            this.render_list_row(ix, stripe, searching, active, cx)
                        }
                        None => this.render_list_filler(line, cx),
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .size_full()
        .track_scroll(&self.list_scroll);

        v_flex().size_full().child(header).child(
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(body)
                .vertical_scrollbar(&self.list_scroll)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.focus_files(window, cx);
                        this.clear_selection(cx);
                    }),
                )
                .context_menu({
                    let view = cx.entity();
                    move |menu, window, cx| super::menus::background_menu(&view, menu, window, cx)
                }),
        )
    }

    /// A header label that sorts by `key` when clicked, with an arrow when
    /// the list is sorted by it.
    fn header_cell(
        &self,
        label: &'static str,
        key: Option<SortKey>,
        right: bool,
        sort: fffinder_core::SortOrder,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let sorted = key == Some(sort.key);
        h_flex()
            .id(label)
            .size_full()
            .px(ui(6.))
            .gap(ui(4.))
            .when(right, |this| this.justify_end())
            .hover(|s| s.bg(cx.theme().list_hover))
            .child(div().truncate().child(label))
            .when(sorted, |this| {
                this.child(
                    Icon::new(if sort.ascending {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .with_size(ui(12.)),
                )
            })
            .when_some(key, |this, key| {
                this.on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.set_sort(key, cx)),
                )
            })
    }

    /// One column's header: drag it onto another to move it, or drag its
    /// right edge to resize it.
    fn column_header(
        &self,
        ix: usize,
        setting: fffinder_core::ColumnSetting,
        sort: fffinder_core::SortOrder,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let column = setting.column;
        let title = column.title();
        let highlight = cx.theme().drop_target;
        let cell = self
            .header_cell(
                title,
                column.sort_key(),
                column == ListColumn::Size,
                sort,
                cx,
            )
            .on_drag(super::list::ColumnMove(ix), move |_, _, _, cx| {
                cx.new(|_| super::list::ColumnPreview(title.into()))
            })
            .drag_over::<super::list::ColumnMove>(move |style, _, _, _| style.bg(highlight))
            .on_drop(
                cx.listener(move |this, moved: &super::list::ColumnMove, _, cx| {
                    this.move_column(moved.0, ix, cx)
                }),
            )
            .test_support();
        let handle = div()
            .id(SharedString::from(format!("resize-{title}")))
            .absolute()
            .top_0()
            .right_0()
            .h_full()
            .w(ui(6.))
            .cursor(CursorStyle::ResizeLeftRight)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.start_column_resize(ix, event.position.x);
                }),
            )
            .on_drag(super::list::ColumnResize(ix), |_, _, _, cx| {
                cx.new(|_| EmptyView)
            })
            .test_support();
        div()
            .relative()
            .w(ui(setting.width))
            .h_full()
            .flex_shrink_0()
            .child(cell)
            .child(handle)
            .into_any_element()
    }

    /// A list row's frame: rounded, and striped every other row.
    fn list_row_base(stripe: bool, cx: &App) -> Div {
        h_flex()
            .h(ui(LIST_ROW))
            .w_full()
            .px(ui(2.))
            .rounded(ui(5.))
            .text_size(ui(12.))
            .when(stripe, |this| this.bg(palette(cx).stripe))
    }

    /// An empty striped row under the items, so stripes fill the view.
    fn render_list_filler(&self, line: usize, cx: &App) -> AnyElement {
        div()
            .w_full()
            .px(ui(8.))
            .child(Self::list_row_base(line % 2 == 1, cx).id(("filler", line)))
            .into_any_element()
    }

    fn render_list_row(
        &self,
        ix: usize,
        stripe: bool,
        searching: bool,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = palette(cx);
        let theme = cx.theme();
        let base = Self::list_row_base(stripe, cx);
        // Rows are inset from the edges, like Finder's rounded selection.
        let row = &self.rows[ix];
        let selected = self.selection.contains(ix);
        let (fg, muted) = if selected && active {
            (pal.selection_text, pal.selection_text.opacity(0.85))
        } else {
            (theme.foreground, theme.muted_foreground)
        };
        let tree = self.tree_mode();
        let disclosure = tree.then(|| {
            let spacer = div().w(ui(14.)).flex_shrink_0();
            if !row.entry.is_navigable() {
                return spacer.into_any_element();
            }
            let path = row.entry.path.clone();
            let open = self.expanded.contains(&path);
            spacer
                .id(("disclosure", path_id(&path)))
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .with_size(ui(12.))
                    .text_color(muted),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.toggle_expanded(path.clone(), cx);
                    }),
                )
                .test_support()
                .into_any_element()
        });
        let cells = self.config.list_columns.iter().map(|setting| {
            let text = match setting.column {
                ListColumn::Modified => row.date.clone(),
                ListColumn::Created => row.created.clone(),
                ListColumn::LastOpened => row.opened.clone(),
                ListColumn::Size => row.size.clone(),
                ListColumn::Kind => row.kind.clone(),
                ListColumn::Permissions => row.permissions.clone(),
                ListColumn::Git => row.git.clone(),
            };
            div()
                .w(ui(setting.width))
                .flex_shrink_0()
                .px(ui(6.))
                .truncate()
                .text_color(muted)
                .when(setting.column == ListColumn::Size, |this| {
                    this.flex().justify_end()
                })
                .when(setting.column == ListColumn::Permissions, |this| {
                    this.font_family(theme.mono_font_family.clone())
                })
                .child(text)
        });
        let el = base
            .id(("row", super::views::path_id(&row.entry.path)))
            .text_color(fg)
            .when(selected, |this| {
                this.bg(if active {
                    pal.selection
                } else {
                    pal.selection_inactive
                })
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w(ui(160.))
                    .gap(ui(6.))
                    .pl(ui(6. + 16. * row.depth as f32))
                    .pr(ui(6.))
                    .overflow_hidden()
                    .children(disclosure)
                    .child(file_icon(&row.entry, ui(16.), false, &pal))
                    .child(name_text(row, selected, &pal)),
            )
            .when(searching, |this| {
                this.child(
                    div()
                        .w(ui(220.))
                        .px(ui(6.))
                        .truncate()
                        .text_color(muted)
                        .child(row.detail.clone()),
                )
            })
            .children(cells)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        self.item_interactions(el, ix, div().w_full().px(ui(8.)), cx)
    }

    // ----- columns -------------------------------------------------------------

    pub(super) fn render_columns(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let active = self.files_focused(window);
        let leaf = self
            .columns
            .get(self.active_column)
            .and_then(|c| c.selected.and_then(|ix| c.entries.get(ix)))
            .filter(|e| !e.is_navigable())
            .cloned();
        let mut panes: Vec<AnyElement> = Vec::new();
        for col in 0..self.columns.len() {
            let count = self.columns[col].entries.len();
            let dir = self.columns[col].dir.clone();
            let list = uniform_list(
                ("column", col),
                count,
                cx.processor(move |this: &mut Self, range: Range<usize>, _, cx| {
                    range
                        .map(|ix| this.render_column_row(col, ix, active, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .size_full()
            .track_scroll(&self.columns[col].scroll);
            let pane = div()
                .id(("column-pane", col))
                .w(ui(COLUMN_WIDTH))
                .h_full()
                .flex_shrink_0()
                .relative()
                .border_r_1()
                .border_color(border)
                .py(ui(4.))
                .child(list)
                .vertical_scrollbar(&self.columns[col].scroll);
            panes.push(super::transfer::folder_drop_target(pane, dir, cx).into_any_element());
        }
        if let Some(entry) = leaf {
            panes.push(
                div()
                    .w(ui(300.))
                    .h_full()
                    .flex_shrink_0()
                    .p(ui(14.))
                    .child(super::preview::render_preview_card(self, &entry, cx))
                    .into_any_element(),
            );
        }
        div()
            .id("columns")
            .size_full()
            .flex()
            .flex_row()
            .overflow_x_scroll()
            .track_scroll(&self.columns_scroll)
            .children(panes)
            .context_menu({
                let view = cx.entity();
                move |menu, window, cx| super::menus::background_menu(&view, menu, window, cx)
            })
    }

    fn render_column_row(
        &self,
        col: usize,
        ix: usize,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = palette(cx);
        let theme = cx.theme();
        let Some(pane) = self.columns.get(col) else {
            return div().into_any_element();
        };
        let Some(entry) = pane.entries.get(ix) else {
            return div().into_any_element();
        };
        let selected = pane.selected == Some(ix);
        let current = col == self.active_column && active;
        let fg = if selected && current {
            pal.selection_text
        } else {
            theme.foreground
        };
        let view = cx.entity();
        let menu_entry = entry.clone();
        let row = h_flex()
            .id(("crow", path_id(&entry.path)))
            .h(ui(COLUMN_ROW))
            .w_full()
            .px(ui(6.))
            .gap(ui(6.))
            .rounded(ui(5.))
            .text_size(ui(12.))
            .text_color(fg)
            .when(selected, |this| {
                this.bg(if current {
                    pal.selection
                } else {
                    pal.selection_inactive
                })
            })
            .child(file_icon(entry, ui(16.), false, &pal))
            .child(
                div().flex_1().truncate().child(styled_name(
                    &SharedString::from(entry.name.clone()),
                    self.ls_colors
                        .as_ref()
                        .filter(|_| !(selected && current))
                        .and_then(|colors| crate::nu_env::name_style(colors, entry))
                        .as_ref(),
                    &[],
                    None,
                )),
            )
            .when(entry.is_navigable(), |this| {
                this.child(
                    Icon::new(IconName::ChevronRight)
                        .with_size(ui(12.))
                        .text_color(if selected && current {
                            fg
                        } else {
                            theme.muted_foreground
                        }),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.select_in_column(col, ix, window, cx);
                    if event.click_count >= 2
                        && let Some(entry) = this.column_selection()
                    {
                        if this.picker.is_some() && !entry.is_navigable() {
                            this.choose(&crate::actions::Choose, window, cx);
                        } else {
                            this.open_entries(vec![entry], window, cx);
                        }
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.item_menu_at = Some(window.mouse_position());
                    if this
                        .columns
                        .get(col)
                        .is_some_and(|p| p.selected != Some(ix))
                    {
                        this.select_in_column(col, ix, window, cx);
                    }
                }),
            )
            .when(entry.is_navigable(), |row| {
                super::transfer::folder_drop_target(row, entry.path.clone(), cx)
            })
            .on_drag(DraggedFiles(vec![entry.path.clone()]), {
                let label: SharedString = entry.name.clone().into();
                let icon = entry.kind.icon_asset();
                move |_, _, _, cx| {
                    cx.new(|_| DragPreview {
                        label: label.clone(),
                        icon,
                    })
                }
            })
            .external_drag_payload(|files: &DraggedFiles, _, _| {
                Some(ExternalDragPayload::Files(FileDragPaths::new(
                    files.0.iter().map(|p| (p.clone(), p.is_dir())),
                )))
            })
            .test_support();
        div()
            .id(("crow-menu", path_id(&menu_entry.path)))
            .w_full()
            .px(ui(6.))
            .child(row)
            .context_menu(move |menu, window, cx| super::menus::item_menu(&view, menu, window, cx))
            .into_any_element()
    }

    // ----- gallery ---------------------------------------------------------------

    pub(super) fn render_gallery(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let entry = self
            .primary_entry()
            .or_else(|| self.rows.first().map(|r| r.entry.clone()));
        if let Some(entry) = &entry {
            self.ensure_preview(entry, cx);
            self.ensure_pdf(entry, cx);
        }
        let sizes: Rc<Vec<Size<Pixels>>> = Rc::new(vec![size(ui(96.), ui(112.)); self.rows.len()]);
        let strip = h_virtual_list(
            cx.entity(),
            "gallery-strip",
            sizes,
            |this: &mut Self, range: Range<usize>, _, cx| {
                range
                    .map(|ix| this.render_strip_tile(ix, cx))
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.strip_scroll)
        .size_full();
        v_flex()
            .size_full()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(ui(18.))
                    .when_some(entry, |this, entry| {
                        this.child(super::preview::render_preview_large(self, &entry, cx))
                    }),
            )
            .child(
                div()
                    .h(ui(120.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(border)
                    .px(ui(8.))
                    .pt(ui(4.))
                    .relative()
                    .child(strip)
                    .horizontal_scrollbar(&self.strip_scroll),
            )
    }

    fn render_strip_tile(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let pal = palette(cx);
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let selected = self.selection.contains(ix);
        let el = v_flex()
            .id(("strip", path_id(&row.entry.path)))
            .w(ui(96.))
            .h(ui(112.))
            .items_center()
            .gap(ui(4.))
            .pt(ui(6.))
            .child(
                div()
                    .p(ui(4.))
                    .rounded(ui(6.))
                    .when(selected, |this| this.bg(pal.tile_selected))
                    .child(file_icon(&row.entry, ui(64.), true, &pal)),
            )
            .child(
                div()
                    .max_w(ui(90.))
                    .px(ui(4.))
                    .rounded(ui(4.))
                    .text_size(ui(11.))
                    .truncate()
                    .when(selected, |this| {
                        this.bg(pal.selection).text_color(pal.selection_text)
                    })
                    .child(styled_name(
                        &row.name,
                        row.name_style.as_ref().filter(|_| !selected),
                        &[],
                        None,
                    )),
            );
        self.item_interactions(el, ix, div(), cx)
    }

    // ----- skeleton ----------------------------------------------------------------

    pub(super) fn render_skeleton(&self, view: ViewMode, cx: &Context<Self>) -> impl IntoElement {
        let _ = cx;
        match view {
            ViewMode::Icons | ViewMode::Gallery => {
                let icon = self.config.icon_size;
                h_flex()
                    .flex_wrap()
                    .gap(ui(24.))
                    .p(ui(GRID_PADDING + 8.))
                    .children((0..18usize).map(|i| {
                        v_flex()
                            .id(("sk-tile", i))
                            .items_center()
                            .gap(ui(8.))
                            .child(Skeleton::new().w(ui(icon)).h(ui(icon)).rounded(ui(8.)))
                            .child(Skeleton::new().w(ui(icon * 0.9)).h(ui(10.)).rounded(ui(4.)))
                    }))
                    .into_any_element()
            }
            _ => v_flex()
                .gap(ui(10.))
                .p(ui(16.))
                .children((0..14usize).map(|i| {
                    h_flex()
                        .id(("sk-row", i))
                        .gap(ui(12.))
                        .child(Skeleton::new().w(ui(16.)).h(ui(16.)).rounded(ui(4.)))
                        .child(
                            Skeleton::new()
                                .w(ui(180. + (i % 4) as f32 * 40.))
                                .h(ui(10.)),
                        )
                        .child(div().flex_1())
                        .child(Skeleton::new().w(ui(120.)).h(ui(10.)))
                        .child(Skeleton::new().w(ui(70.)).h(ui(10.)))
                }))
                .into_any_element(),
        }
    }
}
