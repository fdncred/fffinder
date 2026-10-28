//! PDFs page by page in the gallery and Quick Look. The page is drawn with
//! hayro (see `fffinder_core::pdf`) to fit the preview, can be zoomed in and
//! scrolled, and is drawn again for each zoom so text stays sharp. The
//! buttons under it, or Page Up, Page Down, and Cmd+Plus, Minus, and 0,
//! step through the pages and zoom. Only the PDF on screen is kept, with one
//! page drawn, so a long document doesn't fill memory.

use super::Finder;
use crate::actions::{
    NextPdfPage, PreviewTextBigger, PreviewTextReset, PreviewTextSmaller, PreviousPdfPage,
};
use fffinder_core::pdf::{PageImage, PdfDocument};
use fffinder_core::{FileEntry, FileKind, ViewMode};
use gpui_kit::component::ElementExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// Zoom levels, as multiples of the whole page fitting the preview.
const ZOOMS: [f32; 7] = [1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0];
/// The most pixels a drawn page may have (64 MB of memory).
const PIXEL_BUDGET: f32 = 16_000_000.;
/// PDFs bigger than this, mostly scans, aren't read into memory to draw
/// them; they keep the system's thumbnail.
const PDF_LIMIT: u64 = 200_000_000;

/// Where a PDF is shown. The gallery and Quick Look can show the same PDF
/// at once, each laid out at its own size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Gallery,
    QuickLook,
}

/// The PDF the large preview shows.
pub(super) struct PdfView {
    path: PathBuf,
    modified: Option<SystemTime>,
    doc: Option<Arc<PdfDocument>>,
    pages: usize,
    /// The page shown, from 0. It changes before the page is drawn, and the
    /// previous drawing stays up until then.
    page: usize,
    /// The page's size in points.
    page_size: (f32, f32),
    /// Which of `ZOOMS`.
    zoom: usize,
    /// Each surface's preview size when last drawn, and the display's
    /// pixels per point.
    frames: [Option<(Size<Pixels>, f32)>; 2],
    image: Option<Arc<RenderImage>>,
    /// The page and pixel size last asked to be drawn.
    wanted: Option<(usize, u32)>,
    /// Drawing in progress. Dropping a GPUI task cancels it, so replacing
    /// this abandons a drawing nobody wants anymore.
    _drawing: Option<Task<()>>,
}

impl PdfView {
    /// The page shown, from 1, and how many there are.
    pub(super) fn position(&self) -> (usize, usize) {
        (self.page + 1, self.pages)
    }

    /// The page's size on `surface`: fitted to the preview, then zoomed.
    fn shown_size(&self, surface: Surface) -> Option<Size<Pixels>> {
        let (frame, _) = self.frames[surface as usize]?;
        let (width, height) = self.page_size;
        if width <= 0. || height <= 0. {
            return None;
        }
        let fit = (f32::from(frame.width) / width).min(f32::from(frame.height) / height);
        let scale = fit * ZOOMS[self.zoom];
        Some(size(px(width * scale), px(height * scale)))
    }

    /// The zoom on `surface`, as a percentage of the page's printed size.
    pub(super) fn percent(&self, surface: Surface) -> Option<u32> {
        let shown = self.shown_size(surface)?;
        Some((f32::from(shown.width) / self.page_size.0 * 100.).round() as u32)
    }

    pub(super) fn can_zoom(&self, by: isize) -> bool {
        self.zoom
            .checked_add_signed(by)
            .is_some_and(|zoom| zoom < ZOOMS.len())
    }

    /// How many pixels the page's longer side needs to look sharp where it's
    /// biggest on screen, within the budget, in steps of 64.
    fn page_pixels(&self) -> Option<u32> {
        let biggest = [Surface::Gallery, Surface::QuickLook]
            .into_iter()
            .filter_map(|surface| {
                let (_, scale) = self.frames[surface as usize]?;
                let shown = self.shown_size(surface)?;
                Some((f32::from(shown.width), f32::from(shown.height), scale))
            })
            .map(|(width, height, scale)| {
                let (long, short) = (width.max(height), width.min(height).max(1.));
                let most = (PIXEL_BUDGET * long / short).sqrt();
                (long * scale).min(most)
            })
            .reduce(f32::max)?;
        Some((biggest.max(64.) / 64.).ceil() as u32 * 64)
    }
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
fn drop_later(image: Option<Arc<RenderImage>>, cx: &mut App) {
    if let Some(image) = image {
        cx.defer(move |cx| cx.drop_image(image, None));
    }
}

impl Finder {
    /// Start showing `entry` page by page if it's a PDF that isn't shown
    /// already. It opens at its first page, fitted to the preview.
    pub(super) fn ensure_pdf(&mut self, entry: &FileEntry, cx: &mut Context<Self>) {
        let shown = self
            .pdf
            .as_ref()
            .is_some_and(|pdf| pdf.path == entry.path && pdf.modified == entry.modified);
        if entry.kind != FileKind::Pdf || shown {
            return;
        }
        if let Some(old) = self.pdf.take() {
            drop_later(old.image, cx);
        }
        if entry.size.is_some_and(|size| size > PDF_LIMIT) {
            return;
        }
        let path = entry.path.clone();
        let opening = cx.spawn(async move |this, cx| {
            let opened = cx
                .background_spawn(async move {
                    let doc = PdfDocument::open(&path).ok()?;
                    let size = doc.page_size(0)?;
                    Some((Arc::new(doc), size))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(pdf) = this.pdf.as_mut()
                    && let Some((doc, size)) = opened
                {
                    pdf.pages = doc.page_count();
                    pdf.page_size = size;
                    pdf.doc = Some(doc);
                    this.draw_pdf_page(cx);
                    cx.notify();
                }
            })
            .ok();
            // Quick Look is drawn outside this view; redraw it too.
            cx.refresh();
        });
        self.pdf = Some(PdfView {
            path: entry.path.clone(),
            modified: entry.modified,
            doc: None,
            pages: 0,
            page: 0,
            page_size: (0., 0.),
            zoom: 0,
            frames: [None; 2],
            image: None,
            wanted: None,
            _drawing: Some(opening),
        });
    }

    /// The PDF view for `entry`, when the large preview shows it.
    pub(super) fn pdf_view(&self, entry: &FileEntry) -> Option<&PdfView> {
        self.pdf
            .as_ref()
            .filter(|pdf| pdf.path == entry.path && pdf.doc.is_some())
    }

    /// The page the preview shows, from 1, and how many pages there are.
    pub fn shown_pdf_page(&self) -> Option<(usize, usize)> {
        self.pdf
            .as_ref()
            .filter(|pdf| pdf.doc.is_some())
            .map(PdfView::position)
    }

    /// The PDF's zoom where it's looked at (Quick Look while it's open), as
    /// a percentage of its printed size.
    pub fn shown_pdf_zoom(&self) -> Option<u32> {
        let surface = match self.quick_look_open {
            true => Surface::QuickLook,
            false => Surface::Gallery,
        };
        self.pdf.as_ref().and_then(|pdf| pdf.percent(surface))
    }

    /// Draw the page shown at the size it's shown, unless it's drawn (or
    /// being drawn) already at about that size. A drawing up to twice as
    /// big as needed is kept, so zooming out doesn't draw again.
    fn draw_pdf_page(&mut self, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_mut() else {
            return;
        };
        let (Some(doc), Some(pixels)) = (pdf.doc.clone(), pdf.page_pixels()) else {
            return;
        };
        let page = pdf.page;
        let enough = |(was_page, was): (usize, u32)| {
            was_page == page && was as f32 >= pixels as f32 * 0.9 && was <= pixels * 2
        };
        if pdf.wanted.is_some_and(enough) {
            return;
        }
        pdf.wanted = Some((page, pixels));
        pdf._drawing = Some(cx.spawn(async move |this, cx| {
            let image = cx
                .background_spawn(async move {
                    doc.render_page(page, pixels).ok().and_then(to_render_image)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(pdf) = this.pdf.as_mut() {
                    let old = std::mem::replace(&mut pdf.image, image);
                    drop_later(old, cx);
                    cx.notify();
                }
            })
            .ok();
            cx.refresh();
        }));
    }

    /// `surface`'s preview was laid out at `frame`; fit the page to it.
    /// Returns whether that changed anything.
    fn set_pdf_frame(
        &mut self,
        surface: Surface,
        frame: Size<Pixels>,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pdf) = self.pdf.as_mut() else {
            return false;
        };
        let slot = &mut pdf.frames[surface as usize];
        if *slot == Some((frame, scale)) {
            return false;
        }
        *slot = Some((frame, scale));
        self.draw_pdf_page(cx);
        cx.notify();
        true
    }

    /// Go `by` pages forward or back, staying within the document.
    pub(super) fn step_pdf_page(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_mut() else {
            return;
        };
        let Some(doc) = pdf.doc.clone() else {
            return;
        };
        let page = pdf
            .page
            .saturating_add_signed(by)
            .min(pdf.pages.saturating_sub(1));
        if page == pdf.page {
            return;
        }
        pdf.page = page;
        pdf.page_size = doc.page_size(page).unwrap_or(pdf.page_size);
        self.draw_pdf_page(cx);
        cx.notify();
    }

    /// Zoom in or out `by` steps, or back to the whole page with `None`.
    pub(super) fn zoom_pdf(&mut self, by: Option<isize>, cx: &mut Context<Self>) {
        let Some(pdf) = self.pdf.as_mut() else {
            return;
        };
        pdf.zoom = match by {
            Some(by) if pdf.can_zoom(by) => pdf.zoom.saturating_add_signed(by),
            Some(_) => return,
            None => 0,
        };
        self.draw_pdf_page(cx);
        cx.notify();
    }

    /// Whether the gallery or Quick Look shows a big preview, which the PDF
    /// keys act on.
    fn shows_large_preview(&self) -> bool {
        self.quick_look_open || self.effective_view() == ViewMode::Gallery
    }

    /// Cmd+Plus, Minus, and 0 on the preview: zoom a PDF, or change the
    /// text size of other previews.
    pub(super) fn step_preview_size(&mut self, steps: Option<f32>, cx: &mut Context<Self>) {
        let pdf_shown = self
            .primary_entry()
            .is_some_and(|entry| self.pdf_view(&entry).is_some());
        match (pdf_shown, steps) {
            (true, steps) => self.zoom_pdf(steps.map(|s| s as isize), cx),
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
            true => self.step_preview_size(steps, cx),
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
            true => self.step_pdf_page(1, cx),
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
            true => self.step_pdf_page(-1, cx),
            false => cx.propagate(),
        }
    }

    /// The page, fitted to the preview and zoomed, in a view that scrolls
    /// when it's bigger; `fallback` shows until the page is drawn. Each page
    /// of each document scrolls on its own, so another page or PDF starts
    /// at its top, while zooming keeps the place.
    pub(super) fn render_pdf_page(
        &self,
        pdf: &PdfView,
        view: Entity<Finder>,
        surface: Surface,
        fallback: AnyElement,
    ) -> AnyElement {
        let content = match (pdf.image.clone(), pdf.shown_size(surface)) {
            (Some(image), Some(shown)) => div()
                .min_w_full()
                .min_h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(image)
                        .w(shown.width)
                        .h(shown.height)
                        .flex_shrink_0()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            _ => fallback,
        };
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "pdf-{surface:?}-{}-{}",
                        super::views::path_id(&pdf.path),
                        pdf.page
                    )))
                    .size_full()
                    .overflow_scrollbar()
                    .child(content),
            )
            .on_prepaint(move |bounds, window, cx| {
                let scale = window.scale_factor();
                let changed = view.update(cx, |this, cx| {
                    this.set_pdf_frame(surface, bounds.size, scale, cx)
                });
                if changed {
                    window.refresh();
                }
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would replace the standard one.
    use super::{PageImage, to_render_image};

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
}
