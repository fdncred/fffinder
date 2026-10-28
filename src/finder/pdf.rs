//! PDFs in the gallery and Quick Look, read as in a PDF viewer. The pages
//! scroll by in one column. The toolbar above them goes to the previous or
//! next page and zooms, with a menu for Fit Width, Fit Page, Actual Size,
//! and set sizes; pinching, Cmd+scrolling, and Cmd+Plus, Minus, and 0 zoom
//! too. Pages are drawn with hayro (see `fffinder_core::pdf`) as they come
//! into view, at the size they're shown so text stays sharp, and only the
//! pages in view and their neighbors are kept.

use super::Finder;
use crate::actions::{
    NextPdfPage, PreviewTextBigger, PreviewTextReset, PreviewTextSmaller, PreviousPdfPage,
    QUICK_LOOK,
};
use crate::theme::ui;
use fffinder_core::pdf::{PageImage, PdfDocument};
use fffinder_core::{FileEntry, FileKind, ViewMode};
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, ElementExt as _, Icon, IconName, Sizable as _,
    VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::*;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// How big the pages are shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// The widest page fills the width.
    FitWidth,
    /// The biggest page fits whole.
    FitPage,
    /// Pixels per point: 1.0 is the printed size.
    Scale(f32),
}

/// The zoom menu's sizes, which zooming in and out steps through.
const STEPS: [f32; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// The smallest and biggest zoom, pinching included.
const SCALES: (f32, f32) = (0.1, 8.0);
/// Space above each page, and below the last.
const GAP: f32 = 12.;
/// The most pixels a drawn page may have (64 MB of memory).
const PIXEL_BUDGET: f32 = 16_000_000.;
/// PDFs bigger than this, mostly scans, aren't read into memory to draw
/// them; they keep the system's thumbnail.
const PDF_LIMIT: u64 = 200_000_000;
/// How long after the last pinch or Cmd+scroll the pages are drawn again.
const GESTURE_SETTLE: Duration = Duration::from_millis(150);

/// Where a PDF is shown. The gallery and Quick Look can show the same PDF
/// at once, each scrolled and sized on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Gallery,
    QuickLook,
}

impl Surface {
    const ALL: [Surface; 2] = [Surface::Gallery, Surface::QuickLook];
}

/// The PDF as one surface shows it.
struct Shown {
    /// The space for the pages, and the display's pixels per point.
    frame: Option<(Size<Pixels>, f32)>,
    scroll: VirtualListScrollHandle,
    /// The pages in view when last drawn.
    visible: Range<usize>,
    /// Worked out when the frame or zoom changes rather than every frame.
    layout: Option<Layout>,
}

/// How a surface lays the pages out.
#[derive(Clone)]
struct Layout {
    /// Pixels per point.
    scale: f32,
    /// Each page's line height, as the list takes them.
    lines: Rc<Vec<Size<Pixels>>>,
    /// The column's width: the widest page and its gaps, or the view's.
    width: Pixels,
}

impl Shown {
    fn new() -> Shown {
        Shown {
            frame: None,
            scroll: VirtualListScrollHandle::new(),
            visible: 0..0,
            layout: None,
        }
    }
}

/// A drawn page, and how many pixels its longer side has.
struct Drawn {
    image: Arc<RenderImage>,
    pixels: u32,
}

/// The PDF the large previews show.
pub(super) struct PdfView {
    path: PathBuf,
    modified: Option<SystemTime>,
    doc: Option<Arc<PdfDocument>>,
    /// Each page's size in points.
    pages: Vec<(f32, f32)>,
    zoom: Zoom,
    /// Pinching or Cmd+scrolling: pages stretch, and are drawn again once
    /// it settles. The count tells the last gesture from earlier ones.
    gesture: Option<u64>,
    gestures: u64,
    shown: [Shown; 2],
    drawn: HashMap<usize, Drawn>,
    /// Pages being drawn, at how many pixels. Dropping a GPUI task cancels
    /// it, so pages scrolled past aren't drawn.
    drawing: HashMap<usize, (u32, Task<()>)>,
    _opening: Option<Task<()>>,
}

impl PdfView {
    fn widest(&self) -> f32 {
        self.pages.iter().map(|page| page.0).fold(1., f32::max)
    }

    fn tallest(&self) -> f32 {
        self.pages.iter().map(|page| page.1).fold(1., f32::max)
    }

    /// Pixels per point on `surface`.
    fn scale(&self, surface: Surface) -> Option<f32> {
        let (frame, _) = self.shown[surface as usize].frame?;
        let width = (f32::from(frame.width) - 2. * GAP).max(1.);
        let height = (f32::from(frame.height) - 2. * GAP).max(1.);
        let scale = match self.zoom {
            Zoom::FitWidth => width / self.widest(),
            Zoom::FitPage => (width / self.widest()).min(height / self.tallest()),
            Zoom::Scale(scale) => scale,
        };
        Some(scale.clamp(SCALES.0, SCALES.1))
    }

    /// A page's line in the list: the gap above it, the page, and below
    /// the last page, one more gap.
    fn line_height(&self, page: usize, scale: f32) -> f32 {
        let last = page + 1 == self.pages.len();
        self.pages[page].1 * scale + GAP * if last { 2. } else { 1. }
    }

    fn line_top(&self, page: usize, scale: f32) -> f32 {
        (0..page).map(|page| self.line_height(page, scale)).sum()
    }

    /// The page at `y` down the list.
    fn page_at(&self, y: f32, scale: f32) -> usize {
        let mut bottom = 0.;
        for page in 0..self.pages.len() {
            bottom += self.line_height(page, scale);
            if y < bottom {
                return page;
            }
        }
        self.pages.len().saturating_sub(1)
    }

    /// The page in view on `surface`, from 0: the one a third of the way
    /// down.
    fn current_page(&self, surface: Surface) -> usize {
        let shown = &self.shown[surface as usize];
        let (Some((frame, _)), Some(scale)) = (shown.frame, self.scale(surface)) else {
            return 0;
        };
        let top = -f32::from(shown.scroll.base_handle().offset().y);
        self.page_at(top + f32::from(frame.height) / 3., scale)
    }

    pub(super) fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page in view on `surface`, from 1, and how many there are.
    pub(super) fn position(&self, surface: Surface) -> (usize, usize) {
        (self.current_page(surface) + 1, self.pages.len())
    }

    /// The zoom on `surface`, as a percentage of the printed size.
    pub(super) fn percent(&self, surface: Surface) -> Option<u32> {
        Some((self.scale(surface)? * 100.).round() as u32)
    }

    fn zoom_label(&self, surface: Surface) -> String {
        match (self.zoom, self.percent(surface)) {
            (Zoom::FitWidth, _) => "Fit Width".into(),
            (Zoom::FitPage, _) => "Fit Page".into(),
            (Zoom::Scale(_), Some(percent)) => format!("{percent}%"),
            (Zoom::Scale(scale), None) => format!("{}%", (scale * 100.).round()),
        }
    }

    /// Work out `surface`'s line heights and column width again, after its
    /// frame or the zoom changed.
    fn relayout(&mut self, surface: Surface) {
        let layout = self
            .scale(surface)
            .zip(self.shown[surface as usize].frame)
            .map(|(scale, (frame, _))| {
                let lines = (0..self.pages.len())
                    .map(|page| size(px(0.), px(self.line_height(page, scale))))
                    .collect();
                let width = px(self.widest() * scale + 2. * GAP).max(frame.width);
                Layout {
                    scale,
                    lines: Rc::new(lines),
                    width,
                }
            });
        self.shown[surface as usize].layout = layout;
    }

    /// Pixels the longer side of `page` needs at `scale` on a display with
    /// `density` pixels per point, within the budget, in steps of 64.
    fn pixels_for(&self, page: usize, scale: f32, density: f32) -> u32 {
        let (width, height) = self.pages[page];
        let (long, short) = (width.max(height) * scale, width.min(height).max(1.) * scale);
        let most = (PIXEL_BUDGET * long / short).sqrt();
        let pixels = (long * density).min(most).max(64.);
        (pixels / 64.).ceil() as u32 * 64
    }
}

/// The next zoom step above or below `scale`. Past the last step, a
/// quarter more or less, within the smallest and biggest zoom.
fn step_scale(scale: f32, up: bool) -> f32 {
    let next = match up {
        true => STEPS.iter().find(|step| **step > scale * 1.01),
        false => STEPS.iter().rev().find(|step| **step < scale * 0.99),
    };
    let beyond = match up {
        true => scale * 1.25,
        false => scale / 1.25,
    };
    next.copied().unwrap_or(beyond).clamp(SCALES.0, SCALES.1)
}

/// GPUI keeps images as BGRA.
fn to_render_image(page: PageImage) -> Option<Arc<RenderImage>> {
    let mut pixels = page.rgba;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(page.width, page.height, pixels)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Free a page's texture once no window is being drawn.
fn drop_later(image: Arc<RenderImage>, cx: &mut App) {
    cx.defer(move |cx| cx.drop_image(image, None));
}

impl Finder {
    /// Start showing `entry` as a PDF if it's one that isn't shown already.
    /// It opens at its first page, fitted to the width.
    pub(super) fn ensure_pdf(&mut self, entry: &FileEntry, cx: &mut Context<Self>) {
        let shown = self
            .pdf
            .as_ref()
            .is_some_and(|pdf| pdf.path == entry.path && pdf.modified == entry.modified);
        if entry.kind != FileKind::Pdf || shown {
            return;
        }
        if let Some(old) = self.pdf.take() {
            old.drawn
                .into_values()
                .for_each(|drawn| drop_later(drawn.image, cx));
        }
        if entry.size.is_some_and(|size| size > PDF_LIMIT) {
            return;
        }
        let path = entry.path.clone();
        let opening = cx.spawn(async move |this, cx| {
            let opened = cx
                .background_spawn(async move {
                    let doc = PdfDocument::open(&path).ok()?;
                    let pages = doc.page_sizes();
                    (!pages.is_empty()).then(|| (Arc::new(doc), pages))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(pdf) = this.pdf.as_mut()
                    && let Some((doc, pages)) = opened
                {
                    pdf.doc = Some(doc);
                    pdf.pages = pages;
                    for surface in Surface::ALL {
                        pdf.relayout(surface);
                    }
                    cx.notify();
                }
            })
            .ok();
            // Quick Look is drawn in its own window; redraw it too.
            cx.refresh();
        });
        self.pdf = Some(PdfView {
            path: entry.path.clone(),
            modified: entry.modified,
            doc: None,
            pages: Vec::new(),
            zoom: Zoom::FitWidth,
            gesture: None,
            gestures: 0,
            shown: [Shown::new(), Shown::new()],
            drawn: HashMap::new(),
            drawing: HashMap::new(),
            _opening: Some(opening),
        });
    }

    /// The PDF view for `entry`, when it's open.
    pub(super) fn pdf_view(&self, entry: &FileEntry) -> Option<&PdfView> {
        self.pdf
            .as_ref()
            .filter(|pdf| pdf.path == entry.path && pdf.doc.is_some())
    }

    /// Where the PDF is being read: Quick Look while it's open.
    fn pdf_surface(&self) -> Surface {
        match self.quick_look_open {
            true => Surface::QuickLook,
            false => Surface::Gallery,
        }
    }

    /// The page in view where the PDF is read, from 1, and how many pages
    /// there are.
    pub fn shown_pdf_page(&self) -> Option<(usize, usize)> {
        let surface = self.pdf_surface();
        self.pdf
            .as_ref()
            .filter(|pdf| pdf.doc.is_some())
            .map(|pdf| pdf.position(surface))
    }

    /// The zoom where the PDF is read, as a percentage of its printed size.
    pub fn shown_pdf_zoom(&self) -> Option<u32> {
        let surface = self.pdf_surface();
        self.pdf.as_ref().and_then(|pdf| pdf.percent(surface))
    }

    /// How many pages are drawn, and how many are waiting to be.
    pub fn pdf_pages_for_test(&self) -> (usize, usize) {
        self.pdf.as_ref().map_or((0, 0), |pdf| {
            let waiting = pdf
                .drawing
                .keys()
                .filter(|page| !pdf.drawn.contains_key(page));
            (pdf.drawn.len(), waiting.count())
        })
    }

    /// `surface` stopped showing the PDF: its pages needn't be kept.
    pub(super) fn forget_pdf_surface(&mut self, surface: Surface) {
        if let Some(pdf) = self.pdf.as_mut() {
            pdf.shown[surface as usize] = Shown::new();
        }
    }

    /// `surface` was laid out at `frame`; fit the pages to it. Returns
    /// whether that changed anything.
    fn set_pdf_frame(
        &mut self,
        surface: Surface,
        frame: Size<Pixels>,
        density: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pdf) = self.pdf.as_mut() else {
            return false;
        };
        let shown = &mut pdf.shown[surface as usize];
        if shown.frame == Some((frame, density)) {
            return false;
        }
        shown.frame = Some((frame, density));
        pdf.relayout(surface);
        cx.notify();
        true
    }

    /// Draw the pages in view and their neighbors, at the size they're
    /// shown wherever they're biggest, and let go of the rest. It runs once
    /// a frame, after the lists have said which pages are in view.
    fn want_pdf_pages(&mut self, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_mut() else {
            return;
        };
        let Some(doc) = pdf.doc.clone() else {
            return;
        };
        if pdf.gesture.is_some() {
            return;
        }
        let mut wanted: HashMap<usize, u32> = HashMap::new();
        for surface in Surface::ALL {
            let shown = &pdf.shown[surface as usize];
            let (Some((_, density)), Some(scale)) = (shown.frame, pdf.scale(surface)) else {
                continue;
            };
            let near =
                shown.visible.start.saturating_sub(1)..(shown.visible.end + 1).min(pdf.pages.len());
            for page in near {
                let pixels = pdf.pixels_for(page, scale, density);
                let most = wanted.entry(page).or_default();
                *most = (*most).max(pixels);
            }
        }
        // A drawing between 0.9 and 2 times the size needed will do.
        let enough = |had: u32, need: u32| had as f32 >= need as f32 * 0.9 && had <= need * 2;
        for (_, drawn) in pdf.drawn.extract_if(|page, _| !wanted.contains_key(page)) {
            drop_later(drawn.image, cx);
        }
        let drawn = &pdf.drawn;
        pdf.drawing.retain(|page, (pixels, _)| {
            wanted.get(page).is_some_and(|need| enough(*pixels, *need))
                && !drawn.get(page).is_some_and(|d| enough(d.pixels, *pixels))
        });
        for (page, pixels) in wanted {
            let done = pdf
                .drawn
                .get(&page)
                .is_some_and(|d| enough(d.pixels, pixels));
            if done || pdf.drawing.contains_key(&page) {
                continue;
            }
            let (doc, path) = (doc.clone(), pdf.path.clone());
            let task = cx.spawn(async move |this, cx| {
                let image = cx
                    .background_spawn(async move {
                        doc.render_page(page, pixels).ok().and_then(to_render_image)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(pdf) = this.pdf.as_mut().filter(|pdf| pdf.path == path)
                        && let Some(image) = image
                    {
                        if let Some(old) = pdf.drawn.insert(page, Drawn { image, pixels }) {
                            drop_later(old.image, cx);
                        }
                        cx.notify();
                    }
                })
                .ok();
                cx.refresh();
            });
            pdf.drawing.insert(page, (pixels, task));
        }
    }

    /// Go `by` pages forward or back on `surface`.
    pub(super) fn step_pdf_page(&mut self, by: isize, surface: Surface, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_ref().filter(|pdf| pdf.doc.is_some()) else {
            return;
        };
        let page = pdf
            .current_page(surface)
            .saturating_add_signed(by)
            .min(pdf.pages.len().saturating_sub(1));
        pdf.shown[surface as usize]
            .scroll
            .scroll_to_item(page, ScrollStrategy::Top);
        cx.notify();
    }

    /// Show the pages at `zoom`, keeping what's at the top of each view
    /// where it is.
    pub(super) fn set_pdf_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_mut() else {
            return;
        };
        let anchors: Vec<(Surface, usize, f32, Pixels)> = Surface::ALL
            .into_iter()
            .filter_map(|surface| {
                let scale = pdf.scale(surface)?;
                let offset = pdf.shown[surface as usize].scroll.base_handle().offset();
                let top = -f32::from(offset.y);
                let page = pdf.page_at(top, scale);
                let into = (top - pdf.line_top(page, scale)) / pdf.line_height(page, scale);
                Some((surface, page, into, offset.x))
            })
            .collect();
        pdf.zoom = zoom;
        for surface in Surface::ALL {
            pdf.relayout(surface);
        }
        for (surface, page, into, x) in anchors {
            let Some(scale) = pdf.scale(surface) else {
                continue;
            };
            let top = pdf.line_top(page, scale) + into * pdf.line_height(page, scale);
            let scroll = pdf.shown[surface as usize].scroll.base_handle();
            scroll.set_offset(point(x, px(-top)));
        }
        cx.notify();
    }

    /// Zoom in or out a step on `surface`, or fit the width with `None`.
    pub(super) fn zoom_pdf(&mut self, by: Option<isize>, surface: Surface, cx: &mut Context<Self>) {
        let Some(scale) = self.pdf.as_ref().and_then(|pdf| pdf.scale(surface)) else {
            return;
        };
        let zoom = match by {
            Some(by) => Zoom::Scale(step_scale(scale, by > 0)),
            None => Zoom::FitWidth,
        };
        self.set_pdf_zoom(zoom, cx);
    }

    /// A pinch or Cmd+scroll on `surface`, zooming by `factor`. Pages
    /// stretch as it goes and are drawn again once it settles.
    fn zoom_gesture(&mut self, surface: Surface, factor: f32, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_mut() else {
            return;
        };
        let Some(scale) = pdf.scale(surface) else {
            return;
        };
        pdf.gestures += 1;
        let gesture = pdf.gestures;
        pdf.gesture = Some(gesture);
        self.set_pdf_zoom(Zoom::Scale((scale * factor).clamp(SCALES.0, SCALES.1)), cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GESTURE_SETTLE).await;
            this.update(cx, |this, cx| {
                if let Some(pdf) = this.pdf.as_mut()
                    && pdf.gesture == Some(gesture)
                {
                    pdf.gesture = None;
                    cx.notify();
                }
            })
            .ok();
            cx.refresh();
        })
        .detach();
    }

    /// Whether the gallery or Quick Look shows a big preview, which the PDF
    /// keys act on.
    fn shows_large_preview(&self) -> bool {
        self.quick_look_open || self.effective_view() == ViewMode::Gallery
    }

    /// Cmd+Plus, Minus, and 0 on the preview on `surface`: zoom a PDF, or
    /// change the text size of other previews.
    pub(super) fn step_preview_size(
        &mut self,
        steps: Option<f32>,
        surface: Surface,
        cx: &mut Context<Self>,
    ) {
        let pdf_shown = self
            .primary_entry()
            .is_some_and(|entry| self.pdf_view(&entry).is_some());
        match (pdf_shown, steps) {
            (true, steps) => self.zoom_pdf(steps.map(|s| s as isize), surface, cx),
            (false, Some(steps)) => self.step_preview_font_size(steps, cx),
            (false, None) => self.set_preview_font_size(None, cx),
        }
    }

    pub(super) fn preview_bigger(
        &mut self,
        _: &PreviewTextBigger,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_key(Some(1.), cx);
    }

    pub(super) fn preview_smaller(
        &mut self,
        _: &PreviewTextSmaller,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_key(Some(-1.), cx);
    }

    pub(super) fn preview_reset(
        &mut self,
        _: &PreviewTextReset,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_key(None, cx);
    }

    /// The preview size keys in the gallery; elsewhere they're left alone.
    fn preview_key(&mut self, steps: Option<f32>, cx: &mut Context<Self>) {
        match self.shows_large_preview() {
            true => self.step_preview_size(steps, Surface::Gallery, cx),
            false => cx.propagate(),
        }
    }

    pub(super) fn next_pdf_page(
        &mut self,
        _: &NextPdfPage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.shows_large_preview() {
            true => self.step_pdf_page(1, Surface::Gallery, cx),
            false => cx.propagate(),
        }
    }

    pub(super) fn previous_pdf_page(
        &mut self,
        _: &PreviousPdfPage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.shows_large_preview() {
            true => self.step_pdf_page(-1, Surface::Gallery, cx),
            false => cx.propagate(),
        }
    }

    /// The PDF on `surface`: the toolbar, then the pages in a column that
    /// scrolls both ways. `fallback` shows until the PDF is laid out.
    pub(super) fn render_pdf(
        &self,
        pdf: &PdfView,
        view: &Entity<Finder>,
        surface: Surface,
        fallback: AnyElement,
        cx: &App,
    ) -> AnyElement {
        let shown = &pdf.shown[surface as usize];
        let pages = match &shown.layout {
            Some(layout) => v_virtual_list(
                view.clone(),
                "pdf-list",
                layout.lines.clone(),
                move |this: &mut Finder, range: Range<usize>, _, cx| {
                    // The list also asks for its first line alone, to measure
                    // it; the range it draws comes last.
                    if let Some(pdf) = this.pdf.as_mut() {
                        pdf.shown[surface as usize].visible = range.clone();
                    }
                    range
                        .map(|page| this.render_pdf_line(page, surface, cx))
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&shown.scroll)
            .size_full()
            .into_any_element(),
            None => fallback,
        };
        let (measure, pinch) = (view.clone(), view.clone());
        v_flex()
            .size_full()
            .child(pdf_toolbar(pdf, view, surface, cx))
            .child(
                div()
                    .id("pdf-pages")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    .bg(cx.theme().muted)
                    .child(pages)
                    .vertical_scrollbar(&shown.scroll)
                    .horizontal_scrollbar(&shown.scroll)
                    .on_pinch(move |event, _, cx| {
                        let factor = 1. + event.delta;
                        pinch.update(cx, |this, cx| this.zoom_gesture(surface, factor, cx));
                    })
                    // After the list's own layout, so the pages in view are known.
                    .on_prepaint(move |bounds, window, cx| {
                        let density = window.scale_factor();
                        let changed = measure.update(cx, |this, cx| {
                            let changed = this.set_pdf_frame(surface, bounds.size, density, cx);
                            this.want_pdf_pages(cx);
                            changed
                        });
                        if changed {
                            window.refresh();
                        }
                    })
                    .test_support(),
            )
            .into_any_element()
    }

    /// One page's line: the page, centered in the column, drawn or blank
    /// while it's drawn. Cmd+scrolling over it zooms instead of scrolling.
    fn render_pdf_line(&self, page: usize, surface: Surface, cx: &mut Context<Self>) -> AnyElement {
        let Some(pdf) = self.pdf.as_ref() else {
            return div().into_any_element();
        };
        let Some(Layout { scale, width, .. }) = pdf.shown[surface as usize].layout.clone() else {
            return div().into_any_element();
        };
        let (page_width, page_height) = pdf.pages[page];
        let sheet_size = size(px(page_width * scale), px(page_height * scale));
        let sheet = match pdf.drawn.get(&page) {
            Some(drawn) => img(drawn.image.clone())
                .w(sheet_size.width)
                .h(sheet_size.height)
                .object_fit(ObjectFit::Fill)
                .into_any_element(),
            None => div()
                .w(sheet_size.width)
                .h(sheet_size.height)
                .bg(white())
                .into_any_element(),
        };
        let view = cx.entity();
        div()
            .id(("pdf-page", page))
            .w(width)
            .h(px(pdf.line_height(page, scale)))
            .pt(px(GAP))
            .flex()
            .justify_center()
            .child(div().flex_shrink_0().shadow_sm().child(sheet))
            .on_scroll_wheel(move |event, window, cx| {
                if !event.modifiers.secondary() {
                    return;
                }
                let lines = event.delta.pixel_delta(window.line_height()).y;
                let factor = (1. + f32::from(lines) * 0.01).clamp(0.5, 2.);
                view.update(cx, |this, cx| this.zoom_gesture(surface, factor, cx));
                cx.stop_propagation();
            })
            .into_any_element()
    }
}

/// A small toolbar button that sends `action`, as its keys do.
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

/// Above the pages: the previous page, "Page 2 of 12", and the next page;
/// then zoom out, the zoom, which opens the zoom menu, and zoom in.
fn pdf_toolbar(
    pdf: &PdfView,
    view: &Entity<Finder>,
    surface: Surface,
    cx: &App,
) -> impl IntoElement {
    let (page, pages) = pdf.position(surface);
    let scale = pdf.scale(surface).unwrap_or(1.);
    let menu_view = view.clone();
    h_flex()
        .flex_shrink_0()
        .justify_center()
        .gap(ui(4.))
        .px(ui(8.))
        .py(ui(4.))
        .border_b_1()
        .border_color(cx.theme().border)
        .text_size(ui(12.))
        .child(
            pdf_button(
                "pdf-previous",
                IconName::ChevronLeft,
                "Previous Page",
                Box::new(PreviousPdfPage),
            )
            .disabled(page <= 1),
        )
        .child(
            div()
                .min_w(ui(96.))
                .text_center()
                .child(format!("Page {page} of {pages}")),
        )
        .child(
            pdf_button(
                "pdf-next",
                IconName::ChevronRight,
                "Next Page",
                Box::new(NextPdfPage),
            )
            .disabled(page >= pages),
        )
        .child(div().w(ui(16.)))
        .child(
            pdf_button(
                "pdf-zoom-out",
                Lucide::Minus,
                "Zoom Out",
                Box::new(PreviewTextSmaller),
            )
            .disabled(scale <= SCALES.0),
        )
        .child(
            Button::new("pdf-zoom")
                .ghost()
                .small()
                .min_w(ui(96.))
                .label(pdf.zoom_label(surface))
                .dropdown_caret(true)
                .tooltip("Zoom")
                .dropdown_menu(move |menu, window, cx| zoom_menu(&menu_view, menu, window, cx)),
        )
        .child(
            pdf_button(
                "pdf-zoom-in",
                Lucide::Plus,
                "Zoom In",
                Box::new(PreviewTextBigger),
            )
            .disabled(scale >= SCALES.1),
        )
}

/// The zoom menu: Fit Width, Fit Page, Actual Size, and set sizes.
fn zoom_menu(
    view: &Entity<Finder>,
    mut menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let current = view.read(cx).pdf.as_ref().map(|pdf| pdf.zoom);
    let fits = [
        ("Fit Width", Zoom::FitWidth),
        ("Fit Page", Zoom::FitPage),
        ("Actual Size", Zoom::Scale(1.)),
    ];
    let sizes = STEPS
        .into_iter()
        .filter(|step| *step != 1.)
        .map(|step| (format!("{}%", (step * 100.).round()), Zoom::Scale(step)));
    let choices = fits
        .into_iter()
        .map(|(label, zoom)| (label.to_string(), zoom))
        .chain(sizes);
    for (ix, (label, zoom)) in choices.enumerate() {
        if ix == fits.len() {
            menu = menu.separator();
        }
        menu = menu.item(
            PopupMenuItem::new(label)
                .checked(current == Some(zoom))
                .on_click(
                    window.listener_for(view, move |this: &mut Finder, _: &ClickEvent, _, cx| {
                        this.set_pdf_zoom(zoom, cx)
                    }),
                ),
        );
    }
    menu.min_w(ui(140.))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would replace the standard one.
    use super::{PageImage, step_scale, to_render_image};

    #[test]
    fn pages_become_bgra_images() {
        let page = PageImage {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let image = to_render_image(page).unwrap();
        assert_eq!(
            image.as_bytes(0),
            Some(&[0, 0, 255, 255, 255, 0, 0, 255][..])
        );
        let short = PageImage {
            width: 2,
            height: 2,
            rgba: vec![0; 4],
        };
        assert!(
            to_render_image(short).is_none(),
            "too few pixels for the size"
        );
    }

    #[test]
    fn zoom_steps_from_any_scale() {
        assert_eq!(step_scale(1., true), 1.25);
        assert_eq!(step_scale(1., false), 0.75);
        assert_eq!(step_scale(1.6, true), 2., "from a fitted size");
        assert_eq!(step_scale(1.6, false), 1.5);
        assert_eq!(step_scale(4., true), 5., "past the steps, a quarter more");
        assert_eq!(step_scale(7., true), 8., "up to the biggest zoom");
        assert_eq!(step_scale(0.2, false), 0.16);
        assert_eq!(step_scale(0.1, false), 0.1, "down to the smallest");
    }
}
