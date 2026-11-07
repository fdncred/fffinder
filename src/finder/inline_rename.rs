//! Renaming an item in place: its name becomes a text field in the list,
//! icon, and column views. The gallery has no room to edit a name, so it
//! keeps the dialog.

use super::Finder;
use crate::theme::ui;
use fffinder_core::{FileEntry, ViewMode};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// A name being edited in place.
pub(super) struct InlineRename {
    pub(super) path: PathBuf,
    input: Entity<InputState>,
    _events: Subscription,
}

/// The part of a name to select for retyping: all of a folder's, and a file's
/// up to its extension.
fn name_stem_len(name: &str, is_dir: bool) -> usize {
    if is_dir {
        return name.len();
    }
    name.rfind('.').filter(|&dot| dot > 0).unwrap_or(name.len())
}

impl Finder {
    /// Start renaming the item at `path`, in its row or tile when it has one.
    pub(super) fn begin_rename(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.effective_view() == ViewMode::Gallery {
            self.open_rename_dialog(path, window, cx);
            return;
        }
        if let Some(ix) = self.rows.iter().position(|row| row.entry.path == path) {
            self.scroll_to(ix);
        }
        let name = fffinder_core::entry::display_name(&path);
        let stem = name_stem_len(&name, path.is_dir());
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..stem, cx);
        });
        let events = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.finish_rename(window, cx),
            InputEvent::Blur => this.finish_rename(window, cx),
            _ => {}
        });
        self.renaming = Some(InlineRename {
            path,
            input,
            _events: events,
        });
        cx.notify();
    }

    /// Stop editing and leave the name as it was.
    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping it first means the field losing focus doesn't finish it.
        if self.renaming.take().is_some() {
            self.focus_files(window, cx);
            cx.notify();
        }
    }

    /// Rename the item to what's typed. A name that can't be used says why and
    /// goes on editing.
    pub(super) fn finish_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let empty = renaming.input.read(cx).value().trim().is_empty();
        if empty || self.submit_rename(&renaming.path, &renaming.input, window, cx) {
            self.focus_files(window, cx);
            cx.notify();
            return;
        }
        renaming
            .input
            .update(cx, |state, cx| state.focus(window, cx));
        self.renaming = Some(renaming);
    }

    /// The field that replaces the name at `path`, while it's being edited.
    pub(super) fn rename_field(&self, path: &Path, cx: &mut Context<Self>) -> Option<AnyElement> {
        let renaming = self.renaming.as_ref().filter(|r| r.path == path)?;
        Some(
            div()
                .id("rename-field")
                .flex_1()
                .min_w_0()
                .w_full()
                .text_size(ui(12.))
                // A press in the field edits; it doesn't select or drag the item.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.finish_rename(window, cx)
                }))
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.cancel_rename(window, cx);
                    }
                }))
                .child(Input::new(&renaming.input).id("rename-input").xsmall())
                .into_any_element(),
        )
    }

    /// Show a new item at once, select it, and start naming it.
    pub(super) fn add_and_rename(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = FileEntry::from_path(&path) {
            self.listing.push(entry);
            self.rebuild_rows(cx);
            if let Some(ix) = self.rows.iter().position(|row| row.entry.path == path) {
                self.selection.select_only(ix);
                self.selection_changed(cx);
            }
        }
        self.begin_rename(path, window, cx);
        self.load_listing(Vec::new(), true, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::name_stem_len;

    #[test]
    fn the_stem_stops_before_a_files_extension() {
        assert_eq!(name_stem_len("Roadmap.md", false), 7);
        assert_eq!(name_stem_len("archive.tar.gz", false), 11);
        assert_eq!(name_stem_len("Makefile", false), 8);
        assert_eq!(
            name_stem_len(".gitignore", false),
            10,
            "not a hidden file's dot"
        );
        assert_eq!(
            name_stem_len("my.folder", true),
            9,
            "a folder has no extension"
        );
    }
}
