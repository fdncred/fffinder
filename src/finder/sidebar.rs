//! The sidebar: Recents, Favorites, Smart Folders, Locations, the
//! Trash, and index status. With the folder tree on (see folder_tree.rs),
//! Favorites and Locations open in place to show the folders inside them.
//! The rows are drawn from a list of lines, and only the lines on screen are
//! drawn, so a deep tree costs no more than a short one.

use super::fit::with_cut_off_text;
use super::folder_tree::{Disclosure, NodeKey, Section};
use super::list::ResizeEdge;
use super::toolbar::{TOOLBAR_HEIGHT, pill, pill_button, window_drag_area};
use super::views::{DragPreview, DragSource, DraggedFiles};
use super::{Finder, IndexActivity, group_digits};
use crate::actions::*;
use crate::macos::IconLookup;
use crate::model::Location;
use crate::theme::palette;
use crate::theme::ui;
use fffinder_core::FileKind;
use fffinder_core::config::SIDEBAR_WIDTH_RANGE;
use fffinder_core::places::{Place, PlaceIcon};
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

/// A sidebar row's height, in points.
pub(super) const ROW_HEIGHT: f32 = 28.;
/// A section's heading, with the room above it that sets sections apart.
const HEADING_HEIGHT: f32 = 36.;
/// The room for a row's disclosure triangle.
const DISCLOSURE_WIDTH: f32 = 14.;
/// Between a disclosure triangle and its icon. A level of the tree is
/// indented by both, so a folder's triangle sits under its parent's icon.
const DISCLOSURE_GAP: f32 = 2.;
const TREE_INDENT: f32 = DISCLOSURE_WIDTH + DISCLOSURE_GAP;

/// A Favorites row being dragged to a new place in the list.
#[derive(Clone)]
struct DraggedFavorite(PathBuf);

/// One line of the sidebar.
#[derive(Clone)]
pub(super) enum Line {
    /// A section's name.
    Heading(&'static str),
    Recents,
    /// A favorite or a location, by where it is in its list. `disclosure` is
    /// `None` when the folder tree is off.
    Place {
        key: NodeKey,
        ix: usize,
        disclosure: Option<Disclosure>,
    },
    /// A folder in the tree, inside a place.
    Folder(FolderLine),
    /// A saved search, by where it is in the list of them.
    Smart(usize),
    Trash,
}

/// A folder's line in the tree.
#[derive(Clone)]
pub(super) struct FolderLine {
    pub(super) key: NodeKey,
    pub(super) name: SharedString,
    /// How many levels inside its place it is, 0 right under it.
    pub(super) depth: usize,
    pub(super) disclosure: Disclosure,
}

impl Line {
    /// The line's node in the folder tree, and whether it opens: a place
    /// while the tree is on, or a folder inside one.
    pub(super) fn node(&self) -> Option<(&NodeKey, Disclosure)> {
        match self {
            Line::Place {
                key,
                disclosure: Some(disclosure),
                ..
            }
            | Line::Folder(FolderLine {
                key, disclosure, ..
            }) => Some((key, *disclosure)),
            Line::Heading(_)
            | Line::Recents
            | Line::Place {
                disclosure: None, ..
            }
            | Line::Smart(_)
            | Line::Trash => None,
        }
    }
}

/// The sidebar's lines as last laid out, and a hash of what they came from.
pub(super) struct SidebarLayout {
    key: u64,
    lines: Rc<Vec<Line>>,
    sizes: Rc<Vec<Size<Pixels>>>,
}

/// What a sidebar row for a place shows and does.
struct RowSpec {
    id: ElementId,
    icon: AnyElement,
    label: SharedString,
    target: Location,
    /// The favorite or location it shows, and whether it's a favorite.
    place: Option<(Place, bool)>,
    /// Its node in the folder tree, and whether that opens.
    node: Option<(NodeKey, Disclosure)>,
}

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

    /// A hash of everything the sidebar's lines show, so they're laid out
    /// again only when one of those changes, not on every frame.
    fn sidebar_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.config.sidebar_mode.hash(&mut hasher);
        self.config.show_hidden.hash(&mut hasher);
        crate::theme::ui_scale().to_bits().hash(&mut hasher);
        self.favorites.len().hash(&mut hasher);
        for place in self.favorites.iter().chain(&self.locations) {
            place.path.hash(&mut hasher);
        }
        self.smart_folders().len().hash(&mut hasher);
        if self.tree_on() {
            self.tree.revision().hash(&mut hasher);
            self.current_folder().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The sidebar's lines and their heights, laid out again when what they
    /// show changed.
    fn sidebar_layout(&mut self) -> (Rc<Vec<Line>>, Rc<Vec<Size<Pixels>>>) {
        let key = self.sidebar_key();
        if self.sidebar_layout.as_ref().is_none_or(|l| l.key != key) {
            let lines = self.build_sidebar_lines();
            let sizes = lines
                .iter()
                .map(|line| {
                    let height = match line {
                        Line::Heading(_) => HEADING_HEIGHT,
                        _ => ROW_HEIGHT,
                    };
                    size(px(0.), ui(height))
                })
                .collect();
            self.sidebar_layout = Some(SidebarLayout {
                key,
                lines: Rc::new(lines),
                sizes: Rc::new(sizes),
            });
        }
        let layout = self
            .sidebar_layout
            .as_ref()
            .expect("the layout was just made");
        (layout.lines.clone(), layout.sizes.clone())
    }

    /// The sidebar's lines, laid out again if what they show changed.
    pub(super) fn sidebar_lines(&mut self) -> Rc<Vec<Line>> {
        self.sidebar_layout().0
    }

    fn build_sidebar_lines(&self) -> Vec<Line> {
        let show_hidden = self.config.show_hidden;
        let current = self.current_folder();
        let mut lines = vec![Line::Recents, Line::Heading("Favorites")];
        let places = |lines: &mut Vec<Line>, section: Section, places: &[Place]| {
            for (ix, place) in places.iter().enumerate() {
                let key = NodeKey::place(section, place.path.clone());
                if self.tree_on() {
                    self.tree.push_place(key, ix, show_hidden, current, lines);
                } else {
                    lines.push(Line::Place {
                        key,
                        ix,
                        disclosure: None,
                    });
                }
            }
        };
        places(&mut lines, Section::Favorites, &self.favorites);
        if !self.smart_folders().is_empty() {
            lines.push(Line::Heading("Smart Folders"));
            lines.extend((0..self.smart_folders().len()).map(Line::Smart));
        }
        lines.push(Line::Heading("Locations"));
        places(&mut lines, Section::Locations, &self.locations);
        lines.push(Line::Trash);
        lines
    }

    /// Whether the line is the place the window shows.
    pub(super) fn line_is_active(&self, line: &Line) -> bool {
        let current = self.history.current();
        match line {
            Line::Heading(_) => false,
            Line::Recents => *current == Location::Recents,
            Line::Trash => *current == Location::Trash,
            Line::Smart(ix) => self.smart_is_active(*ix),
            Line::Place { key, .. } if !self.tree_on() => {
                current.folder() == Some(key.path.as_path())
            }
            Line::Place { key, .. } | Line::Folder(FolderLine { key, .. }) => {
                self.tree.selected() == Some(key)
            }
        }
    }

    /// The name a line shows, for typing to select it.
    pub(super) fn line_name(&self, line: &Line) -> Option<SharedString> {
        match line {
            Line::Heading(_) => None,
            Line::Recents => Some("Recents".into()),
            Line::Trash => Some("Trash".into()),
            Line::Smart(ix) => self.smart_folders().get(*ix).map(|f| f.name.clone().into()),
            Line::Place { key, ix, .. } => self.place(key.section, *ix).map(|p| p.name.into()),
            Line::Folder(folder) => Some(folder.name.clone()),
        }
    }

    /// A favorite or a location, by where it is in its list.
    fn place(&self, section: Section, ix: usize) -> Option<Place> {
        match section {
            Section::Favorites => self.favorites.get(ix),
            Section::Locations => self.locations.get(ix),
        }
        .cloned()
    }

    pub(super) fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border) = (theme.sidebar, theme.sidebar_border);
        let (lines, sizes) = self.sidebar_layout();
        if let Some(ix) = self.tree.reveal_in(&lines) {
            self.sidebar_scroll.scroll_to_item(ix, ScrollStrategy::Top);
        }
        let tree = self.tree_on();
        let renaming_here = self.renaming.as_ref().is_some_and(|r| r.in_sidebar);
        let list = v_virtual_list(
            cx.entity(),
            "sidebar-lines",
            sizes,
            move |this: &mut Self, range: Range<usize>, window, cx| {
                range
                    .map(|ix| this.render_sidebar_line(&lines[ix], window, cx))
                    .collect::<Vec<_>>()
            },
        )
        .size_full()
        .px(ui(8.))
        .pb(ui(8.))
        .track_scroll(&self.sidebar_scroll);
        let list = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .relative()
            .child(list)
            .vertical_scrollbar(&self.sidebar_scroll)
            // With the tree, the sidebar takes the keyboard when a row is
            // clicked, as Explorer's does.
            .when(tree, |this| {
                let keys = this
                    .key_context(if renaming_here { TREE_RENAMING } else { TREE })
                    .track_focus(&self.tree_focus);
                self.tree_keys(keys, cx)
            });

        // Edge to edge, as in macOS 27 and the Windows and Linux file
        // managers: attached to the window, with a line between it and the
        // files rather than a floating panel inset from the edges.
        div()
            .w(ui(self.sidebar_width_shown(window)))
            .h_full()
            .flex_shrink_0()
            .child(
                v_flex()
                    .id("sidebar")
                    .size_full()
                    .bg(bg)
                    .border_r_1()
                    .border_color(border)
                    .overflow_hidden()
                    // Drop folders here to add them to Favorites. A folder
                    // from the tree is in the sidebar already, and let go
                    // anywhere but on a folder, its drag just ends.
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        for path in paths.paths().iter().filter(|p| p.is_dir()) {
                            this.add_favorite(path.clone(), cx);
                        }
                    }))
                    .on_drop(cx.listener(|this, files: &DraggedFiles, _, cx| {
                        if files.1 == DragSource::Tree {
                            return;
                        }
                        for path in files.0.iter().filter(|p| p.is_dir()) {
                            this.add_favorite(path.clone(), cx);
                        }
                    }))
                    .child(window_drag_area(
                        h_flex()
                            .h(ui(TOOLBAR_HEIGHT))
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

    fn render_sidebar_line(
        &mut self,
        line: &Line,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = palette(cx);
        let active = self.line_is_active(line);
        match line {
            Line::Heading(label) => section_heading(label, pal.sidebar_heading).into_any_element(),
            Line::Recents => self.sidebar_row(
                RowSpec {
                    id: "recents".into(),
                    icon: glyph("SidebarRecents", Icon::new(Lucide::Clock), pal.sidebar_icon),
                    label: "Recents".into(),
                    target: Location::Recents,
                    place: None,
                    node: None,
                },
                active,
                window,
                cx,
            ),
            Line::Place {
                key,
                ix,
                disclosure,
            } => {
                let Some(place) = self.place(key.section, *ix) else {
                    return div().into_any_element();
                };
                let id: ElementId = match key.section {
                    Section::Favorites => ("fav", *ix).into(),
                    Section::Locations => ("loc", *ix).into(),
                };
                let favorite = key.section == Section::Favorites;
                self.sidebar_row(
                    RowSpec {
                        id,
                        icon: glyph(
                            finder_sidebar_icon(place.icon),
                            place_icon(place.icon),
                            pal.sidebar_icon,
                        ),
                        label: place.name.clone().into(),
                        target: Location::Folder(place.path.clone()),
                        place: Some((place, favorite)),
                        node: disclosure.map(|d| (key.clone(), d)),
                    },
                    active,
                    window,
                    cx,
                )
            }
            Line::Folder(folder) => self.folder_row(folder, active, window, cx),
            Line::Smart(ix) => self.smart_row(*ix, active, cx).into_any_element(),
            Line::Trash => {
                // Drop items on the Trash to trash them.
                let trash = self.sidebar_row(
                    RowSpec {
                        id: "trash".into(),
                        icon: Icon::new(Lucide::Trash)
                            .with_size(ui(16.))
                            .text_color(pal.sidebar_icon)
                            .into_any_element(),
                        label: "Trash".into(),
                        target: Location::Trash,
                        place: None,
                        node: None,
                    },
                    active,
                    window,
                    cx,
                );
                super::trash::trash_drop_target(div().id("trash-drop").child(trash), cx)
                    .into_any_element()
            }
        }
    }

    /// The room for a disclosure triangle before an icon, while the tree is
    /// on, with the triangle when `node` opens.
    fn with_disclosure(
        &self,
        icon: AnyElement,
        node: Option<(&NodeKey, Disclosure)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.tree_on() {
            return icon;
        }
        let slot = div()
            .w(ui(DISCLOSURE_WIDTH))
            .h(ui(DISCLOSURE_WIDTH))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0();
        let triangle = match node {
            Some((key, disclosure @ (Disclosure::Closed | Disclosure::Open))) => {
                let theme = cx.theme();
                let toggle = key.clone();
                slot.id(key.disclosure_id())
                    .rounded(ui(4.))
                    .hover(|s| s.bg(theme.list_hover))
                    .child(
                        Icon::new(if disclosure == Disclosure::Open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .with_size(ui(12.))
                        .text_color(theme.muted_foreground),
                    )
                    // A press here opens or closes the node; it doesn't show
                    // the folder or start a drag.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.toggle_node(toggle.clone(), cx);
                    }))
                    .test_support()
                    .into_any_element()
            }
            Some((_, Disclosure::Unreadable)) => slot
                .id("unreadable")
                .child(
                    Icon::new(Lucide::Lock)
                        .with_size(ui(11.))
                        .text_color(cx.theme().muted_foreground),
                )
                .tooltip(|window, cx| {
                    Tooltip::new("fffinder can’t read this folder").build(window, cx)
                })
                .test_support()
                .into_any_element(),
            Some((_, Disclosure::Leaf)) | None => slot.into_any_element(),
        };
        h_flex()
            .gap(ui(DISCLOSURE_GAP))
            .flex_shrink_0()
            .child(triangle)
            .child(icon)
            .into_any_element()
    }

    /// The background of a row, with the tree's selection drawn stronger
    /// while the tree has the keyboard.
    fn row_frame(&self, id: ElementId, active: bool, window: &Window, cx: &App) -> Stateful<Div> {
        let theme = cx.theme();
        let focused = active && self.tree_focus.is_focused(window);
        h_flex()
            .id(id)
            .h(ui(ROW_HEIGHT))
            .px(ui(8.))
            .gap(ui(8.))
            .rounded(ui(7.))
            .text_size(ui(13.))
            .text_color(theme.sidebar_foreground)
            .when(active, |this| this.bg(theme.sidebar_accent))
            .when(focused, |this| this.border_1().border_color(theme.ring))
            .when(!active, |this| this.hover(|s| s.bg(theme.list_hover)))
    }

    fn sidebar_row(
        &self,
        spec: RowSpec,
        active: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RowSpec {
            id,
            icon,
            label,
            target,
            place,
            node,
        } = spec;
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
        let icon = self.with_disclosure(icon, node.as_ref().map(|(k, d)| (k, *d)), cx);
        let node_key = node.as_ref().map(|(key, _)| key.clone());
        let highlight = cx.theme().drop_target;
        let row = self
            .row_frame(id, active, window, cx)
            .child(icon)
            .child(row_label(label))
            .children(eject_button)
            .when_some(reorder, |row, (path, name)| {
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
                    return;
                }
                if this.tree_on() {
                    this.tree_focus.focus(window, cx);
                }
                match node_key.clone() {
                    Some(key) if event.click_count() >= 2 => this.toggle_node(key, cx),
                    Some(key) => this.open_node(key, window, cx),
                    None => this.navigate(target.clone(), window, cx),
                }
            }))
            .test_support();
        let Some((place, is_favorite)) = place else {
            return row.into_any_element();
        };
        let path: PathBuf = place.path.clone();
        // Files dropped on a place go into it; dropped between rows, folders
        // are added to Favorites. Held over a place, they open it: in the
        // tree as a node, else in the window.
        let row = match &node {
            Some((key, _)) => super::transfer::spring_loaded_node(row, key.clone(), cx),
            None => super::transfer::spring_loaded(row, path.clone(), cx),
        };
        let row = super::transfer::folder_drop_target(row, path.clone(), cx);
        // The menu lives on a wrapper so the row itself stays observable in UI tests.
        div()
            .id(SharedString::from(format!("place-menu:{}", path.display())))
            .child(row)
            .context_menu(move |menu, window, cx| {
                let open_nu = path.clone();
                let reveal = path.clone();
                let remove = path.clone();
                let here = vec![Location::Folder(path.clone())];
                let mut menu = super::menus::open_elsewhere_items(&view, menu, here, window);
                if let Some((key, disclosure)) = &node {
                    menu = tree_items(&view, menu, (key, *disclosure), window, cx);
                }
                menu = menu
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

    /// A folder in the tree: it opens and closes, shows its folder when
    /// clicked, drags like an item, takes dropped items, and has the menu of
    /// a folder.
    fn folder_row(
        &self,
        line: &FolderLine,
        active: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let FolderLine {
            key,
            name,
            depth,
            disclosure,
        } = line;
        let (depth, disclosure) = (*depth, *disclosure);
        let pal = palette(cx);
        let view = cx.entity();
        let path = key.path.clone();
        let label = self
            .rename_field(&path, true, cx)
            .unwrap_or_else(|| row_label(name.clone()).into_any_element());
        let icon = glyph(
            "SidebarGenericFolder",
            Icon::new(IconName::Folder),
            pal.sidebar_icon,
        );
        let (open_key, tab) = (key.clone(), Location::Folder(path.clone()));
        let preview = name.clone();
        let row = self
            .row_frame(key.row_id(), active, window, cx)
            .pl(ui(8. + TREE_INDENT * (depth + 1) as f32))
            .when(crate::system::is_cut(&path, cx), |this| this.opacity(0.5))
            .child(self.with_disclosure(icon, Some((key, disclosure)), cx))
            .child(label)
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener({
                    let tab = tab.clone();
                    move |this, _: &MouseDownEvent, window, cx| {
                        this.open_in_new_tabs(vec![tab.clone()], window, cx)
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.modifiers().secondary(cx) && this.picker.is_none() {
                    this.open_in_new_tabs(vec![tab.clone()], window, cx);
                    return;
                }
                this.tree_focus.focus(window, cx);
                if event.click_count() >= 2 {
                    this.toggle_node(open_key.clone(), cx);
                } else {
                    this.open_node(open_key.clone(), window, cx);
                }
            }))
            .on_drag(
                DraggedFiles(vec![path.clone()], DragSource::Tree),
                move |_, _, _, cx| {
                    cx.new(|_| DragPreview::new(preview.clone(), FileKind::Folder.icon_asset()))
                },
            )
            .external_drag_payload(|files: &DraggedFiles, _, _| {
                Some(ExternalDragPayload::Files(FileDragPaths::new(
                    files.0.iter().map(|p| (p.clone(), true)),
                )))
            })
            .test_support();
        let row = super::transfer::spring_loaded_node(row, key.clone(), cx);
        let row = super::transfer::folder_drop_target(row, path, cx);
        let menu_key = key.clone();
        div()
            .id(key.menu_id())
            .child(row)
            .context_menu(move |menu, window, cx| {
                folder_menu(&view, menu, (&menu_key, disclosure), window, cx)
            })
            .into_any_element()
    }

    /// Whether the Smart Folder's search and filter are what's showing.
    fn smart_is_active(&self, ix: usize) -> bool {
        self.smart_folders().get(ix).is_some_and(|folder| {
            self.is_searching()
                && self.query() == folder.query
                && self.filter_text() == folder.filter.as_deref().unwrap_or("")
        })
    }

    /// A saved search; active while its search and filter are showing.
    fn smart_row(&self, ix: usize, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let pal = palette(cx);
        let name = self.smart_folders()[ix].name.clone();
        let view = cx.entity();
        let icon = glyph(
            "SidebarSmartFolder",
            Icon::new(Lucide::FolderCog),
            pal.sidebar_icon,
        );
        let row = h_flex()
            .id(("smart", ix))
            .h(ui(ROW_HEIGHT))
            .px(ui(8.))
            .gap(ui(8.))
            .rounded(ui(7.))
            .text_size(ui(13.))
            .text_color(theme.sidebar_foreground)
            .when(active, |this| this.bg(theme.sidebar_accent))
            .when(!active, |this| this.hover(|s| s.bg(theme.list_hover)))
            .child(self.with_disclosure(icon, None, cx))
            .child(row_label(name.clone().into()))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if this.tree_on() {
                    this.tree_focus.focus(window, cx);
                }
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

/// The menu items a node of the tree adds for its folder: open or close it,
/// make a folder in it, paste into it, and Get Info.
///
/// # Arguments
///
/// * `view` - The browser
/// * `menu` - The menu so far
/// * `node` - The node, and whether it opens
/// * `window` - The browser's window
/// * `cx` - The app
///
/// # Returns
///
/// The menu with the node's items
fn tree_items(
    view: &Entity<Finder>,
    menu: PopupMenu,
    (key, disclosure): (&NodeKey, Disclosure),
    window: &mut Window,
    cx: &App,
) -> PopupMenu {
    let open = view.read(cx).node_is_open(key);
    let (toggle, make, paste, info) =
        (key.clone(), key.clone(), key.path.clone(), key.path.clone());
    let name = fffinder_core::entry::display_name(&key.path);
    let opens = match disclosure {
        Disclosure::Closed | Disclosure::Open => true,
        Disclosure::Leaf | Disclosure::Unreadable => false,
    };
    menu.when(opens, |menu| {
        menu.item(
            PopupMenuItem::new(if open { "Collapse" } else { "Expand" }).on_click(
                window.listener_for(view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                    this.toggle_node(toggle.clone(), cx)
                }),
            ),
        )
    })
    .separator()
    .item(
        PopupMenuItem::new("New Folder")
            .icon(IconName::FolderOpen)
            .on_click(window.listener_for(
                view,
                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                    this.new_folder_in_tree(make.clone(), window, cx)
                },
            )),
    )
    .item(
        PopupMenuItem::new(format!("Paste Into “{name}”"))
            .icon(Lucide::ClipboardPaste)
            .disabled(crate::system::clipboard_files(cx).is_empty())
            .on_click(window.listener_for(
                view,
                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                    this.paste_into(Some(paste.clone()), Finder::paste_mode(cx), window, cx)
                },
            )),
    )
    .item(
        PopupMenuItem::new("Get Info")
            .icon(IconName::Info)
            .on_click(window.listener_for(
                view,
                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                    this.show_info(vec![info.clone()], window, cx)
                },
            )),
    )
}

/// Right-click on a folder in the tree: what the item menu has for a folder,
/// acting on this one rather than the selected items.
///
/// # Arguments
///
/// * `view` - The browser
/// * `menu` - The menu to fill
/// * `node` - The folder's node, and whether it opens
/// * `window` - The browser's window
/// * `cx` - The app
///
/// # Returns
///
/// The folder's menu
fn folder_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    node: (&NodeKey, Disclosure),
    window: &mut Window,
    cx: &App,
) -> PopupMenu {
    let key = node.0;
    let path = key.path.clone();
    let name = fffinder_core::entry::display_name(&path);
    let is_favorite = view.read(cx).config.favorites.contains(&path);
    let here = vec![Location::Folder(path.clone())];
    let (nu, reveal, cut, copy, copy_path) = (
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
    );
    let (rename, trash, favorite) = (key.clone(), path.clone(), path);
    let menu = super::menus::open_elsewhere_items(view, menu, here, window);
    tree_items(view, menu, node, window, cx)
        .separator()
        .item(
            PopupMenuItem::new("Open Nushell Here")
                .icon(IconName::SquareTerminal)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.open_terminal_at(&nu, window, cx)
                    },
                )),
        )
        .item(
            PopupMenuItem::new(super::menus::reveal_label())
                .icon(IconName::ExternalLink)
                .on_click(move |_, _, cx| crate::system::reveal(&reveal, cx)),
        )
        .separator()
        .item(
            PopupMenuItem::new(format!("Cut “{name}”"))
                .icon(Lucide::Scissors)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.cut_paths(vec![cut.clone()], window, cx)
                    },
                )),
        )
        .item(
            PopupMenuItem::new(format!("Copy “{name}”"))
                .icon(IconName::Copy)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.copy_paths(vec![copy.clone()], window, cx)
                    },
                )),
        )
        .item(
            PopupMenuItem::new("Copy Path")
                .icon(Lucide::Link)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.copy_path_text(std::slice::from_ref(&copy_path), window, cx)
                    },
                )),
        )
        .separator()
        .item(
            PopupMenuItem::new("Rename…")
                .icon(Lucide::Pencil)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.rename_in_tree(rename.clone(), window, cx)
                    },
                )),
        )
        .when(!is_favorite, |menu| {
            menu.item(
                PopupMenuItem::new("Add to Sidebar")
                    .icon(IconName::Star)
                    .on_click(window.listener_for(
                        view,
                        move |this: &mut Finder, _: &ClickEvent, _, cx| {
                            this.add_favorite(favorite.clone(), cx)
                        },
                    )),
            )
        })
        .separator()
        .item(
            PopupMenuItem::new("Move to Trash")
                .icon(Lucide::Trash)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.trash_now(vec![trash.clone()], window, cx)
                    },
                )),
        )
        .min_w(ui(230.))
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
        .h(ui(HEADING_HEIGHT))
        .flex()
        .items_end()
        .pb(ui(4.))
        .px(ui(8.))
        .text_size(ui(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(label)
}
