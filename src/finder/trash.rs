//! The Trash, Undo, and Redo: moving items to the Trash, the Trash view
//! with Put Back and Empty, deleting items for good, and reversing the
//! latest file operation or undo.

use super::Finder;
use super::views::DraggedFiles;
use crate::actions::*;
use crate::model::Location;
use crate::theme::ui;
use crate::trash_bin::{self, Bin, TrashError, TrashStopped, Trashed};
use crate::undo::{self, Change, Step};
use fffinder_core::ops;
use fffinder_core::{FileEntry, Gone, Leaving};
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::input;
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex,
};
use gpui_kit::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// What file work changed that the index learns of at once, rather than
/// from a watcher, or from a Rescan with watching off: items that left the
/// disk, and items put back from the Trash.
#[derive(Debug, Clone, Default, PartialEq)]
struct IndexUpdate {
    gone: Gone,
    back: Vec<PathBuf>,
}

impl IndexUpdate {
    /// Items that left the disk.
    fn left(gone: Gone) -> IndexUpdate {
        IndexUpdate {
            gone,
            back: Vec::new(),
        }
    }

    /// Items put back from the Trash, where they are now.
    fn returned(back: Vec<PathBuf>) -> IndexUpdate {
        IndexUpdate {
            gone: Gone::default(),
            back,
        }
    }

    /// Whether the index has nothing to learn.
    fn is_empty(&self) -> bool {
        self.gone.is_empty() && self.back.is_empty()
    }
}

/// What file work did: whether it failed, and what it changed for the
/// index, even when it stopped part way.
type FileJobOutcome = (Result<(), TrashError>, IndexUpdate);

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
        let paths: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        if !paths.is_empty() {
            self.trash_now(paths, window, cx);
        }
    }

    pub(super) fn in_trash(&self) -> bool {
        !self.is_searching() && matches!(self.location(), Location::Trash)
    }

    /// Move items to the Trash now; Undo puts them back. An item inside
    /// another one being trashed goes with it.
    pub fn trash_now(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        // These may have been dropped on the Trash, which ends a drag.
        self.spring_folder = None;
        if self.wait_for_undo(window, cx) {
            return;
        }
        let bin = trash_bin::bin(cx);
        let paths = ops::outermost(&paths);
        cx.spawn_in(window, async move |this, cx| {
            let tried = paths.clone();
            let (result, gone) = cx
                .background_spawn(async move {
                    let leaving = Leaving::note(&paths);
                    let result = bin.trash(&paths);
                    (result, leaving.gone())
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let next = item_after(&this.rows, &tried);
                // The window leaves a folder that went to the Trash, or was
                // inside one that did, for the folder that held it.
                let leave_for = this
                    .current_folder()
                    .and_then(|current| gone.paths().find(|path| current.starts_with(path)))
                    .and_then(Path::parent)
                    .map(Path::to_path_buf);
                this.selection.clear();
                // What went before a failure can still be put back.
                let (trashed, failed) = match result {
                    Ok(trashed) => (trashed, None),
                    Err(TrashStopped { trashed, error }) => (trashed, Some(error)),
                };
                undo::record("Move to Trash", Change::Trashed(trashed), cx);
                let mut select: Vec<PathBuf> = next.into_iter().collect();
                if let Some(err) = failed {
                    window.push_notification(
                        (
                            NotificationType::Error,
                            format!("Couldn’t move to the Trash: {err}"),
                        ),
                        cx,
                    );
                    // What's still there stays selected, to try again.
                    let left: Vec<PathBuf> = tried
                        .into_iter()
                        .filter(|path| path.symlink_metadata().is_ok())
                        .collect();
                    if !left.is_empty() {
                        select = left;
                    }
                }
                if let Some(parent) = leave_for {
                    this.navigate(Location::Folder(parent), window, cx);
                }
                this.items_changed(IndexUpdate::left(gone), select, cx);
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
        self.run_file_job(
            move || match bin.restore(&items) {
                Ok(back) => (Ok(()), IndexUpdate::returned(back)),
                Err(err) => (Err(err), IndexUpdate::returned(out_of_trash(&items))),
            },
            window,
            cx,
        );
    }

    /// Delete the selection for good, after asking.
    pub(super) fn delete_immediately(
        &mut self,
        _: &DeleteImmediately,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selected_entries();
        if entries.is_empty() || self.wait_for_undo(window, cx) {
            return;
        }
        let title = match entries.as_slice() {
            [one] => format!("Delete “{}” immediately?", one.name),
            many => format!("Delete {} items immediately?", many.len()),
        };
        let trashed = self.in_trash().then(|| self.selected_trashed());
        let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
        let paths = ops::outermost(&paths);
        let bin = trash_bin::bin(cx);
        self.confirm_danger(title, "Delete", window, cx, move || match &trashed {
            Some(items) => delete_trashed(bin.as_ref(), items),
            None => {
                let leaving = Leaving::note(&paths);
                let result = paths
                    .iter()
                    .try_for_each(|p| ops::remove_any(p).map_err(TrashError::from));
                (result, IndexUpdate::left(leaving.gone()))
            }
        });
    }

    /// Delete everything in the Trash for good, after asking.
    pub(super) fn empty_trash(
        &mut self,
        _: &EmptyTrash,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.wait_for_undo(window, cx) {
            return;
        }
        let bin = trash_bin::bin(cx);
        self.confirm_danger(
            "Permanently erase the items in the Trash?".into(),
            "Empty Trash",
            window,
            cx,
            move || match bin.list() {
                Ok(items) => delete_trashed(bin.as_ref(), &items),
                Err(err) => (Err(err), IndexUpdate::default()),
            },
        );
    }

    /// Ask "You can't undo this" before `job`, then run it.
    fn confirm_danger(
        &mut self,
        title: String,
        ok: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl Fn() -> FileJobOutcome + Clone + Send + 'static,
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
        job: impl FnOnce() -> FileJobOutcome + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.wait_for_undo(window, cx) {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let (result, update) = cx.background_spawn(async move { job() }).await;
            this.update_in(cx, |this, window, cx| {
                if let Err(err) = result {
                    window.push_notification((NotificationType::Error, err.to_string()), cx);
                }
                let selected = this.selected_entries().into_iter().map(|e| e.path);
                let next = item_after(&this.rows, &selected.collect::<Vec<_>>());
                this.selection.clear();
                this.items_changed(update, next.into_iter().collect(), cx);
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

    /// Whether an Undo or Redo is still running, which no other file
    /// operation may start during; says so when it is.
    pub(super) fn wait_for_undo(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(step) = undo::running(cx) else {
            return false;
        };
        window.push_notification(
            (
                NotificationType::Warning,
                format!("Wait for {} to finish", step.name()),
            ),
            cx,
        );
        true
    }

    /// Reverse the latest operation (Undo) or undo (Redo), and keep its
    /// opposite for the other one. When it stops part way, what it reversed
    /// is kept for the other one too, and the rest stays to be tried again.
    fn step_file_history(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        if self.wait_for_undo(window, cx) {
            return;
        }
        // Undo could move items a running copy is still reading or writing.
        if self.transfers_running() {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "Wait for copying and moving to finish".to_string(),
                ),
                cx,
            );
            return;
        }
        let Some((operation, running)) = undo::take_last(step, cx) else {
            let message = match step {
                Step::Undo => "There’s nothing to undo",
                Step::Redo => "There’s nothing to redo",
            };
            window.push_notification(message, cx);
            return;
        };
        let bin = trash_bin::bin(cx);
        // Undoing a Move to Trash or a replace puts items back from the Trash.
        let restores = matches!(operation.change, Change::Trashed(_) | Change::Replaced(_));
        cx.spawn_in(window, async move |this, cx| {
            let name = operation.name;
            let (result, gone, running) = cx
                .background_spawn(async move {
                    // Reversing a copy or New Folder, or redoing a Move to
                    // Trash, trashes what it made.
                    let made: &[PathBuf] = match &operation.change {
                        Change::Created(made) => made,
                        Change::Moved(_) | Change::Trashed(_) | Change::Replaced(_) => &[],
                    };
                    let leaving = Leaving::note(made);
                    let result = undo::reverse(&operation.change, bin.as_ref());
                    (result, leaving.gone(), running)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let index_update = |back: &[PathBuf]| IndexUpdate {
                    gone,
                    back: if restores { back.to_vec() } else { Vec::new() },
                };
                // What Undo or Redo moved is followed like any other move.
                let made = match &result {
                    Ok(reversed) => &reversed.opposite,
                    Err(stopped) => &stopped.done.opposite,
                };
                if let Change::Moved(pairs) = made {
                    this.follow_moves(pairs, window, cx);
                }
                match result {
                    Ok(reversed) => {
                        let opposite = undo::Operation {
                            name,
                            change: reversed.opposite,
                        };
                        undo::stepped(step, opposite, cx);
                        this.items_changed(index_update(&reversed.back), reversed.back, cx);
                        window.push_notification(format!("{} {name}", step.past()), cx);
                    }
                    Err(stopped) => {
                        let undo::Stopped { done, left, error } = *stopped;
                        undo::stepped(
                            step,
                            undo::Operation {
                                name,
                                change: done.opposite,
                            },
                            cx,
                        );
                        undo::put_back(step, undo::Operation { name, change: left }, cx);
                        this.items_changed(index_update(&done.back), done.back, cx);
                        let verb = match step {
                            Step::Undo => "undo",
                            Step::Redo => "redo",
                        };
                        window.push_notification(
                            (
                                NotificationType::Error,
                                format!("Couldn’t {verb} {name}: {error}"),
                            ),
                            cx,
                        );
                    }
                }
            })
            .ok();
            // Other file operations may start once the log is up to date.
            drop(running);
        })
        .detach();
    }

    /// Items changed on disk: reload the view, keep only search results
    /// that still exist, and select `select` where it shows.
    pub(super) fn files_changed(&mut self, select: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Search results come from anywhere, and so do open folders' items.
        let shows = |path: &PathBuf| {
            self.is_searching()
                || path.parent() == self.current_folder()
                || path.parent().is_some_and(|dir| self.expanded.contains(dir))
        };
        let here: Vec<PathBuf> = select.into_iter().filter(shows).collect();
        self.load_listing(here, true, cx);
        if self.is_searching() {
            self.results.retain(|r| r.entry.path.exists());
            self.rebuild_rows(cx);
        }
        self.reload_columns(cx);
        self.check_tree_for_changes(cx);
        self.tell_others(cx);
    }

    /// Files changed on disk, some of them by leaving it or coming back from
    /// the Trash. The window catches up now. The index catches up off the
    /// UI thread, and then what this tab shows from it and the other tabs,
    /// so Recents, folder sizes, and searches match the disk without waiting
    /// for a watcher, or for a Rescan with watching off. Rows of items that
    /// left go at once, since forgetting a folder reads every indexed file.
    fn items_changed(&mut self, update: IndexUpdate, select: Vec<PathBuf>, cx: &mut Context<Self>) {
        // The Trash lists what's in it, and where each item came from, from
        // the disk.
        if update.gone.is_empty() || matches!(self.location(), Location::Trash) {
            self.files_changed(select, cx);
        } else {
            self.drop_rows(&update.gone, &select, cx);
        }
        if update.is_empty() {
            return;
        }
        let restored = !update.back.is_empty();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let changed = cx
                .background_spawn(async move {
                    engine.forget(&update.gone) + engine.remember(&update.back)
                })
                .await;
            if changed > 0 {
                this.update(cx, |this, cx| this.index_changed(restored, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Drop the rows of items that left the disk, select `select`, and
    /// reread what comes from the disk rather than the index, here and in
    /// the other tabs, as a reload of the listing would.
    fn drop_rows(&mut self, gone: &Gone, select: &[PathBuf], cx: &mut Context<Self>) {
        let left: HashSet<&Path> = gone.paths().collect();
        let stays = |path: &Path| !inside_any(path, &left);
        self.listing.retain(|entry| stays(&entry.path));
        self.results.retain(|row| stays(&row.entry.path));
        // Folders open in place in the list keep their own items.
        for folder in self.children.values_mut() {
            folder.entries.retain(|entry| stays(&entry.path));
        }
        self.rebuild_rows(cx);
        self.select_paths(select, cx);
        self.tree_saw_listing(cx);
        self.load_free_space(cx);
        self.reload_columns(cx);
        self.check_tree_for_changes(cx);
        self.tell_others(cx);
    }

    /// The index changed: reread what this tab shows from it, and tell the
    /// other tabs, which search again. What this tab shows from the disk is
    /// current already, so a listing loading since, as after New Folder,
    /// keeps the item it's about to select.
    fn index_changed(&mut self, restored: bool, cx: &mut Context<Self>) {
        match self.location() {
            Location::Recents => self.load_listing(Vec::new(), true, cx),
            Location::Folder(_) => self.refresh_folder_stats(cx),
            Location::Results { .. } | Location::Trash => {}
        }
        if self.is_searching() {
            // What left is out of the rows already, but not what came back.
            if restored {
                self.run_search(false, cx);
            } else {
                self.results.retain(|r| r.entry.path.exists());
                self.rebuild_rows(cx);
            }
        }
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

/// The item to select once `gone` have left the list: the first after the
/// last of them, or else the nearest before. An item inside a folder that's
/// going goes with it.
fn item_after(rows: &[super::Row], gone: &[PathBuf]) -> Option<PathBuf> {
    let gone: HashSet<&Path> = gone.iter().map(PathBuf::as_path).collect();
    let is_gone = |row: &super::Row| inside_any(&row.entry.path, &gone);
    let last = rows.iter().rposition(is_gone)?;
    rows[last + 1..]
        .iter()
        .chain(rows[..last].iter().rev())
        .find(|row| !is_gone(row))
        .map(|row| row.entry.path.clone())
}

/// Delete `items` in the Trash for good, noted first: a search location
/// holding the Trash can index it, as fff does hidden folders inside a git
/// repository.
fn delete_trashed(bin: &dyn Bin, items: &[Trashed]) -> FileJobOutcome {
    let leaving = Leaving::note(items.iter().map(|item| &item.path));
    let result = bin.delete(items);
    (result, IndexUpdate::left(leaving.gone()))
}

/// Where the items of `items` that left the Trash are now, after a Put Back
/// that stopped part way.
fn out_of_trash(items: &[Trashed]) -> Vec<PathBuf> {
    items
        .iter()
        .filter(|item| !item.in_trash())
        .filter_map(|item| item.original.clone())
        .collect()
}

/// Whether `path` is one of `gone` or inside one of them.
fn inside_any(path: &Path, gone: &HashSet<&Path>) -> bool {
    path.ancestors().any(|dir| gone.contains(dir))
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

#[cfg(test)]
mod tests {
    use super::{IndexUpdate, delete_trashed, item_after, out_of_trash};
    use crate::finder::Row;
    use crate::trash_bin::{Bin, FolderBin};
    use fffinder_core::{FileEntry, Gone, Leaving};
    use std::path::{Path, PathBuf};

    #[test]
    fn deleting_from_the_trash_notes_what_left() -> Result<(), Box<dyn std::error::Error>> {
        let dir =
            std::env::temp_dir().join(format!("fffinder-delete-trashed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder"))?;
        std::fs::write(dir.join("folder/inner.txt"), "1")?;
        std::fs::write(dir.join("file.txt"), "1")?;
        let bin = FolderBin::new(dir.join("trash"));
        let trashed = bin
            .trash(&[dir.join("file.txt"), dir.join("folder")])
            .map_err(|stopped| stopped.error)?;

        let (result, update) = delete_trashed(&bin, &trashed);

        result?;
        let left: Vec<&Path> = update.gone.paths().collect();
        let in_trash: Vec<&Path> = trashed.iter().map(|t| t.path.as_path()).collect();
        assert_eq!(left, in_trash);
        assert!(update.back.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn what_left_the_trash_is_where_it_came_from() -> Result<(), Box<dyn std::error::Error>> {
        let dir =
            std::env::temp_dir().join(format!("fffinder-out-of-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("a.txt"), "1")?;
        std::fs::write(dir.join("b.txt"), "1")?;
        let bin = FolderBin::new(dir.join("trash"));
        let trashed = bin
            .trash(&[dir.join("a.txt"), dir.join("b.txt")])
            .map_err(|stopped| stopped.error)?;
        let (first, rest) = trashed.split_first().ok_or("the Trash holds nothing")?;
        bin.restore(std::slice::from_ref(first))?;

        assert_eq!(out_of_trash(&trashed), [dir.join("a.txt")]);
        assert!(out_of_trash(rest).is_empty(), "b.txt is still in the Trash");
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn an_index_update_says_whether_there_is_anything_to_learn() {
        assert!(IndexUpdate::default().is_empty());
        assert!(IndexUpdate::left(Gone::default()).is_empty());
        assert!(!IndexUpdate::returned(vec![PathBuf::from("/back")]).is_empty());
        let missing = std::env::temp_dir().join("fffinder-index-update-missing");
        let gone = Leaving::note([&missing]).gone();
        assert!(!IndexUpdate::left(gone).is_empty());
    }

    /// A row for each of `names` under `/list`, which needn't exist.
    fn rows(names: &[&str]) -> Vec<Row> {
        let meta = std::fs::metadata(std::env::temp_dir()).unwrap();
        names
            .iter()
            .map(|name| {
                let entry = FileEntry::from_metadata(Path::new("/list").join(name), &meta, false);
                Row::new(entry, Vec::new(), None)
            })
            .collect()
    }

    fn gone(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| Path::new("/list").join(name))
            .collect()
    }

    fn after(rows: &[Row], names: &[&str]) -> Option<PathBuf> {
        item_after(rows, &gone(names))
    }

    #[test]
    fn the_next_item_is_the_one_after_the_last_to_go() {
        let list = rows(&["a", "b", "c", "d"]);

        assert_eq!(after(&list, &["b"]), Some("/list/c".into()));
        assert_eq!(after(&list, &["a", "c"]), Some("/list/d".into()));
    }

    #[test]
    fn after_the_last_row_the_next_item_is_the_nearest_before_it() {
        let list = rows(&["a", "b", "c"]);

        assert_eq!(after(&list, &["c"]), Some("/list/b".into()));
        assert_eq!(after(&list, &["b", "c"]), Some("/list/a".into()));
    }

    #[test]
    fn an_open_folders_items_go_with_it() {
        let list = rows(&["a", "dir", "dir/one", "dir/two", "z"]);

        assert_eq!(after(&list, &["dir"]), Some("/list/z".into()));

        let at_the_end = rows(&["a", "dir", "dir/one"]);
        assert_eq!(
            after(&at_the_end, &["dir"]),
            Some("/list/a".into()),
            "not an item inside it"
        );
    }

    #[test]
    fn with_nothing_left_or_nothing_listed_there_is_no_next_item() {
        let list = rows(&["a", "b"]);

        assert_eq!(after(&list, &["a", "b"]), None);
        assert_eq!(after(&list, &["not listed"]), None);
    }
}
