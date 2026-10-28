//! File previews for the gallery, the column view, and Quick Look. Structured
//! files (CSV, JSON, TOML, YAML, SQLite, …) are opened with Nushell and shown
//! as tables. Code is highlighted, Markdown rendered, archives listed, fonts
//! shown in their own type, PDFs drawn page by page (see `pdf.rs`), and
//! videos and documents show the system's thumbnail of them.

use super::views::file_icon;
use super::{AppState, Finder};
use crate::actions::{
    MoveDown, MoveLeft, MoveRight, MoveUp, NextPdfPage, PreviewTextBigger, PreviewTextReset,
    PreviewTextSmaller, PreviousPdfPage, QUICK_LOOK, QuickLook,
};
use crate::nushell::{self, NuOutput};
use crate::theme::palette;
use crate::theme::ui;
use fffinder_core::config::{DEFAULT_PREVIEW_FONT_SIZE, FONT_SIZE_RANGE};
use fffinder_core::{FileEntry, FileKind, ViewMode, format};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const TEXT_LIMIT: usize = 64 * 1024;
const TABLE_ROWS: usize = 200;
/// Font files bigger than this aren't loaded to show them.
const FONT_LIMIT: u64 = 50_000_000;

#[derive(Clone)]
pub enum Preview {
    Loading,
    Image,
    Text {
        text: SharedString,
        truncated: bool,
    },
    /// Source colored the way the user's Nushell colors it.
    Code {
        text: SharedString,
        spans: Vec<(std::ops::Range<usize>, HighlightStyle)>,
        /// Comments, which `nu-highlight` leaves plain; drawn in the theme's
        /// secondary text color.
        comments: Vec<std::ops::Range<usize>>,
        truncated: bool,
    },
    Table {
        columns: Vec<SharedString>,
        rows: Vec<Vec<SharedString>>,
        /// A line under the table, such as where it came from.
        footer: Option<SharedString>,
    },
    /// Markdown, rendered.
    Markdown {
        text: SharedString,
    },
    /// A font file, drawn in its own type once `data` is loaded into the
    /// text system.
    Font {
        family: SharedString,
        data: Option<Arc<Vec<u8>>>,
    },
    /// Audio or video, with its running time when the header says.
    Media {
        duration: Option<Duration>,
    },
    Folder {
        items: usize,
    },
    Unavailable,
}

impl Preview {
    /// Whether the preview shows text, so its size can be changed.
    fn has_text(&self) -> bool {
        matches!(
            self,
            Preview::Text { .. }
                | Preview::Code { .. }
                | Preview::Table { .. }
                | Preview::Markdown { .. }
                | Preview::Font { .. }
        )
    }
}

/// The family name stored in a font file.
fn font_family(data: &[u8]) -> Option<String> {
    use ttf_parser::name_id::{FAMILY, TYPOGRAPHIC_FAMILY};
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    [TYPOGRAPHIC_FAMILY, FAMILY].into_iter().find_map(|id| {
        face.names()
            .into_iter()
            .find(|name| name.name_id == id && name.is_unicode())
            .and_then(|name| name.to_string())
    })
}

fn extension(entry: &FileEntry) -> String {
    entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// What's inside a zip or tar archive, as a table.
fn archive_table(entry: &FileEntry) -> Option<Preview> {
    let (listed, more) = fffinder_core::archive::list(&entry.path, TABLE_ROWS).ok()?;
    let count = listed.len();
    let rows = listed
        .into_iter()
        .map(|item| {
            let size = if item.is_dir {
                "--".to_string()
            } else {
                format::human_size(item.size)
            };
            vec![item.name.into(), size.into()]
        })
        .collect();
    let footer = match (count, more) {
        (n, true) => format!("The first {n} items"),
        (1, false) => "1 item".to_string(),
        (n, false) => format!("{n} items"),
    };
    Some(Preview::Table {
        columns: vec!["Name".into(), "Size".into()],
        rows,
        footer: Some(footer.into()),
    })
}

/// Work out the preview for `entry`; `dark` picks the code colors.
fn load_preview(entry: &FileEntry, nu: Option<&Path>, dark: bool) -> Preview {
    if entry.is_dir {
        let items = std::fs::read_dir(&entry.path)
            .map(|d| d.count())
            .unwrap_or(0);
        return Preview::Folder { items };
    }
    let is_nu = entry
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("nu"));
    if let Some(nu) = nu
        && is_nu
        && let Some((source, truncated)) = read_text(&entry.path)
        && let Ok(colored) = nushell::highlight_nu(nu, &source)
    {
        let (text, spans) = crate::ansi::parse(&colored);
        let styled: Vec<_> = spans.iter().map(|(range, _)| range.clone()).collect();
        return Preview::Code {
            comments: nushell::comment_ranges(&text, &styled),
            text: text.into(),
            spans: spans
                .into_iter()
                .map(|(range, style)| (range, style.to_highlight()))
                .collect(),
            truncated,
        };
    }
    if let Some(nu) = nu
        && nushell::is_structured(&entry.path)
        && entry.size.unwrap_or(0) < 50_000_000
    {
        let run = nushell::preview_file(nu, &entry.path);
        match run.output {
            NuOutput::Table { columns, rows } => {
                let total = rows.len();
                let footer = format!(
                    "Opened with Nushell · {total} row{}{}",
                    if total == 1 { "" } else { "s" },
                    if total > TABLE_ROWS {
                        ", the first 200 shown"
                    } else {
                        ""
                    }
                );
                return Preview::Table {
                    columns: columns.into_iter().map(Into::into).collect(),
                    rows: rows
                        .into_iter()
                        .take(TABLE_ROWS)
                        .map(|r| r.into_iter().map(Into::into).collect())
                        .collect(),
                    footer: Some(footer.into()),
                };
            }
            NuOutput::Text(text) => {
                return Preview::Text {
                    truncated: text.len() > TEXT_LIMIT,
                    text: text.chars().take(TEXT_LIMIT).collect::<String>().into(),
                };
            }
            NuOutput::Empty => {}
        }
    }
    let ext = extension(entry);
    if fffinder_core::archive::format(&entry.path).is_some()
        && let Some(table) = archive_table(entry)
    {
        return table;
    }
    if matches!(ext.as_str(), "ttf" | "otf" | "ttc")
        && entry.size.unwrap_or(0) < FONT_LIMIT
        && let Ok(data) = std::fs::read(&entry.path)
        && let Some(family) = font_family(&data)
    {
        return Preview::Font {
            family: family.into(),
            data: Some(Arc::new(data)),
        };
    }
    match entry.kind {
        FileKind::Image => return Preview::Image,
        FileKind::Video | FileKind::Audio => {
            return Preview::Media {
                duration: fffinder_core::media::duration(&entry.path),
            };
        }
        _ => {}
    }
    if matches!(
        entry.kind,
        FileKind::Text | FileKind::Code | FileKind::Data | FileKind::Other | FileKind::Executable
    ) && let Some((text, truncated)) = read_text(&entry.path)
    {
        if matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd") {
            return Preview::Markdown { text: text.into() };
        }
        if let Some(spans) = crate::highlight::highlight(&text, &entry.path, dark) {
            return Preview::Code {
                text: text.into(),
                spans,
                comments: Vec::new(),
                truncated,
            };
        }
        return Preview::Text {
            text: text.into(),
            truncated,
        };
    }
    Preview::Unavailable
}

/// The start of a file as text, or `None` when it looks binary.
pub(super) fn read_text(path: &Path) -> Option<(String, bool)> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; TEXT_LIMIT + 1];
    let mut len = 0;
    while len < buf.len() {
        match file.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(_) => return None,
        }
    }
    buf.truncate(len);
    if buf.iter().take(8192).any(|&b| b == 0) {
        return None;
    }
    let truncated = buf.len() > TEXT_LIMIT;
    buf.truncate(TEXT_LIMIT);
    let text = String::from_utf8_lossy(&buf).into_owned();
    // Lossy decoding of a binary file produces many replacement characters.
    let bad = text.chars().take(4096).filter(|&c| c == '\u{FFFD}').count();
    (bad < 16).then_some((text, truncated))
}

impl Finder {
    /// Start loading the preview for `entry` unless it is cached or loading.
    pub fn ensure_preview(&mut self, entry: &FileEntry, cx: &mut Context<Self>) {
        let path = entry.path.clone();
        if self.previews.contains_key(&path) || self.preview_tasks.contains_key(&path) {
            return;
        }
        if self.previews.len() > 256 {
            self.previews.clear();
        }
        let nu = cx.global::<AppState>().nu.clone();
        let dark = cx.theme().is_dark();
        let entry = entry.clone();
        let task = cx.spawn(async move |this, cx| {
            let mut preview = cx
                .background_spawn({
                    let entry = entry.clone();
                    async move { load_preview(&entry, nu.as_deref(), dark) }
                })
                .await;
            this.update(cx, |this, cx| {
                // A font is drawn in its own type once the text system has it.
                if let Preview::Font { data, .. } = &mut preview
                    && let Some(bytes) = data.take()
                {
                    let bytes = Arc::unwrap_or_clone(bytes);
                    if let Err(err) = cx.text_system().add_fonts(vec![bytes.into()]) {
                        log::warn!("loading a font to preview it: {err}");
                    }
                }
                this.preview_tasks.remove(&entry.path);
                this.previews.insert(entry.path.clone(), preview);
                cx.notify();
            })
            .ok();
            cx.refresh();
        });
        self.preview_tasks.insert(path, task);
    }

    pub fn preview_of(&self, entry: &FileEntry) -> Preview {
        self.previews
            .get(&entry.path)
            .cloned()
            .unwrap_or(Preview::Loading)
    }

    pub fn quick_look(&mut self, _: &QuickLook, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_look_open {
            self.quick_look_open = false;
            // The flag can outlive the dialog; only close what is open.
            if window.has_active_dialog(cx) {
                window.close_dialog(cx);
                return;
            }
        }
        let Some(entry) = self.primary_entry() else {
            return;
        };
        self.focus_before_dialog(window, cx);
        self.ensure_preview(&entry, cx);
        self.ensure_pdf(&entry, cx);
        self.quick_look_open = true;
        let view = cx.entity();
        let focus = self.quick_look_focus.clone();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            // Focus the preview so Space (bound in "QuickLook") closes it again.
            if first.replace(false) {
                focus.focus(window, cx);
            }
            let finder = view.read(cx);
            let Some(entry) = finder.primary_entry() else {
                return dialog;
            };
            let open_path = entry.path.clone();
            let open_view = view.clone();
            let close_view = view.clone();
            let text_size = finder
                .preview_of(&entry)
                .has_text()
                .then(|| finder.config.preview_font_size());
            // Quick Look takes most of the window, so a bigger window shows
            // a bigger preview, and never gets smaller than it used to be.
            let viewport = window.viewport_size();
            let (wide, tall) = (f32::from(viewport.width), f32::from(viewport.height));
            let width = (wide * 0.85).max(f32::from(ui(860.))).min(wide - 32.);
            let height = (tall * 0.85 - f32::from(ui(150.))).max(f32::from(ui(300.)));
            dialog
                .title(
                    h_flex()
                        .gap(ui(8.))
                        .child(file_icon(&entry, ui(20.), false, &palette(cx)))
                        .child(div().truncate().child(entry.name.clone())),
                )
                .w(px(width))
                .child(
                    div()
                        .track_focus(&focus)
                        .key_context(crate::actions::QUICK_LOOK)
                        .on_action({
                            let view = view.clone();
                            move |_: &QuickLook, window, cx| {
                                window.close_dialog(cx);
                                view.update(cx, |this, _| this.quick_look_open = false);
                            }
                        })
                        .on_action(text_size_handler::<PreviewTextBigger>(&view, Some(1.)))
                        .on_action(text_size_handler::<PreviewTextSmaller>(&view, Some(-1.)))
                        .on_action(text_size_handler::<PreviewTextReset>(&view, None))
                        .on_action(step_handler::<MoveUp>(&view, 0, -1))
                        .on_action(step_handler::<MoveDown>(&view, 0, 1))
                        .on_action(step_handler::<MoveLeft>(&view, -1, 0))
                        .on_action(step_handler::<MoveRight>(&view, 1, 0))
                        .on_action(page_handler::<NextPdfPage>(&view, 1))
                        .on_action(page_handler::<PreviousPdfPage>(&view, -1))
                        .h(px(height))
                        .child(render_preview_large(finder, &view, &entry, cx)),
                )
                .footer(
                    DialogFooter::new()
                        .child(h_flex().flex_1().when_some(text_size, |this, size| {
                            this.child(text_size_buttons(&view, size))
                        }))
                        .child(
                            Button::new("ql-reveal")
                                .label(super::menus::reveal_label())
                                .on_click({
                                    let path = entry.path.clone();
                                    move |_, _, cx| crate::system::reveal(&path, cx)
                                }),
                        )
                        .child(Button::new("ql-open").primary().label("Open").on_click(
                            move |_, window, cx| {
                                // `close_dialog` doesn't run `on_close`.
                                window.close_dialog(cx);
                                open_view.update(cx, |this, cx| {
                                    this.quick_look_open = false;
                                    // A file no app claims opens in an editor.
                                    this.open_files(vec![open_path.clone()], window, cx);
                                });
                            },
                        )),
                )
                .on_close(move |_, _, cx| {
                    close_view.update(cx, |this, _| this.quick_look_open = false);
                })
        });
    }

    /// Quick Look's arrow keys, as in Finder. With several items selected
    /// they step through those and keep the selection. Otherwise they move
    /// the selection the way the view does, except that in the list and in
    /// columns Left and Right go to the previous and next item. Quick Look
    /// then shows the new item.
    fn quick_look_step(
        &mut self,
        dx: isize,
        dy: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.effective_view();
        if view != ViewMode::Columns && self.selection.len() > 1 {
            self.selection.cycle_cursor(dx + dy);
            if let Some(ix) = self.selection.cursor() {
                self.scroll_to(ix);
            }
            cx.notify();
        } else if matches!(view, ViewMode::List | ViewMode::Columns) {
            self.move_selection(0, dx + dy, false, window, cx);
        } else {
            self.move_selection(dx, dy, false, window, cx);
        }
        if let Some(entry) = self.primary_entry() {
            self.ensure_preview(&entry, cx);
            self.ensure_pdf(&entry, cx);
        }
        // Selecting in a column focuses the files; keep the keys coming here.
        self.quick_look_focus.focus(window, cx);
    }
}

/// Pages through a PDF from Quick Look, `by` pages at a time.
fn page_handler<A: Action>(
    view: &Entity<Finder>,
    by: isize,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |_, _, cx| view.update(cx, |this, cx| this.step_pdf_page(by, cx))
}

/// Moves through the items from Quick Look by `dx` and `dy`.
fn step_handler<A: Action>(
    view: &Entity<Finder>,
    dx: isize,
    dy: isize,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |_, window, cx| {
        view.update(cx, |this, cx| this.quick_look_step(dx, dy, window, cx));
        // The dialog is drawn by the window's root, not by the Finder view.
        window.refresh();
    }
}

/// Steps the preview text size by `steps` points, or resets it for `None`.
fn step_text_size(view: &Entity<Finder>, steps: Option<f32>, window: &mut Window, cx: &mut App) {
    view.update(cx, |this, cx| this.step_preview_size(steps, cx));
    // The dialog is drawn by the window's root, not by the Finder view.
    window.refresh();
}

fn text_size_handler<A: Action>(
    view: &Entity<Finder>,
    steps: Option<f32>,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |_, window, cx| step_text_size(&view, steps, window, cx)
}

/// Quick Look's smaller/bigger text buttons, and the size, which resets it.
fn text_size_buttons(view: &Entity<Finder>, size: f32) -> impl IntoElement {
    let (min, max) = FONT_SIZE_RANGE;
    let view = view.clone();
    size_stepper(
        "ql-text",
        format!("{size} pt"),
        [size <= min, size >= max, size == DEFAULT_PREVIEW_FONT_SIZE],
        Some([&PreviewTextSmaller, &PreviewTextBigger, &PreviewTextReset]),
        move |steps, window, cx| step_text_size(&view, steps, window, cx),
    )
}

/// A small "A" (smaller), a big "A" (bigger), and the current size, which
/// resets it; `step` gets -1, +1, or `None`. The buttons are `{id}-smaller`,
/// `{id}-bigger` and `{id}-reset`; `disabled` follows the same order, and
/// `keys` are Quick Look actions whose shortcuts the tooltips show.
pub(super) fn size_stepper(
    id: &str,
    label: String,
    disabled: [bool; 3],
    keys: Option<[&dyn Action; 3]>,
    step: impl Fn(Option<f32>, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let step = std::rc::Rc::new(step);
    let tips = ["Smaller Text", "Bigger Text", "Actual Size"];
    let buttons = [
        ("smaller", Some(-1.)),
        ("bigger", Some(1.)),
        ("reset", None),
    ];
    h_flex()
        .gap(ui(2.))
        .children(buttons.into_iter().enumerate().map(|(ix, (name, steps))| {
            let step = step.clone();
            let button = Button::new(SharedString::from(format!("{id}-{name}")))
                .ghost()
                .small()
                .disabled(disabled[ix])
                .on_click(move |_, window, cx| step(steps, window, cx));
            let button = match ix {
                0 => button.child(div().text_size(ui(11.)).child("A")),
                1 => button.child(div().text_size(ui(16.)).child("A")),
                _ => button.label(label.clone()),
            };
            match keys {
                Some(keys) => button.tooltip_with_action(tips[ix], keys[ix], Some(QUICK_LOOK)),
                None => button.tooltip(tips[ix]),
            }
        }))
}

/// A big preview with a caption: the gallery's main area and Quick Look.
pub fn render_preview_large(
    finder: &Finder,
    view: &Entity<Finder>,
    entry: &FileEntry,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let pdf = finder.pdf_view(entry);
    let body = preview_body(
        finder,
        entry,
        ui(220.),
        finder.config.preview_font_size(),
        cx,
    );
    // A PDF shows the system's thumbnail of its first page until hayro has
    // drawn the page.
    let body = match pdf {
        Some(pdf) => finder.render_pdf_page(pdf, view.clone(), body),
        None => body,
    };
    v_flex()
        .size_full()
        .gap(ui(10.))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .rounded(ui(8.))
                .overflow_hidden()
                .child(body),
        )
        .when_some(pdf, |this, pdf| this.child(pdf_controls(pdf)))
        .child(
            h_flex()
                .justify_center()
                .gap(ui(8.))
                .text_size(ui(12.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(entry.name.clone()),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(info_line(finder, entry)),
                ),
        )
        .into_any_element()
}

/// A compact card for the column view's last column.
pub fn render_preview_card(finder: &Finder, entry: &FileEntry, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let row = |label: &'static str, value: String| {
        h_flex()
            .gap(ui(8.))
            .text_size(ui(11.))
            .child(div().w(ui(70.)).text_color(muted).child(label))
            .child(div().flex_1().truncate().child(value))
    };
    v_flex()
        .size_full()
        .gap(ui(12.))
        .child(
            div()
                .h(ui(240.))
                .w_full()
                .rounded(ui(8.))
                .overflow_hidden()
                .child(preview_body(
                    finder,
                    entry,
                    ui(120.),
                    DEFAULT_PREVIEW_FONT_SIZE,
                    cx,
                )),
        )
        .child(
            div()
                .text_size(ui(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(entry.name.clone()),
        )
        .child(row("Kind", entry.kind_label()))
        .when_some(entry.size, |this, size| {
            this.child(row("Size", format::human_size(size)))
        })
        .when_some(
            match finder.preview_of(entry) {
                Preview::Media { duration } => duration,
                _ => None,
            },
            |this, duration| {
                this.child(row(
                    "Duration",
                    fffinder_core::media::format_duration(duration),
                ))
            },
        )
        .when_some(entry.modified, |this, t| {
            this.child(row("Modified", format::human_date(t)))
        })
        .child(row(
            "Where",
            entry.parent().map(super::tildify).unwrap_or_default(),
        ))
        .into_any_element()
}

/// A small button under a PDF that sends `action`, as its keys do.
fn pdf_button(
    id: &'static str,
    icon: impl Into<Icon>,
    tip: &'static str,
    action: Box<dyn Action>,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .icon(icon)
        .tooltip_with_action(tip, action.as_ref(), Some(QUICK_LOOK))
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

/// Under a PDF: the previous page, "Page 2 of 12", and the next page when
/// it has more than one; then zoom out, the zoom, which fits the page
/// again, and zoom in.
fn pdf_controls(pdf: &super::pdf::PdfView) -> impl IntoElement {
    let (page, pages) = pdf.position();
    let zoom = pdf.percent().map_or("Fit".to_string(), |p| format!("{p}%"));
    h_flex()
        .justify_center()
        .gap(ui(6.))
        .text_size(ui(12.))
        .when(pages > 1, |this| {
            this.child(
                pdf_button(
                    "pdf-previous",
                    IconName::ChevronLeft,
                    "Previous Page",
                    Box::new(PreviousPdfPage),
                )
                .disabled(page <= 1),
            )
            .child(format!("Page {page} of {pages}"))
            .child(
                pdf_button(
                    "pdf-next",
                    IconName::ChevronRight,
                    "Next Page",
                    Box::new(NextPdfPage),
                )
                .disabled(page >= pages),
            )
            .child(div().w(ui(12.)))
        })
        .child(
            pdf_button(
                "pdf-zoom-out",
                Lucide::Minus,
                "Zoom Out",
                Box::new(PreviewTextSmaller),
            )
            .disabled(!pdf.can_zoom(-1)),
        )
        .child(
            Button::new("pdf-zoom-fit")
                .ghost()
                .small()
                .label(zoom)
                .tooltip_with_action("Fit Page", &PreviewTextReset, Some(QUICK_LOOK))
                .on_click(|_, window, cx| {
                    window.dispatch_action(PreviewTextReset.boxed_clone(), cx)
                }),
        )
        .child(
            pdf_button(
                "pdf-zoom-in",
                Lucide::Plus,
                "Zoom In",
                Box::new(PreviewTextBigger),
            )
            .disabled(!pdf.can_zoom(1)),
        )
}

fn info_line(finder: &Finder, entry: &FileEntry) -> String {
    let mut parts = vec![entry.kind_label()];
    if let Some((_, pages)) = finder.pdf_view(entry).map(|pdf| pdf.position()) {
        parts.push(match pages {
            1 => "1 page".to_string(),
            n => format!("{n} pages"),
        });
    }
    if let Preview::Media {
        duration: Some(duration),
    } = finder.preview_of(entry)
    {
        parts.push(fffinder_core::media::format_duration(duration));
    }
    if let Some(size) = entry.size {
        parts.push(format::human_size(size));
    }
    if let Some(time) = entry.modified {
        parts.push(format::human_date(time));
    }
    parts.join(" · ")
}

/// The preview itself; `text_size` is in points.
fn preview_body(
    finder: &Finder,
    entry: &FileEntry,
    icon: Pixels,
    text_size: f32,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let pal = palette(cx);
    let centered_icon = |extra: Option<String>| {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(ui(10.))
            .child(file_icon(entry, icon, false, &pal))
            .when_some(extra, |this, text| {
                this.child(
                    div()
                        .text_size(ui(12.))
                        .text_color(theme.muted_foreground)
                        .child(text),
                )
            })
            .into_any_element()
    };
    // QuickLook's picture of the file (macOS), or its icon.
    let picture = |caption: Option<String>| {
        let (shown, muted) = (entry.clone(), theme.muted_foreground);
        large_thumbnail(entry, move || {
            icon_with_caption(&shown, icon, &pal, muted, caption.clone())
        })
    };
    let is_image = entry.kind == FileKind::Image;
    match finder.preview_of(entry) {
        _ if is_image => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(entry.path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .with_fallback({
                        // Formats GPUI can't decode (HEIC, RAW, PSD) use QuickLook's.
                        let (entry, muted) = (entry.clone(), theme.muted_foreground);
                        move || {
                            let shown = entry.clone();
                            large_thumbnail(&entry, move || {
                                icon_with_caption(&shown, icon, &pal, muted, None)
                            })
                        }
                    }),
            )
            .into_any_element(),
        Preview::Loading => v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(Spinner::new().small())
            .into_any_element(),
        Preview::Image | Preview::Unavailable => picture(None),
        Preview::Media { duration } => picture(duration.map(fffinder_core::media::format_duration)),
        Preview::Markdown { text } => div()
            .id(("markdown-preview", super::views::path_id(&entry.path)))
            .size_full()
            .bg(theme.background)
            .px(ui(16.))
            .py(ui(12.))
            .text_size(ui(text_size + 1.5))
            .child(
                TextView::markdown(("markdown", super::views::path_id(&entry.path)), text)
                    .selectable(true)
                    .scrollable(true)
                    .size_full(),
            )
            .into_any_element(),
        Preview::Font { family, .. } => {
            let samples = [
                (text_size + 2., "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
                (text_size + 2., "abcdefghijklmnopqrstuvwxyz"),
                (text_size + 2., "0123456789 !?&@#%*()[]{}"),
                (
                    text_size * 2.4,
                    "The quick brown fox jumps over the lazy dog",
                ),
            ];
            v_flex()
                .id(("font-preview", super::views::path_id(&entry.path)))
                .size_full()
                .bg(theme.background)
                .p(ui(16.))
                .gap(ui(10.))
                .overflow_y_scrollbar()
                .child(
                    div()
                        .text_size(ui(12.))
                        .text_color(theme.muted_foreground)
                        .child(family.clone()),
                )
                .children(samples.into_iter().map(|(size, sample)| {
                    div()
                        .font_family(family.clone())
                        .text_size(ui(size))
                        .child(sample)
                }))
                .into_any_element()
        }
        Preview::Folder { items } => centered_icon(Some(if items == 1 {
            "1 item".into()
        } else {
            format!("{items} items")
        })),
        Preview::Text { text, truncated } => div()
            .id(("text-preview", super::views::path_id(&entry.path)))
            .size_full()
            .bg(theme.muted)
            .p(ui(12.))
            .overflow_y_scrollbar()
            .font_family(theme.mono_font_family.clone())
            .text_size(ui(text_size))
            .child(text)
            .when(truncated, |this| {
                this.child(
                    div()
                        .pt(ui(8.))
                        .text_color(theme.muted_foreground)
                        .child("… (preview truncated)"),
                )
            })
            .into_any_element(),
        Preview::Code {
            text,
            mut spans,
            comments,
            truncated,
        } => div()
            .id(("code-preview", super::views::path_id(&entry.path)))
            .size_full()
            .bg(theme.muted)
            .p(ui(12.))
            .overflow_y_scrollbar()
            .font_family(theme.mono_font_family.clone())
            .text_size(ui(text_size))
            .child({
                let comment = HighlightStyle {
                    color: Some(theme.muted_foreground),
                    ..Default::default()
                };
                spans.extend(comments.into_iter().map(|range| (range, comment)));
                spans.sort_by_key(|(range, _)| range.start);
                StyledText::new(text).with_highlights(spans)
            })
            .when(truncated, |this| {
                this.child(
                    div()
                        .pt(ui(8.))
                        .text_color(theme.muted_foreground)
                        .child("… (preview truncated)"),
                )
            })
            .into_any_element(),
        Preview::Table {
            columns,
            rows,
            footer,
        } => {
            let header_bg = theme.muted;
            let border = theme.border;
            let ncols = columns.len().max(1);
            // Columns widen with the text.
            let cell = 140. * text_size / DEFAULT_PREVIEW_FONT_SIZE;
            let width = ui((ncols as f32 * cell).max(300.));
            v_flex()
                .size_full()
                .bg(theme.background)
                .border_1()
                .border_color(border)
                .rounded(ui(8.))
                .overflow_hidden()
                .child(
                    div()
                        .id(("table-preview", super::views::path_id(&entry.path)))
                        .flex_1()
                        .min_h_0()
                        .overflow_scrollbar()
                        .child(
                            v_flex()
                                .min_w(width)
                                .text_size(ui(text_size))
                                .child(
                                    h_flex()
                                        .bg(header_bg)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .children(
                                            columns.iter().map(|c| table_cell(c.clone(), cell)),
                                        ),
                                )
                                .children(rows.iter().enumerate().map(|(ix, row)| {
                                    h_flex()
                                        .when(ix % 2 == 1, |this| this.bg(pal.stripe))
                                        .children(row.iter().map(|c| table_cell(c.clone(), cell)))
                                })),
                        ),
                )
                .when_some(footer, |this, footer| {
                    this.child(
                        h_flex()
                            .px(ui(10.))
                            .py(ui(4.))
                            .border_t_1()
                            .border_color(border)
                            .text_size(ui(10.5))
                            .text_color(theme.muted_foreground)
                            .child(footer),
                    )
                })
                .into_any_element()
        }
    }
}

/// A file's icon, centered, with an optional line under it.
fn icon_with_caption(
    entry: &FileEntry,
    icon: Pixels,
    pal: &crate::theme::Palette,
    muted: Hsla,
    caption: Option<String>,
) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(ui(10.))
        .child(file_icon(entry, icon, false, pal))
        .when_some(caption, |this, text| {
            this.child(div().text_size(ui(12.)).text_color(muted).child(text))
        })
        .into_any_element()
}

/// QuickLook's picture of `entry` filling the space (macOS), or `fallback`
/// while it loads and when there is none.
fn large_thumbnail(
    entry: &FileEntry,
    fallback: impl Fn() -> AnyElement + Clone + 'static,
) -> AnyElement {
    if !crate::thumbnails::wanted(entry) {
        return fallback();
    }
    img(crate::thumbnails::image(entry, 1024))
        .size_full()
        .object_fit(ObjectFit::Contain)
        .with_loading(fallback.clone())
        .with_fallback(fallback)
        .into_any_element()
}

fn table_cell(text: SharedString, width: f32) -> impl IntoElement {
    div()
        .w(ui(width))
        .flex_shrink_0()
        .px(ui(8.))
        .py(ui(3.))
        .truncate()
        .child(text)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would replace the standard one.
    use super::{Preview, load_preview};
    use crate::nushell;
    use fffinder_core::FileEntry;
    use fffinder_core::ops::Progress;

    /// Runs only where `nu` is installed.
    #[test]
    fn nushell_scripts_are_highlighted() {
        let Some(nu) = nushell::find_nu(None) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("fff-highlight-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("demo.nu");
        std::fs::write(
            &script,
            "# Say hi\ndef greet [name: string] {\n  $\"hi ($name) #1\"\n}\n",
        )
        .unwrap();
        let entry = FileEntry::from_path(&script).unwrap();
        match load_preview(&entry, Some(&nu), false) {
            Preview::Code {
                text,
                spans,
                comments,
                ..
            } => {
                assert!(text.starts_with("# Say hi"), "{text}");
                assert_eq!(comments.len(), 1, "the comment is found");
                assert_eq!(&text[comments[0].clone()], "# Say hi");
                assert!(!text.contains('\u{1b}'));
                assert!(!spans.is_empty(), "nu-highlight colored something");
            }
            _ => panic!("expected highlighted code"),
        }
        let plain = dir.join("notes.txt");
        std::fs::write(&plain, "def greet").unwrap();
        let entry = FileEntry::from_path(&plain).unwrap();
        assert!(matches!(
            load_preview(&entry, Some(&nu), false),
            Preview::Text { .. }
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn previews_by_kind() {
        let dir = std::env::temp_dir().join(format!("fff-previews-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pack")).unwrap();
        let write = |name: &str, text: &str| {
            std::fs::write(dir.join(name), text).unwrap();
            FileEntry::from_path(dir.join(name)).unwrap()
        };
        let load = |entry: &FileEntry| load_preview(entry, None, false);

        let code = write("main.py", "def main():\n    return 1\n");
        assert!(matches!(load(&code), Preview::Code { spans, .. } if spans.len() > 1));
        let notes = write("notes.md", "# Title\n\nSome *text*.\n");
        assert!(matches!(load(&notes), Preview::Markdown { .. }));
        assert!(matches!(
            load(&write("plain.txt", "words")),
            Preview::Text { .. }
        ));

        std::fs::write(dir.join("pack/a.txt"), "a").unwrap();
        let zip = dir.join("pack.zip");
        fffinder_core::archive::compress(&[dir.join("pack")], &zip, &Progress::default()).unwrap();
        match load(&FileEntry::from_path(&zip).unwrap()) {
            Preview::Table { rows, footer, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(footer.as_deref(), Some("2 items"));
            }
            _ => panic!("an archive lists its contents"),
        }

        // 1 s of 8 kHz, 8-bit mono WAV.
        let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(b"\x01\0\x08\0data");
        wav.extend_from_slice(&8000u32.to_le_bytes());
        std::fs::write(dir.join("tone.wav"), &wav).unwrap();
        let tone = FileEntry::from_path(dir.join("tone.wav")).unwrap();
        assert!(matches!(
            load(&tone),
            Preview::Media { duration: Some(d) } if d.as_secs() == 1
        ));

        let fonts = [
            "/System/Library/Fonts/Supplemental/Georgia.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
        ];
        if let Some(font) = fonts.iter().find(|f| std::path::Path::new(f).is_file()) {
            match load(&FileEntry::from_path(font).unwrap()) {
                Preview::Font { family, data } => {
                    assert!(!family.is_empty() && data.is_some());
                }
                _ => panic!("a font shows itself"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
