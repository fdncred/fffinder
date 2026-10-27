//! Copying and moving files: the clipboard, drops, the name clash dialog,
//! and the progress row with its Stop button. The work itself is
//! `fffinder_core::ops`, on a background thread.

use super::Finder;
use super::views::DraggedFiles;
use crate::actions::MoveItemsHere;
use crate::theme::ui;
use fffinder_core::entry::display_name;
use fffinder_core::ops::{self, Clash, Mode, Progress, Report, Transfer};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input;
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A transfer only gets a progress row once it has run this long.
const SHOW_AFTER: Duration = Duration::from_millis(300);

/// A copy or move under way.
pub(super) struct Job {
    mode: Mode,
    count: usize,
    progress: Arc<Progress>,
    started: Instant,
}

/// "“name”" for one item, "N items" for more.
fn items_label(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!("“{}”", display_name(one)),
        many => format!("{} items", many.len()),
    }
}

impl Finder {
    /// Cmd+C in the files: copy the selected items.
    pub(super) fn copy_files(
        &mut self,
        _: &input::Copy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            return;
        }
        crate::system::copy_files(&paths, cx);
        window.push_notification(format!("Copied {}", items_label(&paths)), cx);
    }

    /// Cmd+V in the files: copy what's on the clipboard into this folder.
    pub(super) fn paste_files(
        &mut self,
        _: &input::Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste(Mode::Copy, window, cx);
    }

    /// Cmd+Alt+V: move what's on the clipboard into this folder.
    pub(super) fn move_items_here(
        &mut self,
        _: &MoveItemsHere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste(Mode::Move, window, cx);
    }

    fn paste(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let sources = crate::system::clipboard_files(cx);
        let dest = self
            .current_folder()
            .filter(|_| !self.is_searching())
            .map(Path::to_path_buf);
        let warning = match (&dest, sources.is_empty()) {
            (_, true) => "There are no files on the clipboard",
            (None, false) => "Open a folder to paste into",
            (Some(_), false) => "",
        };
        match dest {
            Some(dest) if warning.is_empty() => {
                self.start_transfer(sources, dest, mode, window, cx)
            }
            _ => window.push_notification((NotificationType::Warning, warning), cx),
        }
    }

    /// Files dropped on `dest`. Dragged within fffinder they move, or copy
    /// with Alt held or to another volume; from another app they copy.
    pub(super) fn drop_files(
        &mut self,
        paths: Vec<PathBuf>,
        dest: PathBuf,
        from_here: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let copy = !from_here
            || copy_modifier(window.modifiers())
            || paths.first().is_some_and(|p| !ops::same_volume(p, &dest));
        let mode = if copy { Mode::Copy } else { Mode::Move };
        self.start_transfer(paths, dest, mode, window, cx);
    }

    /// Check a transfer, ask about name clashes, then start it.
    pub(super) fn start_transfer(
        &mut self,
        sources: Vec<PathBuf>,
        dest: PathBuf,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let transfer = Transfer {
            sources,
            dest,
            mode,
        };
        if let Err(err) = transfer.check() {
            window.push_notification((NotificationType::Warning, err.to_string()), cx);
            return;
        }
        // Dropping items on the folder they're already in does nothing.
        if mode == Mode::Move && transfer.sources.iter().all(|s| transfer.in_place(s)) {
            return;
        }
        let clashes = transfer.clashes();
        if clashes.is_empty() {
            self.run_transfer(transfer, Clash::Skip, window, cx);
        } else {
            self.confirm_clashes(transfer, &clashes, window, cx);
        }
    }

    fn confirm_clashes(
        &mut self,
        transfer: Transfer,
        clashes: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        let folder = display_name(&transfer.dest);
        let title = match clashes {
            [one] => format!(
                "An item named “{}” already exists in “{folder}”",
                display_name(one)
            ),
            many => format!("{} items already exist in “{folder}”", many.len()),
        };
        let verb = match transfer.mode {
            Mode::Copy => "copying",
            Mode::Move => "moving",
        };
        let detail = format!("Replace them with the ones you’re {verb}, skip them, or keep both?");
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let choice = |id: &'static str, label: &'static str, clash: Option<Clash>| {
                let transfer = transfer.clone();
                Button::new(id).label(label).on_click(window.listener_for(
                    &view,
                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                        window.close_dialog(cx);
                        this.focus_files(window, cx);
                        if let Some(clash) = clash {
                            this.run_transfer(transfer.clone(), clash, window, cx);
                        }
                    },
                ))
            };
            dialog
                .title(title.clone())
                .w(ui(480.))
                .child(
                    div()
                        .text_size(ui(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(detail.clone()),
                )
                .footer(
                    DialogFooter::new()
                        .child(choice("clash-stop", "Stop", None))
                        .child(choice("clash-skip", "Skip", Some(Clash::Skip)))
                        .child(choice("clash-keep", "Keep Both", Some(Clash::KeepBoth)))
                        .child(choice("clash-replace", "Replace", Some(Clash::Replace)).danger()),
                )
        });
    }

    /// Do the transfer on a background thread, with a progress row.
    pub(super) fn run_transfer(
        &mut self,
        transfer: Transfer,
        clash: Clash,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let progress = Arc::new(Progress::default());
        self.transfers.push(Job {
            mode: transfer.mode,
            count: transfer.sources.len(),
            progress: progress.clone(),
            started: cx.background_executor().now(),
        });
        self.poll_transfers(cx);
        cx.spawn_in(window, async move |this, cx| {
            let report = cx
                .background_spawn({
                    let progress = progress.clone();
                    async move { ops::run(&transfer, clash, &progress) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.transfers
                    .retain(|job| !Arc::ptr_eq(&job.progress, &progress));
                this.transfer_finished(&report, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Redraw the progress rows while transfers run.
    fn poll_transfers(&mut self, cx: &mut Context<Self>) {
        self.transfers_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let running = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        !this.transfers.is_empty()
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        });
    }

    fn transfer_finished(&mut self, report: &Report, window: &mut Window, cx: &mut Context<Self>) {
        let landed: Vec<PathBuf> = report.done.iter().map(|(_, to)| to.clone()).collect();
        let here: Vec<PathBuf> = landed
            .iter()
            .filter(|p| p.parent() == self.current_folder())
            .cloned()
            .collect();
        self.load_listing(here, true, cx);
        if self.is_searching() {
            self.results.retain(|r| r.entry.path.exists());
            self.rebuild_rows(cx);
        }
        self.reload_columns(cx);
        if let Some((path, why)) = report.failed.first() {
            let more = match report.failed.len() {
                1 => String::new(),
                n => format!(" ({} more items failed)", n - 1),
            };
            window.push_notification(
                (
                    NotificationType::Error,
                    format!(
                        "Couldn’t copy or move “{}”: {why}{more}",
                        display_name(path)
                    ),
                ),
                cx,
            );
        } else if report.cancelled {
            window.push_notification("Stopped", cx);
        }
    }

    pub(super) fn transfers_running(&self) -> bool {
        !self.transfers.is_empty()
    }

    /// One row per transfer that has run long enough to show.
    pub(super) fn render_transfers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let theme = cx.theme();
        let (border, muted, bar) = (theme.border, theme.muted_foreground, theme.status_bar);
        let rows = self
            .transfers
            .iter()
            .enumerate()
            .filter(|(_, job)| now.saturating_duration_since(job.started) >= SHOW_AFTER)
            .map(|(ix, job)| {
                let verb = match job.mode {
                    Mode::Copy => "Copying",
                    Mode::Move => "Moving",
                };
                let what = match job.count {
                    1 => format!("“{}”", job.progress.current()),
                    n => format!("{n} items: {}", job.progress.current()),
                };
                let fraction = job.progress.fraction();
                let progress = job.progress.clone();
                h_flex()
                    .id(("transfer", ix))
                    .h(ui(36.))
                    .flex_shrink_0()
                    .px(ui(14.))
                    .gap(ui(10.))
                    .bg(bar)
                    .border_t_1()
                    .border_color(border)
                    .text_size(ui(12.))
                    .child(
                        Icon::new(match job.mode {
                            Mode::Copy => IconName::Copy,
                            Mode::Move => IconName::FolderOpen,
                        })
                        .with_size(ui(14.))
                        .text_color(muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!("{verb} {what}")),
                    )
                    .child(
                        div()
                            .w(ui(160.))
                            .child(ProgressBar::new(("transfer-bar", ix)).value(fraction * 100.)),
                    )
                    .child(
                        div()
                            .w(ui(36.))
                            .text_color(muted)
                            .child(format!("{:.0}%", fraction * 100.)),
                    )
                    .child(
                        Button::new(("transfer-stop", ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Stop")
                            .on_click(move |_, _, _| progress.cancel()),
                    )
            });
        div().flex_shrink_0().children(rows)
    }
}

/// The key that makes a drag copy instead of move: Option on macOS, as in
/// Finder, and Ctrl on Linux and Windows, as in their file managers.
pub(super) fn copy_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.alt
    } else {
        modifiers.control
    }
}

/// Make a folder's row or tile take dropped files: dragged items move into
/// it, and files from other apps are copied in.
pub(super) fn folder_drop_target<E: InteractiveElement>(
    el: E,
    dest: PathBuf,
    cx: &mut Context<Finder>,
) -> E {
    let highlight = cx.theme().drop_target;
    let (internal, external, itself) = (dest.clone(), dest.clone(), dest);
    el.drag_over::<DraggedFiles>(move |style, _, _, _| style.bg(highlight))
        .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(highlight))
        // A folder can't take itself; the drop goes to the view around it.
        .can_drop(move |dragged, _, _| {
            dragged
                .downcast_ref::<DraggedFiles>()
                .is_none_or(|files| !files.0.contains(&itself))
        })
        .on_drop(cx.listener(move |this, files: &DraggedFiles, window, cx| {
            this.drop_files(files.0.clone(), internal.clone(), true, window, cx)
        }))
        .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
            this.drop_files(paths.paths().to_vec(), external.clone(), false, window, cx)
        }))
}
