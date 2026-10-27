//! Dialogs: settings, Go to Folder, rename, Move to Trash, and search syntax.

use super::{AppState, Finder, group_digits};
use crate::actions::*;
use crate::model::Location;
use crate::theme::ui;
use fffinder_core::terminals::{self, CUSTOM, Platform, Terminal};
use fffinder_core::{Appearance, FileEntry, SearchCase, config};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// Expand `~` and make relative paths relative to `base`.
fn resolve_path(text: &str, base: Option<&Path>) -> PathBuf {
    let text = text.trim();
    let expanded = if text == "~" {
        dirs::home_dir().unwrap_or_default()
    } else if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        PathBuf::from(text)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base.map(|b| b.join(&expanded)).unwrap_or(expanded)
    }
}

impl Finder {
    /// A dialog returns focus to whatever had it when it opened. From a menu
    /// that is the menu's button, which leaves the keyboard dead in the file
    /// views, so focus the files first.
    pub(super) fn focus_before_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !window.has_focused_input(cx) {
            self.focus_files(window, cx);
        }
    }
}

/// fff-search's options: how folders are indexed, and how searches run.
fn search_engine_controls(
    finder: &Finder,
    view: &Entity<Finder>,
    muted: Hsla,
    window: &mut Window,
) -> impl IntoElement {
    let search = finder.config.search.clone();
    type Field = fn(&mut fffinder_core::SearchSettings) -> &mut bool;
    let switches: [(&'static str, &'static str, bool, Field); 5] = [
        (
            "follow-symlinks",
            "Follow symbolic links",
            search.follow_symlinks,
            |s| &mut s.follow_symlinks,
        ),
        (
            "watch-changes",
            "Watch folders for changes",
            search.watch,
            |s| &mut s.watch,
        ),
        (
            "git-recency",
            "Rank recent git changes higher",
            search.git_recency,
            |s| &mut s.git_recency,
        ),
        (
            "content-index",
            "Index file contents",
            search.content_index,
            |s| &mut s.content_index,
        ),
        (
            "preload-files",
            "Keep often-used files in memory",
            search.preload_files,
            |s| &mut s.preload_files,
        ),
    ];
    let case = match search.case {
        SearchCase::Smart => 0,
        SearchCase::Sensitive => 1,
        SearchCase::Insensitive => 2,
    };
    let pick = |id: &'static str,
                choices: &'static [u64],
                current: u64,
                label: fn(u64) -> String,
                set: fn(&mut Finder, u64, &mut Context<Finder>)| {
        let view = view.clone();
        choice_stepper(id, choices, current, label, move |value, window, cx| {
            view.update(cx, |this, cx| set(this, value, cx));
            window.refresh();
        })
    };
    v_flex()
        .gap(ui(8.))
        .child(h_flex().flex_wrap().gap_x(ui(16.)).gap_y(ui(6.)).children(
            switches.into_iter().map(|(id, label, checked, field)| {
                Switch::new(id)
                    .small()
                    .label(label)
                    .checked(checked)
                    .on_change(window.listener_for(
                        view,
                        move |this: &mut Finder, on: &bool, _, cx| {
                            let on = *on;
                            this.update_search_settings(move |s| *field(s) = on, cx)
                        },
                    ))
            }),
        ))
        .child(setting_row(
            "Letter case in content search",
            "Smart ignores case unless the text has a capital letter",
            ButtonGroup::new("search-case")
                .small()
                .outline()
                .child(Button::new("case-smart").label("Smart").selected(case == 0))
                .child(
                    Button::new("case-sensitive")
                        .label("Match Case")
                        .selected(case == 1),
                )
                .child(
                    Button::new("case-insensitive")
                        .label("Ignore Case")
                        .selected(case == 2),
                )
                .on_click(window.listener_for(
                    view,
                    |this: &mut Finder, ixs: &Vec<usize>, _, cx| {
                        let case = match ixs.first() {
                            Some(1) => SearchCase::Sensitive,
                            Some(2) => SearchCase::Insensitive,
                            _ => SearchCase::Smart,
                        };
                        this.update_search_settings(|s| s.case = case, cx)
                    },
                )),
            muted,
        ))
        .child(setting_row(
            "Results shown",
            "The most items a search lists",
            pick(
                "result-limit",
                &[100, 200, 500, 1000, 2000, 5000],
                finder.config.result_limit as u64,
                |n| n.to_string(),
                |this, n, cx| this.set_result_limit(n as usize, cx),
            ),
            muted,
        ))
        .child(setting_row(
            "Content search skips files over",
            "Bigger files aren't searched for text",
            pick(
                "max-file-size",
                &[1, 2, 5, 10, 20, 50, 100, 250, 500, 1000],
                search.max_file_size_mb,
                |n| format!("{n} MB"),
                |this, n, cx| this.update_search_settings(|s| s.max_file_size_mb = n, cx),
            ),
            muted,
        ))
        .child(setting_row(
            "Content search time limit",
            "Show what was found so far after this long",
            pick(
                "time-limit",
                &[0, 1, 2, 5, 10, 30, 60],
                search.time_limit_secs,
                |n| {
                    if n == 0 {
                        "None".into()
                    } else {
                        format!("{n} s")
                    }
                },
                |this, n, cx| this.update_search_settings(|s| s.time_limit_secs = n, cx),
            ),
            muted,
        ))
        .child(setting_row(
            "Search threads",
            "For name search; Auto uses one per processor core",
            pick(
                "threads",
                &[0, 1, 2, 4, 8, 16, 32],
                search.threads as u64,
                |n| if n == 0 { "Auto".into() } else { n.to_string() },
                |this, n, cx| this.update_search_settings(|s| s.threads = n as usize, cx),
            ),
            muted,
        ))
}

/// "−" and "+" buttons that step through `choices`, with the current one
/// between them (`{id}-less`, `{id}-value`, `{id}-more`).
fn choice_stepper(
    id: &str,
    choices: &'static [u64],
    current: u64,
    label: fn(u64) -> String,
    pick: impl Fn(u64, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    // A hand-edited value between choices steps from the next one up.
    let at = choices
        .iter()
        .position(|&c| c >= current)
        .unwrap_or(choices.len() - 1);
    let pick = std::rc::Rc::new(pick);
    let step = |name: &str, glyph: &'static str, to: Option<usize>| {
        let pick = pick.clone();
        Button::new(SharedString::from(format!("{id}-{name}")))
            .ghost()
            .small()
            .label(glyph)
            .disabled(to.is_none())
            .on_click(move |_, window, cx| {
                if let Some(ix) = to {
                    pick(choices[ix], window, cx)
                }
            })
    };
    h_flex()
        .gap(ui(2.))
        .child(step("less", "−", at.checked_sub(1)))
        .child(
            div().min_w(ui(64.)).flex().justify_center().child(
                Button::new(SharedString::from(format!("{id}-value")))
                    .ghost()
                    .small()
                    .label(label(current)),
            ),
        )
        .child(step(
            "more",
            "+",
            (at + 1 < choices.len()).then_some(at + 1),
        ))
}

/// A setting's name and what it affects, with its control beside them.
fn setting_row(
    title: &'static str,
    detail: &'static str,
    control: impl IntoElement,
    muted: Hsla,
) -> impl IntoElement {
    h_flex()
        .gap(ui(12.))
        .child(
            v_flex()
                .flex_1()
                .child(div().text_size(ui(12.)).child(title))
                .child(div().text_size(ui(11.)).text_color(muted).child(detail)),
        )
        .child(control)
}

/// Settings › Text size, as a percentage.
fn ui_scale_stepper(finder: &Finder, view: &Entity<Finder>) -> impl IntoElement {
    let (min, max) = config::UI_SCALE_RANGE;
    let scale = finder.config.ui_scale();
    let view = view.clone();
    super::preview::size_stepper(
        "ui-text",
        format!("{:.0}%", scale * 100.),
        [scale <= min, scale >= max, scale == 1.0],
        None,
        move |steps, window, cx| {
            view.update(cx, |this, cx| match steps {
                Some(steps) => this.step_ui_scale(steps, cx),
                None => this.set_ui_scale(None, cx),
            });
            window.refresh();
        },
    )
}

/// Settings › Nushell text size, in points.
fn nu_text_stepper(finder: &Finder, view: &Entity<Finder>) -> impl IntoElement {
    let (min, max) = config::FONT_SIZE_RANGE;
    let size = finder.config.nu_font_size();
    let view = view.clone();
    super::preview::size_stepper(
        "nu-text",
        format!("{size} pt"),
        [
            size <= min,
            size >= max,
            size == config::DEFAULT_NU_FONT_SIZE,
        ],
        None,
        move |steps, window, cx| {
            view.update(cx, |this, cx| match steps {
                Some(steps) => this.step_nu_font_size(steps, cx),
                None => this.set_nu_font_size(None, cx),
            });
            window.refresh();
        },
    )
}

fn section(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .pt(ui(6.))
        .text_size(ui(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

impl Finder {
    pub(super) fn show_settings(
        &mut self,
        _: &ShowSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        // Pick up terminals installed since the last look.
        self.detect_terminals(cx);
        let terminal = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(custom_terminal_hint())
                .default_value(self.terminal_setting().command.unwrap_or_default())
        });
        self.dialog_sub = Some(
            cx.subscribe(&terminal, |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let value = input.read(cx).value().trim().to_string();
                    this.set_terminal_command(value, cx);
                }
            }),
        );
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let finder = view.read(cx);
            let theme = cx.theme();
            let muted = theme.muted_foreground;
            let border = theme.border;
            let roots = finder.config.search_roots.clone();
            let status = finder.index_status.clone();
            let appearance = finder.config.appearance;
            let hidden = finder.config.show_hidden;
            let nu = cx.global::<AppState>().nu.clone();

            let mut root_list = v_flex()
                .border_1()
                .border_color(border)
                .rounded(ui(8.))
                .overflow_hidden();
            for (ix, root) in roots.iter().enumerate() {
                let files = status.iter().find(|s| &s.path == root).map(|s| {
                    if s.scanning {
                        format!("indexing… {}", group_digits(s.indexed_files))
                    } else {
                        format!("{} files", group_digits(s.indexed_files))
                    }
                });
                let remove = root.clone();
                root_list = root_list.child(
                    h_flex()
                        .px(ui(10.))
                        .py(ui(6.))
                        .gap(ui(8.))
                        .when(ix > 0, |this| this.border_t_1().border_color(border))
                        .child(Icon::new(IconName::Folder).with_size(ui(15.)).text_color(theme.primary))
                        .child(div().flex_1().truncate().text_size(ui(12.)).child(super::tildify(root)))
                        .when_some(files, |this, f| this.child(div().text_size(ui(11.)).text_color(muted).child(f)))
                        .child(
                            Button::new(("remove-root", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Stop indexing this folder")
                                .on_click(window.listener_for(&view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                                    let roots: Vec<PathBuf> =
                                        this.config.search_roots.iter().filter(|r| **r != remove).cloned().collect();
                                    this.set_search_roots(roots, cx);
                                })),
                        ),
                );
            }
            if roots.is_empty() {
                root_list = root_list.child(
                    div()
                        .p(ui(10.))
                        .text_size(ui(12.))
                        .text_color(muted)
                        .child("No folders are indexed. “This Computer” searches return nothing."),
                );
            }

            let disk_root = if cfg!(windows) { PathBuf::from("C:\\") } else { PathBuf::from("/") };
            let has_disk = roots.contains(&disk_root);

            dialog
                .title("Settings")
                .w(ui(600.))
                .child(
                    v_flex()
                        .gap(ui(10.))
                        .child(section("SEARCH LOCATIONS", cx))
                        .child(
                            div()
                                .text_size(ui(12.))
                                .text_color(muted)
                                .child("“This Computer” searches these folders. Indexes stay live while fffinder runs."),
                        )
                        .child(root_list)
                        .child(
                            h_flex()
                                .gap(ui(8.))
                                .child(Button::new("add-root").small().label("Add Folder…").on_click(
                                    window.listener_for(&view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                                        this.prompt_add_root(cx)
                                    }),
                                ))
                                .when(!has_disk, |this| {
                                    this.child(
                                        Button::new("add-disk")
                                            .small()
                                            .label("Index Entire Disk")
                                            .tooltip("Search everything; the first scan takes longer and uses more memory")
                                            .on_click(window.listener_for(&view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                                                this.set_search_roots(vec![disk_root.clone()], cx)
                                            })),
                                    )
                                })
                                .child(Button::new("reset-roots").small().ghost().label("Reset").on_click(
                                    window.listener_for(&view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                                        this.set_search_roots(config::default_search_roots(), cx)
                                    }),
                                ))
                                .child(div().flex_1())
                                .child(
                                    Button::new("rescan")
                                        .small()
                                        .icon(IconName::RefreshCw)
                                        .label("Rescan")
                                        .on_click(window.listener_for(&view, |this: &mut Finder, _: &ClickEvent, window, cx| {
                                            this.rescan(&Rescan, window, cx)
                                        })),
                                ),
                        )
                        .child(section("SEARCH ENGINE", cx))
                        .child(
                            div()
                                .text_size(ui(12.))
                                .text_color(muted)
                                .child("fff-search’s options. Changing a switch rebuilds the indexes."),
                        )
                        .child(search_engine_controls(finder, &view, muted, window))
                        .child(section("APPEARANCE", cx))
                        .child(
                            h_flex()
                                .gap(ui(12.))
                                .child(
                                    ButtonGroup::new("appearance")
                                        .small()
                                        .outline()
                                        .child(Button::new("ap-system").label("System").selected(appearance == Appearance::System))
                                        .child(Button::new("ap-light").label("Light").selected(appearance == Appearance::Light))
                                        .child(Button::new("ap-dark").label("Dark").selected(appearance == Appearance::Dark))
                                        .on_click(window.listener_for(&view, |this: &mut Finder, ixs: &Vec<usize>, window, cx| {
                                            let appearance = match ixs.first() {
                                                Some(1) => Appearance::Light,
                                                Some(2) => Appearance::Dark,
                                                _ => Appearance::System,
                                            };
                                            this.set_appearance(appearance, window, cx);
                                        })),
                                )
                                .child(
                                    Switch::new("show-hidden")
                                        .small()
                                        .label("Show hidden files")
                                        .checked(hidden)
                                        // The dialog sits outside the Finder view, so actions
                                        // dispatched from here never reach it; call it directly.
                                        .on_change(window.listener_for(&view, |this: &mut Finder, show: &bool, _, cx| {
                                            this.set_show_hidden(*show, cx)
                                        })),
                                ),
                        )
                        .child(setting_row(
                            "Text size",
                            "Every text, control, and dialog in the window",
                            ui_scale_stepper(finder, &view),
                            muted,
                        ))
                        .child(section("NUSHELL", cx))
                        .child(
                            div().text_size(ui(12.)).child(match &nu {
                                Some(path) => format!("Using {}", path.display()),
                                None => "nu was not found on PATH or in the usual install locations.".to_string(),
                            }),
                        )
                        .child(terminal_controls(finder, &view, &terminal, window, cx))
                        .child(
                            v_flex()
                                .gap(ui(4.))
                                .child(
                                    Switch::new("use-nu-env")
                                        .small()
                                        .label("Use Nushell’s file colors and editor")
                                        .checked(finder.config.use_nu_environment)
                                        .on_change(window.listener_for(
                                            &view,
                                            |this: &mut Finder, on: &bool, _, cx| {
                                                this.set_use_nu_environment(*on, cx)
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id("nu-env-summary")
                                        .text_size(ui(11.))
                                        .text_color(muted)
                                        .child(finder.nu_env_summary()),
                                ),
                        )
                        .child(setting_row(
                            "Nushell text size",
                            "The pipeline panel and the Nushell Actions menu",
                            nu_text_stepper(finder, &view),
                            muted,
                        ))
                        .child(
                            div()
                                .text_size(ui(11.))
                                .text_color(muted)
                                .child(format!("Settings file: {}", config::config_path().display())),
                        ),
                )
                .footer(
                    DialogFooter::new().child(DialogClose::new().child(Button::new("settings-done").primary().label("Done"))),
                )
        });
    }

    fn prompt_add_root(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Index".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| {
                    let mut roots = this.config.search_roots.clone();
                    roots.extend(paths);
                    roots.dedup();
                    this.set_search_roots(roots, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn go_to_folder(
        &mut self,
        _: &GoToFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("~/Documents, /usr/local, C:\\Users…")
                .default_value(
                    self.current_folder()
                        .map(super::tildify)
                        .unwrap_or_default(),
                )
        });
        let view = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            }
            let (ok_input, go_input) = (input.clone(), input.clone());
            let ok_view = view.clone();
            dialog
                .title("Go to Folder")
                .w(ui(480.))
                .child(Input::new(&input).id("goto-input").cleanable(true))
                // Return confirms the dialog; it closes only when the path exists.
                .on_ok(move |_, window, cx| {
                    ok_view.update(cx, |this, cx| this.submit_go_to(&ok_input, window, cx))
                })
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("goto-cancel").label("Cancel")))
                        .child(Button::new("goto-go").primary().label("Go").on_click(
                            window.listener_for(
                                &view,
                                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                    if this.submit_go_to(&go_input, window, cx) {
                                        window.close_dialog(cx);
                                        this.focus_files(window, cx);
                                    }
                                },
                            ),
                        )),
                )
        });
    }

    /// Open the folder (or reveal the file) typed in Go to Folder. Returns
    /// whether the dialog should close.
    fn submit_go_to(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = input.read(cx).value().to_string();
        let path = resolve_path(&text, self.current_folder());
        if path.is_dir() {
            self.navigate(Location::Folder(path), window, cx);
        } else if path.exists() {
            self.reveal_in_place(&path, window, cx);
        } else {
            window.push_notification(
                (
                    gpui_kit::component::notification::NotificationType::Warning,
                    format!("{} doesn’t exist", path.display()),
                ),
                cx,
            );
            return false;
        }
        true
    }

    pub(super) fn open_rename_dialog(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        let name = fffinder_core::entry::display_name(&path);
        let stem_len = if path.is_dir() {
            name.len()
        } else {
            name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len())
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
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
            let (ok_path, button_path) = (path.clone(), path.clone());
            let ok_view = view.clone();
            dialog
                .title("Rename")
                .w(ui(440.))
                .child(Input::new(&input).id("rename-input"))
                .on_ok(move |_, window, cx| {
                    ok_view.update(cx, |this, cx| {
                        this.submit_rename(&ok_path, &ok_input, window, cx)
                    })
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(Button::new("rename-cancel").label("Cancel")),
                        )
                        .child(Button::new("rename-ok").primary().label("Rename").on_click(
                            window.listener_for(
                                &view,
                                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                    if this.submit_rename(&button_path, &button_input, window, cx) {
                                        window.close_dialog(cx);
                                        this.focus_files(window, cx);
                                    }
                                },
                            ),
                        )),
                )
        });
    }

    /// Rename `path` to the typed name. Returns whether the dialog should close.
    fn submit_rename(
        &mut self,
        path: &Path,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let new_name = input.read(cx).value().trim().to_string();
        let mut notify = |kind, message: String| {
            window.push_notification((kind, message), cx);
        };
        use gpui_kit::component::notification::NotificationType::{Error, Warning};
        if new_name.is_empty() {
            return false;
        }
        if new_name.contains(['/', '\\']) {
            notify(Warning, "A name can’t contain “/” or “\\”".into());
            return false;
        }
        let target = path.with_file_name(&new_name);
        if target == path {
            return true;
        }
        if target.exists() {
            notify(Warning, format!("“{new_name}” already exists"));
            return false;
        }
        if let Err(err) = std::fs::rename(path, &target) {
            notify(Error, format!("Couldn’t rename: {err}"));
            return false;
        }
        crate::undo::record(
            "Rename",
            crate::undo::Change::Moved(vec![(path.to_path_buf(), target.clone())]),
            cx,
        );
        self.previews.remove(path);
        for row in self.results.iter_mut().filter(|r| r.entry.path == path) {
            if let Some(entry) = FileEntry::from_path(&target) {
                *row = super::Row::new(entry, Vec::new(), None);
            }
        }
        self.load_listing(vec![target], true, cx);
        true
    }

    /// Name and save the current search and filter as a Smart Folder.
    pub(super) fn save_smart_folder_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_searching() {
            return;
        }
        self.focus_before_dialog(window, cx);
        let query = self.query.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(query));
        let view = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            }
            let (ok_input, button_input) = (input.clone(), input.clone());
            let ok_view = view.clone();
            let summary = {
                let finder = view.read(cx);
                let mut parts = vec![format!("“{}”", finder.query)];
                if !finder.filter_text().is_empty() {
                    parts.push(format!("where {}", finder.filter_text()));
                }
                parts.join(" ")
            };
            dialog
                .title("Save Smart Folder")
                .w(ui(440.))
                .child(
                    v_flex()
                        .gap(ui(8.))
                        .child(Input::new(&input).id("smart-name"))
                        .child(
                            div()
                                .text_size(ui(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("Shows {summary}, kept up to date.")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    ok_view.update(cx, |this, cx| {
                        this.submit_smart_folder(&ok_input, window, cx)
                    })
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(Button::new("smart-cancel").label("Cancel")),
                        )
                        .child(Button::new("smart-save").primary().label("Save").on_click(
                            window.listener_for(
                                &view,
                                move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                    if this.submit_smart_folder(&button_input, window, cx) {
                                        window.close_dialog(cx);
                                        this.focus_files(window, cx);
                                    }
                                },
                            ),
                        )),
                )
        });
    }

    fn submit_smart_folder(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return false;
        }
        let Some(folder) = self.current_smart_folder(name.clone()) else {
            return true;
        };
        self.add_smart_folder(folder, cx);
        window.push_notification(format!("Saved “{name}” in the sidebar"), cx);
        true
    }

    pub(super) fn confirm_trash(
        &mut self,
        entries: Vec<FileEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        let count = entries.len();
        let title = if count == 1 {
            format!("Move “{}” to the Trash?", entries[0].name)
        } else {
            format!("Move {count} items to the Trash?")
        };
        let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let paths = paths.clone();
            let view = view.clone();
            alert
                .confirm()
                .title(title.clone())
                .description("You can restore items from the Trash.")
                .ok_text("Move to Trash")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| this.trash_now(paths.clone(), window, cx));
                    true
                })
        });
    }

    pub(super) fn search_help(
        &mut self,
        _: &SearchHelp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        const ROWS: [(&str, &str); 10] = [
            (
                "budget report",
                "Fuzzy words, typo tolerant; every word must match",
            ),
            ("*.pdf  *.md", "Extensions (several are ORed)"),
            (
                "src/**/*.rs",
                "Glob against the path inside the search location",
            ),
            ("/notes/ todo", "Only paths inside a folder named “notes”"),
            ("main src/", "Folder filter written at the end"),
            (
                "git:modified",
                "Git status: modified, untracked, staged, clean",
            ),
            (
                "!*.log  !test",
                "Exclude an extension, glob, folder, or text",
            ),
            ("main.rs:42", "Line suffix, ignored for matching"),
            ("Kind menu", "Restrict to folders, documents, images, …"),
            ("Contents", "Search inside files instead of names"),
        ];
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            dialog
                .title("Search Syntax")
                .w(ui(560.))
                .child(
                    v_flex()
                        .gap(ui(2.))
                        .text_size(ui(12.))
                        .children(ROWS.iter().map(|(syntax, meaning)| {
                            h_flex()
                                .gap(ui(14.))
                                .py(ui(3.))
                                .child(
                                    div()
                                        .w(ui(150.))
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.primary)
                                        .child(*syntax),
                                )
                                .child(div().flex_1().child(*meaning))
                        }))
                        .child(
                            div()
                                .pt(ui(10.))
                                .text_color(theme.muted_foreground)
                                .child("Searches run 50 ms after you stop typing; Return searches now and Down Arrow moves into the results. Hidden files are only indexed inside git repositories."),
                        ),
                )
                .footer(DialogFooter::new().child(DialogClose::new().child(Button::new("help-done").primary().label("Done"))))
        });
    }
}

fn custom_terminal_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "e.g. open -na WezTerm --args start --cwd {dir} -- {nu}"
    } else if cfg!(windows) {
        "e.g. wt.exe -d {dir} {nu}"
    } else {
        "e.g. foot --working-directory={dir} {nu}"
    }
}

/// The "Open Nushell Here" terminal: a menu of the terminals found, a button
/// to try it, and the command field when the choice is a custom command.
fn terminal_controls(
    finder: &Finder,
    view: &Entity<Finder>,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let setting = finder.terminal_setting();
    let found = finder.terminals();
    let env_terminal = std::env::var("TERMINAL").ok();
    let automatic = found.and_then(|f| terminals::automatic(f, env_terminal.as_deref()));
    let custom = setting.app.as_deref() == Some(CUSTOM);
    let chosen = setting.app.as_deref().filter(|id| *id != CUSTOM);
    let shown = match chosen {
        Some(id) => found.and_then(|f| f.iter().find(|t| t.id == id)),
        None => automatic,
    };
    let label: SharedString = if found.is_none() {
        "Looking for terminals…".into()
    } else if custom {
        "Custom Command".into()
    } else {
        match (chosen, shown) {
            (None, Some(t)) => format!("Automatic ({})", t.name).into(),
            (None, None) => "Automatic (none found)".into(),
            (Some(_), Some(t)) => t.name.clone().into(),
            (Some(id), None) => format!("{id} (not installed)").into(),
        }
    };
    let detail = if custom {
        "Runs the command below.".to_string()
    } else if let Some(t) = shown {
        t.location
            .as_deref()
            .map(super::tildify)
            .unwrap_or_else(|| "Opens nu in a new console window.".into())
    } else if chosen.is_some() {
        "Not installed here, so Automatic is used.".to_string()
    } else if found.is_some() {
        "No terminal was found; use a custom command.".to_string()
    } else {
        String::new()
    };
    let detail = format!("{detail} Saved for {}.", Platform::current().label());

    let menu_view = view.clone();
    let picker = Button::new("terminal-app")
        .small()
        .outline()
        .label(label)
        .dropdown_caret(true)
        .disabled(found.is_none())
        .dropdown_menu(move |menu, window, cx| {
            let finder = menu_view.read(cx);
            let current = finder.terminal_setting().app;
            let found = finder
                .terminals()
                .map(<[Terminal]>::to_vec)
                .unwrap_or_default();
            let env_terminal = std::env::var("TERMINAL").ok();
            let automatic = terminals::automatic(&found, env_terminal.as_deref())
                .map_or_else(|| "none found".to_string(), |t| t.name.clone());
            let pick = |app: Option<String>| {
                window.listener_for(
                    &menu_view,
                    move |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_terminal_app(app.clone(), cx)
                    },
                )
            };
            let mut menu = menu
                .item(
                    PopupMenuItem::new(format!("Automatic ({automatic})"))
                        .checked(current.is_none())
                        .on_click(pick(None)),
                )
                .separator();
            for t in &found {
                menu = menu.item(
                    PopupMenuItem::new(t.name.clone())
                        .checked(current.as_deref() == Some(t.id.as_str()))
                        .on_click(pick(Some(t.id.clone()))),
                );
            }
            menu.separator()
                .item(
                    PopupMenuItem::new("Custom Command")
                        .checked(current.as_deref() == Some(CUSTOM))
                        .on_click(pick(Some(CUSTOM.into()))),
                )
                .separator()
                .item(PopupMenuItem::new("Detect Again").on_click(
                    window.listener_for(&menu_view, |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.detect_terminals(cx)
                    }),
                ))
                .min_w(ui(240.))
        });

    v_flex()
        .gap(ui(6.))
        .child(
            h_flex()
                .gap(ui(8.))
                .child(div().text_size(ui(12.)).child("Open Nushell Here in"))
                .child(picker)
                .child(
                    Button::new("try-terminal")
                        .small()
                        .ghost()
                        .label("Try It")
                        .tooltip("Open Nushell in the current folder now")
                        .on_click(window.listener_for(
                            view,
                            |this: &mut Finder, _: &ClickEvent, window, cx| {
                                let dir = this
                                    .current_folder()
                                    .map(Path::to_path_buf)
                                    .or_else(dirs::home_dir);
                                if let Some(dir) = dir {
                                    this.open_terminal_at(&dir, window, cx);
                                }
                            },
                        )),
                ),
        )
        .child(div().text_size(ui(11.)).text_color(muted).child(detail))
        .when(custom, |this| {
            this.child(
                v_flex()
                    .gap(ui(4.))
                    .child(
                        Input::new(input)
                            .id("terminal-input")
                            .small()
                            .font_family(theme.mono_font_family.clone()),
                    )
                    .child(div().text_size(ui(11.)).text_color(muted).child(
                        "{dir} becomes the folder and {nu} the path to nu. Quote words that contain spaces.",
                    )),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::resolve_path;
    use std::path::{Path, PathBuf};

    #[test]
    fn resolves_paths() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(resolve_path("~", None), home);
        assert_eq!(resolve_path("~/x", None), home.join("x"));
        assert_eq!(
            resolve_path("sub", Some(Path::new("/base"))),
            PathBuf::from("/base/sub")
        );
        assert_eq!(
            resolve_path(" /abs ", Some(Path::new("/base"))),
            PathBuf::from("/abs")
        );
    }
}
