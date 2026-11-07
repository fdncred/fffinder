//! Finishing the path typed in Go to Folder: the matches listed under the
//! field, and Tab.

use crate::theme::ui;
use fffinder_core::listing::{self, PathCompletion};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// How many matches to list; Tab still counts them all.
const SHOWN: usize = 8;

/// The items that match the typed path. It's a view of its own so it redraws
/// when the folder has been read, without the dialog around it doing so.
pub(super) struct PathCompletions {
    input: Entity<InputState>,
    base: Option<PathBuf>,
    matches: Vec<PathCompletion>,
    /// Reads the folder; a newer read replaces it.
    task: Task<()>,
    _changes: Subscription,
}

impl PathCompletions {
    /// A list that follows `input`, with relative paths starting from `base`.
    pub(super) fn new(
        input: Entity<InputState>,
        base: Option<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<PathCompletions> {
        cx.new(|cx| PathCompletions {
            _changes: cx.subscribe_in(
                &input,
                window,
                |this: &mut PathCompletions, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        this.refresh(cx);
                    }
                },
            ),
            input,
            base,
            matches: Vec::new(),
            task: Task::ready(()),
        })
    }

    /// Read the folder the typed path has reached, off the UI thread, since
    /// it may be a slow disk or a server.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().to_string();
        let base = self.base.clone();
        self.task = cx.spawn(async move |this, cx| {
            let matches = cx
                .background_spawn(async move { listing::complete_path(&typed, base.as_deref()) })
                .await;
            this.update(cx, |this, cx| {
                this.matches = matches;
                cx.notify();
            })
            .ok();
        });
    }

    /// Tab: finish the path as far as every match agrees.
    pub(super) fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = listing::common_completion(&self.matches) {
            self.fill(text, window, cx);
        }
    }

    fn fill(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.refresh(cx);
    }
}

impl Render for PathCompletions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (hover, muted) = (theme.list_hover, theme.muted_foreground);
        let extra = self.matches.len().saturating_sub(SHOWN);
        let rows = self
            .matches
            .iter()
            .take(SHOWN)
            .enumerate()
            .map(|(ix, item)| {
                let text = item.text.clone();
                h_flex()
                    .id(("goto-match", ix))
                    .gap(ui(6.))
                    .px(ui(6.))
                    .py(ui(2.))
                    .rounded(ui(4.))
                    .text_size(ui(12.))
                    .hover(move |style| style.bg(hover))
                    .child(div().truncate().child(item.name.clone()))
                    .when(item.is_dir, |this| {
                        this.child(div().text_color(muted).child(std::path::MAIN_SEPARATOR_STR))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.fill(text.clone(), window, cx)
                    }))
                    .test_support()
            });
        v_flex().gap(ui(1.)).children(rows).when(extra > 0, |this| {
            this.child(
                div()
                    .px(ui(6.))
                    .text_size(ui(11.))
                    .text_color(muted)
                    .child(format!("and {extra} more")),
            )
        })
    }
}
