//! Frames as sixel images, for Windows terminals without kitty graphics, such
//! as Windows Terminal.
//!
//! Only the cells that changed since the last frame are sent, one image for
//! each run of changed rows, so a hover or a blinking caret costs a few
//! kilobytes rather than the whole window. Each image is drawn at the cursor
//! in bands of six pixel rows. A band that runs into a row the image doesn't
//! redraw blanks it in terminals that keep one image per cell, such as
//! WezTerm, and one that runs past the bottom of the screen scrolls the whole
//! screen up. So images are grown to a whole number of bands by redrawing the
//! rows next to them, and end without moving to a next band. Terminals that
//! put the cursor below an image instead of on its last row scroll anyway at
//! the bottom, unless they have mode 8452, which `tty` turns on.
//!
//! An image has at most 256 colors. Regions with no more than that keep their
//! exact colors, so a color looks the same in every image; busier ones, such
//! as photos, get 256 chosen by Wu's method, without dithering, which would
//! speckle flat areas.

use quantette::deps::palette::Srgb;
use quantette::{ImageRef, PaletteSize, Pipeline, QuantizeMethod};
use std::collections::HashMap;
use std::io::Write as _;

use super::frames::Frame;
use super::tty::Caps;

/// Pixel rows in one band of a sixel image.
const BAND: u32 = 6;
/// Colors in one image: Windows Terminal has 256 color registers.
const MAX_COLORS: usize = 256;

/// Turns frames into escape codes.
#[derive(Default)]
pub struct Encoder {
    /// The frame on screen, to send only what changed since.
    shown: Option<Frame>,
}

impl Encoder {
    pub fn new(_caps: &Caps) -> Encoder {
        Encoder::default()
    }

    /// Append the images that turn the frame on screen into `frame`.
    pub fn encode(&mut self, frame: Frame, out: &mut Vec<u8>) {
        if frame.cols == 0 || frame.rows == 0 {
            return;
        }
        let cell = CellSize {
            width: frame.width / u32::from(frame.cols),
            height: frame.height / u32::from(frame.rows),
        };
        if cell.width == 0 || cell.height == 0 {
            return;
        }
        let changed = match &self.shown {
            Some(shown)
                if (shown.width, shown.height, shown.cols, shown.rows)
                    == (frame.width, frame.height, frame.cols, frame.rows) =>
            {
                changed_cells(shown, &frame, cell)
            }
            _ => vec![Cells {
                left: 0,
                right: frame.cols,
                top: 0,
                bottom: frame.rows,
            }],
        };
        for cells in changed {
            for image in placements(cells, frame.rows, cell.height) {
                image.encode(&frame, cell, out);
            }
        }
        self.shown = Some(frame);
    }
}

/// One cell in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CellSize {
    width: u32,
    height: u32,
}

/// A rectangle of whole cells, as half-open ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cells {
    left: u16,
    right: u16,
    top: u16,
    bottom: u16,
}

/// One image: the cells it redraws, and its height in pixels, which is less
/// than theirs only on a screen too short to fit a whole band.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    cells: Cells,
    height: u32,
}

/// The cells that differ between two frames of the same size: for each run
/// of changed rows, the columns from the first change to the last.
fn changed_cells(old: &Frame, new: &Frame, cell: CellSize) -> Vec<Cells> {
    let stride = new.width as usize * 4;
    let (cell_width, cell_height) = (cell.width as usize, cell.height as usize);
    let mut runs = Vec::new();
    let mut run: Option<Cells> = None;
    for row in 0..new.rows {
        let top = usize::from(row) * cell_height;
        let span = (top..top + cell_height)
            .filter_map(|y| {
                let line = y * stride..(y + 1) * stride;
                changed_span(&old.rgba[line.clone()], &new.rgba[line])
            })
            .reduce(|(first, last), (start, end)| (first.min(start), last.max(end)));
        match span {
            Some((first, last)) => {
                // Columns fit in u16, since they come from the terminal's grid.
                let (left, right) = ((first / cell_width) as u16, (last / cell_width + 1) as u16);
                match &mut run {
                    Some(cells) => {
                        cells.left = cells.left.min(left);
                        cells.right = cells.right.max(right);
                        cells.bottom = row + 1;
                    }
                    None => {
                        run = Some(Cells {
                            left,
                            right,
                            top: row,
                            bottom: row + 1,
                        });
                    }
                }
            }
            None => runs.extend(run.take()),
        }
    }
    runs.extend(run);
    runs
}

/// The first and last pixel that differ between two lines of pixels.
fn changed_span(old: &[u8], new: &[u8]) -> Option<(usize, usize)> {
    if old == new {
        return None;
    }
    let pixels = || old.as_chunks::<4>().0.iter().zip(new.as_chunks::<4>().0);
    let first = pixels().position(|(a, b)| a != b)?;
    let last = pixels().rposition(|(a, b)| a != b)?;
    Some((first, last))
}

/// The images that redraw `cells` on a screen `rows` tall, each a whole
/// number of bands. The cells grow down to make whole bands, or up where the
/// screen ends, redrawing those rows as they are. A screen that isn't itself
/// a whole number of bands tall gets its top rows in an image of their own,
/// drawn first, whose last band the next image paints over. On a screen too
/// short for even that, the last pixel rows that don't fill a band are left
/// as they were.
fn placements(cells: Cells, rows: u16, cell_height: u32) -> Vec<Placement> {
    let image = |top: u16, bottom: u16| Placement {
        cells: Cells {
            top,
            bottom,
            ..cells
        },
        height: u32::from(bottom - top) * cell_height,
    };
    // Rows come in groups this many tall to make whole bands.
    let group = (BAND / gcd(cell_height, BAND)) as u16;
    let short = (group - (cells.bottom - cells.top) % group) % group;
    let bottom = (cells.bottom + short).min(rows);
    let above = short - (bottom - cells.bottom);
    if above <= cells.top {
        return vec![image(cells.top - above, bottom)];
    }
    // The cells take in the whole screen, which isn't whole bands tall.
    let split = rows % group;
    if split == rows {
        let whole = image(0, rows);
        return vec![Placement {
            height: whole.height - whole.height % BAND,
            ..whole
        }];
    }
    vec![image(0, split), image(split, rows)]
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Placement {
    /// Append the cursor move and the image.
    fn encode(&self, frame: &Frame, cell: CellSize, out: &mut Vec<u8>) {
        let Cells {
            left, right, top, ..
        } = self.cells;
        let width = usize::from(right - left) * cell.width as usize;
        let height = self.height as usize;
        let (palette, indices) = quantize(
            frame,
            usize::from(left) * cell.width as usize,
            usize::from(top) * cell.height as usize,
            width,
            height,
        );
        // DCS P1=0 P2=1 (pixels not drawn stay as they were) q, then square
        // pixels and the image's size.
        let _ = write!(
            out,
            "\x1b[{};{}H\x1bP0;1;0q\"1;1;{width};{height}",
            top + 1,
            left + 1
        );
        for (number, [red, green, blue]) in palette.iter().enumerate() {
            let _ = write!(out, "#{number};2;{red};{green};{blue}");
        }
        bands(&indices, palette.len(), width, out);
        out.extend_from_slice(b"\x1b\\");
    }
}

/// The sixel bands of an image whose rows are `indices` into `colors`
/// colors, `width` to a row. A band ends with `-` to start the next, except
/// the last: one after it would move the cursor below the image, which at the
/// bottom scrolls the screen.
fn bands(indices: &[u8], colors: usize, width: usize, out: &mut Vec<u8>) {
    let height = indices.len() / width.max(1);
    // One line of sixel bits for each color, and the colors in this band in
    // the order they first appear.
    let mut bits = vec![0u8; colors * width];
    let mut used: Vec<u8> = Vec::with_capacity(colors);
    let mut seen = [false; MAX_COLORS];
    let count = height.div_ceil(BAND as usize);
    for band in 0..count {
        for bit in 0..BAND as usize {
            let y = band * BAND as usize + bit;
            if y >= height {
                break;
            }
            let row = &indices[y * width..][..width];
            for (x, &color) in row.iter().enumerate() {
                if !seen[usize::from(color)] {
                    seen[usize::from(color)] = true;
                    used.push(color);
                }
                bits[usize::from(color) * width + x] |= 1 << bit;
            }
        }
        for (ix, color) in used.drain(..).enumerate() {
            if ix > 0 {
                out.push(b'$');
            }
            let _ = write!(out, "#{color}");
            let line = &mut bits[usize::from(color) * width..][..width];
            runs(line, out);
            line.fill(0);
            seen[usize::from(color)] = false;
        }
        if band + 1 < count {
            out.push(b'-');
        }
    }
}

/// One color's line of a band, with repeats run-length encoded and blank
/// sixels at the end left off.
fn runs(line: &[u8], out: &mut Vec<u8>) {
    let end = line
        .iter()
        .rposition(|bits| *bits != 0)
        .map_or(0, |x| x + 1);
    let mut x = 0;
    while x < end {
        let bits = line[x];
        let run = line[x..end]
            .iter()
            .take_while(|next| **next == bits)
            .count();
        let sixel = b'?' + bits;
        if run > 3 {
            let _ = write!(out, "!{run}");
            out.push(sixel);
        } else {
            out.extend(std::iter::repeat_n(sixel, run));
        }
        x += run;
    }
}

/// A color as sixel takes it: percentages of red, green, and blue.
fn percent(value: u8) -> u8 {
    ((u16::from(value) * 100 + 127) / 255) as u8
}

/// The palette of a region of `frame`, and each pixel's index into it.
fn quantize(
    frame: &Frame,
    left: usize,
    top: usize,
    width: usize,
    rows: usize,
) -> (Vec<[u8; 3]>, Vec<u8>) {
    let stride = frame.width as usize * 4;
    let pixels = || {
        (top..top + rows).flat_map(move |y| {
            frame.rgba[y * stride + left * 4..][..width * 4]
                .as_chunks::<4>()
                .0
        })
    };
    exact_colors(pixels(), width * rows).unwrap_or_else(|| reduced_colors(pixels(), width, rows))
}

/// The region's own colors, when there are at most 256 of them.
fn exact_colors<'a>(
    pixels: impl Iterator<Item = &'a [u8; 4]>,
    count: usize,
) -> Option<(Vec<[u8; 3]>, Vec<u8>)> {
    let mut palette: Vec<[u8; 3]> = Vec::with_capacity(MAX_COLORS);
    let mut lookup: HashMap<[u8; 3], u8> = HashMap::with_capacity(MAX_COLORS);
    let mut indices = Vec::with_capacity(count);
    // Neighbors are usually the same color, so most pixels skip the lookup.
    let mut last: Option<([u8; 3], u8)> = None;
    for pixel in pixels {
        let color = [percent(pixel[0]), percent(pixel[1]), percent(pixel[2])];
        let index = match last {
            Some((seen, index)) if seen == color => index,
            _ => {
                let index = match lookup.get(&color) {
                    Some(index) => *index,
                    None if palette.len() == MAX_COLORS => return None,
                    None => {
                        let index = palette.len() as u8;
                        palette.push(color);
                        lookup.insert(color, index);
                        index
                    }
                };
                last = Some((color, index));
                index
            }
        };
        indices.push(index);
    }
    Some((palette, indices))
}

/// 256 colors for the region, chosen by Wu's method.
fn reduced_colors<'a>(
    pixels: impl Iterator<Item = &'a [u8; 4]>,
    width: usize,
    rows: usize,
) -> (Vec<[u8; 3]>, Vec<u8>) {
    let colors: Vec<Srgb<u8>> = pixels
        .map(|pixel| Srgb::new(pixel[0], pixel[1], pixel[2]))
        .collect();
    let image = ImageRef::new(width as u32, rows as u32, colors.as_slice())
        .expect("a region is width by rows pixels");
    let (palette, indices) = Pipeline::new()
        .palette_size(PaletteSize::MAX)
        .quantize_method(QuantizeMethod::Wu)
        .ditherer(None)
        .parallel(true)
        .input_image(image)
        .output_srgb8_indexed_image()
        .into_parts();
    let palette = palette
        .into_iter()
        .map(|color| {
            [
                percent(color.red),
                percent(color.green),
                percent(color.blue),
            ]
        })
        .collect();
    (palette, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellSize = CellSize {
        width: 10,
        height: 20,
    };

    /// A frame of Windows Terminal's 10x20 cells, colored by `paint(x, y)`.
    fn frame(cols: u16, rows: u16, paint: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
        let (width, height) = (u32::from(cols) * CELL.width, u32::from(rows) * CELL.height);
        let rgba = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [red, green, blue] = paint(x, y);
                [red, green, blue, 255]
            })
            .collect();
        Frame {
            rgba,
            width,
            height,
            cols,
            rows,
        }
    }

    /// Each image in the output: the cell its cursor move goes to, from 0,
    /// and the image's escape code.
    fn images(out: &[u8]) -> Vec<((u16, u16), &[u8])> {
        let mut found = Vec::new();
        let mut rest = out;
        while let Some(start) = rest.windows(2).position(|pair| pair == b"\x1b[") {
            let code = &rest[start + 2..];
            let h = code
                .iter()
                .position(|byte| *byte == b'H')
                .expect("a cursor move");
            let place = std::str::from_utf8(&code[..h]).expect("ASCII");
            let (row, col) = place.split_once(';').expect("row;col");
            let image = &code[h + 1..];
            let end = image
                .windows(2)
                .position(|pair| pair == b"\x1b\\")
                .expect("an end")
                + 2;
            let cell = (
                row.parse::<u16>().expect("a row") - 1,
                col.parse::<u16>().expect("a column") - 1,
            );
            found.push((cell, &image[..end]));
            rest = &image[end..];
        }
        found
    }

    /// The screen after drawing each image of `out` onto `screen`, the way a
    /// terminal would, decoding them with icy_sixel. Undrawn pixels stay.
    fn draw(screen: &mut Frame, out: &[u8]) {
        let stride = screen.width as usize * 4;
        for ((row, col), code) in images(out) {
            let image = icy_sixel::SixelImage::decode(code).expect("a valid sixel image");
            let (x0, y0) = (
                usize::from(col) * CELL.width as usize,
                usize::from(row) * CELL.height as usize,
            );
            for y in 0..image.height {
                for x in 0..image.width {
                    let pixel = &image.pixels[(y * image.width + x) * 4..][..4];
                    if pixel[3] == 0 {
                        continue;
                    }
                    let at = (y0 + y) * stride + (x0 + x) * 4;
                    screen.rgba[at..at + 4].copy_from_slice(pixel);
                }
            }
        }
    }

    /// The largest difference in any channel between two screens.
    fn worst_error(a: &Frame, b: &Frame) -> u8 {
        a.rgba
            .iter()
            .zip(&b.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0)
    }

    fn encoder() -> Encoder {
        Encoder::new(&Caps::default())
    }

    /// The average difference per channel between two screens.
    fn mean_error(a: &Frame, b: &Frame) -> f64 {
        let total: u64 = a
            .rgba
            .iter()
            .zip(&b.rgba)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        total as f64 / a.rgba.len() as f64
    }

    fn blank(cols: u16, rows: u16) -> Frame {
        frame(cols, rows, |_, _| [0, 0, 0])
    }

    #[test]
    fn a_few_colors_come_through_exactly() {
        for colors in [1u32, 2, 256] {
            let source = frame(12, 6, |x, y| {
                let n = (x + y * 120) % colors;
                [(n * 7 % 256) as u8, (n * 13 % 256) as u8, n as u8]
            });
            let mut out = Vec::new();
            encoder().encode(source.clone(), &mut out);
            let mut screen = blank(12, 6);
            draw(&mut screen, &out);
            // Percentages are within half a percent, 1.3 of 255.
            assert!(worst_error(&screen, &source) <= 2, "{colors} colors");
        }
    }

    #[test]
    fn many_colors_are_reduced_to_256_close_ones() {
        // A smooth gradient across red and green, as in a photo.
        let source = frame(30, 9, |x, y| {
            [(x * 255 / 299) as u8, (y * 255 / 179) as u8, 128]
        });
        let mut out = Vec::new();
        encoder().encode(source.clone(), &mut out);
        for (_, code) in images(&out) {
            let text = std::str::from_utf8(code).unwrap();
            // Definitions are `#number;2;red;green;blue`; selections are
            // `#number` alone.
            let defined = text
                .split('#')
                .skip(1)
                .filter(|code| {
                    code.trim_start_matches(|c: char| c.is_ascii_digit())
                        .starts_with(";2;")
                })
                .count();
            assert!(defined <= MAX_COLORS, "{defined} colors in one image");
        }
        let mut screen = blank(30, 9);
        draw(&mut screen, &out);
        // Wu's method works in Oklab, so it trades exact channels for what
        // looks close; on average a channel is off by a few levels.
        let mean = mean_error(&screen, &source);
        assert!(mean <= 4., "off by {mean} on average");
    }

    /// Whether a terminal would scroll for any image: one does when its last
    /// band runs past the bottom of the screen.
    fn scrolls(out: &[u8], rows: u16) -> bool {
        images(out).iter().any(|((row, _), code)| {
            let bands = code.iter().filter(|byte| **byte == b'-').count() + 1;
            u32::from(*row) * CELL.height + bands as u32 * BAND > u32::from(rows) * CELL.height
        })
    }

    #[test]
    fn no_image_scrolls_the_screen() {
        for rows in [1, 2, 3, 4, 5, 30, 31, 32] {
            let source = frame(4, rows, |x, y| [(x * 3) as u8, (y % 251) as u8, 9]);
            let mut out = Vec::new();
            let mut encoder = encoder();
            encoder.encode(source, &mut out);
            assert!(!scrolls(&out, rows), "a whole screen of {rows} rows");
            assert!(!out.windows(3).any(|code| code == b"-\x1b\\"));
            // Then a change in the bottom row only.
            let source = frame(4, rows, |x, y| {
                if y >= (u32::from(rows) - 1) * CELL.height {
                    [200, 100, 50]
                } else {
                    [(x * 3) as u8, (y % 251) as u8, 9]
                }
            });
            out.clear();
            encoder.encode(source, &mut out);
            assert!(!scrolls(&out, rows), "the bottom row of {rows}");
        }
    }

    #[test]
    fn whole_screens_come_through_at_every_height() {
        // Rows that can't make whole bands at the bottom go in their own
        // image above; only a screen under 3 rows loses its last pixels.
        for rows in [3, 4, 5, 31, 32] {
            // Few colors, so they come through exactly, changing every pixel
            // row so a band out of place would show.
            let source = frame(4, rows, |x, y| {
                [(x / 10 * 60) as u8, (y % 7 * 30) as u8, 77]
            });
            let mut out = Vec::new();
            encoder().encode(source.clone(), &mut out);
            let mut screen = blank(4, rows);
            draw(&mut screen, &out);
            assert!(worst_error(&screen, &source) <= 2, "{rows} rows");
        }
    }

    fn rows(top: u16, bottom: u16) -> Cells {
        Cells {
            left: 2,
            right: 5,
            top,
            bottom,
        }
    }

    #[test]
    fn images_grow_to_whole_bands() {
        let placed = |cells, rows, cell_height| -> Vec<(u16, u16, u32)> {
            placements(cells, rows, cell_height)
                .iter()
                .map(|image| (image.cells.top, image.cells.bottom, image.height))
                .collect()
        };
        // Windows Terminal's 20-pixel cells come in threes: down from the
        // middle, up from the bottom.
        assert_eq!(placed(rows(9, 10), 30, 20), [(9, 12, 60)]);
        assert_eq!(placed(rows(29, 30), 30, 20), [(27, 30, 60)]);
        assert_eq!(placed(rows(28, 30), 30, 20), [(27, 30, 60)]);
        // WezTerm's 45-pixel cells come in twos; 24-pixel ones are whole.
        assert_eq!(placed(rows(10, 11), 24, 45), [(10, 12, 90)]);
        assert_eq!(placed(rows(23, 24), 24, 45), [(22, 24, 90)]);
        assert_eq!(placed(rows(29, 30), 30, 24), [(29, 30, 24)]);
        // A whole screen of 35 rows: two rows on top, drawn first, then 33.
        assert_eq!(placed(rows(0, 35), 35, 20), [(0, 2, 40), (2, 35, 660)]);
        // Two rows can't make a band of 20-pixel cells: 36 of 40 pixels.
        assert_eq!(placed(rows(1, 2), 2, 20), [(0, 2, 36)]);
    }

    #[test]
    fn only_the_changed_cells_are_sent() {
        let before = frame(20, 10, |_, _| [30, 30, 30]);
        let after = frame(20, 10, |x, y| {
            // A hover over cells 3 to 5 of row 4.
            if (30..60).contains(&x) && (80..100).contains(&y) {
                [60, 90, 200]
            } else {
                [30, 30, 30]
            }
        });
        let mut encoder = encoder();
        let mut out = Vec::new();
        encoder.encode(before.clone(), &mut out);
        out.clear();
        encoder.encode(after.clone(), &mut out);
        let sent = images(&out);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, (4, 3));
        // Rows 4 to 6: the hover's row, and two more to make whole bands.
        assert!(sent[0].1.starts_with(b"\x1bP0;1;0q\"1;1;30;60#0;2;"));
        let mut screen = before;
        draw(&mut screen, &out);
        assert!(worst_error(&screen, &after) <= 2);

        out.clear();
        encoder.encode(after, &mut out);
        assert!(out.is_empty(), "nothing changed");
    }

    #[test]
    fn runs_repeat_and_trailing_blanks_are_dropped() {
        let mut out = Vec::new();
        runs(&[1, 1, 1, 1, 1, 2, 2, 0, 0], &mut out);
        assert_eq!(out, b"!5@AA");
    }
}
