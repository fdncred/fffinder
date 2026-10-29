//! Use Groups in the list and icon views: items in groups by kind, date,
//! size, or name (see `fffinder_core::grouping`), each under a header that
//! collapses it. A collapsed group's items leave `rows`, so selection, the
//! arrow keys, and Select All skip them, as in Finder.

use super::{Finder, Row};
use crate::actions::ToggleGroups;
use crate::theme::ui;
use fffinder_core::ViewMode;
use fffinder_core::grouping::{self, GroupKey};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use std::ops::Range;
use std::rc::Rc;

/// A group header's height in the icon view.
pub(super) const ICON_HEADER: f32 = 30.;

/// A line of the list view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ListLine {
    Header(usize),
    /// An item, and whether it's striped (every other one in its group).
    Row(usize, bool),
}

/// A line of the icon view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IconLine {
    Header(usize),
    Tiles(Range<usize>),
}

/// The icon view's lines and their heights, kept until the rows, the
/// number of columns, or the tile size change, so scrolling doesn't
/// rebuild them every frame.
pub(super) struct IconLayout {
    key: (usize, f32, f32),
    pub(super) lines: Vec<IconLine>,
    pub(super) sizes: Rc<Vec<Size<Pixels>>>,
}

/// Rows of `cols` tiles over `ranges`, each range starting a new row,
/// under a header for each group when `headers`.
fn icon_lines(ranges: &[Range<usize>], headers: bool, cols: usize) -> Vec<IconLine> {
    ranges
        .iter()
        .enumerate()
        .flat_map(|(g, range)| {
            let end = range.end;
            let tiles = range
                .clone()
                .step_by(cols)
                .map(move |start| IconLine::Tiles(start..(start + cols).min(end)));
            headers
                .then_some(IconLine::Header(g))
                .into_iter()
                .chain(tiles)
        })
        .collect()
}

/// Which line of the icon view holds item `ix`.
fn icon_line_of(ranges: &[Range<usize>], headers: bool, cols: usize, ix: usize) -> usize {
    let mut line = 0;
    for range in ranges {
        line += usize::from(headers);
        if range.contains(&ix) {
            return line + (ix - range.start) / cols;
        }
        line += range.len().div_ceil(cols);
    }
    line.saturating_sub(1)
}

/// Up or Down from item `ix` in the icon view: the same column in the line
/// above or below, past group headers, or that line's last item when it's
/// shorter. `None` at the top or bottom.
fn grid_step(ranges: &[Range<usize>], cols: usize, ix: usize, down: bool) -> Option<usize> {
    let k = ranges.iter().position(|r| r.contains(&ix))?;
    let range = &ranges[k];
    let (line, col) = ((ix - range.start) / cols, (ix - range.start) % cols);
    let at =
        |range: &Range<usize>, line: usize| (range.start + line * cols + col).min(range.end - 1);
    let last_line = |range: &Range<usize>| (range.len() - 1) / cols;
    if down && line < last_line(range) {
        return Some(at(range, line + 1));
    }
    if !down && line > 0 {
        return Some(at(range, line - 1));
    }
    if down {
        ranges[k + 1..]
            .iter()
            .find(|r| !r.is_empty())
            .map(|r| at(r, 0))
    } else {
        ranges[..k]
            .iter()
            .rev()
            .find(|r| !r.is_empty())
            .map(|r| at(r, last_line(r)))
    }
}

/// The first index in `0..len` for which `before` is false, when it's true
/// up to some index and false after.
fn partition_index(len: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let mid = low + (high - low) / 2;
        if before(mid) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

impl Finder {
    pub(super) fn grouping_active(&self) -> bool {
        self.config.use_groups && matches!(self.effective_view(), ViewMode::List | ViewMode::Icons)
    }

    /// Put `rows` in groups when this view uses them, leaving out the items
    /// of collapsed groups.
    pub(super) fn group_rows(&mut self, rows: Vec<Row>) -> Vec<Row> {
        self.icon_layout = None;
        if !self.grouping_active() {
            self.groups.clear();
            return rows;
        }
        let (key, today) = (self.config.group_by, grouping::today());
        let (rows, groups) = grouping::arrange(
            rows,
            |row| row.depth,
            |row| grouping::group_of(&row.entry, key, today),
            |label| self.collapsed_groups.contains(label),
        );
        self.groups = groups;
        rows
    }

    /// The group headers shown, in order.
    pub fn group_headers(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.label.clone()).collect()
    }

    /// Group the views by `key`, or stop grouping them.
    pub fn set_group_by(&mut self, key: Option<GroupKey>, cx: &mut Context<Self>) {
        let had_last_opened = self.wants_last_opened();
        match key {
            Some(key) => {
                if key != self.config.group_by {
                    self.collapsed_groups.clear();
                }
                self.config.use_groups = true;
                self.config.group_by = key;
            }
            None => self.config.use_groups = false,
        }
        self.schedule_save(cx);
        if self.wants_last_opened() && !had_last_opened {
            // Last-opened dates are only read when something needs them.
            self.load_listing(Vec::new(), true, cx);
        }
        self.rebuild_rows(cx);
    }

    /// View › Use Groups: group by the last choice, or stop.
    pub(super) fn toggle_groups(
        &mut self,
        _: &ToggleGroups,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (!self.config.use_groups).then_some(self.config.group_by);
        self.set_group_by(key, cx);
    }

    fn toggle_group(&mut self, label: &str, cx: &mut Context<Self>) {
        if !self.collapsed_groups.remove(label) {
            self.collapsed_groups.insert(label.to_string());
        }
        self.rebuild_rows(cx);
    }

    /// The items' ranges the icon view lays out: one per group, or all.
    fn tile_ranges(&self) -> Vec<Range<usize>> {
        match self.groups.is_empty() {
            true => std::iter::once(0..self.rows.len()).collect(),
            false => self.groups.iter().map(|g| g.range.clone()).collect(),
        }
    }

    // ----- list ----------------------------------------------------------

    pub(super) fn list_line_count(&self) -> usize {
        self.rows.len() + self.groups.len()
    }

    /// What line `line` of the list shows; `None` past the end. A group's
    /// header is line `g + range.start`: one header for each group before
    /// it, plus their items.
    pub(super) fn list_line(&self, line: usize) -> Option<ListLine> {
        if self.groups.is_empty() {
            return (line < self.rows.len()).then_some(ListLine::Row(line, line % 2 == 1));
        }
        let header = |g: usize| g + self.groups[g].range.start;
        let g = partition_index(self.groups.len(), |g| header(g) <= line).checked_sub(1)?;
        let range = &self.groups[g].range;
        match line - header(g) {
            0 => Some(ListLine::Header(g)),
            offset => {
                let ix = range.start + offset - 1;
                (ix < range.end).then_some(ListLine::Row(ix, (offset - 1) % 2 == 1))
            }
        }
    }

    /// The list line showing item `ix`.
    pub(super) fn list_line_of(&self, ix: usize) -> usize {
        if self.groups.is_empty() {
            return ix;
        }
        let g = self.groups.partition_point(|group| group.range.end <= ix);
        g + ix + 1
    }

    // ----- icons ---------------------------------------------------------

    /// The icon view's lines for tiles `tile_h` tall, made when needed.
    pub(super) fn icon_layout(&mut self, tile_h: f32, first_pad: f32) -> &IconLayout {
        let cols = self.grid_cols.max(1);
        let key = (cols, tile_h, crate::theme::ui_scale());
        if self.icon_layout.as_ref().is_none_or(|l| l.key != key) {
            let lines = icon_lines(&self.tile_ranges(), !self.groups.is_empty(), cols);
            let sizes = lines
                .iter()
                .enumerate()
                .map(|(ix, line)| {
                    let height = match line {
                        IconLine::Header(_) => ICON_HEADER,
                        IconLine::Tiles(_) => tile_h,
                    };
                    let pad = if ix == 0 { first_pad } else { 0. };
                    size(px(0.), ui(height + pad))
                })
                .collect();
            self.icon_layout = Some(IconLayout {
                key,
                lines,
                sizes: Rc::new(sizes),
            });
        }
        self.icon_layout.as_ref().expect("the layout was just made")
    }

    /// The icon view line showing item `ix`.
    pub(super) fn icon_line_index(&self, ix: usize) -> usize {
        let cols = self.grid_cols.max(1);
        icon_line_of(&self.tile_ranges(), !self.groups.is_empty(), cols, ix)
    }

    /// Up or Down from item `ix` in the icon view.
    pub(super) fn icon_step(&self, ix: usize, down: bool) -> Option<usize> {
        grid_step(&self.tile_ranges(), self.grid_cols.max(1), ix, down)
    }

    /// A group's header: an arrow that shows whether it's collapsed, its
    /// name, and how many items it holds. Clicking it collapses or expands
    /// the group.
    pub(super) fn render_group_header(
        &self,
        g: usize,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let group = &self.groups[g];
        let label = group.label.clone();
        h_flex()
            .id(SharedString::from(format!("group-{label}")))
            .h(ui(height))
            .w_full()
            .flex_shrink_0()
            .items_end()
            .px(ui(10.))
            .pb(ui(3.))
            .gap(ui(6.))
            .border_b_1()
            .border_color(theme.border)
            .text_size(ui(12.))
            .child(
                Icon::new(if group.collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .with_size(ui(12.))
                .text_color(muted),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child(label.clone()))
            .child(div().text_color(muted).child(group.total.to_string()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_group(&label, cx)))
            .test_support()
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would replace the standard one.
    use super::{IconLine, grid_step, icon_line_of, icon_lines, partition_index};

    #[test]
    fn lays_out_grouped_icons() {
        // Three columns; groups of 4 items, none (collapsed), and 2.
        let ranges = [0..4, 4..4, 4..6];
        let lines = icon_lines(&ranges, true, 3);
        assert_eq!(
            lines,
            [
                IconLine::Header(0),
                IconLine::Tiles(0..3),
                IconLine::Tiles(3..4),
                IconLine::Header(1),
                IconLine::Header(2),
                IconLine::Tiles(4..6),
            ]
        );
        let lines_of: Vec<usize> = (0..6)
            .map(|ix| icon_line_of(&ranges, true, 3, ix))
            .collect();
        assert_eq!(lines_of, [1, 1, 1, 2, 5, 5]);
        assert_eq!(
            icon_lines(std::slice::from_ref(&(0..5)), false, 2).len(),
            3,
            "no headers"
        );
        assert_eq!(icon_line_of(std::slice::from_ref(&(0..5)), false, 2, 4), 2);
    }

    #[test]
    fn arrow_keys_cross_groups_in_the_same_column() {
        let ranges = [0..4, 4..4, 4..6];
        // Down from the last column of the first line: item 3 is alone on
        // the next line.
        assert_eq!(grid_step(&ranges, 3, 2, true), Some(3));
        // Down from the group's last line skips the collapsed group.
        assert_eq!(grid_step(&ranges, 3, 3, true), Some(4));
        assert_eq!(grid_step(&ranges, 3, 5, true), None, "the bottom");
        // Up into a shorter last line lands on its last item.
        assert_eq!(grid_step(&ranges, 3, 5, false), Some(3));
        assert_eq!(grid_step(&ranges, 3, 3, false), Some(0));
        assert_eq!(grid_step(&ranges, 3, 1, false), None, "the top");
    }

    #[test]
    fn finds_partition_points() {
        assert_eq!(partition_index(5, |i| i < 3), 3);
        assert_eq!(partition_index(0, |_| true), 0);
        assert_eq!(partition_index(4, |_| true), 4);
    }
}
