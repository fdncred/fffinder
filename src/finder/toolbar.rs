//! The unified toolbar: title, back/forward, view switcher, sort and action
//! menus, Nushell menu, the search scope, and the search field.

use super::{Finder, SearchScope};
use crate::actions::*;
use crate::theme::palette;
use crate::theme::ui;
use fffinder_core::{GroupKey, SortKey, ViewMode};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::ElementExt as _;
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
/// The narrowest the search field gets, in points.
const SEARCH_MIN_WIDTH: f32 = 120.;
/// How far the search field narrows once nothing else can go.
const SEARCH_LAST_WIDTH: f32 = 80.;

// The controls are icons in fixed-size buttons, so what fits is arithmetic.
// They're drawn with these same numbers, so the two can't disagree, and a test
// in `tests/basics.rs` measures the drawn toolbar against the sums. All are in
// points.
const PILL_BUTTON_WIDTH: f32 = 36.;
const MENU_BUTTON_WIDTH: f32 = 52.;
const WINDOW_BUTTON_WIDTH: f32 = 28.;
const PILL_PADDING: f32 = 2.;
const PILL_GAP: f32 = 2.;
/// A pill's border, on its two sides together.
const PILL_BORDERS: f32 = 2.;
const TOOLBAR_GAP: f32 = 8.;
const TOOLBAR_LEFT: f32 = 12.;
const TOOLBAR_RIGHT: f32 = 10.;
const TITLE_PADDING: f32 = 4.;

/// How wide a pill of `count` buttons, each `button` wide, is. A `const fn`
/// can run while compiling, which lets the constants below use it.
const fn pill_width(count: f32, button: f32) -> f32 {
    PILL_BORDERS + 2. * PILL_PADDING + count * button + (count - 1.) * PILL_GAP
}

const ONE_BUTTON_PILL: f32 = pill_width(1., PILL_BUTTON_WIDTH);
const NAV_PILL: f32 = pill_width(2., PILL_BUTTON_WIDTH);
const VIEWS_PILL: f32 = pill_width(4., PILL_BUTTON_WIDTH);
const MENU_PILL: f32 = pill_width(1., MENU_BUTTON_WIDTH);

/// What each of the toolbar's folding controls takes up, with the gap after
/// it: the folder search button, the view buttons beyond what a single menu
/// button needs, the Nushell menu, and the Sort menu.
const SCOPE_ROOM: f32 = ONE_BUTTON_PILL + TOOLBAR_GAP;
const VIEWS_ROOM: f32 = VIEWS_PILL - MENU_PILL;
const NUSHELL_ROOM: f32 = MENU_PILL + TOOLBAR_GAP;
const SORT_ROOM: f32 = MENU_PILL + TOOLBAR_GAP;
/// What the toolbar's extras take up, with the gap beside them: the Show
/// Sidebar button, and the three window buttons where fffinder draws them.
const SHOW_SIDEBAR_ROOM: f32 = ONE_BUTTON_PILL + TOOLBAR_GAP;
const WINDOW_CONTROLS_ROOM: f32 = 3. * WINDOW_BUTTON_WIDTH + 2. * PILL_GAP + TOOLBAR_GAP;
/// The room the toolbar needs with everything showing and the search field at
/// its narrowest: its padding, the title's, the seven gaps between the title,
/// back and forward, the views, Sort, Nushell, Actions, folder search, and
/// the search field, and then each of those.
const TOOLBAR_FULL_WIDTH: f32 = TOOLBAR_LEFT
    + TITLE_PADDING
    + TOOLBAR_RIGHT
    + 7. * TOOLBAR_GAP
    + NAV_PILL
    + VIEWS_PILL
    + 3. * MENU_PILL
    + ONE_BUTTON_PILL
    + SEARCH_MIN_WIDTH;
/// The room it needs with everything that can fold folded. The sidebar leaves
/// it at least this much.
pub const TOOLBAR_MIN_WIDTH: f32 =
    TOOLBAR_FULL_WIDTH - SCOPE_ROOM - VIEWS_ROOM - NUSHELL_ROOM - SORT_ROOM;

/// How much of the toolbar fits, from all of it down. Each step also folds
/// what the ones before it folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ToolbarFit {
    /// Sort is in the Actions menu.
    SortInActions,
    /// The Nushell menu is in the Actions menu.
    NushellInActions,
    /// The view buttons are one menu.
    ViewsAsMenu,
    /// The folder search button is gone; the scope bar has the same choice.
    NoFolderSearch,
    Everything,
}

impl ToolbarFit {
    /// What fits in a toolbar `room` points wide, once its own extras are
    /// taken off.
    pub(super) fn for_room(room: f32) -> ToolbarFit {
        let mut needed = TOOLBAR_FULL_WIDTH;
        for (fit, saved) in [
            (ToolbarFit::Everything, SCOPE_ROOM),
            (ToolbarFit::NoFolderSearch, VIEWS_ROOM),
            (ToolbarFit::ViewsAsMenu, NUSHELL_ROOM),
            (ToolbarFit::NushellInActions, SORT_ROOM),
        ] {
            if room >= needed {
                return fit;
            }
            needed -= saved;
        }
        ToolbarFit::SortInActions
    }
}

pub const TRAFFIC_LIGHTS: f32 = 78.;

/// A rounded group of toolbar buttons.
pub fn pill(cx: &App) -> Div {
    let pal = palette(cx);
    h_flex()
        .h(ui(34.))
        .p(ui(PILL_PADDING))
        .gap(ui(PILL_GAP))
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
        .w(ui(PILL_BUTTON_WIDTH))
        .h(ui(28.))
        .text_color(cx.theme().foreground)
}

/// A pill button that opens a menu, with a caret after its icon.
pub fn menu_button(id: impl Into<ElementId>, icon: impl Into<Icon>, cx: &App) -> Button {
    // The button puts its icon and caret at its two edges; the padding moves
    // them in from the pill's rounded ends.
    pill_button(id, icon, cx)
        .w(ui(MENU_BUTTON_WIDTH))
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

/// Whether fffinder draws the window's buttons, because nothing else does.
fn window_controls_shown(window: &Window) -> bool {
    !cfg!(any(target_os = "macos", target_os = "windows"))
        && matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Minimize, maximize, and close buttons for a Linux window whose title bar
/// fffinder draws, because the window manager draws none. They're narrower
/// than the pill buttons so the toolbar still fits the default window.
fn window_controls(window: &Window, cx: &mut Context<Finder>) -> Option<Div> {
    if !window_controls_shown(window) {
        return None;
    }
    let supported = window.window_controls();
    let (maximize_icon, maximize_tip) = if window.is_maximized() {
        (IconName::WindowRestore, "Restore")
    } else {
        (IconName::WindowMaximize, "Maximize")
    };
    let button = |id: &'static str, icon: IconName, tip: &'static str, cx: &App| {
        pill_button(id, icon, cx)
            .w(ui(WINDOW_BUTTON_WIDTH))
            .tooltip(tip)
    };
    let controls = h_flex()
        .gap(ui(PILL_GAP))
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

/// The icon for a view of the files.
fn view_icon(mode: ViewMode) -> Lucide {
    match mode {
        ViewMode::Icons => Lucide::LayoutGrid,
        ViewMode::List => Lucide::List,
        ViewMode::Columns => Lucide::Columns3,
        ViewMode::Gallery => Lucide::GalleryHorizontalEnd,
    }
}

/// "as Icons", "as List", and the rest, with `mode` checked.
pub(super) fn view_mode_items(
    view: &Entity<Finder>,
    menu: PopupMenu,
    mode: ViewMode,
    window: &mut Window,
) -> PopupMenu {
    ViewMode::ALL.into_iter().fold(menu, |menu, option| {
        menu.item(
            PopupMenuItem::new(format!("as {}", option.title()))
                .checked(option == mode)
                .on_click(
                    window.listener_for(view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_view_mode(option, cx)
                    }),
                ),
        )
    })
}

impl Finder {
    /// How much of the toolbar fits in the width it was last drawn at, after
    /// what the toolbar has to show on this system.
    fn toolbar_room(&self, traffic: bool, window: &Window) -> ToolbarFit {
        let mut extras = 0.;
        if !self.config.show_sidebar {
            extras += SHOW_SIDEBAR_ROOM;
        }
        if traffic {
            extras += TRAFFIC_LIGHTS - TOOLBAR_LEFT;
        }
        if window_controls_shown(window) {
            extras += WINDOW_CONTROLS_ROOM;
        }
        let room = f32::from(self.toolbar_width) / crate::theme::ui_scale() - extras;
        ToolbarFit::for_room(room)
    }

    pub(super) fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pal = palette(cx);
        let view = cx.entity();
        let mode = self.config.view_mode;
        // A terminal window has no traffic lights to make room for.
        let traffic = cfg!(target_os = "macos")
            && !self.config.show_sidebar
            && !crate::term::active()
            && !self.split_pane;
        let can_back = self.history.can_back();
        let can_forward = self.history.can_forward();
        let fit = self.toolbar_room(traffic, window);
        self.toolbar_fit = fit;

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

        // In a narrow toolbar, one button with the current view's icon.
        let views_menu = {
            let view = view.clone();
            pill(cx).child(
                menu_button("view-menu", view_icon(mode), cx)
                    .tooltip("View")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, _| {
                        view_mode_items(&view, menu, mode, window)
                    }),
            )
        };

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
            .min_w(ui(if fit == ToolbarFit::SortInActions {
                SEARCH_LAST_WIDTH
            } else {
                SEARCH_MIN_WIDTH
            }))
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
        let measured = cx.entity();
        window_drag_area(
            h_flex()
                .id("toolbar")
                .h(ui(TOOLBAR_HEIGHT))
                .w_full()
                .flex_shrink_0()
                .overflow_hidden()
                .on_prepaint(move |bounds, window, cx| {
                    let refit = measured.update(cx, |this, _| {
                        this.toolbar_width = bounds.size.width;
                        this.toolbar_room(traffic, window) != this.toolbar_fit
                    });
                    // Only a change in what fits needs another frame. Asking for
                    // one takes this: a notify while drawing waits for the next
                    // event, which may not come after a window is zoomed.
                    if refit {
                        window.request_animation_frame();
                    }
                })
                // macOS draws the window buttons at a fixed size.
                .pl(if traffic {
                    px(TRAFFIC_LIGHTS)
                } else {
                    ui(TOOLBAR_LEFT)
                })
                .pr(ui(TOOLBAR_RIGHT))
                .gap(ui(TOOLBAR_GAP))
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
                        .min_w_0()
                        .pl(ui(TITLE_PADDING))
                        .text_size(ui(15.))
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(title),
                )
                .child(nav)
                .map(|this| match fit >= ToolbarFit::NoFolderSearch {
                    true => this.child(views),
                    false => this.child(views_menu),
                })
                .when(fit >= ToolbarFit::NushellInActions, |this| this.child(sort))
                .when(fit >= ToolbarFit::ViewsAsMenu, |this| this.child(nushell))
                .child(actions)
                .when(fit == ToolbarFit::Everything, |this| this.child(scope))
                .child(search)
                .children(window_controls(window, cx)),
        )
    }
}

/// The sort keys and the two directions, for a menu that lists them.
pub(super) fn sort_key_items(
    view: &Entity<Finder>,
    mut menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let finder = view.read(cx);
    let sort = finder.current_sort();
    let mut keys: Vec<SortKey> = Vec::new();
    if finder.is_searching() {
        keys.push(SortKey::Relevance);
    }
    keys.extend(SortKey::BROWSE);
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
}

fn sort_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let finder = view.read(cx);
    let hidden = finder.config.show_hidden;
    let folders_first = finder.config.folders_first;
    sort_key_items(view, menu.label("Sort By"), window, cx)
        .separator()
        .item(
            PopupMenuItem::new("Keep Folders on Top")
                .checked(folders_first)
                .action(Box::new(ToggleFoldersFirst)),
        )
        .item(
            PopupMenuItem::new("Show Hidden Files")
                .checked(hidden)
                .action(Box::new(ToggleHidden)),
        )
        .separator()
        .submenu("Group By", window, cx, {
            let view = view.clone();
            move |mut menu, window, cx| {
                let config = &view.read(cx).config;
                let grouped = config.use_groups.then_some(config.group_by);
                for key in [None].into_iter().chain(GroupKey::ALL.map(Some)) {
                    menu = menu.item(
                        PopupMenuItem::new(key.map_or("None", GroupKey::label))
                            .checked(grouped == key)
                            .on_click(window.listener_for(
                                &view,
                                move |this: &mut Finder, _: &ClickEvent, _, cx| {
                                    this.set_group_by(key, cx)
                                },
                            )),
                    );
                }
                menu
            }
        })
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
    // What a narrow toolbar has no room for comes first.
    let fit = view.read(cx).toolbar_fit;
    let menu = match fit >= ToolbarFit::NushellInActions {
        true => menu,
        false => {
            let view = view.clone();
            menu.submenu(
                "Sort and View Options",
                window,
                cx,
                move |menu, window, cx| sort_menu(&view, menu, window, cx),
            )
        }
    };
    let menu = match fit >= ToolbarFit::ViewsAsMenu {
        true => menu,
        false => {
            let view = view.clone();
            menu.submenu("Nushell", window, cx, move |menu, window, cx| {
                nushell_menu(&view, menu, window, cx)
            })
            .separator()
        }
    };
    let finder = view.read(cx);
    let has_selection = !finder.selected_entries().is_empty();
    let in_folder = finder.current_folder().is_some();
    let folder = finder.current_folder().map(|p| p.to_path_buf());
    let is_favorite = folder
        .as_ref()
        .is_some_and(|f| finder.config.favorites.contains(f));
    let split = finder.has_split_view(cx);
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
    .separator()
    .item(
        PopupMenuItem::new("New Tab")
            .icon(Lucide::Plus)
            .disabled(finder.picker.is_some())
            .action(Box::new(NewTab)),
    )
    .item(
        PopupMenuItem::new("Split View")
            .icon(Lucide::Columns2)
            .checked(split)
            .disabled(finder.picker.is_some())
            .action(Box::new(ToggleSplitView)),
    )
    .item(
        PopupMenuItem::new("Preview Pane")
            .icon(Lucide::PanelRight)
            .checked(finder.config.show_preview_pane)
            .action(Box::new(TogglePreviewPane)),
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
    // The menu bar's Edit › Undo and Redo, for systems without one.
    .item(
        PopupMenuItem::new("Undo")
            .icon(Lucide::Undo2)
            .action(Box::new(gpui_kit::component::input::Undo)),
    )
    .item(
        PopupMenuItem::new("Redo")
            .icon(Lucide::Redo2)
            .action(Box::new(gpui_kit::component::input::Redo)),
    )
    .separator()
    .item(
        PopupMenuItem::new("Filter with Nushell…")
            .icon(Lucide::Funnel)
            .action(Box::new(ToggleFilter)),
    )
    .item(PopupMenuItem::new("Go to Folder…").action(Box::new(GoToFolder)))
    .item(
        PopupMenuItem::new("Connect to Server…")
            .icon(Lucide::Server)
            .action(Box::new(ConnectToServer)),
    )
    .item(PopupMenuItem::new("Search Syntax…").action(Box::new(SearchHelp)))
    .item(
        PopupMenuItem::new("Settings…")
            .icon(IconName::Settings)
            .action(Box::new(ShowSettings)),
    )
    .min_w(ui(220.))
    // It's longer than a short window, or a narrow toolbar's extras make it so.
    .scrollable(true)
    .max_h(window.viewport_size().height - ui(16.))
}

#[cfg(test)]
mod tests {
    use super::{TOOLBAR_FULL_WIDTH, TOOLBAR_MIN_WIDTH, ToolbarFit};

    #[test]
    fn the_toolbar_folds_one_step_at_a_time() {
        let full = TOOLBAR_FULL_WIDTH;

        assert_eq!(ToolbarFit::for_room(full), ToolbarFit::Everything);
        assert_eq!(ToolbarFit::for_room(full - 1.), ToolbarFit::NoFolderSearch);
        assert_eq!(ToolbarFit::for_room(full - 50.), ToolbarFit::NoFolderSearch);
        assert_eq!(ToolbarFit::for_room(full - 51.), ToolbarFit::ViewsAsMenu);
        assert_eq!(
            ToolbarFit::for_room(full - 149.),
            ToolbarFit::NushellInActions
        );
        assert_eq!(
            ToolbarFit::for_room(TOOLBAR_MIN_WIDTH - 200.),
            ToolbarFit::SortInActions
        );
    }
}
