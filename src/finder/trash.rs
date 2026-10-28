//! The Trash, Undo, and Redo: moving items to the Trash, the Trash view
//! with Put Back and Empty, deleting items for good, and reversing the
//! latest file operation or undo.

use super::Finder;
use super::views::DraggedFiles;
use crate::actions::*;
use crate::model::Location;
use crate::theme::ui;
use crate::trash_bin::{self, Trashed};
use crate::undo::{self, Change, Step};
use fffinder_core::FileEntry;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::input;
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex,
};
use gpui_kit::*;
use std::path::{Path, PathBuf};

impl Finder {
    /// Cmd+Backspace: move the selection to the Trash, or in the Trash, put
    /// it back.
    pub(super) fn move_to_trash(
        &mut self,
        _: &MoveToTrash,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.in_trash() {
            self.put_back(&PutBack, window, cx);
            return;
        }
        let entries = self.selected_entries();
        if !entries.is_empty() {
            self.confirm_trash(entries, window, cx);
        }
    }

    pub(super) fn in_trash(&self) -> bool {
        !self.is_searching() && matches!(self.location(), Location::Trash)
    }

    /// Move items to the Trash now; Undo puts them back.
    pub fn trash_now(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let bin = trash_bin::bin(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { bin.trash(&paths) }).await;
            this.update_in(cx, |this, window, cx| {
                this.selection.clear();
                match result {
                    Ok(items) => undo::record("Move to Trash", Change::Trashed(items), cx),
                    Err(err) => window.push_notification(
                        (
                            NotificationType::Error,
                            format!("Couldn’t move to the Trash: {err}"),
                        ),
                        cx,
                    ),
                }
                this.files_changed(Vec::new(), cx);
            })
            .ok();
        })
        .detach();
    }

    /// The selected items in the Trash.
    fn selected_trashed(&self) -> Vec<Trashed> {
        let selected: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        self.trashed
            .iter()
            .filter(|t| selected.contains(&t.path))
            .cloned()
            .collect()
    }

    /// Put the selected items in the Trash back where they came from.
    pub(super) fn put_back(&mut self, _: &PutBack, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.selected_trashed();
        if items.is_empty() || !self.in_trash() {
            return;
        }
        let bin = trash_bin::bin(cx);
        self.run_file_job(move || bin.restore(&items).map(|_| ()), window, cx);
    }

    /// Delete the selection for good, after asking.
    pub(super) fn delete_immediately(
        &mut self,
        _: &DeleteImmediately,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selected_entries();
        if entries.is_empty() {
            return;
        }
        let title = match entries.as_slice() {
            [one] => format!("Delete “{}” immediately?", one.name),
            many => format!("Delete {} items immediately?", many.len()),
        };
        let trashed = self.in_trash().then(|| self.selected_trashed());
        let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
        let bin = trash_bin::bin(cx);
        self.confirm_danger(title, "Delete", window, cx, move || match &trashed {
            Some(items) => bin.delete(items),
            None => paths
                .iter()
                .try_for_each(|p| fffinder_core::ops::remove_any(p).map_err(|e| e.to_string())),
        });
    }

    /// Delete everything in the Trash for good, after asking.
    pub(super) fn empty_trash(
        &mut self,
        _: &EmptyTrash,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bin = trash_bin::bin(cx);
        self.confirm_danger(
            "Permanently erase the items in the Trash?".into(),
            "Empty Trash",
            window,
            cx,
            move || bin.delete(&bin.list()?),
        );
    }

    /// Ask "You can't undo this" before `job`, then run it.
    fn confirm_danger(
        &mut self,
        title: String,
        ok: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl Fn() -> Result<(), String> + Clone + Send + 'static,
    ) {
        self.focus_before_dialog(window, cx);
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, job) = (view.clone(), job.clone());
            alert
                .confirm()
                .title(title.clone())
                .description("You can’t undo this.")
                .ok_text(ok)
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let job = job.clone();
                    view.update(cx, |this, cx| this.run_file_job(job, window, cx));
                    true
                })
        });
    }

    /// Run file work off the UI thread, then reload what changed.
    fn run_file_job(
        &mut self,
        job: impl FnOnce() -> Result<(), String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            this.update_in(cx, |this, window, cx| {
                if let Err(err) = result {
                    window.push_notification((NotificationType::Error, err), cx);
                }
                this.selection.clear();
                this.files_changed(Vec::new(), cx);
            })
            .ok();
        })
        .detach();
    }

    /// Cmd+Z in the files: reverse the latest file operation.
    pub(super) fn undo_file_operation(
        &mut self,
        _: &input::Undo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_file_history(Step::Undo, window, cx);
    }

    pub(super) fn redo_file_operation(
        &mut self,
        _: &input::Redo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_file_history(Step::Redo, window, cx);
    }

    /// Reverse the latest operation (Undo) or undo (Redo), and keep its
    /// opposite for the other one.
    fn step_file_history(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let Some(operation) = undo::take_last(step, cx) else {
            let message = match step {
                Step::Undo => "There’s nothing to undo",
                Step::Redo => "There’s nothing to redo",
            };
            window.push_notification(message, cx);
            return;
        };
        let bin = trash_bin::bin(cx);
        cx.spawn_in(window, async move |this, cx| {
            let name = operation.name;
            let result = cx
                .background_spawn(async move { undo::reverse(&operation.change, bin.as_ref()) })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(reversed) => {
                    let opposite = undo::Operation {
                        name,
                        change: reversed.opposite,
                    };
                    undo::stepped(step, opposite, cx);
                    this.files_changed(reversed.back, cx);
                    window.push_notification(format!("{} {name}", step.past()), cx);
                }
                Err(err) => {
                    this.files_changed(Vec::new(), cx);
                    let verb = match step {
                        Step::Undo => "undo",
                        Step::Redo => "redo",
                    };
                    window.push_notification(
                        (
                            NotificationType::Error,
                            format!("Couldn’t {verb} {name}: {err}"),
                        ),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    /// Items changed on disk: reload the view, keep only search results
    /// that still exist, and select `select` where it shows.
    pub(super) fn files_changed(&mut self, select: Vec<PathBuf>, cx: &mut Context<Self>) {
        let here: Vec<PathBuf> = select
            .into_iter()
            .filter(|p| p.parent() == self.current_folder())
            .collect();
        self.load_listing(here, true, cx);
        if self.is_searching() {
            self.results.retain(|r| r.entry.path.exists());
            self.rebuild_rows(cx);
        }
        self.reload_columns(cx);
        self.tell_others(cx);
    }

    /// The bar over the Trash, with its Empty button.
    pub(super) fn render_trash_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .h(ui(36.))
            .flex_shrink_0()
            .px(ui(16.))
            .gap(ui(10.))
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .text_size(ui(12.))
            .text_color(theme.muted_foreground)
            .child(div().flex_1().child("Trash"))
            .child(
                Button::new("empty-trash")
                    .small()
                    .label("Empty…")
                    .disabled(self.rows.is_empty())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.empty_trash(&EmptyTrash, window, cx)
                    })),
            )
    }
}

/// Make an element take files dropped on the Trash: they go there at once,
/// and Undo puts them back.
pub(super) fn trash_drop_target<E: InteractiveElement>(el: E, cx: &mut Context<Finder>) -> E {
    let highlight = cx.theme().drop_target;
    el.drag_over::<DraggedFiles>(move |style, _, _, _| style.bg(highlight))
        .on_drop(cx.listener(|this, files: &DraggedFiles, window, cx| {
            this.trash_now(files.0.clone(), window, cx)
        }))
}

/// The folder an item in the Trash came from, as the list shows it.
pub(super) fn original_folder(item: &Trashed) -> String {
    item.original
        .as_deref()
        .and_then(Path::parent)
        .map(super::tildify)
        .unwrap_or_default()
}

/// Whether every one of `entries` is in the Trash with a known origin.
pub(super) fn can_put_back(trashed: &[Trashed], entries: &[FileEntry]) -> bool {
    entries.iter().all(|e| {
        trashed
            .iter()
            .any(|t| t.path == e.path && t.original.is_some())
    })
}
