//! Context menus for items and for empty space.

use super::Finder;
use crate::actions::*;
use crate::theme::ui;
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::IconName;
use gpui_kit::component::input;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};

pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Show in File Manager"
    }
}

/// The "Nushell Actions" submenu: the commands in actions.nu, drawn at the
/// Nushell text size from Settings.
pub fn actions_submenu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let view = view.clone();
    menu.submenu(
        "Nushell Actions",
        window,
        cx,
        move |mut menu, window, cx| {
            let finder = view.read(cx);
            let actions = finder.user_actions().to_vec();
            let size = finder.config.nu_font_size();
            if let Some(err) = finder.user_actions_error() {
                let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or(err);
                let label = format!("actions.nu has an error: {}", first.trim());
                menu = menu.item(nu_item("nu-actions-error", label, size).disabled(true));
            }
            for (ix, action) in actions.iter().enumerate() {
                menu = menu.item(
                    nu_item(("nu-action", ix), action.label.clone(), size)
                        .icon(Lucide::Zap)
                        .on_click(window.listener_for(
                            &view,
                            move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                this.run_user_action(ix, window, cx)
                            },
                        )),
                );
            }
            menu.separator()
                .item(
                    nu_item("nu-actions-edit", "Edit Actions…", size)
                        .icon(Lucide::SquarePen)
                        .on_click(window.listener_for(
                            &view,
                            |this: &mut Finder, _: &ClickEvent, window, cx| {
                                this.edit_user_actions(window, cx)
                            },
                        )),
                )
                .item(
                    nu_item("nu-actions-reload", "Reload Actions", size).on_click(
                        window.listener_for(&view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                            this.load_user_actions(cx)
                        }),
                    ),
                )
                .min_w(ui(220.))
        },
    )
}

/// A menu item whose text is `size` points; standard items are a fixed size.
fn nu_item(id: impl Into<ElementId>, label: impl Into<SharedString>, size: f32) -> PopupMenuItem {
    let (id, label) = (id.into(), label.into());
    PopupMenuItem::element(move |_, _| {
        div()
            .id(id.clone())
            .aria_label(label.clone())
            .text_size(ui(size))
            .child(label.clone())
            .test_support()
    })
}

/// Right-click on one or more selected items.
pub fn item_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let finder = view.read(cx);
    let entries = finder.selected_entries();
    let Some(first) = entries.first().cloned() else {
        return menu;
    };
    if finder.in_trash() {
        return trash_item_menu(finder, &entries, menu);
    }
    let single = entries.len() == 1;
    let searching = finder.is_searching() || finder.current_folder().is_none();
    let folder_for_nu: Option<PathBuf> = if first.is_dir {
        Some(first.path.clone())
    } else {
        first.parent().map(Path::to_path_buf)
    };
    let is_favorite = finder.config.favorites.contains(&first.path);
    let picker = finder.picker.is_some();

    let enclosing = first.path.clone();
    let nu_dir = folder_for_nu.clone();
    let favorite = first.path.clone();
    let opens_files = !picker && entries.iter().all(|e| !e.is_dir);

    let menu = menu
        .when(picker, |menu| {
            menu.item(
                PopupMenuItem::new(if single { "Choose" } else { "Choose Items" })
                    .action(Box::new(Choose)),
            )
            .separator()
        })
        .item(
            PopupMenuItem::new("Open")
                .icon(IconName::FolderOpen)
                .action(Box::new(OpenSelected)),
        );
    let menu = if opens_files {
        super::open_with::open_with_submenus(view, first.path.clone(), single, menu, window, cx)
    } else {
        menu
    };
    let menu = menu
        .item(
            PopupMenuItem::new("Quick Look")
                .icon(IconName::Eye)
                .action(Box::new(QuickLook)),
        )
        .item(
            PopupMenuItem::new("Get Info")
                .icon(IconName::Info)
                .action(Box::new(GetInfo)),
        )
        .when(entries.iter().any(|e| !e.is_dir), |menu| {
            menu.item(
                PopupMenuItem::new("Open in Editor")
                    .icon(Lucide::SquarePen)
                    .action(Box::new(OpenInEditor)),
            )
        })
        .when(searching && single, |menu| {
            menu.item(
                PopupMenuItem::new("Show in Enclosing Folder")
                    .icon(IconName::Folder)
                    .on_click(window.listener_for(
                        view,
                        move |this: &mut Finder, _: &ClickEvent, window, cx| {
                            this.reveal_in_place(&enclosing, window, cx)
                        },
                    )),
            )
        })
        .item(
            PopupMenuItem::new(reveal_label())
                .icon(IconName::ExternalLink)
                .action(Box::new(RevealInFileManager)),
        )
        .separator()
        .item(
            PopupMenuItem::new(if single {
                format!("Copy “{}”", first.name)
            } else {
                format!("Copy {} Items", entries.len())
            })
            .icon(IconName::Copy)
            .action(Box::new(input::Copy)),
        )
        .item(
            PopupMenuItem::new(if single { "Copy Path" } else { "Copy Paths" })
                .icon(Lucide::Link)
                .action(Box::new(CopyPath)),
        )
        .item(
            PopupMenuItem::new("Copy as Nushell List")
                .icon(Lucide::ClipboardCopy)
                .action(Box::new(CopyAsNuon)),
        )
        .when_some(nu_dir, |menu, dir| {
            menu.item(
                PopupMenuItem::new("Open Nushell Here")
                    .icon(IconName::SquareTerminal)
                    .on_click(window.listener_for(
                        view,
                        move |this: &mut Finder, _: &ClickEvent, window, cx| {
                            this.open_terminal_at(&dir, window, cx)
                        },
                    )),
            )
        })
        .item(
            PopupMenuItem::new("Run Nushell Pipeline…")
                .icon(Lucide::Terminal)
                .on_click(window.listener_for(
                    view,
                    |this: &mut Finder, _: &ClickEvent, window, cx| {
                        if !this.nu_open {
                            this.toggle_nu_panel(&ToggleNuPanel, window, cx);
                        }
                    },
                )),
        );
    let menu = actions_submenu(view, menu, window, cx);
    menu.separator()
        .item(
            PopupMenuItem::new(if single {
                "Rename…".to_string()
            } else {
                format!("Rename {} Items…", entries.len())
            })
            .icon(Lucide::Pencil)
            .action(Box::new(Rename)),
        )
        .when(single && first.is_navigable() && !is_favorite, |menu| {
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
                .action(Box::new(MoveToTrash)),
        )
        .min_w(ui(230.))
}

/// Right-click on items in the Trash.
fn trash_item_menu(
    finder: &Finder,
    entries: &[fffinder_core::FileEntry],
    menu: PopupMenu,
) -> PopupMenu {
    let single = entries.len() == 1;
    menu.item(
        PopupMenuItem::new("Put Back")
            .icon(Lucide::Undo2)
            .disabled(!super::trash::can_put_back(&finder.trashed, entries))
            .action(Box::new(PutBack)),
    )
    .item(
        PopupMenuItem::new("Quick Look")
            .icon(IconName::Eye)
            .action(Box::new(QuickLook)),
    )
    .item(
        PopupMenuItem::new(reveal_label())
            .icon(IconName::ExternalLink)
            .action(Box::new(RevealInFileManager)),
    )
    .item(
        PopupMenuItem::new(if single { "Copy Path" } else { "Copy Paths" })
            .icon(Lucide::Link)
            .action(Box::new(CopyPath)),
    )
    .separator()
    .item(
        PopupMenuItem::new("Delete Immediately…")
            .icon(Lucide::Trash)
            .action(Box::new(DeleteImmediately)),
    )
    .min_w(ui(220.))
}

/// Right-click on empty space in a view.
pub fn background_menu(
    view: &Entity<Finder>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    // Menus don't stop propagation, so a right-click on an item reaches this
    // menu too. An empty menu is never shown.
    let position = window.mouse_position();
    if view.update(cx, |f, _| f.take_item_menu_click(position)) {
        return menu;
    }
    let finder = view.read(cx);
    if finder.in_trash() {
        return menu
            .item(
                PopupMenuItem::new("Empty Trash…")
                    .icon(Lucide::Trash)
                    .disabled(finder.rows_for_test().is_empty())
                    .action(Box::new(EmptyTrash)),
            )
            .min_w(ui(220.));
    }
    let folder = finder.current_folder().map(Path::to_path_buf);
    let hidden = finder.config.show_hidden;
    let is_favorite = folder
        .as_ref()
        .is_some_and(|f| finder.config.favorites.contains(f));
    let nu_folder = folder.clone();
    // Pasting goes into the folder shown, so not into search results.
    let pasteable = match finder.is_searching() || folder.is_none() {
        true => 0,
        false => crate::system::clipboard_files(cx).len(),
    };
    let items = |n: usize| match n {
        1 => "Item".to_string(),
        n => format!("{n} Items"),
    };
    menu.when(folder.is_some(), |menu| {
        menu.item(
            PopupMenuItem::new("New Folder")
                .icon(Lucide::FolderPlus)
                .action(Box::new(NewFolder)),
        )
    })
    .when(pasteable > 0, |menu| {
        menu.item(
            PopupMenuItem::new(format!("Paste {}", items(pasteable)))
                .icon(Lucide::ClipboardPaste)
                .action(Box::new(input::Paste)),
        )
        .item(
            PopupMenuItem::new(format!("Move {} Here", items(pasteable)))
                .icon(Lucide::FolderInput)
                .action(Box::new(MoveItemsHere)),
        )
    })
    .when_some(nu_folder, |menu, dir| {
        menu.item(
            PopupMenuItem::new("Open Nushell Here")
                .icon(IconName::SquareTerminal)
                .on_click(window.listener_for(
                    view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.open_terminal_at(&dir, window, cx)
                    },
                )),
        )
    })
    .item(
        PopupMenuItem::new("Run Nushell Pipeline…")
            .icon(Lucide::Terminal)
            .action(Box::new(ToggleNuPanel)),
    )
    .when(folder.is_some(), |menu| {
        menu.item(
            PopupMenuItem::new("Get Info")
                .icon(IconName::Info)
                .action(Box::new(GetInfo)),
        )
    })
    .separator()
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
    .item(
        PopupMenuItem::new("Show Hidden Files")
            .checked(hidden)
            .action(Box::new(ToggleHidden)),
    )
    .item(
        PopupMenuItem::new("Refresh")
            .icon(IconName::RefreshCw)
            .action(Box::new(Refresh)),
    )
    .min_w(ui(220.))
}
