//! Tabs and split view, from the browser's side: the tab bar under the
//! toolbar, the tab and split actions (handed to the window's
//! [`Workspace`]), and F5 and F6 between the two panes.

use super::Finder;
use crate::actions::*;
use crate::model::Location;
use crate::theme::ui;
use crate::workspace::Workspace;
use fffinder_core::ops::Mode;
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// A tab being dragged to a new place in the tab bar.
#[derive(Clone)]
struct DraggedTab(usize);

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Finder {
    /// Put this browser in a window's tabs.
    pub fn set_host(&mut self, host: WeakEntity<Workspace>) {
        self.host = Some(host);
    }

    /// Make this browser the split view's second pane: no sidebar, and its
    /// view changes aren't saved over the main pane's.
    pub fn become_split_pane(&mut self, cx: &mut Context<Self>) {
        self.split_pane = true;
        self.persist = false;
        self.config.show_sidebar = false;
        cx.notify();
    }

    pub fn is_split_pane(&self) -> bool {
        self.split_pane
    }

    /// Whether the window is showing split view.
    pub fn has_split_view(&self, cx: &App) -> bool {
        self.split_pane
            || self
                .host
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|host| host.read(cx).split_pane().is_some())
    }

    /// Reload after another browser in the window changed files.
    pub fn reload_quietly(&mut self, cx: &mut Context<Self>) {
        self.load_listing(Vec::new(), true, cx);
        if self.is_searching() {
            self.run_search(false, cx);
        }
        self.reload_columns(cx);
        self.reload_children(cx);
    }

    /// Tell the window's other tabs and pane that files changed.
    pub(super) fn tell_others(&self, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        let me = cx.entity_id();
        cx.defer(move |cx| {
            host.update(cx, |w, cx| w.files_changed(me, cx)).ok();
        });
    }

    /// Run `f` on the window's tabs once this browser is done updating.
    fn with_host(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) -> bool {
        let Some(host) = self.host.clone() else {
            return false;
        };
        window.defer(cx, move |window, cx| {
            host.update(cx, |w, cx| f(w, window, cx)).ok();
        });
        true
    }

    /// Where a new tab or pane opens: the folder shown, or home.
    fn here(&self) -> Location {
        self.current_folder()
            .map(|f| Location::Folder(f.to_path_buf()))
            .or_else(|| dirs::home_dir().map(Location::Folder))
            .unwrap_or(Location::Recents)
    }

    /// Cmd+T. Without tabs (a picker), a new window instead.
    pub(super) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        let start = self.here();
        if !self.with_host(window, cx, move |w, window, cx| {
            w.new_tab(start, window, cx)
        }) {
            window.dispatch_action(NewWindow.boxed_clone(), cx);
        }
    }

    pub(super) fn show_next_tab(
        &mut self,
        _: &ShowNextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_host(window, cx, |w, window, cx| w.step_tab(1, window, cx));
    }

    pub(super) fn show_previous_tab(
        &mut self,
        _: &ShowPreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_host(window, cx, |w, window, cx| w.step_tab(-1, window, cx));
    }

    pub(super) fn move_tab_to_new_window(
        &mut self,
        _: &MoveTabToNewWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity_id();
        self.with_host(window, cx, move |w, window, cx| {
            w.move_tab_to_new_window(me, window, cx)
        });
    }

    pub(super) fn toggle_split_view(
        &mut self,
        _: &ToggleSplitView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picker.is_some() {
            return;
        }
        let start = self.here();
        self.with_host(window, cx, move |w, window, cx| {
            w.toggle_split(start, window, cx)
        });
    }

    /// Cmd+W: close the split pane or this tab, and the window when it's
    /// the last tab.
    pub(super) fn close_tab_or_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity_id();
        let tabs = self
            .host
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .map(|host| host.read(cx).tabs().len())
            .unwrap_or(1);
        if self.split_pane {
            let start = self.here();
            self.with_host(window, cx, move |w, window, cx| {
                w.toggle_split(start, window, cx)
            });
        } else if tabs > 1 {
            self.with_host(window, cx, move |w, window, cx| w.close_tab(me, window, cx));
        } else {
            self.close_window(window);
        }
    }

    /// F5 and F6: copy or move the selection into the other pane's folder.
    fn transfer_to_other_pane(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity_id();
        let other = self
            .host
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .and_then(|host| host.read(cx).other_pane(me));
        let Some(other) = other else {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "Turn on split view to copy or move between panes",
                ),
                cx,
            );
            return;
        };
        let Some(dest) = other.read(cx).drop_folder() else {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "The other pane isn’t showing a folder",
                ),
                cx,
            );
            return;
        };
        let sources: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if !sources.is_empty() {
            self.start_transfer(sources, dest, mode, window, cx);
        }
    }

    pub(super) fn copy_to_other_pane(
        &mut self,
        _: &CopyToOtherPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer_to_other_pane(Mode::Copy, window, cx);
    }

    pub(super) fn move_to_other_pane(
        &mut self,
        _: &MoveToOtherPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer_to_other_pane(Mode::Move, window, cx);
    }

    /// The tabs, when the window has more than one; the split pane has none.
    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.split_pane {
            return None;
        }
        let host = self.host.as_ref()?.upgrade()?;
        let me = cx.entity_id();
        let (titles, active) = {
            let workspace = host.read(cx);
            if workspace.tabs().len() < 2 {
                return None;
            }
            // This browser is being drawn, so it can't be read through its entity.
            let titles: Vec<(EntityId, String)> = workspace
                .tabs()
                .iter()
                .map(|tab| {
                    let id = tab.entity_id();
                    let title = if id == me {
                        self.title()
                    } else {
                        tab.read(cx).title()
                    };
                    (id, title)
                })
                .collect();
            (titles, workspace.active_index())
        };
        let theme = cx.theme();
        let (bar, border, muted, hover) = (
            theme.tab_bar,
            theme.border,
            theme.muted_foreground,
            theme.list_hover,
        );
        let selected_bg = theme.background;
        let highlight = theme.drop_target;
        let tabs = titles.into_iter().enumerate().map(|(ix, (id, title))| {
            let on = ix == active;
            let weak = host.downgrade();
            let (select, close, menu_host) = (weak.clone(), weak.clone(), weak.clone());
            let drop_host = weak;
            let label: SharedString = title.clone().into();
            h_flex()
                .id(("tab", ix))
                .flex_1()
                .min_w(ui(80.))
                .max_w(ui(240.))
                .h(ui(28.))
                .px(ui(10.))
                .gap(ui(6.))
                .rounded(ui(6.))
                .text_size(ui(12.))
                .when(on, |this| {
                    this.bg(selected_bg).border_1().border_color(border)
                })
                .when(!on, |this| this.text_color(muted).hover(|s| s.bg(hover)))
                .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                .child(
                    div()
                        .id(("close-tab", ix))
                        .p(ui(2.))
                        .rounded(ui(4.))
                        .hover(|s| s.bg(hover))
                        .child(
                            Icon::new(IconName::Close)
                                .with_size(ui(11.))
                                .text_color(muted),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let close = close.clone();
                            window.defer(cx, move |window, cx| {
                                close.update(cx, |w, cx| w.close_tab(id, window, cx)).ok();
                            });
                        })
                        .test_support(),
                )
                .on_click(move |_, window, cx| {
                    let select = select.clone();
                    window.defer(cx, move |window, cx| {
                        select.update(cx, |w, cx| w.select_tab(ix, window, cx)).ok();
                    });
                })
                .on_drag(DraggedTab(ix), |tab, _, _, cx| cx.new(|_| tab.clone()))
                .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(highlight))
                .on_drop(move |dragged: &DraggedTab, window, cx| {
                    let (from, host) = (dragged.0, drop_host.clone());
                    window.defer(cx, move |_, cx| {
                        host.update(cx, |w, cx| w.move_tab(from, ix, cx)).ok();
                    });
                })
                .test_support()
                .context_menu(move |menu, _, _| {
                    let (close, split) = (menu_host.clone(), menu_host.clone());
                    menu.item(
                        PopupMenuItem::new("Close Tab").on_click(move |_, window, cx| {
                            let close = close.clone();
                            window.defer(cx, move |window, cx| {
                                close.update(cx, |w, cx| w.close_tab(id, window, cx)).ok();
                            });
                        }),
                    )
                    .item(
                        PopupMenuItem::new("Move Tab to New Window").on_click(
                            move |_, window, cx| {
                                let split = split.clone();
                                window.defer(cx, move |window, cx| {
                                    split
                                        .update(cx, |w, cx| {
                                            w.move_tab_to_new_window(id, window, cx)
                                        })
                                        .ok();
                                });
                            },
                        ),
                    )
                })
        });
        Some(
            h_flex()
                .id("tab-bar")
                .h(ui(36.))
                .flex_shrink_0()
                .px(ui(10.))
                .gap(ui(6.))
                .bg(bar)
                .border_b_1()
                .border_color(border)
                .children(tabs)
                .child(
                    div()
                        .id("new-tab")
                        .p(ui(4.))
                        .rounded(ui(5.))
                        .hover(|s| s.bg(hover))
                        .child(Icon::new(Lucide::Plus).with_size(ui(14.)).text_color(muted))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.new_tab(&NewTab, window, cx)
                        }))
                        .test_support(),
                )
                .test_support()
                .into_any_element(),
        )
    }
}
