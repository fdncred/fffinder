//! PDF pages drawn for previews with hayro, a pure-Rust renderer, so they
//! look the same on every system.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use std::path::Path;

/// Why a PDF or one of its pages couldn't be drawn.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("couldn’t read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the PDF is password-protected")]
    Encrypted,
    #[error("the file isn’t a PDF that can be read")]
    Invalid,
    #[error("the PDF has no page {0}")]
    NoPage(usize),
    #[error("the page couldn’t be drawn")]
    Render,
}

/// A page drawn on white, as straight RGBA rows from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A PDF read into memory once, to draw any of its pages.
pub struct PdfDocument {
    pdf: Pdf,
}

impl PdfDocument {
    /// Read the PDF at `path`.
    ///
    /// # Errors
    ///
    /// When the file can't be read, is encrypted, or isn't a PDF.
    pub fn open(path: &Path) -> Result<PdfDocument, PdfError> {
        Self::from_bytes(std::fs::read(path)?)
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<PdfDocument, PdfError> {
        let pdf = Pdf::new(bytes).map_err(|err| match err {
            LoadPdfError::Decryption(_) => PdfError::Encrypted,
            LoadPdfError::Invalid => PdfError::Invalid,
        })?;
        Ok(PdfDocument { pdf })
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Draw page `index` (from 0) so its longer side is `longest_side`
    /// pixels.
    ///
    /// # Errors
    ///
    /// When there is no such page, or it has no size, or drawing it fails.
    pub fn render_page(&self, index: usize, longest_side: u32) -> Result<PageImage, PdfError> {
        let page = self
            .pdf
            .pages()
            .get(index)
            .ok_or(PdfError::NoPage(index + 1))?;
        let (width, height) = page.render_dimensions();
        let longer = width.max(height);
        if !longer.is_finite() || longer <= 0.0 {
            return Err(PdfError::Render);
        }
        // hayro draws into 16-bit sizes.
        let scale = longest_side.min(u32::from(u16::MAX)) as f32 / longer;
        let settings = PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        };
        // A malformed file must not take the app down with it; Python's
        // `try/except BaseException` is the nearest thing to `catch_unwind`.
        let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            hayro::render(
                page,
                &RenderCache::new(),
                &InterpreterSettings::default(),
                &RenderSettings::default(),
                &settings,
            )
        }))
        .map_err(|_| PdfError::Render)?;
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        if width == 0 || height == 0 {
            return Err(PdfError::Render);
        }
        // On a white background every pixel is opaque, so the premultiplied
        // pixels are already straight RGBA.
        Ok(PageImage {
            width,
            height,
            rgba: pixmap.data_as_u8_slice().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGES: &[u8] = include_bytes!("../testdata/pages.pdf");

    #[test]
    fn draws_each_page() {
        let doc = PdfDocument::from_bytes(PAGES.to_vec()).unwrap();
        assert_eq!(doc.page_count(), 2);

        // Page 1 is 200 × 100 points, black on its left half.
        let page = doc.render_page(0, 400).unwrap();
        assert_eq!((page.width, page.height), (400, 200));
        let pixel = |p: &PageImage, x: u32, y: u32| {
            let at = ((y * p.width + x) * 4) as usize;
            p.rgba[at..at + 4].to_vec()
        };
        assert_eq!(pixel(&page, 50, 100), [0, 0, 0, 255]);
        assert_eq!(pixel(&page, 350, 100), [255, 255, 255, 255]);

        // Page 2 is 100 × 200, black on its bottom half.
        let page = doc.render_page(1, 100).unwrap();
        assert_eq!((page.width, page.height), (50, 100));
        assert_eq!(pixel(&page, 25, 75), [0, 0, 0, 255]);
        assert_eq!(pixel(&page, 25, 25), [255, 255, 255, 255]);

        assert!(matches!(doc.render_page(2, 100), Err(PdfError::NoPage(3))));
        assert!(matches!(
            PdfDocument::from_bytes(b"not a pdf".to_vec()),
            Err(PdfError::Invalid)
        ));
    }
}
