//! The unified toolbar: title, back/forward, view switcher, sort and action
//! menus, Nushell menu, the search scope, and the search field.

use super::{Finder, SearchScope};
use crate::actions::*;
use crate::theme::palette;
use crate::theme::ui;
use fffinder_core::{SortKey, ViewMode};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;

pub const TOOLBAR_HEIGHT: f32 = 52.;
/// Room for the macOS traffic lights when they sit over this view.
pub const TRAFFIC_LIGHTS: f32 = 78.;

/// A rounded group of toolbar buttons.
pub fn pill(cx: &App) -> Div {
    let pal = palette(cx);
    h_flex()
        .h(ui(34.))
        .p(ui(2.))
        .gap(ui(2.))
        .rounded_full()
        .bg(pal.pill)
        .border_1()
        .border_color(pal.pill_border)
        .flex_shrink_0()
        // Clicks on controls must not start a window drag.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

pub fn pill_button(id: impl Into<ElementId>, icon: impl Into<Icon>, cx: &App) -> Button {
    Button::new(id)
        .ghost()
        .icon(icon)
        .rounded(ButtonRounded::Size(ui(15.)))
        .w(ui(36.))
        .h(ui(28.))
        .text_color(cx.theme().foreground)
}

/// A pill button that opens a menu, with a caret after its icon.
pub fn menu_button(id: impl Into<ElementId>, icon: impl Into<Icon>, cx: &App) -> Button {
    // The button puts its icon and caret at its two edges; the padding moves
    // them in from the pill's rounded ends.
    pill_button(id, icon, cx)
        .w(ui(52.))
        .px(ui(8.))
        .dropdown_caret(true)
}

thread_local! {
    /// A left press in empty toolbar space that hasn't moved yet (Linux).
    static DRAG_ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Start a window drag (or zoom on double-click) from empty toolbar space.
/// Windows has a system title bar for that. On Linux there is one only when
/// the window manager decorates the window; otherwise the toolbar is the
/// title bar, so right-click also shows the window menu there.
pub fn window_drag_area<E: InteractiveElement>(el: E) -> E {
    if cfg!(target_os = "macos") {
        el.on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count >= 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
    } else if cfg!(target_os = "windows") {
        el
    } else {
        // Moving the window takes the pointer, so wait for the press to
        // become a drag; a second click then still maximizes.
        el.on_mouse_down(MouseButton::Left, |event, window, _| {
            let double = event.click_count >= 2;
            DRAG_ARMED.set(!double);
            if double {
                window.zoom_window();
            }
        })
        .on_mouse_down_out(|_, _, _| DRAG_ARMED.set(false))
        .on_mouse_up(MouseButton::Left, |_, _, _| DRAG_ARMED.set(false))
        .on_mouse_move(|event, window, _| {
            if event.pressed_button == Some(MouseButton::Left) && DRAG_ARMED.replace(false) {
                window.start_window_move();
            }
        })
        .on_mouse_down(MouseButton::Right, |event, window, _| {
            if matches!(window.window_decorations(), Decorations::Client { .. }) {
                window.show_window_menu(event.position);
            }
        })
    }
}

/// Minimize, maximize, and close buttons for a Linux window whose title bar
/// fffinder draws, because the window manager draws none. They're narrower
/// than the pill buttons so the toolbar still fits the default window.
fn window_controls(window: &Window, cx: &mut Context<Finder>) -> Option<Div> {
    if cfg!(any(target_os = "macos", target_os = "windows"))
        || !matches!(window.window_decorations(), Decorations::Client { .. })
    {
        return None;
    }
    let supported = window.window_controls();
    let (maximize_icon, maximize_tip) = if window.is_maximized() {
        (IconName::WindowRestore, "Restore")
    } else {
        (IconName::WindowMaximize, "Maximize")
    };
    let button = |id: &'static str, icon: IconName, tip: &'static str, cx: &App| {
        pill_button(id, icon, cx).w(ui(28.)).tooltip(tip)
    };
    let controls = h_flex()
        .gap(ui(2.))
        .flex_shrink_0()
        // Clicks on the buttons must not start a window drag.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(supported.minimize, |this| {
            this.child(
                button("window-minimize", IconName::WindowMinimize, "Minimize", cx)
                    .on_click(|_, window, _| window.minimize_window()),
            )
        })
        .when(supported.maximize, |this| {
            this.child(
                button("window-maximize", maximize_icon, maximize_tip, cx)
                    .on_click(|_, window, _| window.zoom_window()),
            )
        })
        .child(
            button("window-close", IconName::WindowClose, "Close Window", cx)
                .on_click(cx.listener(|this, _, window, _| this.close_window(window))),
        );
    Some(controls)
}

impl Finder {
    pub(super) fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pal = palette(cx);
        let view = cx.entity();
        let mode = self.config.view_mode;
        // A terminal window has no traffic lights to make room for.
        let traffic =
            cfg!(target_os = "macos") && !self.config.show_sidebar && !crate::term::active();
        let can_back = self.history.can_back();
        let can_forward = self.history.can_forward();

        let nav = pill(cx)
            .child(
                pill_button("back", IconName::ChevronLeft, cx)
                    .tooltip("Back")
                    .disabled(!can_back)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.go_back(&GoBack, window, cx)
                    })),
            )
            .child(
                pill_button("forward", IconName::ChevronRight, cx)
                    .tooltip("Forward")
                    .disabled(!can_forward)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.go_forward(&GoForward, window, cx)
                    })),
            );

        let view_button = |id: &'static str,
                           icon: Lucide,
                           tip: &'static str,
                           m: ViewMode,
                           cx: &mut Context<Self>| {
            pill_button(id, icon, cx)
                .tooltip(tip)
                .selected(mode == m)
                .when(mode == m, |b| b.bg(pal.pill_selected))
                .on_click(cx.listener(move |this, _, _, cx| this.set_view_mode(m, cx)))
        };
        let views = pill(cx)
            .child(view_button(
                "view-icons",
                Lucide::LayoutGrid,
                "as Icons",
                ViewMode::Icons,
                cx,
            ))
            .child(view_button(
                "view-list",
                Lucide::List,
                "as List",
                ViewMode::List,
                cx,
            ))
            .child(view_button(
                "view-columns",
                Lucide::Columns3,
                "as Columns",
                ViewMode::Columns,
                cx,
            ))
            .child(view_button(
                "view-gallery",
                Lucide::GalleryHorizontalEnd,
                "as Gallery",
                ViewMode::Gallery,
                cx,
            ));

        let sort_view = view.clone();
        let sort = pill(cx).child(
            menu_button("sort", Lucide::ArrowUpDown, cx)
                .tooltip("Sort and View Options")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                    sort_menu(&sort_view, menu, window, cx)
                }),
        );

        let nu_view = view.clone();
        let nushell = pill(cx).child(
            menu_button("nushell", IconName::SquareTerminal, cx)
                .tooltip("Nushell")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                    nushell_menu(&nu_view, menu, window, cx)
                }),
        );

        let actions_view = view.clone();
        let actions = pill(cx).child(
            menu_button("actions", IconName::Ellipsis, cx)
                .tooltip("Actions")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                    actions_menu(&actions_view, menu, window, cx)
                }),
        );

        // Searches only the folder the window shows, like the scope bar's
        // folder button, but before the search starts.
        let folder = self
            .current_folder()
            .map(fffinder_core::entry::display_name);
        let in_folder = folder.is_some() && self.scope == SearchScope::Folder;
        let scope_tip = match &folder {
            Some(name) => format!("Search Only in “{name}”"),
            None => "Search Only in This Folder".to_string(),
        };
        let scope = pill(cx).child(
            pill_button("search-scope", Lucide::FolderSearch, cx)
                .tooltip(scope_tip)
                .disabled(folder.is_none())
                .selected(in_folder)
                .when(in_folder, |b| b.bg(pal.pill_selected))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let scope = if in_folder {
                        SearchScope::Computer
                    } else {
                        SearchScope::Folder
                    };
                    this.set_scope(scope, cx);
                })),
        );

        let search = h_flex()
            .w(ui(250.))
            // Narrows before the toolbar overflows the window.
            .min_w(ui(160.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // Down arrow moves from the search field into the results.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "down" && !this.rows.is_empty() {
                    this.focus_files(window, cx);
                    if this.selection.is_empty() {
                        this.selection.select_only(0);
                        this.scroll_to(0);
                        this.selection_changed(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                Input::new(&self.search_input)
                    .id("search")
                    .prefix(
                        Icon::new(IconName::Search)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .cleanable(true)
                    .h(ui(34.))
                    .rounded_full()
                    .bg(pal.pill)
                    .border_color(pal.pill_border)
                    .w_full(),
            );

        let title = self.title();
        window_drag_area(
            h_flex()
                .id("toolbar")
                .h(ui(TOOLBAR_HEIGHT))
                .w_full()
                .flex_shrink_0()
                // macOS draws the window buttons at a fixed size.
                .pl(if traffic { px(TRAFFIC_LIGHTS) } else { ui(12.) })
                .pr(ui(10.))
                .gap(ui(8.))
                .when(!self.config.show_sidebar, |this| {
                    this.child(
                        pill(cx).child(
                            pill_button("show-sidebar", IconName::PanelLeft, cx)
                                .tooltip("Show Sidebar")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.toggle_sidebar(&ToggleSidebar, window, cx)
                                })),
                        ),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(ui(60.))
                        .pl(ui(4.))
                        .text_size(ui(15.))
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(title),
                )
                .child(nav)
                .child(views)
                .child(sort)
                .child(nushell)
                .child(actions)
                .child(scope)
                .child(search)
                .children(window_controls(window, cx)),
        )
    }
}

fn sort_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let finder = view.read(cx);
    let sort = finder.current_sort();
    let searching = finder.is_searching();
    let hidden = finder.config.show_hidden;
    let mut keys: Vec<SortKey> = Vec::new();
    if searching {
        keys.push(SortKey::Relevance);
    }
    keys.extend(SortKey::BROWSE);
    let mut menu = menu.label("Sort By");
    for key in keys {
        menu = menu.item(
            PopupMenuItem::new(key.label())
                .checked(sort.key == key)
                .on_click(
                    window.listener_for(view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_sort(key, cx)
                    }),
                ),
        );
    }
    menu.separator()
        .item(
            PopupMenuItem::new("Ascending")
                .checked(sort.ascending)
                .disabled(sort.key == SortKey::Relevance)
                .on_click(
                    window.listener_for(view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_ascending(true, cx)
                    }),
                ),
        )
        .item(
            PopupMenuItem::new("Descending")
                .checked(!sort.ascending)
                .disabled(sort.key == SortKey::Relevance)
                .on_click(
                    window.listener_for(view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_ascending(false, cx)
                    }),
                ),
        )
        .separator()
        .item(
            PopupMenuItem::new("Show Hidden Files")
                .checked(hidden)
                .action(Box::new(ToggleHidden)),
        )
        .min_w(ui(200.))
}

fn nushell_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let finder = view.read(cx);
    let has_selection = !finder.selected_entries().is_empty();
    let panel_open = finder.nu_open;
    let menu = menu
        .item(
            PopupMenuItem::new("Open Nushell Here")
                .icon(IconName::SquareTerminal)
                .action(Box::new(OpenInNushell)),
        )
        .item(
            PopupMenuItem::new(if panel_open {
                "Hide Pipeline Panel"
            } else {
                "Run a Pipeline…"
            })
            .icon(Lucide::Terminal)
            .action(Box::new(ToggleNuPanel)),
        );
    super::menus::actions_submenu(view, menu, window, cx)
        .separator()
        .item(
            PopupMenuItem::new("Copy as Nushell List")
                .icon(IconName::Copy)
                .disabled(!has_selection)
                .action(Box::new(CopyAsNuon)),
        )
        .min_w(ui(220.))
}

fn actions_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let finder = view.read(cx);
    let has_selection = !finder.selected_entries().is_empty();
    let in_folder = finder.current_folder().is_some();
    let folder = finder.current_folder().map(|p| p.to_path_buf());
    let is_favorite = folder
        .as_ref()
        .is_some_and(|f| finder.config.favorites.contains(f));
    menu.item(
        PopupMenuItem::new("New Folder")
            .icon(Lucide::FolderPlus)
            .disabled(!in_folder)
            .action(Box::new(NewFolder)),
    )
    .item(
        PopupMenuItem::new("New File…")
            .icon(Lucide::FilePlus)
            .disabled(!in_folder)
            .action(Box::new(NewFile)),
    )
    .item(
        PopupMenuItem::new("Quick Look")
            .icon(IconName::Eye)
            .disabled(!has_selection)
            .action(Box::new(QuickLook)),
    )
    .item(
        PopupMenuItem::new("Get Info")
            .icon(IconName::Info)
            .disabled(!has_selection && !in_folder)
            .action(Box::new(GetInfo)),
    )
    .item(
        PopupMenuItem::new("Rename…")
            .icon(Lucide::Pencil)
            .disabled(!has_selection)
            .action(Box::new(Rename)),
    )
    .separator()
    .item(
        PopupMenuItem::new("Copy Path")
            .icon(IconName::Copy)
            .disabled(!has_selection)
            .action(Box::new(CopyPath)),
    )
    .item(
        PopupMenuItem::new(super::menus::reveal_label())
            .icon(IconName::ExternalLink)
            .disabled(!has_selection)
            .action(Box::new(RevealInFileManager)),
    )
    .when_some(folder.filter(|_| !is_favorite), |menu, folder| {
        menu.item(
            PopupMenuItem::new("Add Folder to Sidebar")
                .icon(IconName::Star)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.add_favorite(folder.clone(), cx)
                    },
                )),
        )
    })
    .separator()
    .item(
        PopupMenuItem::new("Move to Trash")
            .icon(Lucide::Trash)
            .disabled(!has_selection)
            .action(Box::new(MoveToTrash)),
    )
    .item(
        PopupMenuItem::new("Delete Immediately…")
            .disabled(!has_selection)
            .action(Box::new(DeleteImmediately)),
    )
    // The menu bar's Edit › Undo, for systems without one.
    .item(
        PopupMenuItem::new("Undo")
            .icon(Lucide::Undo2)
            .action(Box::new(gpui_kit::component::input::Undo)),
    )
    .separator()
    .item(
        PopupMenuItem::new("Filter with Nushell…")
            .icon(Lucide::Funnel)
            .action(Box::new(ToggleFilter)),
    )
    .item(PopupMenuItem::new("Go to Folder…").action(Box::new(GoToFolder)))
    .item(PopupMenuItem::new("Search Syntax…").action(Box::new(SearchHelp)))
    .item(
        PopupMenuItem::new("Settings…")
            .icon(IconName::Settings)
            .action(Box::new(ShowSettings)),
    )
    .min_w(ui(220.))
}
