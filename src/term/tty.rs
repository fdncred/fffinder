//! The terminal itself: taking it over, asking what it supports, its size,
//! writing to it, and putting it back the way it was.
//!
//! All drawing goes to the terminal (`/dev/tty`, or the console on Windows),
//! never stdout, so `--pick` can still print its JSON to stdout for a script.

use anyhow::{Context as _, bail};
use crossterm::terminal::WindowSize;
use fffinder_core::config;
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod console;
#[cfg(unix)]
mod unix;

#[cfg(windows)]
use console as sys;
#[cfg(unix)]
use unix as sys;

/// The window is never laid out smaller than this, in logical pixels, unless
/// the terminal is too small even at a scale of 1. Matches the GUI's minimum.
const MIN_LOGICAL: (f32, f32) = (850., 470.);
/// How long to wait for the terminal to answer the startup queries.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// The terminal, open for writing. Frames and restoring share it, so neither
/// interrupts the other's escape codes.
static OUT: OnceLock<Mutex<File>> = OnceLock::new();
/// Set while the terminal is taken over.
static ENTERED: AtomicBool = AtomicBool::new(false);

/// What the terminal said it supports.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Caps {
    /// Images through the kitty graphics protocol.
    pub kitty: bool,
    /// Images handed over in POSIX shared memory instead of escape codes.
    /// Only works when the terminal runs on this machine.
    pub shared_memory: bool,
    /// Sixel images, from the primary device attributes.
    pub sixel: bool,
    /// Mode 8452, which leaves the cursor beside a sixel image rather than
    /// below it, so an image at the bottom doesn't scroll the screen.
    pub sixel_cursor: bool,
    /// Mouse positions in pixels (mode 1016) rather than cells.
    pub pixel_mouse: bool,
    /// One cell's size in pixels, from `CSI 16 t`.
    pub cell: Option<(u16, u16)>,
}

/// The terminal's size in cells and in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    /// Pixels across all the columns.
    pub width: u32,
    /// Pixels down all the rows.
    pub height: u32,
}

impl Geometry {
    /// One cell's size in pixels.
    pub fn cell(&self) -> (f32, f32) {
        (
            self.width as f32 / f32::from(self.cols.max(1)),
            self.height as f32 / f32::from(self.rows.max(1)),
        )
    }
}

/// A terminal that has been taken over, and what it can do.
#[derive(Debug, Clone, Copy)]
pub struct Session {
    pub caps: Caps,
    pub geometry: Geometry,
    /// `FFFINDER_TERM_SCALE`, when set to a number.
    pub requested_scale: Option<f32>,
}

impl Session {
    /// Put the terminal in raw mode, check that it can show images, and
    /// switch to the alternate screen with mouse and keyboard reporting on.
    /// From then on stderr goes to `term.log` in the data folder, and the
    /// terminal is put back on exit or on a panic in the main thread.
    ///
    /// # Errors
    ///
    /// Fails, leaving the terminal as it was, when there is no terminal, it
    /// doesn't answer, it can't show images this platform sends, or it doesn't
    /// report its size in pixels.
    pub fn start() -> anyhow::Result<Session> {
        let out = sys::open().context("fffinder --term needs a terminal")?;
        sys::prepare(&out).context("could not set the terminal up")?;
        crossterm::terminal::enable_raw_mode().context("could not put the terminal in raw mode")?;
        let checked = probe(&out).and_then(|caps| {
            sys::check(&caps)?;
            let geometry = geometry(caps.cell)?;
            sys::start_input().context("could not read the mouse")?;
            Ok((caps, geometry))
        });
        let (caps, geometry) = match checked {
            Ok(found) => found,
            Err(err) => {
                let _ = crossterm::terminal::disable_raw_mode();
                sys::restore_modes();
                return Err(err);
            }
        };

        let out = OUT.get_or_init(|| Mutex::new(out));
        write_locked(out, sys::enter(&caps).as_bytes())
            .context("could not write to the terminal")?;
        ENTERED.store(true, Ordering::SeqCst);
        install_restore_hooks();
        redirect_stderr();
        install_logger();

        let requested_scale = config::env_var("FFFINDER_TERM_SCALE")
            .and_then(|value| value.to_str()?.parse().ok())
            .filter(|scale: &f32| scale.is_finite() && *scale > 0.);
        let cell = geometry.cell();
        log::info!(
            "{}x{} cells of {:.1}x{:.1} px ({}x{} px), scale {}, frames as {}, mouse in {}",
            geometry.cols,
            geometry.rows,
            cell.0,
            cell.1,
            geometry.width,
            geometry.height,
            scale(&geometry, requested_scale),
            sys::frames_as(&caps),
            if caps.pixel_mouse { "pixels" } else { "cells" },
        );
        Ok(Session {
            caps,
            geometry,
            requested_scale,
        })
    }
}

/// Writes log records to stderr, which is `term.log` by then: fffinder's
/// from info up, everyone else's warnings and errors. The app has no other
/// logger.
struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let least = if metadata.target().starts_with("fffinder") {
            log::Level::Info
        } else {
            log::Level::Warn
        };
        metadata.level() <= least
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{} {}: {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

fn install_logger() {
    static LOGGER: StderrLog = StderrLog;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// The terminal's size now. Pixels come from the terminal's own report, or
/// from the cell size when it gives none, as the Windows console never does.
///
/// # Errors
///
/// Fails when the size can't be read, or there are no pixels to go by.
pub fn geometry(cell: Option<(u16, u16)>) -> anyhow::Result<Geometry> {
    let WindowSize {
        rows,
        columns,
        width,
        height,
    } = sys::window_size().context("could not read the terminal size")?;
    let (width, height) = match (width, height, cell) {
        (width, height, _) if width > 0 && height > 0 => (u32::from(width), u32::from(height)),
        (_, _, Some((cell_width, cell_height))) => (
            u32::from(cell_width) * u32::from(columns),
            u32::from(cell_height) * u32::from(rows),
        ),
        _ => bail!("the terminal doesn't report its size in pixels"),
    };
    Ok(Geometry {
        cols: columns,
        rows,
        width,
        height,
    })
}

/// Device pixels per logical pixel. `requested` wins; otherwise 2 when cells
/// are Retina-sized, lowered as far as 1 to keep the window's minimum size.
pub fn scale(geometry: &Geometry, requested: Option<f32>) -> f32 {
    requested.unwrap_or_else(|| {
        let natural: f32 = if geometry.cell().1 >= 24. { 2. } else { 1. };
        let fit =
            (geometry.width as f32 / MIN_LOGICAL.0).min(geometry.height as f32 / MIN_LOGICAL.1);
        natural.min(fit).max(1.)
    })
}

/// Write to the terminal, whole, without other output landing in between.
///
/// # Errors
///
/// Fails once the terminal is gone.
pub fn write(bytes: &[u8]) -> io::Result<()> {
    match OUT.get() {
        Some(out) if ENTERED.load(Ordering::SeqCst) => write_locked(out, bytes),
        _ => Ok(()),
    }
}

fn write_locked(out: &Mutex<File>, bytes: &[u8]) -> io::Result<()> {
    let mut file = out.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    sys::write_all(&mut file, bytes)?;
    file.flush()
}

/// Put the terminal back: images gone, reporting off, main screen, cooked
/// mode, stderr back on the terminal. Does nothing the second time.
pub fn restore() {
    if !ENTERED.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Some(out) = OUT.get() {
        // Wait a moment for a frame being written, so these codes don't land
        // inside it, then keep the lock so no later frame follows them.
        let deadline = Instant::now() + Duration::from_millis(250);
        let guard = loop {
            match out.try_lock() {
                Ok(guard) => break Some(guard),
                Err(TryLockError::Poisoned(poisoned)) => break Some(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(TryLockError::WouldBlock) => break None,
            }
        };
        match guard {
            Some(mut file) => {
                let _ = sys::write_all(&mut file, sys::LEAVE.as_bytes());
                let _ = file.flush();
                std::mem::forget(file);
            }
            None => {
                if let Ok(mut file) = sys::open() {
                    let _ = sys::write_all(&mut file, sys::LEAVE.as_bytes());
                }
            }
        }
    }
    sys::restore_modes();
    let _ = crossterm::terminal::disable_raw_mode();
    sys::restore_stderr();
}

/// Restore on any exit that runs the C runtime's exit handlers, and on a
/// panic in the main thread. Other threads' panics only reach the log,
/// since the window keeps running.
fn install_restore_hooks() {
    extern "C" fn restore_at_exit() {
        restore();
    }
    // SAFETY: registers a plain function with no captured state.
    unsafe { libc::atexit(restore_at_exit) };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            restore();
        }
        previous(info);
    }));
}

/// Send stderr, which would scribble over the window, to `term.log` in the
/// data folder, or nowhere if that can't be opened.
fn redirect_stderr() {
    let path = config::data_dir().join("term.log");
    let log = path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| File::create(&path))
        .or_else(|_| OpenOptions::new().write(true).open(sys::NULL_DEVICE));
    if let Ok(log) = log {
        sys::redirect_stderr(log);
    }
}

/// Ask what the terminal supports and read its answers, ending with the
/// primary device attributes every terminal answers.
fn probe(out: &File) -> anyhow::Result<Caps> {
    let replies = sys::probe(out).context("could not read the terminal's answers")?;
    if !has_device_attributes(&replies) {
        bail!("the terminal didn't answer within {PROBE_TIMEOUT:?}");
    }
    Ok(parse_replies(&replies))
}

/// The capabilities in the terminal's answers to [`probe`]'s queries.
fn parse_replies(replies: &[u8]) -> Caps {
    let text = String::from_utf8_lossy(replies);
    Caps {
        kitty: text.contains("\x1b_Gi=31;OK"),
        shared_memory: text.contains("\x1b_Gi=32;OK"),
        // Extension 4 is sixel graphics.
        sixel: device_attributes(replies).is_some_and(|params| {
            params
                .split(|byte| *byte == b';')
                .any(|param| param == b"4")
        }),
        // 1 set, 2 reset, 3 permanently set; 0 is unknown, 4 permanently reset.
        pixel_mouse: mode_report(&text, 1016).is_some_and(|state| matches!(state, 1..=3)),
        sixel_cursor: mode_report(&text, 8452).is_some_and(|state| matches!(state, 1..=3)),
        cell: cell_size(&text),
    }
}

/// The state in a DECRPM answer, `CSI ? mode ; state $ y`.
fn mode_report(text: &str, mode: u16) -> Option<u8> {
    let prefix = format!("\x1b[?{mode};");
    let start = text.find(&prefix)? + prefix.len();
    let rest = &text[start..];
    rest[..rest.find("$y")?].parse().ok()
}

/// The cell size in an XTWINOPS answer, `CSI 6 ; height ; width t`.
fn cell_size(text: &str) -> Option<(u16, u16)> {
    let start = text.find("\x1b[6;")? + 4;
    let rest = &text[start..];
    let (height, width) = rest[..rest.find('t')?].split_once(';')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// The parameters of the primary device attributes in the answers,
/// `CSI ? … c`, such as `62;22;52`.
fn device_attributes(replies: &[u8]) -> Option<&[u8]> {
    replies.windows(3).enumerate().find_map(|(start, window)| {
        if window != b"\x1b[?" {
            return None;
        }
        let params = &replies[start + 3..];
        let end = params
            .iter()
            .position(|byte| !(byte.is_ascii_digit() || *byte == b';'))?;
        (params[end] == b'c').then_some(&params[..end])
    })
}

/// Whether the answers include primary device attributes.
fn has_device_attributes(replies: &[u8]) -> bool {
    device_attributes(replies).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHOSTTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\\x1b[?1016;2$y\
        \x1b[6;34;16t\x1b[?62;22;52c";
    /// Windows Terminal 1.22's answers to `CSI 16 t` and `CSI c`.
    const WINDOWS_TERMINAL: &[u8] = b"\x1b[6;20;10t\x1b[?61;4;6;7;14;21;22;23;24;28;32;42c";

    #[test]
    fn reads_a_full_set_of_answers() {
        assert_eq!(
            parse_replies(GHOSTTY),
            Caps {
                kitty: true,
                shared_memory: true,
                sixel: false,
                sixel_cursor: false,
                pixel_mouse: true,
                cell: Some((16, 34)),
            }
        );
        assert!(has_device_attributes(GHOSTTY));
    }

    #[test]
    fn reads_windows_terminals_sixel_and_cell_size() {
        assert_eq!(
            parse_replies(WINDOWS_TERMINAL),
            Caps {
                sixel: true,
                cell: Some((10, 20)),
                ..Caps::default()
            }
        );
        // A sixel terminal that leaves the cursor beside an image.
        assert!(parse_replies(b"\x1b[?8452;2$y\x1b[?62;4c").sixel_cursor);
        assert!(!parse_replies(b"\x1b[?8452;0$y\x1b[?62;4c").sixel_cursor);
        // 42 isn't 4, and neither is a 4 inside another sequence.
        assert!(!parse_replies(b"\x1b[?62;42;14c").sixel);
        assert!(!parse_replies(b"\x1b[?4;2$y\x1b[?62;22c").sixel);
    }

    #[test]
    fn reads_refusals_and_silence() {
        let replies = b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;EBADF:shm\x1b\\\x1b[?1016;0$y\x1b[?6c";
        let caps = parse_replies(replies);
        assert!(caps.kitty);
        assert!(!caps.shared_memory);
        assert!(!caps.pixel_mouse);
        assert_eq!(caps.cell, None);
        assert_eq!(parse_replies(b"\x1b[?1;2c"), Caps::default());
    }

    #[test]
    fn device_attributes_are_not_confused_with_mode_reports() {
        assert!(!has_device_attributes(b"\x1b[?1016;2$y"));
        assert!(!has_device_attributes(b"\x1b[?62;22"));
        assert!(has_device_attributes(b"noise\x1b[?1016;2$y\x1b[?62;4c"));
        assert_eq!(
            device_attributes(b"\x1b[?1016;2$y\x1b[?62;4c"),
            Some(&b"62;4"[..])
        );
    }

    fn grid(width: u32, height: u32, rows: u16) -> Geometry {
        Geometry {
            cols: 100,
            rows,
            width,
            height,
        }
    }

    #[test]
    fn scale_is_two_for_retina_cells_and_shrinks_to_fit() {
        assert_eq!(scale(&grid(3000, 1800, 50), None), 2.);
        assert_eq!(scale(&grid(1500, 900, 50), None), 1.);
        // Retina cells, but 2x would leave a 700x400 window; the width is the
        // tighter fit.
        let small = scale(&grid(1400, 800, 25), None);
        assert!((small - 1400. / 850.).abs() < 1e-4, "{small}");
        assert_eq!(scale(&grid(600, 300, 25), None), 1.);
        assert_eq!(scale(&grid(3000, 1800, 50), Some(1.5)), 1.5);
        // Windows Terminal's 10x20 cells.
        assert_eq!(scale(&grid(1200, 600, 30), None), 1.);
    }
}
