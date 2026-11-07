//! Using the user's Nushell environment: file colors from `LS_COLORS`, and
//! "Open in Editor" with `buffer_editor` (in a terminal for editors like
//! nvim or hx).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fffinder_core::terminals::Plan;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::NotificationType;
use gpui_kit::*;

use super::{AppState, Finder, InTerminal};
use crate::nu_env::{self, NuEnv};

impl Finder {
    /// Read `nu --login`'s environment in the background.
    pub fn load_nu_env(&mut self, cx: &mut Context<Self>) {
        let Some(nu) = cx.global::<AppState>().nu.clone() else {
            self.nu_env_error = Some("nu was not found.".into());
            cx.notify();
            return;
        };
        self.nu_env_loading = true;
        self.nu_env_task = cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { nu_env::load(&nu) }).await;
            this.update(cx, |this, cx| {
                this.nu_env_loading = false;
                match result {
                    Ok(env) => {
                        this.ls_colors = Some(Arc::new(env.ls_colors()));
                        this.nu_env = Some(Arc::new(env));
                        this.nu_env_error = None;
                    }
                    Err(err) => this.nu_env_error = Some(err),
                }
                this.rebuild_rows(cx);
            })
            .ok();
        });
        cx.notify();
    }

    /// Turn Nushell's colors and editor on or off.
    pub fn set_use_nu_environment(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.config.use_nu_environment == on {
            return;
        }
        self.config.use_nu_environment = on;
        self.settings_changed(cx);
        self.apply_nu_environment(cx);
    }

    /// Load Nushell's colors and editor, or drop them, as the setting says.
    pub(super) fn apply_nu_environment(&mut self, cx: &mut Context<Self>) {
        if self.config.use_nu_environment {
            self.load_nu_env(cx);
        } else {
            self.nu_env = None;
            self.ls_colors = None;
            self.nu_env_error = None;
            self.nu_env_task = Task::ready(());
            self.rebuild_rows(cx);
        }
        cx.notify();
    }

    /// A line for Settings: what was read, or why not.
    pub fn nu_env_summary(&self) -> String {
        if !self.config.use_nu_environment {
            return "Off: names use the theme's colors and files open in their default app.".into();
        }
        if self.nu_env_loading {
            return "Reading your Nushell environment…".into();
        }
        if let Some(err) = &self.nu_env_error {
            return format!("Couldn’t read it: {err}");
        }
        let Some(env) = &self.nu_env else {
            return String::new();
        };
        let colors = if env.ls_colors.as_deref().is_some_and(|s| !s.is_empty()) {
            "$env.LS_COLORS"
        } else {
            "Nushell's default ls colors"
        };
        let editor = env
            .editor()
            .map(|words| format!("opens files in {}", words.join(" ")))
            .unwrap_or_else(|| "no editor is set, so files open in their default app".into());
        format!("Colors from {colors}; {editor}.")
    }

    #[doc(hidden)]
    pub fn set_nu_env_for_test(&mut self, env: NuEnv, cx: &mut Context<Self>) {
        self.config.use_nu_environment = true;
        self.ls_colors = Some(Arc::new(env.ls_colors()));
        self.nu_env = Some(Arc::new(env));
        self.rebuild_rows(cx);
    }

    /// Open files with their default apps. A file no app claims opens in an
    /// editor when it holds text; for anything else a notification says so.
    pub fn open_files(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = Vec::new();
        for path in paths {
            if !path.is_file() || crate::system::has_default_app(&path, cx) != Some(false) {
                crate::system::open(&path, cx);
            } else if super::preview::read_text(&path).is_some() {
                text.push(path);
            } else {
                notify(
                    format!("No application is set to open “{}”.", display_name(&path)),
                    window,
                    cx,
                );
            }
        }
        if !text.is_empty() {
            self.edit_files(text, window, cx);
        }
    }

    /// "Open in Editor": Nushell's `buffer_editor` when that option is on,
    /// otherwise each file's default app (or a text editor, as for Open).
    pub fn open_in_editor(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files: Vec<PathBuf> = paths.into_iter().filter(|p| !p.is_dir()).collect();
        if files.is_empty() {
            return;
        }
        match self.nu_editor() {
            Some((env, words)) => {
                let program = env.resolve(&words[0]);
                self.launch_editor(program, &words, files, window, cx);
            }
            None => self.open_files(files, window, cx),
        }
    }

    /// Nushell's `buffer_editor`, when the option is on and one is set.
    fn nu_editor(&self) -> Option<(Arc<NuEnv>, Vec<String>)> {
        let env = self
            .nu_env
            .as_ref()
            .filter(|_| self.config.use_nu_environment)?;
        env.editor().map(|words| (env.clone(), words))
    }

    /// Open text files in an editor: Nushell's when that's on, else the
    /// system's text editor, else `$VISUAL` or `$EDITOR`.
    fn edit_files(&mut self, files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((env, words)) = self.nu_editor() {
            let program = env.resolve(&words[0]);
            self.launch_editor(program, &words, files, window, cx);
            return;
        }
        let mut rest = Vec::new();
        for file in files {
            let Some(task) = crate::system::edit_text(&file, cx) else {
                rest.push(file);
                continue;
            };
            let name = display_name(&file);
            cx.spawn_in(window, async move |_, cx| {
                if let Err(err) = task.await {
                    cx.update(|window, cx| {
                        notify(
                            format!("Couldn’t open “{name}” in a text editor: {err}"),
                            window,
                            cx,
                        )
                    })
                    .ok();
                }
            })
            .detach();
        }
        let Some(first) = rest.first() else {
            return;
        };
        let editor = ["VISUAL", "EDITOR"]
            .iter()
            .filter_map(|var| std::env::var(var).ok())
            .map(|value| {
                value
                    .split_whitespace()
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .find(|words| !words.is_empty());
        match editor {
            Some(words) => {
                let program = PathBuf::from(&words[0]);
                self.launch_editor(program, &words, rest, window, cx);
            }
            None => notify(
                format!(
                    "No application opens “{}”, and no editor is set. Turn on \
                     “Use Nushell’s file colors and editor” in Settings, or set $VISUAL.",
                    display_name(first)
                ),
                window,
                cx,
            ),
        }
    }

    /// Start `words` (an editor and its arguments) on `files`: a terminal
    /// editor like nvim or hx in the chosen terminal, a windowed one directly.
    fn launch_editor(
        &mut self,
        program: PathBuf,
        words: &[String],
        files: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut args: Vec<String> = words[1..].to_vec();
        args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
        let dir = files[0]
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        if nu_env::needs_terminal(&words[0]) {
            self.open_in_terminal(&dir, InTerminal::Command { program, args }, window, cx);
            return;
        }
        let plan = Plan {
            program,
            args,
            cwd: dir,
            script: None,
            new_console: false,
            fallback: None,
        };
        let launched = crate::system::launch(plan, cx);
        cx.spawn_in(window, async move |_, cx| {
            if let Err(err) = launched.await {
                cx.update(|window, cx| {
                    notify(format!("Couldn’t start the editor: {err}"), window, cx)
                })
                .ok();
            }
        })
        .detach();
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn notify(message: String, window: &mut Window, cx: &mut App) {
    window.push_notification((NotificationType::Warning, message), cx);
}
