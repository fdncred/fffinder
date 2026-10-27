//! Renaming several items with a Nushell closure, previewed before anything
//! changes.

use crate::theme::ui;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fffinder_core::FileEntry;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AppState, Finder};
use crate::nushell;
use crate::rename::{self, Outcome, Planned};

const START: &str = "{|f| $f.name }";

/// One-click starting points.
const EXAMPLES: [(&str, &str); 5] = [
    ("Lowercase", "{|f| $f.name | str downcase }"),
    (
        "Spaces to dashes",
        "{|f| $f.name | str replace --all ' ' '-' }",
    ),
    (
        "Number",
        "{|f| $\"($f.index | fill --alignment right --character '0' --width 3) ($f.name)\" }",
    ),
    (
        "Date first",
        "{|f| $\"($f.modified | format date '%Y-%m-%d') ($f.name)\" }",
    ),
    ("New extension", "{|f| $\"($f.stem).txt\" }"),
];

/// Old name, new name, and outcome (for tests).
pub type PreviewRow = (String, String, Outcome);

pub struct BatchRename {
    entries: Vec<FileEntry>,
    input: Entity<InputState>,
    /// New names checked against each other and the folders, or the
    /// closure's error.
    preview: Option<Result<Vec<Planned>, String>>,
    running: bool,
    task: Task<()>,
    debounce: Task<()>,
    _input_events: Subscription,
}

impl Finder {
    pub(super) fn open_batch_rename(
        &mut self,
        entries: Vec<FileEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<AppState>().nu.is_none() {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "Renaming several items uses Nushell, and nu was not found.",
                ),
                cx,
            );
            return;
        }
        self.focus_before_dialog(window, cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(START));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        if let Some(batch) = this.batch.as_mut() {
                            batch.debounce = cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(Duration::from_millis(300))
                                    .await;
                                this.update(cx, |this, cx| this.preview_batch(cx)).ok();
                            });
                        }
                    }
                    InputEvent::PressEnter { .. } => this.preview_batch(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            );
        self.batch = Some(BatchRename {
            entries,
            input: input.clone(),
            preview: None,
            running: false,
            task: Task::ready(()),
            debounce: Task::ready(()),
            _input_events: events,
        });
        self.preview_batch(cx);

        let view = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            }
            let close_view = view.clone();
            dialog
                .title(format!(
                    "Rename {} Items",
                    view.read(cx).batch.as_ref().map_or(0, |b| b.entries.len())
                ))
                .w(ui(760.))
                .child(batch_body(&view, &input, window, cx))
                // Return previews again instead of renaming.
                .on_ok(|_, _, _| false)
                .on_close(move |_, _, cx| close_view.update(cx, |this, _| this.batch = None))
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(Button::new("batch-cancel").label("Cancel")),
                        )
                        .child(
                            Button::new("batch-apply")
                                .primary()
                                .label("Rename")
                                .disabled(!view.read(cx).batch_ready())
                                .on_click(window.listener_for(
                                    &view,
                                    |this: &mut Finder, _: &ClickEvent, window, cx| {
                                        this.apply_batch(window, cx)
                                    },
                                )),
                        ),
                )
        });
    }

    /// Run the closure over the items and check the names it gives.
    fn preview_batch(&mut self, cx: &mut Context<Self>) {
        let Some(nu) = cx.global::<AppState>().nu.clone() else {
            return;
        };
        let Some(batch) = self.batch.as_mut() else {
            return;
        };
        let closure = batch.input.read(cx).value().to_string();
        let entries = batch.entries.clone();
        let cwd = entries
            .first()
            .and_then(|e| e.path.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        batch.running = true;
        batch.task = cx.spawn(async move |this, cx| {
            let preview = cx
                .background_spawn(async move {
                    let names = nushell::rename_names(&nu, &closure, &entries, &cwd)?;
                    let items: Vec<(PathBuf, String)> =
                        entries.iter().map(|e| e.path.clone()).zip(names).collect();
                    Ok(rename::plan(&items))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(batch) = this.batch.as_mut() {
                    batch.running = false;
                    batch.preview = Some(preview);
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// The preview has at least one rename and no problems.
    fn batch_ready(&self) -> bool {
        let Some(BatchRename {
            preview: Some(Ok(planned)),
            running: false,
            ..
        }) = &self.batch
        else {
            return false;
        };
        planned.iter().any(|p| p.outcome == Outcome::Rename)
            && !planned
                .iter()
                .any(|p| matches!(p.outcome, Outcome::Problem(_)))
    }

    fn apply_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.batch_ready() {
            return;
        }
        let Some(BatchRename {
            preview: Some(Ok(planned)),
            ..
        }) = self.batch.take()
        else {
            return;
        };
        match rename::apply(&planned) {
            Ok(moved) => {
                window.close_dialog(cx);
                for (from, _) in planned.iter().map(|p| (&p.from, ())) {
                    self.previews.remove(from);
                }
                self.load_listing(moved.clone(), true, cx);
                if self.is_searching() {
                    self.run_search(false, cx);
                }
                self.focus_files(window, cx);
                let n = moved.len();
                window.push_notification(
                    if n == 1 {
                        "Renamed 1 item".to_string()
                    } else {
                        format!("Renamed {n} items")
                    },
                    cx,
                );
            }
            Err(err) => {
                window.push_notification((NotificationType::Error, err), cx);
                self.batch = None;
                window.close_dialog(cx);
            }
        }
    }

    /// Old name, new name, and outcome for each item (for tests).
    #[doc(hidden)]
    pub fn batch_preview_for_test(&self) -> Option<Result<Vec<PreviewRow>, String>> {
        let preview = self.batch.as_ref()?.preview.as_ref()?;
        Some(preview.as_ref().map_err(Clone::clone).map(|planned| {
            planned
                .iter()
                .map(|p| {
                    (
                        p.from
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        p.new_name.clone(),
                        p.outcome.clone(),
                    )
                })
                .collect()
        }))
    }

    pub fn batch_rename_open(&self) -> bool {
        self.batch.is_some()
    }
}

fn batch_body(
    view: &Entity<Finder>,
    input: &Entity<InputState>,
    _window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let finder = view.read(cx);
    let batch = finder.batch.as_ref();
    let running = batch.is_some_and(|b| b.running);
    let preview = batch.and_then(|b| b.preview.clone());

    let examples = h_flex()
        .gap(ui(6.))
        .flex_wrap()
        .children(EXAMPLES.iter().enumerate().map(|(ix, (label, closure))| {
            let input = input.clone();
            let view = view.clone();
            div()
                .id(("batch-example", ix))
                .px(ui(8.))
                .py(ui(2.))
                .rounded(ui(6.))
                .border_1()
                .border_color(theme.border)
                .text_size(ui(11.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .child(*label)
                .on_click(move |_, window, cx| {
                    input.update(cx, |s, cx| s.set_value(*closure, window, cx));
                    view.update(cx, |this, cx| this.preview_batch(cx));
                })
                .test_support()
        }));

    let mut list = v_flex()
        .id("batch-preview")
        .max_h(ui(300.))
        .overflow_y_scrollbar()
        .border_1()
        .border_color(theme.border)
        .rounded(ui(8.))
        .text_size(ui(12.));
    let mut summary = String::new();
    let mut error = None;
    match &preview {
        Some(Ok(planned)) => {
            let renames = planned
                .iter()
                .filter(|p| p.outcome == Outcome::Rename)
                .count();
            let unchanged = planned
                .iter()
                .filter(|p| p.outcome == Outcome::Unchanged)
                .count();
            let problems = planned.len() - renames - unchanged;
            summary = format!(
                "{renames} will be renamed · {unchanged} unchanged{}",
                if problems > 0 {
                    format!(" · {problems} to fix")
                } else {
                    String::new()
                }
            );
            for (ix, p) in planned.iter().enumerate() {
                let old = p
                    .from
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let (color, note) = match &p.outcome {
                    Outcome::Rename => (theme.foreground, None),
                    Outcome::Unchanged => (muted, None),
                    Outcome::Problem(why) => (theme.danger, Some(why.clone())),
                };
                list = list.child(
                    h_flex()
                        .id(("batch-row", ix))
                        .px(ui(10.))
                        .py(ui(4.))
                        .gap(ui(8.))
                        .when(ix > 0, |this| this.border_t_1().border_color(theme.border))
                        .child(div().w(ui(260.)).truncate().text_color(muted).child(old))
                        .child(
                            Icon::new(IconName::ArrowRight)
                                .with_size(ui(12.))
                                .text_color(muted),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().text_color(color).child(p.new_name.clone()))
                                .when_some(note, |this, note| {
                                    this.child(
                                        div()
                                            .text_size(ui(10.5))
                                            .text_color(theme.danger)
                                            .child(note),
                                    )
                                }),
                        ),
                );
            }
        }
        Some(Err(err)) => error = Some(err.clone()),
        None => {}
    }

    v_flex()
        .gap(ui(10.))
        .child(
            div().text_size(ui(12.)).text_color(muted).child(
                "A Nushell closure gets each item (name, stem, ext, index, path, type, kind, size, modified) and returns its new name.",
            ),
        )
        .child(
            h_flex()
                .gap(ui(8.))
                .child(
                    Input::new(input)
                        .id("batch-closure")
                        .flex_1()
                        .font_family(theme.mono_font_family.clone()),
                )
                .when(running, |this| this.child(Spinner::new().small())),
        )
        .child(examples)
        .when_some(error, |this, error| {
            this.child(
                div()
                    .id("batch-error-scroll")
                    .max_h(ui(120.))
                    .overflow_y_scrollbar()
                    .p(ui(8.))
                    .rounded(ui(6.))
                    .bg(theme.danger.opacity(0.12))
                    .text_color(theme.danger)
                    .font_family(theme.mono_font_family.clone())
                    .text_size(ui(11.))
                    .child(div().id("batch-error").test_support().child(error)),
            )
        })
        .when(!summary.is_empty(), |this| {
            this.child(list).child(
                div()
                    .id("batch-summary")
                    .text_size(ui(11.))
                    .text_color(muted)
                    .child(summary)
                    .test_support(),
            )
        })
}
