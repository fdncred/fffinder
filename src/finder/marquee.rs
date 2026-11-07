//! Selecting by dragging a box around items, in the list and icon views.

use super::Finder;
use super::groups::{IconLine, ListLine};
use super::views::{GRID_PADDING, ICON_PADDING, LIST_ROW, tile_size};
use crate::theme::ui;
use fffinder_core::ViewMode;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::scroll::ScrollbarHandle;
use gpui_kit::*;
use std::collections::BTreeSet;
use std::time::Duration;

/// How far the mouse moves before a press becomes a drag, in points.
const DRAG_THRESHOLD: f32 = 4.;
/// How often the view scrolls while the mouse is past its top or bottom edge.
const AUTOSCROLL_TICK: Duration = Duration::from_millis(16);
/// Points the view scrolls each tick, for each point the mouse is past the
/// edge, up to [`AUTOSCROLL_MAX`].
const AUTOSCROLL_PER_POINT: f32 = 0.25;
const AUTOSCROLL_MAX: f32 = 16.;

/// A drag in the empty parts of a view.
pub(super) struct Marquee {
    /// Where the press was, in the view's content, so it stays with the items
    /// as the view scrolls.
    anchor: Point<Pixels>,
    /// Where the mouse is now, in the window.
    mouse: Point<Pixels>,
    /// The mouse has moved far enough for this not to be a click.
    dragging: bool,
    /// What stays selected whatever the box covers: the selection before the
    /// press when Cmd or Shift was held, else nothing.
    kept: BTreeSet<usize>,
    /// Scrolls the view while the mouse is past its top or bottom edge.
    autoscroll: Option<Task<()>>,
}

/// The sides of the box in the view's content, as `(left, top, right, bottom)`.
fn box_sides(a: Point<Pixels>, b: Point<Pixels>) -> (Pixels, Pixels, Pixels, Pixels) {
    (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

/// How far `y` is past the top or bottom of `area`: negative above it,
/// positive below it, and 0 inside it.
fn past_edge(y: Pixels, area: &Bounds<Pixels>) -> f32 {
    if y < area.top() {
        f32::from(y - area.top())
    } else if y > area.bottom() {
        f32::from(y - area.bottom())
    } else {
        0.
    }
}

impl Finder {
    /// What scrolls the view the box is drawn in, if this view has a box.
    fn marquee_view(&self) -> Option<&dyn ScrollbarHandle> {
        match self.effective_view() {
            ViewMode::List => Some(&self.list_scroll),
            ViewMode::Icons => Some(&self.grid_scroll),
            ViewMode::Columns | ViewMode::Gallery => None,
        }
    }

    /// The mouse went down where a drag draws a box. Call it before the press
    /// changes the selection.
    pub(super) fn begin_marquee(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(scroller) = self.marquee_view() else {
            return;
        };
        let anchor = event.position - scroller.viewport_bounds().origin - scroller.offset();
        let adding = event.modifiers.secondary(cx) || event.modifiers.shift;
        let kept = match adding {
            true => self.selection.indices().collect(),
            false => BTreeSet::new(),
        };
        self.marquee = Some(Marquee {
            anchor,
            mouse: event.position,
            dragging: false,
            kept,
            autoscroll: None,
        });
        cx.notify();
    }

    /// The mouse moved anywhere in the window during a press.
    fn marquee_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_marquee(cx);
            return;
        }
        let Some(area) = self.marquee_view().map(|view| view.viewport_bounds()) else {
            return;
        };
        let Some(offset) = self.marquee_view().map(|view| view.offset()) else {
            return;
        };
        let Some(marquee) = self.marquee.as_mut() else {
            return;
        };
        marquee.mouse = event.position;
        let start = area.origin + offset + marquee.anchor;
        let moved = (event.position.x - start.x)
            .abs()
            .max((event.position.y - start.y).abs());
        marquee.dragging |= moved >= ui(DRAG_THRESHOLD);
        if !marquee.dragging {
            return;
        }
        if past_edge(event.position.y, &area) != 0. && marquee.autoscroll.is_none() {
            marquee.autoscroll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(AUTOSCROLL_TICK).await;
                    let keep_going = this.update(cx, |this, cx| this.marquee_autoscroll(cx));
                    if !matches!(keep_going, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        self.select_in_marquee(cx);
    }

    /// Scroll toward the mouse once. Returns whether it's still past an edge.
    fn marquee_autoscroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mouse) = self.marquee.as_ref().map(|marquee| marquee.mouse) else {
            return false;
        };
        let Some(scroller) = self.marquee_view() else {
            return false;
        };
        let area = scroller.viewport_bounds();
        let past = past_edge(mouse.y, &area);
        if past == 0. {
            if let Some(marquee) = self.marquee.as_mut() {
                marquee.autoscroll = None;
            }
            return false;
        }
        let step = (past * AUTOSCROLL_PER_POINT).clamp(-AUTOSCROLL_MAX, AUTOSCROLL_MAX);
        let room = (scroller.content_size().height - area.size.height).max(Pixels::ZERO);
        let offset = scroller.offset();
        // Scroll offsets count down from 0 as the content moves up.
        let y = (offset.y - ui(step)).clamp(-room, Pixels::ZERO);
        scroller.set_offset(point(offset.x, y));
        self.select_in_marquee(cx);
        true
    }

    /// Select what the box covers, along with what was already kept.
    fn select_in_marquee(&mut self, cx: &mut Context<Self>) {
        let Some(scroller) = self.marquee_view() else {
            return;
        };
        let origin = scroller.viewport_bounds().origin + scroller.offset();
        let Some(marquee) = self.marquee.as_ref() else {
            return;
        };
        let sides = box_sides(marquee.anchor, marquee.mouse - origin);
        let mut items = marquee.kept.clone();
        match self.effective_view() {
            ViewMode::List => self.rows_in_box(sides, &mut items),
            ViewMode::Icons => self.tiles_in_box(sides, &mut items),
            ViewMode::Columns | ViewMode::Gallery => return,
        }
        self.selection.select_items(items);
        self.selection_changed(cx);
        cx.notify();
    }

    /// The rows of the list that the box reaches, top to bottom.
    fn rows_in_box(
        &self,
        (_, top, _, bottom): (Pixels, Pixels, Pixels, Pixels),
        items: &mut BTreeSet<usize>,
    ) {
        let row = ui(LIST_ROW);
        let line = |y: Pixels| (y / row).max(0.) as usize;
        let last = self.list_line_count().saturating_sub(1);
        for line in line(top)..=line(bottom).min(last) {
            if let Some(ListLine::Row(ix, _)) = self.list_line(line) {
                items.insert(ix);
            }
        }
    }

    /// The tiles of the icon view that the box touches.
    fn tiles_in_box(
        &self,
        (left, top, right, bottom): (Pixels, Pixels, Pixels, Pixels),
        items: &mut BTreeSet<usize>,
    ) {
        let Some(layout) = self.icon_layout.as_ref() else {
            return;
        };
        let (tile_width, _) = tile_size(self.config.icon_size);
        let (padding, width) = (ui(GRID_PADDING), ui(tile_width));
        // A tile's sides are blank margin; the box has to reach its icon.
        let margin = ui((tile_width - self.config.icon_size - ICON_PADDING).max(0.) / 2.);
        let mut y = Pixels::ZERO;
        for (line, size) in layout.lines.iter().zip(layout.sizes.iter()) {
            if y > bottom {
                break;
            }
            let end = y + size.height;
            if let IconLine::Tiles(tiles) = line
                && end >= top
            {
                for (column, ix) in tiles.clone().enumerate() {
                    let start = padding + width * column as f32;
                    if start + margin <= right && start + width - margin >= left {
                        items.insert(ix);
                    }
                }
            }
            y = end;
        }
    }

    /// The mouse went up, or the press ended some other way.
    fn end_marquee(&mut self, cx: &mut Context<Self>) {
        self.marquee = None;
        cx.notify();
    }

    /// Follows the mouse over the whole window during a press, so the box
    /// keeps going when the mouse leaves the view.
    pub(super) fn marquee_capture(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        moved.update(cx, |this, cx| this.marquee_moved(event, cx));
                    }
                });
                let released = view.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        released.update(cx, |this, cx| this.end_marquee(cx));
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// The box itself, inside the view it selects in.
    pub(super) fn render_marquee(&self, cx: &App) -> Option<impl IntoElement> {
        let marquee = self.marquee.as_ref().filter(|marquee| marquee.dragging)?;
        let scroller = self.marquee_view()?;
        let area = scroller.viewport_bounds();
        // Drawn relative to the view, and only as far as the view goes.
        let inside = |corner: Point<Pixels>| {
            point(
                corner.x.clamp(Pixels::ZERO, area.size.width),
                corner.y.clamp(Pixels::ZERO, area.size.height),
            )
        };
        let (left, top, right, bottom) = box_sides(
            inside(marquee.anchor + scroller.offset()),
            inside(marquee.mouse - area.origin),
        );
        // The text color shows up over the selection color, in either theme.
        let color = cx.theme().foreground;
        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(right - left)
                .h(bottom - top)
                .bg(color.opacity(0.12))
                .border_1()
                .border_color(color.opacity(0.55)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{box_sides, past_edge};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn the_box_has_the_same_sides_whichever_way_it_is_dragged() {
        let (a, b) = (point(px(30.), px(5.)), point(px(10.), px(20.)));

        assert_eq!(box_sides(a, b), box_sides(b, a));
        assert_eq!(box_sides(a, b), (px(10.), px(5.), px(30.), px(20.)));
    }

    #[test]
    fn past_edge_measures_from_the_nearest_edge() {
        let area = Bounds::new(point(px(0.), px(100.)), size(px(50.), px(200.)));

        assert_eq!(past_edge(px(90.), &area), -10.);
        assert_eq!(past_edge(px(150.), &area), 0.);
        assert_eq!(past_edge(px(330.), &area), 30.);
    }
}
