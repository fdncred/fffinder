//! Commands that make new items: Duplicate, New File, Make Alias, Make
//! Symbolic Link, Compress, and Extract, which is also what opening a zip
//! or tar archive does.

use super::Finder;
use super::transfer::{COMPRESS, EXTRACT};
use crate::actions::*;
use crate::theme::ui;
use crate::undo::{self, Change};
use fffinder_core::archive;
use fffinder_core::entry::display_name;
use fffinder_core::ops::{Mode, free_name, split_extension};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

impl Finder {
    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    /// Cmd+D: copy each selected item beside itself, as "name copy".
    pub(super) fn duplicate(&mut self, _: &Duplicate, window: &mut Window, cx: &mut Context<Self>) {
        let mut by_folder: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for path in self.selected_paths() {
            if let Some(dir) = path.parent() {
                by_folder.entry(dir.to_path_buf()).or_default().push(path);
            }
        }
        for (dir, sources) in by_folder {
            self.start_transfer(sources, dir, Mode::Copy, window, cx);
        }
    }

    /// Ask for a name, then make an empty file with it in the folder shown.
    pub(super) fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.drop_folder() else {
            return;
        };
        self.focus_before_dialog(window, cx);
        let name = display_name(&free_name(&dir, |n| match n {
            1 => "untitled.txt".to_string(),
            n => format!("untitled {n}.txt"),
        }));
        let stem_len = split_extension(&name, false).0.len();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let view = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.set_selected_range(0..stem_len, cx);
                });
            }
            let (ok_input, button_input) = (input.clone(), input.clone());
            let (ok_dir, button_dir) = (dir.clone(), dir.clone());
            let ok_view = view.clone();
            dialog
                .title("New File")
                .w(ui(440.))
                .child(Input::new(&input).id("new-file-input"))
                .on_ok(move |_, window, cx| {
                    ok_view.update(cx, |this, cx| {
                        this.submit_new_file(&ok_dir, &ok_input, window, cx)
                    })
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("new-file-cancel").label("Cancel")),
                        )
                        .child(
                            Button::new("new-file-create")
                                .primary()
                                .label("Create")
                                .on_click(window.listener_for(
                                    &view,
                                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                        if this.submit_new_file(
                                            &button_dir,
                                            &button_input,
                                            window,
                                            cx,
                                        ) {
                                            window.close_dialog(cx);
                                            this.focus_files(window, cx);
                                        }
                                    },
                                )),
                        ),
                )
        });
    }

    /// Make the file typed in New File. Returns whether the dialog closes.
    fn submit_new_file(
        &mut self,
        dir: &Path,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.wait_for_undo(window, cx) {
            return false;
        }
        let name = input.read(cx).value().trim().to_string();
        if let Some(problem) = crate::rename::name_problem(&name) {
            window.push_notification((NotificationType::Warning, problem), cx);
            return false;
        }
        let path = dir.join(&name);
        match std::fs::File::create_new(&path) {
            Ok(_) => {
                undo::record("New File", Change::Created(vec![path.clone()]), cx);
                self.load_listing(vec![path], true, cx);
                true
            }
            Err(err) => {
                let message = if err.kind() == std::io::ErrorKind::AlreadyExists {
                    format!("“{name}” already exists")
                } else {
                    format!("Couldn’t make “{name}”: {err}")
                };
                window.push_notification((NotificationType::Warning, message), cx);
                false
            }
        }
    }

    /// A Finder alias (macOS) or a shortcut (Windows) to each selected item,
    /// beside it; on Linux, a symbolic link.
    pub(super) fn make_alias(
        &mut self,
        _: &MakeAlias,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.make_links(true, window, cx);
    }

    /// A symbolic link to each selected item, beside it.
    pub(super) fn make_symlink(
        &mut self,
        _: &MakeSymlink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.make_links(false, window, cx);
    }

    fn make_links(&mut self, alias: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.wait_for_undo(window, cx) {
            return;
        }
        let mut made = Vec::new();
        for target in self.selected_paths() {
            let (Some(dir), name) = (target.parent(), display_name(&target)) else {
                continue;
            };
            let what = if alias { "alias" } else { "link" };
            let link = match alias && cfg!(windows) {
                // Explorer's names: "name - Shortcut.lnk", "name - Shortcut (2).lnk".
                true => free_name(dir, |n| match n {
                    1 => format!("{name} - Shortcut.lnk"),
                    n => format!("{name} - Shortcut ({n}).lnk"),
                }),
                false => free_name(dir, |n| match n {
                    1 => format!("{name} {what}"),
                    n => format!("{name} {what} {n}"),
                }),
            };
            match make_link(&target, &link, alias) {
                Ok(()) => made.push(link),
                Err(err) => window.push_notification(
                    (
                        NotificationType::Error,
                        format!("Couldn’t make a {what} to “{name}”: {err}"),
                    ),
                    cx,
                ),
            }
        }
        let name = match alias {
            true => crate::actions::alias_label().unwrap_or("Make Symbolic Link"),
            false => "Make Symbolic Link",
        };
        undo::record(name, Change::Created(made.clone()), cx);
        self.files_changed(made, cx);
    }

    /// Put the selected items in a zip beside them.
    pub(super) fn compress(&mut self, _: &Compress, window: &mut Window, cx: &mut Context<Self>) {
        let sources = self.selected_paths();
        let Some(zip) = archive::zip_path(&sources) else {
            return;
        };
        let count = sources.len();
        self.run_job(
            COMPRESS,
            count,
            move |progress| {
                archive::compress(&sources, &zip, progress)
                    .map(|()| vec![zip])
                    .map_err(|e| e.to_string())
            },
            window,
            cx,
        );
    }

    /// Unpack the selected archives beside themselves.
    pub(super) fn extract(&mut self, _: &Extract, window: &mut Window, cx: &mut Context<Self>) {
        let archives = self
            .selected_paths()
            .into_iter()
            .filter(|p| p.is_file() && archive::format(p).is_some())
            .collect();
        self.extract_archives(archives, window, cx);
    }

    pub(super) fn extract_archives(
        &mut self,
        archives: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if archives.is_empty() {
            return;
        }
        let count = archives.len();
        self.run_job(
            EXTRACT,
            count,
            move |progress| {
                archives
                    .iter()
                    .map(|a| archive::extract(a, progress))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())
            },
            window,
            cx,
        );
    }
}

/// Make a Finder alias (macOS), a shortcut (Windows), or a symbolic link at
/// `link` to `target`,
/// which is in the same folder, so the link keeps working if the folder
/// moves.
fn make_link(target: &Path, link: &Path, alias: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if alias {
        return crate::macos::make_alias(target, link);
    }
    #[cfg(windows)]
    if alias {
        return crate::win::make_shortcut(target, link);
    }
    let _ = alias;
    let name = target.file_name().map(Path::new).unwrap_or(target);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(name, link);
    #[cfg(windows)]
    let made = if target.is_dir() {
        std::os::windows::fs::symlink_dir(name, link)
    } else {
        std::os::windows::fs::symlink_file(name, link)
    };
    made.map_err(|e| e.to_string())
}

/// Whether an item is an archive that Extract (and opening it) unpacks.
pub(super) fn is_archive(path: &Path) -> bool {
    path.is_file() && archive::format(path).is_some()
}
