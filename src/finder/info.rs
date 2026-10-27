//! Get Info (Cmd+I): an item's kind, sizes, dates, link target, download
//! source, default app, owner, and permissions, which can be changed. With
//! several items selected it shows their totals. Folder sizes and the
//! default app are found in the background and fill in when ready.

use super::{Finder, group_digits, tildify};
use crate::actions::GetInfo;
use crate::theme::{palette, ui};
use fffinder_core::info::{self, FolderSize, Info};
use fffinder_core::{FileEntry, format};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The open Get Info panel.
pub(super) struct InfoPanel {
    items: Vec<Info>,
    /// What the selected folders hold; `None` while it's being added up.
    folders: Option<FolderSize>,
    opens_with: Option<String>,
    /// Stops the folder walk when the panel is replaced.
    stop: Arc<AtomicBool>,
}

impl Drop for InfoPanel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Permission bits by row (who) and column (what), for the checkboxes.
const WHO: [(&str, &str, u32); 3] = [
    ("owner", "Owner", 6),
    ("group", "Group", 3),
    ("everyone", "Everyone", 0),
];
const WHAT: [(&str, &str, u32); 3] = [
    ("read", "Read", 4),
    ("write", "Write", 2),
    ("execute", "Execute", 1),
];

/// "1.2 MB (1,234,567 bytes)", or "41 bytes".
fn size_text(bytes: u64) -> String {
    match bytes {
        0..1000 => format::human_size(bytes),
        _ => format!(
            "{} ({} bytes)",
            format::human_size(bytes),
            group_digits(bytes as usize)
        ),
    }
}

impl Finder {
    /// Cmd+I: show Get Info for the selection, or for the folder shown.
    pub(super) fn get_info(&mut self, _: &GetInfo, window: &mut Window, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            paths.extend(self.current_folder().map(Path::to_path_buf));
        }
        let items: Vec<Info> = paths.iter().filter_map(|p| info::read(p).ok()).collect();
        if items.is_empty() {
            return;
        }
        let folders: Vec<PathBuf> = items
            .iter()
            .filter(|i| i.is_dir)
            .map(|i| i.path.clone())
            .collect();
        let single_file = match items.as_slice() {
            [one] if !one.is_dir => Some(one.path.clone()),
            _ => None,
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.info = Some(InfoPanel {
            folders: folders.is_empty().then(FolderSize::default),
            items,
            opens_with: None,
            stop: stop.clone(),
        });
        cx.spawn_in(window, async move |this, cx| {
            let (sizes, opens_with) = cx
                .background_spawn(async move {
                    let opens_with = single_file.and_then(|p| crate::system::default_app_name(&p));
                    let sizes = folders
                        .iter()
                        .map(|f| info::folder_size(f, &stop))
                        .fold(FolderSize::default(), |a, b| a + b);
                    (sizes, opens_with)
                })
                .await;
            this.update_in(cx, |this, window, _| {
                if let Some(panel) = &mut this.info {
                    panel.folders = Some(sizes);
                    panel.opens_with = opens_with;
                    window.refresh();
                }
            })
            .ok();
        })
        .detach();
        self.open_info_dialog(window, cx);
    }

    fn open_info_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_before_dialog(window, cx);
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let finder = view.read(cx);
            let Some(panel) = &finder.info else {
                return dialog;
            };
            let title = match panel.items.as_slice() {
                [one] => format!("{} Info", one.name),
                many => format!("{} Items", many.len()),
            };
            let body = match panel.items.as_slice() {
                [one] => single_item(one, panel, &view, window, cx).into_any_element(),
                many => several_items(many, panel, cx).into_any_element(),
            };
            dialog.title(title).w(ui(460.)).child(body).footer(
                DialogFooter::new().child(
                    DialogClose::new().child(Button::new("info-done").primary().label("Done")),
                ),
            )
        });
    }

    /// Flip one permission bit of the item in the panel.
    fn toggle_permission(&mut self, bit: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.info.as_mut().and_then(|p| p.items.first_mut()) else {
            return;
        };
        let mode = item
            .mode
            .unwrap_or(if item.readonly { 0o444 } else { 0o644 })
            ^ bit;
        match info::set_mode(&item.path, mode) {
            Ok(()) => {
                item.mode = item.mode.map(|_| mode);
                item.readonly = mode & 0o200 == 0;
            }
            Err(err) => window.push_notification(
                (
                    NotificationType::Error,
                    format!("Couldn’t change the permissions: {err}"),
                ),
                cx,
            ),
        }
        window.refresh();
    }
}

/// A label and its value, with `id` on the value so tests can read it.
fn row(id: &'static str, label: &'static str, value: impl Into<SharedString>, cx: &App) -> Div {
    let value = value.into();
    h_flex()
        .items_start()
        .gap(ui(10.))
        .text_size(ui(12.))
        .child(
            div()
                .w(ui(96.))
                .flex_shrink_0()
                .text_right()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .aria_label(value.clone())
                .child(value)
                .test_support(),
        )
}

fn heading(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .pt(ui(8.))
        .text_size(ui(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

fn date_row(
    id: &'static str,
    label: &'static str,
    time: Option<std::time::SystemTime>,
    cx: &App,
) -> Option<Div> {
    time.map(|t| row(id, label, format::long_date(t), cx))
}

/// The size line: a file's bytes, or what folders hold once counted.
fn size_line(files: u64, on_disk: u64, has_folders: bool, panel: &InfoPanel) -> String {
    match panel.folders {
        None => "Calculating…".into(),
        Some(folders) if has_folders => format!(
            "{}, {} on disk, for {} items",
            size_text(files + folders.bytes),
            format::human_size(on_disk + folders.on_disk),
            group_digits(folders.items as usize)
        ),
        Some(_) => format!(
            "{}, {} on disk",
            size_text(files),
            format::human_size(on_disk)
        ),
    }
}

fn single_item(
    item: &Info,
    panel: &InfoPanel,
    view: &Entity<Finder>,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let pal = palette(cx);
    let icon = FileEntry::from_path(&item.path)
        .map(|entry| super::views::file_icon(&entry, ui(48.), true, &pal));
    let size = size_line(
        item.size.unwrap_or(0),
        item.on_disk.unwrap_or(0),
        item.is_dir,
        panel,
    );
    let summary = match (item.size, item.is_dir) {
        (Some(bytes), false) => format!("{} · {}", item.kind, format::human_size(bytes)),
        _ => item.kind.clone(),
    };
    v_flex()
        .gap(ui(6.))
        .child(
            h_flex().gap(ui(12.)).pb(ui(4.)).children(icon).child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .id("info-name")
                            .text_size(ui(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(item.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(ui(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(summary),
                    ),
            ),
        )
        .child(heading("GENERAL", cx))
        .child(row("info-kind", "Kind", item.kind.clone(), cx))
        .child(row("info-size", "Size", size, cx))
        .when_some(item.path.parent(), |this, parent| {
            this.child(row("info-where", "Where", tildify(parent), cx))
        })
        .children(date_row("info-created", "Created", item.created, cx))
        .children(date_row("info-modified", "Modified", item.modified, cx))
        .children(date_row("info-opened", "Last opened", item.last_opened, cx))
        .when_some(item.link_target.as_ref(), |this, target| {
            this.child(row(
                "info-original",
                "Original",
                target.display().to_string(),
                cx,
            ))
        })
        .when(!item.where_from.is_empty(), |this| {
            this.child(row(
                "info-where-from",
                "Where from",
                item.where_from.join("\n"),
                cx,
            ))
        })
        .when_some(panel.opens_with.clone(), |this, app| {
            this.child(row("info-opens-with", "Opens with", app, cx))
        })
        .child(heading("SHARING & PERMISSIONS", cx))
        .when_some(item.owner.clone(), |this, owner| {
            this.child(row("info-owner", "Owner", owner, cx))
        })
        .when_some(item.group.clone(), |this, group| {
            this.child(row("info-group", "Group", group, cx))
        })
        .child(permissions(item, view, window, cx))
}

/// Checkboxes for each permission bit (Unix), or a read-only box.
fn permissions(item: &Info, view: &Entity<Finder>, window: &mut Window, cx: &App) -> AnyElement {
    let toggle = |id: SharedString,
                  label: &'static str,
                  on: bool,
                  bit: u32,
                  window: &mut Window| {
        Checkbox::new(id).label(label).checked(on).on_click(
            window.listener_for(view, move |this: &mut Finder, _: &bool, window, cx| {
                this.toggle_permission(bit, window, cx)
            }),
        )
    };
    let Some(mode) = item.mode else {
        return row("info-permissions", "Permissions", "", cx)
            .child(toggle(
                "perm-readonly".into(),
                "Read-only",
                item.readonly,
                0o200,
                window,
            ))
            .into_any_element();
    };
    let mut grid = v_flex().gap(ui(4.)).text_size(ui(12.));
    for (who_id, who, shift) in WHO {
        let mut line = h_flex().gap(ui(12.)).child(
            div()
                .w(ui(96.))
                .text_right()
                .text_color(cx.theme().muted_foreground)
                .child(who),
        );
        for (what_id, what, bit) in WHAT {
            let bit = bit << shift;
            let id = SharedString::from(format!("perm-{who_id}-{what_id}"));
            line = line.child(toggle(id, what, mode & bit != 0, bit, window));
        }
        grid = grid.child(line);
    }
    grid.child(row(
        "info-mode",
        "Mode",
        format!("{} ({:o})", info::mode_string(mode), mode),
        cx,
    ))
    .into_any_element()
}

fn several_items(items: &[Info], panel: &InfoPanel, cx: &App) -> impl IntoElement {
    let mut kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let kind = match kinds.as_slice() {
        [one] => one.to_string(),
        _ => format!("{} kinds", kinds.len()),
    };
    let files: u64 = items.iter().filter_map(|i| i.size).sum();
    let on_disk: u64 = items.iter().filter_map(|i| i.on_disk).sum();
    let has_folders = items.iter().any(|i| i.is_dir);
    let mut parents: Vec<&Path> = items.iter().filter_map(|i| i.path.parent()).collect();
    parents.dedup();
    v_flex()
        .gap(ui(6.))
        .child(row("info-kind", "Kind", kind, cx))
        .child(row(
            "info-size",
            "Size",
            size_line(files, on_disk, has_folders, panel),
            cx,
        ))
        .when_some(
            match parents.as_slice() {
                [one] => Some(tildify(one)),
                _ => None,
            },
            |this, parent| this.child(row("info-where", "Where", parent, cx)),
        )
}
