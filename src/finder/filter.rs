//! Nushell filters over what the window shows, and Smart Folders (saved
//! searches with a filter). fff-search has no size or date filters; a
//! `where` condition in Nushell has all of them.

use crate::theme::ui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fffinder_core::{FileEntry, SmartFolder};
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AppState, Finder, Row, SearchScope};
use crate::actions::ToggleFilter;
use crate::model::Location;
use crate::nushell;

/// Search results fetched while a filter is on, so it has enough to work with.
pub const FILTER_SEARCH_LIMIT: usize = 5000;

impl Finder {
    pub(super) fn on_filter_event(
        &mut self,
        state: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                // Wait for a pause in typing; each run starts a `nu` process.
                let text = state.read(cx).value().to_string();
                self.filter_debounce = cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(400))
                        .await;
                    this.update(cx, |this, cx| this.set_filter(text, cx)).ok();
                });
            }
            InputEvent::PressEnter { .. } => {
                let text = state.read(cx).value().to_string();
                self.filter_debounce = Task::ready(());
                self.set_filter(text, cx);
            }
            InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    /// Filter what the window shows with a Nushell `where` condition, or a
    /// pipeline starting with `|`. Empty turns the filter off.
    pub fn set_filter(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text == self.filter {
            return;
        }
        let on_off = self.filter.is_empty() != text.is_empty();
        self.filter = text;
        self.filter_keep = None;
        self.filter_source = None;
        self.filter_error = None;
        self.filtering = false;
        self.filter_task = Task::ready(());
        // Turning it on or off changes how many results a search fetches.
        if on_off && self.is_searching() {
            self.run_search(false, cx);
        }
        self.rebuild_rows(cx);
    }

    pub fn filter_text(&self) -> &str {
        &self.filter
    }

    pub fn filter_error(&self) -> Option<&str> {
        self.filter_error.as_deref()
    }

    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    pub fn filter_bar_open(&self) -> bool {
        self.filter_open || !self.filter.is_empty()
    }

    /// Keep only the rows the filter selected, starting a new run when the
    /// rows changed. Rows stay as they are until the first run finishes.
    pub(super) fn apply_filter_to(&mut self, rows: &mut Vec<Row>, cx: &mut Context<Self>) {
        if self.filter.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = rows.iter().map(|r| r.entry.path.clone()).collect();
        if self.filter_source.as_ref() != Some(&paths) {
            self.filter_source = Some(paths);
            let entries: Vec<FileEntry> = rows.iter().map(|r| r.entry.clone()).collect();
            self.start_filter(entries, cx);
        }
        if let Some(keep) = &self.filter_keep {
            rows.retain(|r| keep.contains(&r.entry.path));
        }
    }

    fn start_filter(&mut self, entries: Vec<FileEntry>, cx: &mut Context<Self>) {
        let Some(nu) = cx.global::<AppState>().nu.clone() else {
            self.filter_error =
                Some("Nushell (nu) was not found, so filters can't run.".to_string());
            return;
        };
        let filter = self.filter.clone();
        let cwd = self
            .current_folder()
            .map(Path::to_path_buf)
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir);
        self.filtering = true;
        // Replacing the task cancels a run for rows that are gone.
        self.filter_task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if entries.is_empty() {
                        return Ok(Vec::new());
                    }
                    nushell::filter_paths(&nu, &filter, &entries, &cwd)
                })
                .await;
            this.update(cx, |this, cx| {
                this.filtering = false;
                match result {
                    Ok(paths) => {
                        this.filter_keep = Some(paths.into_iter().collect::<HashSet<_>>());
                        this.filter_error = None;
                    }
                    Err(err) => {
                        this.filter_keep = None;
                        this.filter_error = Some(err);
                    }
                }
                this.rebuild_rows(cx);
            })
            .ok();
        });
    }

    pub(super) fn toggle_filter(
        &mut self,
        _: &ToggleFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.filter_bar_open() {
            self.close_filter(window, cx);
        } else {
            self.filter_open = true;
            self.filter_input.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
        }
    }

    pub fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_open = false;
        self.filter_debounce = Task::ready(());
        self.filter_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.set_filter(String::new(), cx);
        self.focus_files(window, cx);
        cx.notify();
    }

    // ----- Smart Folders --------------------------------------------------------------

    pub fn smart_folders(&self) -> &[SmartFolder] {
        &self.config.smart_folders
    }

    /// The current search and filter, ready to save.
    pub fn current_smart_folder(&self, name: String) -> Option<SmartFolder> {
        if !self.is_searching() {
            return None;
        }
        Some(SmartFolder {
            name,
            query: self.query.clone(),
            kind: self.config.kind_filter,
            scope: match self.scope {
                SearchScope::Folder => self.current_folder().map(Path::to_path_buf),
                SearchScope::Computer => None,
            },
            contents: self.search_contents,
            filter: (!self.filter.is_empty()).then(|| self.filter.clone()),
        })
    }

    pub fn add_smart_folder(&mut self, folder: SmartFolder, cx: &mut Context<Self>) {
        self.config.smart_folders.retain(|f| f.name != folder.name);
        self.config.smart_folders.push(folder);
        self.settings_changed(cx);
        cx.notify();
    }

    pub fn remove_smart_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        self.config.smart_folders.retain(|f| f.name != name);
        self.settings_changed(cx);
        cx.notify();
    }

    /// Run a saved search: its place, kind, filter, and words.
    pub fn open_smart_folder(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.config.smart_folders.get(ix).cloned() else {
            return;
        };
        if let Some(scope) = folder.scope.as_ref().filter(|s| s.is_dir()) {
            self.navigate(Location::Folder(scope.clone()), window, cx);
            self.scope = SearchScope::Folder;
        } else {
            self.end_search(window, cx);
            self.scope = SearchScope::Computer;
        }
        self.config.kind_filter = folder.kind;
        self.search_contents = folder.contents;
        let filter = folder.filter.unwrap_or_default();
        self.filter_open = !filter.is_empty();
        self.filter_input
            .update(cx, |s, cx| s.set_value(filter.clone(), window, cx));
        self.set_filter(filter, cx);
        self.search_input
            .update(cx, |s, cx| s.set_value(folder.query.clone(), window, cx));
        self.set_query(folder.query, true, cx);
        cx.notify();
    }

    // ----- the bar ------------------------------------------------------------------

    pub(super) fn render_filter_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let searching = self.is_searching();
        let error = self.filter_error.clone();
        v_flex()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(
                h_flex()
                    .h(ui(36.))
                    .px(ui(14.))
                    .gap(ui(8.))
                    .text_size(ui(12.))
                    .child(
                        Icon::new(Lucide::Funnel)
                            .with_size(ui(14.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(muted)
                            .child("where"),
                    )
                    .child(
                        Input::new(&self.filter_input)
                            .id("filter-input")
                            .small()
                            .flex_1()
                            .font_family(theme.mono_font_family.clone()),
                    )
                    .when(self.filtering, |this| {
                        this.child(Spinner::new().xsmall().color(muted))
                    })
                    .when(searching, |this| {
                        this.child(
                            Button::new("save-smart-folder")
                                .small()
                                .ghost()
                                .icon(Lucide::FolderCog)
                                .label("Save")
                                .tooltip("Save this search and filter as a Smart Folder")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.save_smart_folder_dialog(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("close-filter")
                            .small()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Remove the filter")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.close_filter(window, cx)
                            })),
                    ),
            )
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .id("filter-error-scroll")
                        .max_h(ui(90.))
                        .overflow_y_scrollbar()
                        .mx(ui(14.))
                        .mb(ui(8.))
                        .p(ui(8.))
                        .rounded(ui(6.))
                        .bg(theme.danger.opacity(0.12))
                        .text_color(theme.danger)
                        .font_family(theme.mono_font_family.clone())
                        .text_size(ui(11.))
                        .child(div().id("filter-error").test_support().child(error)),
                )
            })
    }
}
