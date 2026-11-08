//! Renaming an item in place: its name becomes a text field in the list,
//! icon, and column views, or in the sidebar's folder tree. The gallery has
//! no room to edit a name, so it keeps the dialog.

use super::Finder;
use crate::theme::ui;
use fffinder_core::{FileEntry, ViewMode};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};

/// A name being edited in place.
pub(super) struct InlineRename {
    pub(super) path: PathBuf,
    /// The field is in the sidebar's folder tree, not the file views, and
    /// the keyboard goes back there afterward.
    pub(super) in_sidebar: bool,
    input: Entity<InputState>,
    /// What was typed when it was last refused, so leaving the field doesn't
    /// say why a second time.
    refused: Option<SharedString>,
    /// Whether the last frame drew the field. Scrolling its row away or
    /// removing its item unmounts it, and only drawing can tell. A `Cell`
    /// lets the drawing code, which only reads `self`, record this.
    drawn: Cell<bool>,
    _events: Subscription,
}

/// The part of a name to select for retyping: all of a folder's, and a file's
/// up to its extension.
pub(super) fn name_stem_len(name: &str, is_dir: bool) -> usize {
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
        // The field only exists once its row is on screen.
        if let Some(ix) = self.rows.iter().position(|row| row.entry.path == path) {
            self.scroll_to(ix);
        }
        for pane in &self.columns {
            if let Some(ix) = pane.entries.iter().position(|entry| entry.path == path) {
                pane.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        }
        self.start_rename_field(path, false, window, cx);
    }

    /// Start renaming the folder at `path` in its row of the sidebar's tree.
    pub(super) fn begin_sidebar_rename(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_rename_field(path, true, window, cx);
    }

    /// Make the field that edits the name at `path`, with the part to
    /// retype selected.
    fn start_rename_field(
        &mut self,
        path: PathBuf,
        in_sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = fffinder_core::entry::display_name(&path);
        let stem = name_stem_len(&name, path.is_dir());
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..stem, cx);
        });
        let events = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.finish_rename(window, cx),
            InputEvent::Blur => this.rename_field_blurred(window, cx),
            _ => {}
        });
        self.renaming = Some(InlineRename {
            path,
            in_sidebar,
            input,
            refused: None,
            drawn: Cell::new(true),
            _events: events,
        });
        cx.notify();
    }

    /// Stop editing and leave the name as it was.
    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping it first means the field losing focus doesn't finish it.
        if let Some(renaming) = self.renaming.take() {
            self.keyboard_back(renaming.in_sidebar, window, cx);
            cx.notify();
        }
    }

    /// Rename the item to what's typed. A name that can't be used says why and
    /// goes on editing.
    pub(super) fn finish_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut renaming) = self.renaming.take() else {
            return;
        };
        let typed = renaming.input.read(cx).value();
        if typed.trim().is_empty()
            || self.submit_rename(&renaming.path, &renaming.input, window, cx)
        {
            self.keyboard_back_from(&renaming, window, cx);
            return;
        }
        renaming.refused = Some(typed);
        renaming
            .input
            .update(cx, |state, cx| state.focus(window, cx));
        self.renaming = Some(renaming);
    }

    /// The mouse went down elsewhere, or the keyboard went to something else.
    /// That finishes the rename, except that a name already refused isn't
    /// refused again: one click away both presses the mouse and moves the
    /// keyboard.
    fn leave_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.as_ref() else {
            return;
        };
        if renaming.refused.as_ref() == Some(&renaming.input.read(cx).value()) {
            renaming
                .input
                .update(cx, |state, cx| state.focus(window, cx));
            return;
        }
        self.finish_rename(window, cx);
    }

    /// The field lost the keyboard.
    fn rename_field_blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // gpui reports the window going inactive as a blur. Going to another
        // app, perhaps to copy the rest of a name, leaves the edit as it is.
        if !window.is_window_active() {
            return;
        }
        if self.renaming.as_ref().is_some_and(|r| !r.drawn.get()) {
            self.end_rename(window, cx);
        } else {
            self.leave_rename(window, cx);
        }
    }

    /// End the edit because the field can't stay: its row scrolled away, its
    /// item went, a dialog is opening over it, or its tab is being hidden. The
    /// file keys go nowhere while focus stays on a field that isn't there. This
    /// keeps a name that can be used, and otherwise the old one.
    pub(crate) fn end_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let typed = renaming.input.read(cx).value();
        if !typed.trim().is_empty() && renaming.refused.as_ref() != Some(&typed) {
            self.submit_rename(&renaming.path, &renaming.input, window, cx);
        }
        self.keyboard_back_from(&renaming, window, cx);
    }

    /// The edit is over, so the keyboard goes back to where the name was
    /// edited, unless it's already gone on to something else, like the
    /// search field.
    fn keyboard_back_from(
        &mut self,
        renaming: &InlineRename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if renaming.input.read(cx).focus_handle(cx).is_focused(window) {
            self.keyboard_back(renaming.in_sidebar, window, cx);
        }
        cx.notify();
    }

    /// Give the keyboard back to the sidebar's tree, or to the files.
    fn keyboard_back(&mut self, to_sidebar: bool, window: &mut Window, cx: &mut Context<Self>) {
        if to_sidebar {
            self.tree_focus.focus(window, cx);
        } else {
            self.focus_files(window, cx);
        }
    }

    /// The window became active again. Its field may have gone while it was
    /// inactive, when no blur says so.
    pub(super) fn check_rename_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.as_ref().is_some_and(|r| !r.drawn.get()) {
            self.end_rename(window, cx);
        }
    }

    /// A new frame is starting: the field is drawn only if a row asks for it.
    pub(super) fn rename_frame_started(&self) {
        if let Some(renaming) = &self.renaming {
            renaming.drawn.set(false);
        }
    }

    /// The field that replaces the name at `path`, while it's being edited.
    /// The same folder can show in the views and in the sidebar's tree, and
    /// the field is drawn only where the edit began.
    ///
    /// # Arguments
    ///
    /// * `path` - The item a row or tile shows
    /// * `in_sidebar` - Whether the row is in the sidebar
    /// * `cx` - The browser's context
    ///
    /// # Returns
    ///
    /// The field, when that item's name is being edited there
    pub(super) fn rename_field(
        &self,
        path: &Path,
        in_sidebar: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let renaming = self
            .renaming
            .as_ref()
            .filter(|r| r.path == path && r.in_sidebar == in_sidebar)?;
        renaming.drawn.set(true);
        Some(
            div()
                .id("rename-field")
                .flex_1()
                .min_w_0()
                .w_full()
                .text_size(ui(12.))
                // A press in the field edits; it doesn't select or drag the item.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.leave_rename(window, cx)
                    }),
                )
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
