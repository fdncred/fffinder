//! The floating sidebar: Recents, Favorites, Locations, and index status.

use super::fit::with_cut_off_text;
use super::list::ResizeEdge;
use super::toolbar::{TOOLBAR_HEIGHT, pill, pill_button, window_drag_area};
use super::views::DraggedFiles;
use super::{Finder, IndexActivity, group_digits};
use crate::actions::*;
use crate::macos::IconLookup;
use crate::model::Location;
use crate::theme::palette;
use crate::theme::ui;
use fffinder_core::config::SIDEBAR_WIDTH_RANGE;
use fffinder_core::places::{Place, PlaceIcon};
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// A Favorites row being dragged to a new place in the list.
#[derive(Clone)]
struct DraggedFavorite(PathBuf);

/// Finder's own sidebar icon for a place, in CoreTypes.bundle.
fn finder_sidebar_icon(icon: PlaceIcon) -> &'static str {
    match icon {
        PlaceIcon::Applications => "SidebarApplicationsFolder",
        PlaceIcon::Desktop => "SidebarDesktopFolder",
        PlaceIcon::Documents => "SidebarDocumentsFolder",
        PlaceIcon::Downloads => "SidebarDownloadsFolder",
        PlaceIcon::Pictures => "SidebarPicturesFolder",
        PlaceIcon::Music => "SidebarMusicFolder",
        PlaceIcon::Movies => "SidebarMoviesFolder",
        PlaceIcon::Home => "SidebarHomeFolder",
        PlaceIcon::Cloud => "SidebariCloud",
        PlaceIcon::Drive => "SidebarInternalDisk",
        PlaceIcon::External => "SidebarExternalDisk",
        PlaceIcon::Folder => "SidebarGenericFolder",
        PlaceIcon::Network => "SidebarServerDrive",
    }
}

/// Finder's sidebar icon tinted with the accent color on macOS, or `fallback`.
fn glyph(finder_icon: &str, fallback: Icon, color: Hsla) -> AnyElement {
    let rgba = Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    let packed = byte(rgba.r) << 24 | byte(rgba.g) << 16 | byte(rgba.b) << 8 | byte(rgba.a);
    let later = match crate::macos::sidebar_icon(finder_icon, packed) {
        IconLookup::Ready(path) => {
            return img(path).size(ui(16.)).flex_shrink_0().into_any_element();
        }
        IconLookup::Later => true,
        IconLookup::Unavailable => false,
    };
    div()
        .relative()
        .flex_shrink_0()
        .child(fallback.with_size(ui(16.)).text_color(color))
        .when(later, |this| this.child(super::views::redraw_soon()))
        .into_any_element()
}

pub fn place_icon(icon: PlaceIcon) -> Icon {
    match icon {
        PlaceIcon::Applications => Icon::new(Lucide::AppWindow),
        PlaceIcon::Desktop => Icon::new(Lucide::Monitor),
        PlaceIcon::Documents => Icon::new(IconName::FileText),
        PlaceIcon::Downloads => Icon::new(Lucide::Download),
        PlaceIcon::Pictures => Icon::new(Lucide::Image),
        PlaceIcon::Music => Icon::new(Lucide::Music),
        PlaceIcon::Movies => Icon::new(Lucide::Film),
        PlaceIcon::Home => Icon::new(Lucide::House),
        PlaceIcon::Cloud => Icon::new(Lucide::Cloud),
        PlaceIcon::Drive => Icon::new(IconName::HardDrive),
        PlaceIcon::External => Icon::new(Lucide::Usb),
        PlaceIcon::Folder => Icon::new(IconName::Folder),
        PlaceIcon::Network => Icon::new(Lucide::Server),
    }
}

impl Finder {
    /// The sidebar's width in points: what was chosen, unless the toolbar
    /// beside it would run out of the window.
    pub(super) fn sidebar_width_shown(&self, window: &Window) -> f32 {
        let room = f32::from(window.viewport_size().width) / crate::theme::ui_scale();
        let (min, _) = SIDEBAR_WIDTH_RANGE;
        self.config
            .sidebar_width()
            .min(room - super::toolbar::TOOLBAR_MIN_WIDTH)
            .max(min)
    }

    /// The strip over the sidebar's right edge that drags it, drawn over
    /// everything so the toolbar beside it can't take the press.
    pub(super) fn sidebar_handle(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        self.resize_handle("resize-sidebar", ResizeEdge::Sidebar, cx)
            .absolute()
            .top_0()
            .h_full()
            .left(ui(self.sidebar_width_shown(window) - 3.))
            .w(ui(6.))
    }

    pub(super) fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border) = (theme.sidebar, theme.sidebar_border);
        let pal = palette(cx);
        let current = self.history.current().clone();

        let mut list = v_flex()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(ui(8.))
            .pb(ui(8.))
            .child(self.sidebar_row(
                "recents",
                glyph("SidebarRecents", Icon::new(Lucide::Clock), pal.sidebar_icon),
                "Recents".into(),
                current == Location::Recents,
                Location::Recents,
                None,
                cx,
            ));

        list = list.child(section_heading("Favorites", pal.sidebar_heading));
        let favorites = self.favorites.clone();
        for (ix, place) in favorites.into_iter().enumerate() {
            let active = current.folder() == Some(place.path.as_path());
            list = list.child(self.sidebar_row(
                ("fav", ix),
                glyph(
                    finder_sidebar_icon(place.icon),
                    place_icon(place.icon),
                    pal.sidebar_icon,
                ),
                place.name.clone().into(),
                active,
                Location::Folder(place.path.clone()),
                Some((place, true)),
                cx,
            ));
        }

        if !self.smart_folders().is_empty() {
            list = list.child(section_heading("Smart Folders", pal.sidebar_heading));
            for ix in 0..self.smart_folders().len() {
                list = list.child(self.smart_row(ix, cx));
            }
        }

        list = list.child(section_heading("Locations", pal.sidebar_heading));
        let locations = self.locations.clone();
        for (ix, place) in locations.into_iter().enumerate() {
            let active = current.folder() == Some(place.path.as_path());
            list = list.child(self.sidebar_row(
                ("loc", ix),
                glyph(
                    finder_sidebar_icon(place.icon),
                    place_icon(place.icon),
                    pal.sidebar_icon,
                ),
                place.name.clone().into(),
                active,
                Location::Folder(place.path.clone()),
                Some((place, false)),
                cx,
            ));
        }

        // Drop items on the Trash to trash them.
        let trash = self.sidebar_row(
            "trash",
            Icon::new(Lucide::Trash)
                .with_size(ui(16.))
                .text_color(pal.sidebar_icon)
                .into_any_element(),
            "Trash".into(),
            current == Location::Trash,
            Location::Trash,
            None,
            cx,
        );
        list = list.child(super::trash::trash_drop_target(
            div().id("trash-drop").child(trash),
            cx,
        ));

        div()
            .w(ui(self.sidebar_width_shown(window)))
            .h_full()
            .flex_shrink_0()
            .p(ui(8.))
            .pr(ui(0.))
            .child(
                v_flex()
                    .id("sidebar")
                    .size_full()
                    .rounded(ui(14.))
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    // Drop folders here to add them to Favorites.
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        for path in paths.paths().iter().filter(|p| p.is_dir()) {
                            this.add_favorite(path.clone(), cx);
                        }
                    }))
                    .on_drop(cx.listener(|this, files: &DraggedFiles, _, cx| {
                        for path in files.0.iter().filter(|p| p.is_dir()) {
                            this.add_favorite(path.clone(), cx);
                        }
                    }))
                    .child(window_drag_area(
                        h_flex()
                            .h(ui(TOOLBAR_HEIGHT - 8.))
                            .flex_shrink_0()
                            .justify_end()
                            .px(ui(6.))
                            .child(
                                pill(cx)
                                    .bg(gpui_kit::transparent_black())
                                    .border_color(gpui_kit::transparent_black())
                                    .child(
                                        pill_button("hide-sidebar", IconName::PanelLeft, cx)
                                            .tooltip("Hide Sidebar")
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, window, cx| {
                                                    this.toggle_sidebar(&ToggleSidebar, window, cx)
                                                },
                                            )),
                                    ),
                            ),
                    ))
                    .child(list)
                    .child(self.render_index_footer(cx)),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn sidebar_row(
        &self,
        id: impl Into<ElementId>,
        icon: AnyElement,
        label: SharedString,
        active: bool,
        target: Location,
        place: Option<(Place, bool)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let view = cx.entity();
        let eject = place
            .as_ref()
            .filter(|(p, _)| p.ejectable)
            .map(|(p, _)| p.path.clone());
        let eject_button = eject.clone().map(|path| {
            div()
                .id(("eject", super::views::path_id(&path)))
                .p(ui(2.))
                .rounded(ui(4.))
                .hover(|s| s.bg(theme.list_hover))
                .child(
                    Icon::new(Lucide::Eject)
                        .with_size(ui(13.))
                        .text_color(theme.muted_foreground),
                )
                .tooltip(|window, cx| Tooltip::new("Eject").build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.eject(path.clone(), window, cx);
                }))
                .test_support()
        });
        // Favorites can be dragged into another order.
        let reorder = place
            .as_ref()
            .filter(|(_, is_favorite)| *is_favorite)
            .map(|(place, _)| (place.path.clone(), label.clone()));
        let row = h_flex()
            .id(id)
            .h(ui(28.))
            .px(ui(8.))
            .gap(ui(8.))
            .rounded(ui(7.))
            .text_size(ui(13.))
            .text_color(theme.sidebar_foreground)
            .when(active, |this| this.bg(theme.sidebar_accent))
            .when(!active, |this| this.hover(|s| s.bg(theme.list_hover)))
            .child(icon)
            .child(row_label(label))
            .children(eject_button)
            .when_some(reorder, |row, (path, name)| {
                let highlight = theme.drop_target;
                let onto = path.clone();
                row.on_drag(DraggedFavorite(path), move |_, _, _, cx| {
                    cx.new(|_| super::list::DragLabel(name.clone()))
                })
                .drag_over::<DraggedFavorite>(move |style, _, _, _| style.bg(highlight))
                .on_drop(cx.listener(
                    move |this, moved: &DraggedFavorite, _, cx| {
                        this.move_favorite(&moved.0, &onto, cx)
                    },
                ))
            })
            .on_mouse_down(MouseButton::Middle, {
                let target = target.clone();
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.open_in_new_tabs(vec![target.clone()], window, cx)
                })
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.modifiers().secondary(cx) && this.picker.is_none() {
                    this.open_in_new_tabs(vec![target.clone()], window, cx);
                } else {
                    this.navigate(target.clone(), window, cx);
                }
            }))
            .test_support();
        let Some((place, is_favorite)) = place else {
            return row.into_any_element();
        };
        let path: PathBuf = place.path.clone();
        // Files dropped on a place go into it; dropped between rows, folders
        // are added to Favorites.
        let row = super::transfer::spring_loaded(row, path.clone(), cx);
        let row = super::transfer::folder_drop_target(row, path.clone(), cx);
        // The menu lives on a wrapper so the row itself stays observable in UI tests.
        div()
            .id(SharedString::from(format!("place-menu:{}", path.display())))
            .child(row)
            .context_menu(move |menu, window, _| {
                let open_nu = path.clone();
                let reveal = path.clone();
                let remove = path.clone();
                let here = vec![Location::Folder(path.clone())];
                let mut menu = super::menus::open_elsewhere_items(&view, menu, here, window)
                    .separator()
                    .item(
                        PopupMenuItem::new("Open Nushell Here")
                            .icon(IconName::SquareTerminal)
                            .on_click(window.listener_for(
                                &view,
                                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                    this.open_terminal_at(&open_nu, window, cx)
                                },
                            )),
                    )
                    .item(
                        PopupMenuItem::new(super::menus::reveal_label())
                            .icon(IconName::ExternalLink)
                            .on_click(move |_, _, cx| crate::system::reveal(&reveal, cx)),
                    );
                if let Some(path) = eject.clone() {
                    menu = menu.separator().item(
                        PopupMenuItem::new("Eject").icon(Lucide::Eject).on_click(
                            window.listener_for(
                                &view,
                                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                    this.eject(path.clone(), window, cx)
                                },
                            ),
                        ),
                    );
                }
                if is_favorite {
                    menu =
                        menu.separator()
                            .item(PopupMenuItem::new("Remove from Sidebar").on_click(
                                window.listener_for(
                                    &view,
                                    move |this: &mut Finder, _: &ClickEvent, _, cx| {
                                        this.remove_favorite(&remove, cx)
                                    },
                                ),
                            ));
                }
                menu
            })
            .into_any_element()
    }

    /// A saved search; active while its search and filter are showing.
    fn smart_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let pal = palette(cx);
        let folder = &self.smart_folders()[ix];
        let active = self.is_searching()
            && self.query() == folder.query
            && self.filter_text() == folder.filter.as_deref().unwrap_or("");
        let name = folder.name.clone();
        let view = cx.entity();
        let row = h_flex()
            .id(("smart", ix))
            .h(ui(28.))
            .px(ui(8.))
            .gap(ui(8.))
            .rounded(ui(7.))
            .text_size(ui(13.))
            .text_color(theme.sidebar_foreground)
            .when(active, |this| this.bg(theme.sidebar_accent))
            .when(!active, |this| this.hover(|s| s.bg(theme.list_hover)))
            .child(glyph(
                "SidebarSmartFolder",
                Icon::new(Lucide::FolderCog),
                pal.sidebar_icon,
            ))
            .child(row_label(name.clone().into()))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open_smart_folder(ix, window, cx)
            }))
            .test_support();
        div()
            .id(("smart-menu", ix))
            .child(row)
            .context_menu(move |menu, window, _| {
                let name = name.clone();
                menu.item(
                    PopupMenuItem::new("Remove from Sidebar").on_click(window.listener_for(
                        &view,
                        move |this: &mut Finder, _: &ClickEvent, _, cx| {
                            this.remove_smart_folder(&name, cx)
                        },
                    )),
                )
            })
    }

    fn render_index_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let spinner = || Spinner::new().xsmall().color(muted).into_any_element();
        let (icon, text) = match self.index_activity() {
            Some(IndexActivity::Scanning(n)) => {
                (spinner(), format!("Indexing… {}", group_digits(n)))
            }
            Some(IndexActivity::Finishing) => (spinner(), "Finishing index…".to_string()),
            None => (
                Icon::new(Lucide::ScanSearch)
                    .with_size(ui(14.))
                    .text_color(muted)
                    .into_any_element(),
                format!("{} files indexed", group_digits(self.indexed_files())),
            ),
        };
        h_flex()
            .id("index-footer")
            .h(ui(34.))
            .flex_shrink_0()
            .px(ui(14.))
            .gap(ui(8.))
            .border_t_1()
            .border_color(theme.sidebar_border)
            .text_size(ui(11.))
            .text_color(muted)
            .child(icon)
            .child(div().flex_1().truncate().child(text))
            .child(
                div()
                    .id("index-settings")
                    .p(ui(3.))
                    .rounded(ui(5.))
                    .hover(|s| s.bg(theme.list_hover))
                    .child(
                        Icon::new(IconName::Settings)
                            .with_size(ui(14.))
                            .text_color(muted),
                    )
                    .tooltip(|window, cx| Tooltip::new("Settings").build(window, cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.show_settings(&ShowSettings, window, cx)
                    }))
                    .test_support(),
            )
    }
}

/// A sidebar row's name, with all of it in a tooltip when the sidebar is too
/// narrow for it.
fn row_label(label: SharedString) -> impl IntoElement {
    with_cut_off_text(
        div().id("label").flex_1().truncate(),
        StyledText::new(label.clone()),
        label,
        |shown| shown,
    )
}

fn section_heading(label: &'static str, color: Hsla) -> impl IntoElement {
    div()
        .pt(ui(14.))
        .pb(ui(4.))
        .px(ui(8.))
        .text_size(ui(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(label)
}
