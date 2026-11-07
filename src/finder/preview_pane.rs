//! The preview pane: a panel beside the list and icon views that shows the
//! selected item. The gallery and column views have previews of their own.

use super::Finder;
use super::list::ResizeEdge;
use super::views::PANE_EDGE;
use crate::actions::TogglePreviewPane;
use crate::theme::ui;
use fffinder_core::ViewMode;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::*;

impl Finder {
    /// Whether this view shows the preview pane.
    pub(super) fn shows_preview_pane(&self) -> bool {
        self.config.show_preview_pane
            && matches!(self.effective_view(), ViewMode::List | ViewMode::Icons)
    }

    /// View › Toggle Preview Pane.
    pub(super) fn toggle_preview_pane(
        &mut self,
        _: &TogglePreviewPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.show_preview_pane = !self.config.show_preview_pane;
        self.settings_changed(cx);
        self.selection_changed(cx);
        cx.notify();
    }

    /// The pane, with the strip at its left edge that drags it.
    pub(super) fn render_preview_pane(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.shows_preview_pane() {
            return None;
        }
        let handle = self
            .resize_handle("resize-preview", ResizeEdge::PreviewPane, cx)
            .w(ui(PANE_EDGE))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .test_support();
        let body = self.preview_pane_body(cx);
        Some(
            gpui_kit::component::h_flex()
                .id("preview-pane")
                .h_full()
                .flex_shrink_0()
                .w(ui(self.config.preview_pane_width()))
                .child(handle)
                .child(div().flex_1().min_w_0().h_full().p(ui(14.)).child(body))
                .test_support()
                .into_any_element(),
        )
    }

    /// What the pane shows: one item, a summary of several, or nothing.
    fn preview_pane_body(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let selected = match self.effective_view() {
            ViewMode::Columns => self.column_selection().map_or(0, |_| 1),
            _ => self.selection.len(),
        };
        if selected > 1 {
            let count = SharedString::from(format!("{selected} items"));
            return v_flex()
                .size_full()
                .gap(ui(4.))
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("preview-pane-many")
                        .aria_label(count.clone())
                        .text_size(ui(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(count)
                        .test_support(),
                )
                .children(self.selected_size().map(|size| {
                    div()
                        .text_size(ui(11.))
                        .text_color(muted)
                        .child(fffinder_core::format::human_size(size))
                }))
                .into_any_element();
        }
        match self.primary_entry() {
            Some(entry) => super::preview::render_preview_card(self, &entry, cx),
            None => v_flex()
                .id("preview-pane-empty")
                .size_full()
                .items_center()
                .justify_center()
                .text_size(ui(12.))
                .text_color(muted)
                .child("No Selection")
                .test_support()
                .into_any_element(),
        }
    }
}
