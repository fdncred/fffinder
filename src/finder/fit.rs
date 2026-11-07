//! Text that may not fit its space: a tooltip with all of it when it's cut
//! off, and measuring how much room it needs.

use crate::theme::ui;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;
use std::cmp::Reverse;

/// How wide a tooltip grows before its text wraps, in points.
const TOOLTIP_MAX_WIDTH: f32 = 560.;

/// How many of a column's longest texts to measure when fitting it. Width
/// follows length closely enough that the widest is among them, and shaping
/// every row of a large list would hold up the click.
pub(super) const FIT_SAMPLE: usize = 256;

/// Add text that may be cut short to `el`, and give `el` a tooltip with all of
/// the text for when it is. They go on together because the tooltip asks the
/// text what was drawn, and panics if it never was.
///
/// # Arguments
///
/// * `el` - What the mouse rests on to see the tooltip
/// * `shown` - The text element, which draws `full` and may cut it short
/// * `full` - The whole text
/// * `wrap` - Whatever goes around `shown` inside `el`; `|shown| shown` for nothing
///
/// # Returns
///
/// `el` with the text and the tooltip
pub(super) fn with_cut_off_text<E, C>(
    el: E,
    shown: StyledText,
    full: SharedString,
    wrap: impl FnOnce(StyledText) -> C,
) -> E
where
    E: ParentElement + StatefulInteractiveElement,
    C: IntoElement,
{
    let tooltip = cut_off_tooltip(&shown, full);
    el.child(wrap(shown)).tooltip(tooltip)
}

/// A tooltip builder that shows all of `full` only when `shown` cut it short.
/// Text layout decides what fits, so this asks it at hover time instead of
/// measuring anything while drawing.
fn cut_off_tooltip(
    shown: &StyledText,
    full: SharedString,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let layout = shown.layout().clone();
    move |window, cx| {
        // `TextLayout::text` is what was drawn, with its ellipsis if it has one.
        if layout.text() == full.as_ref() {
            return cx.new(|_| EmptyView).into();
        }
        let text = full.clone();
        Tooltip::element(move |_, _| {
            div()
                .id("cut-off-text")
                .aria_label(text.clone())
                .max_w(ui(TOOLTIP_MAX_WIDTH))
                .text_size(ui(12.))
                .child(text.clone())
                .test_support()
        })
        .build(window, cx)
    }
}

/// How wide `text` is when drawn in `font` at `size`.
fn text_width(text: SharedString, font: &Font, size: Pixels, window: &Window) -> Pixels {
    // A single shaped line can't hold a line break, which a file name can.
    let text: SharedString = match text.contains('\n') {
        true => text.replace('\n', " ").into(),
        false => text,
    };
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, size, &[run], None)
        .width
}

/// Keep only the `keep` longest `(text, _)` items, by bytes.
fn longest<T>(mut items: Vec<(SharedString, T)>, keep: usize) -> Vec<(SharedString, T)> {
    if items.len() > keep {
        // A partial sort: the longest end up before `keep`, in no order.
        items.select_nth_unstable_by_key(keep, |(text, _)| Reverse(text.len()));
        items.truncate(keep);
    }
    items
}

/// The room in points that the widest of `texts` needs. Each comes with the
/// points its row adds around the text, for padding, icons, and indents.
///
/// # Arguments
///
/// * `texts` - Each text and the points around it
/// * `font` - The font the texts are drawn in
/// * `size` - The size they're drawn at, already scaled
/// * `window` - The window, for its text system
///
/// # Returns
///
/// The widest text plus its extra points, or 0 when there are no texts
pub(super) fn widest_points(
    texts: Vec<(SharedString, f32)>,
    font: &Font,
    size: Pixels,
    window: &Window,
) -> f32 {
    let scale = crate::theme::ui_scale();
    longest(texts, FIT_SAMPLE)
        .into_iter()
        .map(|(text, extra)| f32::from(text_width(text, font, size, window)) / scale + extra)
        .fold(0., f32::max)
}

#[cfg(test)]
mod tests {
    use super::longest;
    use gpui_kit::SharedString;

    #[test]
    fn longest_keeps_the_longest_texts() {
        let items: Vec<(SharedString, f32)> = ["aaaa", "b", "ccc", "dd"]
            .into_iter()
            .map(|text| (text.into(), 0.))
            .collect();

        let mut kept: Vec<String> = longest(items.clone(), 2)
            .into_iter()
            .map(|(text, _)| text.to_string())
            .collect();
        kept.sort();

        assert_eq!(kept, ["aaaa", "ccc"]);
        assert_eq!(
            longest(items, 9).len(),
            4,
            "fewer than asked keeps them all"
        );
    }
}
