//! The Open With and Always Open With submenus and what they do.

use super::Finder;
use crate::apps::{self, OpenApp};
use crate::theme::ui;
use fffinder_core::FileEntry;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

impl Finder {
    /// The selected files (not folders), for Open With.
    fn files_to_open(&self) -> Vec<PathBuf> {
        self.selected_entries()
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path)
            .collect()
    }

    /// Open the selected files with `app`.
    pub(super) fn open_with_app(
        &mut self,
        app: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = self.files_to_open();
        if files.is_empty() {
            return;
        }
        let task = apps::open_with(&app, &files, cx);
        self.report_app_error(task, "Couldn’t open", window, cx);
        let engine = self.engine.clone();
        cx.background_spawn(async move {
            for file in files {
                engine.track_open(&file, None);
            }
        })
        .detach();
    }

    /// Make `app` the default for the file's type, then open the file.
    pub(super) fn always_open_with(
        &mut self,
        app: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.files_to_open().into_iter().next() else {
            return;
        };
        let task = apps::make_default(&app, &file, cx);
        self.report_app_error(task, "Couldn’t change the default app", window, cx);
        self.open_with_app(app, window, cx);
    }

    /// "Other…": pick an app with the file chooser, or on Windows use the
    /// system's own Open with dialog.
    pub(super) fn open_with_other(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if apps::SYSTEM_CHOOSER {
            if let Some(file) = self.files_to_open().into_iter().next() {
                let task = apps::system_chooser(&file, cx);
                self.report_app_error(task, "Couldn’t open", window, cx);
            }
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(app) = paths.pop()
            {
                this.update_in(cx, |this, window, cx| this.open_with_app(app, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn report_app_error(
        &self,
        task: Task<std::io::Result<()>>,
        what: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |_, cx| {
            if let Err(err) = task.await {
                cx.update(|window, cx| {
                    window
                        .push_notification((NotificationType::Error, format!("{what}: {err}")), cx)
                })
                .ok();
            }
        })
        .detach();
    }
}

fn app_label(app: &OpenApp) -> String {
    if app.is_default {
        format!("{} (default)", app.name)
    } else {
        app.name.clone()
    }
}

/// Builds a submenu's items from the apps found for a file.
type AppItems =
    Rc<dyn Fn(PopupMenu, &[OpenApp], &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// Fill a submenu with `items` for the apps that open `file`, once they're
/// known: at once when they already are, or after a lookup off the UI
/// thread, with a placeholder until then.
fn with_apps(
    menu: PopupMenu,
    file: &Path,
    items: AppItems,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    if let Some(found) = apps::known_apps_for(file, cx) {
        return items(menu, &found, window, cx);
    }
    let lookup = apps::apps_for(file, cx);
    cx.spawn_in(window, async move |menu, cx| {
        let found = lookup.await;
        menu.update_in(cx, |menu, window, cx| {
            menu.rebuild(window, cx, |menu, window, cx| {
                items(menu, &found, window, cx)
            })
        })
        .ok();
    })
    .detach();
    menu.item(PopupMenuItem::new("Looking for Apps…").disabled(true))
        .min_w(ui(200.))
}

/// The Open With items: each app, then "Other…".
fn open_with_items(view: Entity<Finder>) -> AppItems {
    Rc::new(move |mut menu, found, window, _| {
        for app in found {
            let path = app.path.clone();
            menu = menu.item(
                PopupMenuItem::new(app_label(app)).on_click(window.listener_for(
                    &view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        this.open_with_app(path.clone(), window, cx)
                    },
                )),
            );
        }
        if found.is_empty() && !apps::SYSTEM_CHOOSER {
            menu = menu.item(PopupMenuItem::new("No Apps Found").disabled(true));
        }
        menu.separator()
            .item(
                PopupMenuItem::new("Other…")
                    .icon(Lucide::AppWindow)
                    .on_click(window.listener_for(
                        &view,
                        |this: &mut Finder, _: &ClickEvent, window, cx| {
                            this.open_with_other(window, cx)
                        },
                    )),
            )
            .min_w(ui(200.))
    })
}

/// The Always Open With items: each app, the default checked.
fn always_open_with_items(view: Entity<Finder>) -> AppItems {
    Rc::new(move |mut menu, found, window, _| {
        for app in found {
            let path = app.path.clone();
            menu = menu.item(
                PopupMenuItem::new(app.name.clone())
                    .checked(app.is_default)
                    .on_click(window.listener_for(
                        &view,
                        move |this: &mut Finder, _: &ClickEvent, window, cx| {
                            this.always_open_with(path.clone(), window, cx)
                        },
                    )),
            );
        }
        menu.min_w(ui(200.))
    })
}

/// The "Open With" submenu for the selected files in `entries`, and for one
/// file the "Always Open With" submenu. The apps are looked up when it opens.
pub(super) fn open_with_submenus(
    view: &Entity<Finder>,
    entries: &[FileEntry],
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(file) = entries.first().map(|e| e.path.clone()) else {
        return menu;
    };
    let (open_file, open_items) = (file.clone(), open_with_items(view.clone()));
    let menu = menu.submenu("Open With", window, cx, move |menu, window, cx| {
        with_apps(menu, &open_file, open_items.clone(), window, cx)
    });
    if entries.len() > 1 || apps::SYSTEM_CHOOSER {
        return menu;
    }
    let always_items = always_open_with_items(view.clone());
    menu.submenu("Always Open With", window, cx, move |menu, window, cx| {
        with_apps(menu, &file, always_items.clone(), window, cx)
    })
}
