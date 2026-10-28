//! PDFs page by page in the gallery and Quick Look. The page shown is drawn
//! with hayro (see `fffinder_core::pdf`), and the buttons under it, or Page
//! Up and Page Down in Quick Look, step through the pages. Only the PDF on
//! screen is kept, with one page drawn, so paging through a long document
//! doesn't fill memory.

use super::Finder;
use crate::actions::{NextPdfPage, PreviousPdfPage};
use fffinder_core::pdf::{PageImage, PdfDocument};
use fffinder_core::{FileEntry, FileKind};
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// The longer side of a drawn page, in pixels: sharp at Quick Look's size on
/// a 2× display.
const PAGE_PIXELS: u32 = 1600;
/// PDFs bigger than this, mostly scans, aren't read into memory to draw
/// them; they keep the system's thumbnail.
const PDF_LIMIT: u64 = 200_000_000;

/// The PDF the large preview shows.
pub(super) struct PdfView {
    path: PathBuf,
    modified: Option<SystemTime>,
    doc: Option<Arc<PdfDocument>>,
    pages: usize,
    /// The page shown, from 0. It changes before the page is drawn, and the
    /// previous page stays up until then.
    page: usize,
    image: Option<Arc<RenderImage>>,
    /// Drawing in progress. Dropping a GPUI task cancels it, so replacing
    /// this abandons a page nobody wants anymore.
    _drawing: Option<Task<()>>,
}

impl PdfView {
    /// The page drawn, ready to show.
    pub(super) fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }

    /// The page shown, from 1, and how many there are.
    pub(super) fn position(&self) -> (usize, usize) {
        (self.page + 1, self.pages)
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
    /// already. It opens at its first page.
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
        let drawing = cx.spawn(async move |this, cx| {
            let opened = cx
                .background_spawn(async move {
                    let doc = PdfDocument::open(&path).ok()?;
                    let image = doc
                        .render_page(0, PAGE_PIXELS)
                        .ok()
                        .and_then(to_render_image);
                    Some((Arc::new(doc), image))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(pdf) = this.pdf.as_mut()
                    && let Some((doc, image)) = opened
                {
                    pdf.pages = doc.page_count();
                    pdf.doc = Some(doc);
                    pdf.image = image;
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
            image: None,
            _drawing: Some(drawing),
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
        pdf._drawing = Some(cx.spawn(async move |this, cx| {
            let image = cx
                .background_spawn(async move {
                    doc.render_page(page, PAGE_PIXELS)
                        .ok()
                        .and_then(to_render_image)
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
        cx.notify();
    }

    pub(super) fn next_pdf_page(
        &mut self,
        _: &NextPdfPage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_pdf_page(1, cx);
    }

    pub(super) fn previous_pdf_page(
        &mut self,
        _: &PreviousPdfPage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_pdf_page(-1, cx);
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
