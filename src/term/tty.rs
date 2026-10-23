//! The terminal itself: taking it over, asking what it supports, its size,
//! writing to it, and putting it back the way it was.
//!
//! All drawing goes to `/dev/tty`, never stdout, so `--pick` can still print
//! its JSON to stdout for a script.

use anyhow::{Context as _, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use fffinder_core::config;
use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use super::kitty;

/// The window is never laid out smaller than this, in logical pixels, unless
/// the terminal is too small even at a scale of 1. Matches the GUI's minimum.
const MIN_LOGICAL: (f32, f32) = (850., 470.);
/// How long to wait for the terminal to answer the startup queries.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Escape codes that set the terminal up, sent once the probe is done.
const ENTER: &str = concat!(
    "\x1b[?1049h",                                  // alternate screen
    "\x1b[?25l",                                    // hide the text cursor
    "\x1b[22;0t",                                   // save the window title
    "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h", // every mouse event, SGR encoded
    "\x1b[?1004h",                                  // focus in and out
    "\x1b[?2004h",                                  // bracketed paste
    // Kitty keyboard protocol: unambiguous codes, presses, repeats, and
    // releases, shifted keys from the layout, and modifier keys on their own.
    "\x1b[>15u",
);
/// Mouse positions in pixels instead of cells; sent only when supported.
const PIXEL_MOUSE: &str = "\x1b[?1016h";
/// Undoes [`ENTER`] and [`PIXEL_MOUSE`].
const LEAVE: &str = concat!(
    // Ends an image the writer may have been stopped in the middle of.
    "\x1b\\",
    "\x1b_Ga=d,d=A,q=2\x1b\\",
    "\x1b[<u",
    "\x1b[?2004l\x1b[?1004l",
    "\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l",
    "\x1b]22;default\x1b\\",
    "\x1b[23;0t",
    "\x1b[?25h",
    "\x1b[?1049l",
);

/// The terminal, open for writing. Frames and restoring share it, so neither
/// interrupts the other's escape codes.
static OUT: OnceLock<Mutex<File>> = OnceLock::new();
/// Set while the terminal is taken over.
static ENTERED: AtomicBool = AtomicBool::new(false);
/// The original stderr while it's redirected to the log, or -1.
static SAVED_STDERR: AtomicI32 = AtomicI32::new(-1);

/// What the terminal said it supports.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Caps {
    /// Images through the kitty graphics protocol.
    pub kitty: bool,
    /// Images handed over in POSIX shared memory instead of escape codes.
    /// Only works when the terminal runs on this machine.
    pub shared_memory: bool,
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
    /// Put the terminal in raw mode, check that it can show kitty images, and
    /// switch to the alternate screen with mouse and keyboard reporting on.
    /// From then on stderr goes to `term.log` in the data folder, and the
    /// terminal is put back on exit or on a panic in the main thread.
    ///
    /// # Errors
    ///
    /// Fails, leaving the terminal as it was, when there is no terminal, it
    /// doesn't answer, it has no kitty graphics, or it doesn't report its
    /// size in pixels.
    pub fn start() -> anyhow::Result<Session> {
        let out = OpenOptions::new()
            .write(true)
            .open("/dev/tty")
            .context("fffinder --term needs a terminal")?;
        crossterm::terminal::enable_raw_mode().context("could not put the terminal in raw mode")?;
        let checked = probe(&out).and_then(|caps| {
            if !caps.kitty {
                bail!(
                    "this terminal can't show kitty graphics; fffinder --term works in \
                     kitty, Ghostty, WezTerm, and Konsole (not inside tmux)"
                );
            }
            Ok((caps, geometry(caps.cell)?))
        });
        let (caps, geometry) = match checked {
            Ok(found) => found,
            Err(err) => {
                let _ = crossterm::terminal::disable_raw_mode();
                return Err(err);
            }
        };

        let mut enter = String::from(ENTER);
        if caps.pixel_mouse {
            enter.push_str(PIXEL_MOUSE);
        }
        let out = OUT.get_or_init(|| Mutex::new(out));
        write_locked(out, enter.as_bytes()).context("could not write to the terminal")?;
        ENTERED.store(true, Ordering::SeqCst);
        install_restore_hooks();
        redirect_stderr();
        install_logger();

        let requested_scale = config::env_var("FFFINDER_TERM_SCALE")
            .and_then(|value| value.to_str()?.parse().ok())
            .filter(|scale: &f32| scale.is_finite() && *scale > 0.);
        let cell = geometry.cell();
        log::info!(
            "{}x{} cells of {:.1}x{:.1} px ({}x{} px), scale {}, frames by {}, mouse in {}",
            geometry.cols,
            geometry.rows,
            cell.0,
            cell.1,
            geometry.width,
            geometry.height,
            scale(&geometry, requested_scale),
            if caps.shared_memory {
                "shared memory"
            } else {
                "escape codes"
            },
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

/// The terminal's size now. Pixels come from the kernel's window size, or
/// from the cell size when the terminal leaves those at zero.
///
/// # Errors
///
/// Fails when the size can't be read, or there are no pixels to go by.
pub fn geometry(cell: Option<(u16, u16)>) -> anyhow::Result<Geometry> {
    let size = crossterm::terminal::window_size().context("could not read the terminal size")?;
    let (width, height) = match (size.width, size.height, cell) {
        (width, height, _) if width > 0 && height > 0 => (u32::from(width), u32::from(height)),
        (_, _, Some((cell_width, cell_height))) => (
            u32::from(cell_width) * u32::from(size.columns),
            u32::from(cell_height) * u32::from(size.rows),
        ),
        _ => bail!("the terminal doesn't report its size in pixels"),
    };
    Ok(Geometry {
        cols: size.columns,
        rows: size.rows,
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
    file.write_all(bytes)?;
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
                let _ = file.write_all(LEAVE.as_bytes());
                let _ = file.flush();
                std::mem::forget(file);
            }
            None => {
                if let Ok(mut file) = OpenOptions::new().write(true).open("/dev/tty") {
                    let _ = file.write_all(LEAVE.as_bytes());
                }
            }
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();
    let saved = SAVED_STDERR.swap(-1, Ordering::SeqCst);
    if saved >= 0 {
        // SAFETY: `saved` is the descriptor `redirect_stderr` duplicated.
        unsafe {
            libc::dup2(saved, libc::STDERR_FILENO);
            libc::close(saved);
        }
    }
}

/// Restore on any exit, including `std::process::exit` from the picker, and
/// on a panic in the main thread. Other threads' panics only reach the log,
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
        .or_else(|_| OpenOptions::new().write(true).open("/dev/null"));
    let Ok(log) = log else {
        return;
    };
    // SAFETY: duplicates and replaces standard descriptors this process owns.
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved >= 0 && libc::dup2(log.as_raw_fd(), libc::STDERR_FILENO) >= 0 {
            SAVED_STDERR.store(saved, Ordering::SeqCst);
        }
    }
}

/// Ask what the terminal supports and read its answers, ending with the
/// primary device attributes every terminal answers.
fn probe(mut out: &File) -> anyhow::Result<Caps> {
    let shm_name = format!("/fff-probe-{}", std::process::id());
    let shm = kitty::write_shm(&shm_name, &[0; 3]).is_ok();
    let mut query = String::from("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    if shm {
        query.push_str(&format!(
            "\x1b_Gi=32,s=1,v=1,a=q,t=s,f=24,S=3;{}\x1b\\",
            BASE64.encode(&shm_name)
        ));
    }
    query.push_str("\x1b[?1016$p\x1b[16t\x1b[c");
    out.write_all(query.as_bytes())?;
    out.flush()?;
    let replies = read_replies(PROBE_TIMEOUT);
    if shm {
        // The terminal removes it after reading it, if it did.
        kitty::unlink_shm(&shm_name);
    }
    let replies = replies.context("could not read the terminal's answers")?;
    if !has_device_attributes(&replies) {
        bail!("the terminal didn't answer within {PROBE_TIMEOUT:?}");
    }
    Ok(parse_replies(&replies))
}

/// Read from the terminal until it has sent its device attributes, or the
/// time is up. Uses `select`, since macOS can't `poll` a terminal.
fn read_replies(timeout: Duration) -> io::Result<Vec<u8>> {
    let mut tty = File::open("/dev/tty")?;
    let fd = tty.as_raw_fd();
    let deadline = Instant::now() + timeout;
    let mut replies = Vec::new();
    let mut chunk = [0u8; 1024];
    while !has_device_attributes(&replies) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut wait = libc::timeval {
            tv_sec: left.as_secs() as libc::time_t,
            tv_usec: left.subsec_micros() as libc::suseconds_t,
        };
        // SAFETY: `fd_set` is plain data, set up by FD_ZERO and FD_SET for one
        // open descriptor below FD_SETSIZE.
        let ready = unsafe {
            let mut readable: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut readable);
            libc::FD_SET(fd, &mut readable);
            libc::select(
                fd + 1,
                &mut readable,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut wait,
            )
        };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ready == 0 {
            break;
        }
        let read = tty.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        replies.extend_from_slice(&chunk[..read]);
    }
    Ok(replies)
}

/// The capabilities in the terminal's answers to [`probe`]'s queries.
fn parse_replies(replies: &[u8]) -> Caps {
    let text = String::from_utf8_lossy(replies);
    Caps {
        kitty: text.contains("\x1b_Gi=31;OK"),
        shared_memory: text.contains("\x1b_Gi=32;OK"),
        // 1 set, 2 reset, 3 permanently set; 0 is unknown, 4 permanently reset.
        pixel_mouse: mode_report(&text, 1016).is_some_and(|state| matches!(state, 1..=3)),
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

/// Whether the answers include primary device attributes, `CSI ? … c`.
fn has_device_attributes(replies: &[u8]) -> bool {
    replies.windows(3).enumerate().any(|(start, window)| {
        window == b"\x1b[?"
            && replies[start + 3..]
                .iter()
                .find(|byte| !(byte.is_ascii_digit() || **byte == b';'))
                == Some(&b'c')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHOSTTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\\x1b[?1016;2$y\
        \x1b[6;34;16t\x1b[?62;22;52c";

    #[test]
    fn reads_a_full_set_of_answers() {
        assert_eq!(
            parse_replies(GHOSTTY),
            Caps {
                kitty: true,
                shared_memory: true,
                pixel_mouse: true,
                cell: Some((16, 34)),
            }
        );
        assert!(has_device_attributes(GHOSTTY));
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
    }
}
