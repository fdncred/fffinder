//! The Open With and Always Open With submenus and what they do.

use super::Finder;
use crate::apps::{self, OpenApp};
use crate::theme::ui;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::*;
use std::path::PathBuf;

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

/// The "Open With" submenu for the selected files, and for one file the
/// "Always Open With" submenu. The apps are looked up when it opens.
pub(super) fn open_with_submenus(
    view: &Entity<Finder>,
    file: PathBuf,
    single: bool,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let open_view = view.clone();
    let open_file = file.clone();
    let menu = menu.submenu("Open With", window, cx, move |mut menu, window, cx| {
        let found = apps::apps_for(&open_file, cx);
        for app in &found {
            let path = app.path.clone();
            menu = menu.item(
                PopupMenuItem::new(app_label(app)).on_click(window.listener_for(
                    &open_view,
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
                        &open_view,
                        |this: &mut Finder, _: &ClickEvent, window, cx| {
                            this.open_with_other(window, cx)
                        },
                    )),
            )
            .min_w(ui(200.))
    });
    if !single || apps::SYSTEM_CHOOSER {
        return menu;
    }
    let view = view.clone();
    menu.submenu(
        "Always Open With",
        window,
        cx,
        move |mut menu, window, cx| {
            for app in apps::apps_for(&file, cx) {
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
        },
    )
}
