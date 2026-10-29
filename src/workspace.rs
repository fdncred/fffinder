//! A window's tabs and split view. Each tab is a whole browser ([`Finder`])
//! with its own history, search, and filter; the window shows the active
//! one. Split view adds a second browser beside it, which stays put when
//! you switch tabs, so F5 and F6 can copy and move between the two.

use crate::finder::{Finder, PickerOptions};
use crate::model::Location;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub struct Workspace {
    tabs: Vec<Entity<Finder>>,
    active: usize,
    /// The other pane, while split view is on.
    split: Option<Entity<Finder>>,
}

/// What a new browser in this window starts with.
fn new_finder(
    start: Location,
    query: Option<String>,
    picker: Option<PickerOptions>,
    host: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Finder> {
    cx.new(|cx| {
        let mut finder = Finder::new(start, query, picker, window, cx);
        finder.set_host(host);
        finder
    })
}

impl Workspace {
    /// A window's tabs, starting with one browser at `start`, searching for
    /// `query` when given, and choosing files for another app when `picker`
    /// is.
    pub fn new(
        start: Location,
        query: Option<String>,
        picker: Option<PickerOptions>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Workspace {
        let host = cx.entity().downgrade();
        let first = new_finder(start, query, picker, host, window, cx);
        Workspace {
            tabs: vec![first],
            active: 0,
            split: None,
        }
    }

    /// The browser the window shows in its main pane.
    pub fn active(&self) -> &Entity<Finder> {
        &self.tabs[self.active]
    }

    /// Every tab's browser, in order.
    pub fn tabs(&self) -> &[Entity<Finder>] {
        &self.tabs
    }

    /// Where the active tab is among [`Self::tabs`].
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The browser beside the active one in split view, if the window is
    /// split.
    pub fn split_pane(&self) -> Option<&Entity<Finder>> {
        self.split.as_ref()
    }

    /// The pane opposite `of` in split view: the split pane for the active
    /// tab, and the active tab for the split pane.
    pub fn other_pane(&self, of: EntityId) -> Option<Entity<Finder>> {
        let split = self.split.as_ref()?;
        if split.entity_id() == of {
            Some(self.active().clone())
        } else if self.active().entity_id() == of {
            Some(split.clone())
        } else {
            None
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let finder = self.active().read(cx);
        window.set_window_title(&finder.title());
        finder.files_focus_handle().focus(window, cx);
    }

    /// Open a tab at `start`, after the active one, and show it.
    pub fn new_tab(&mut self, start: Location, window: &mut Window, cx: &mut Context<Self>) {
        let host = cx.entity().downgrade();
        let tab = new_finder(start, None, None, host, window, cx);
        self.active += 1;
        self.tabs.insert(self.active, tab);
        self.focus_active(window, cx);
        self.tabs_changed(cx);
    }

    /// Show the tab at `ix`; one past the last does nothing.
    pub fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active = ix;
            self.focus_active(window, cx);
            self.tabs_changed(cx);
        }
    }

    /// Move `steps` tabs along, wrapping around.
    pub fn step_tab(&mut self, steps: isize, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.tabs.len() as isize;
        let ix = (self.active as isize + steps).rem_euclid(len) as usize;
        self.select_tab(ix, window, cx);
    }

    /// Close the tab holding `finder`; the last tab closes the window.
    pub fn close_tab(&mut self, finder: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t.entity_id() == finder) else {
            return;
        };
        if self.tabs.len() == 1 {
            window.remove_window();
            return;
        }
        self.tabs.remove(ix);
        if self.active > ix || self.active == self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.focus_active(window, cx);
        self.tabs_changed(cx);
    }

    /// Put the tab at `from` where the one at `to` is.
    pub fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let active = self.active().entity_id();
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self
            .tabs
            .iter()
            .position(|t| t.entity_id() == active)
            .unwrap_or(0);
        self.tabs_changed(cx);
    }

    /// Open the tab holding `finder` in a window of its own, at the same
    /// place and search, and close it here.
    pub fn move_tab_to_new_window(
        &mut self,
        finder: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() < 2 {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|t| t.entity_id() == finder).cloned() else {
            return;
        };
        let (location, query) = {
            let finder = tab.read(cx);
            let query = finder.is_searching().then(|| finder.query().to_string());
            (finder.location().clone(), query)
        };
        self.close_tab(finder, window, cx);
        cx.defer(move |cx| {
            crate::open_window(location, query, None, cx);
        });
    }

    /// Turn split view on, with the other pane at `start`, or off.
    pub fn toggle_split(&mut self, start: Location, window: &mut Window, cx: &mut Context<Self>) {
        if self.split.take().is_none() {
            let host = cx.entity().downgrade();
            let pane = new_finder(start, None, None, host, window, cx);
            pane.update(cx, |pane, cx| pane.become_split_pane(cx));
            pane.read(cx).files_focus_handle().focus(window, cx);
            self.split = Some(pane);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Files changed in `changed`: reload every other browser in the window.
    pub fn files_changed(&mut self, changed: EntityId, cx: &mut Context<Self>) {
        let others: Vec<Entity<Finder>> = self
            .tabs
            .iter()
            .chain(self.split.as_ref())
            .filter(|f| f.entity_id() != changed)
            .cloned()
            .collect();
        for finder in others {
            finder.update(cx, |f, cx| f.reload_quietly(cx));
        }
    }

    fn tabs_changed(&mut self, cx: &mut Context<Self>) {
        // The active tab draws the tab bar.
        self.active().update(cx, |_, cx| cx.notify());
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        div()
            .id("workspace")
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.active().clone()),
            )
            .when_some(self.split.clone(), |this, pane| {
                this.child(div().w(px(1.)).h_full().flex_shrink_0().bg(border))
                    .child(
                        div()
                            .id("split-pane")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(pane),
                    )
            })
    }
}
