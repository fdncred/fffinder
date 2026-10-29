//! The Windows console, as a terminal such as Windows Terminal or WezTerm hosts
//! it: escape codes and images go out through `CONOUT$`, and keys and the
//! mouse come in as console input records, which crossterm reads.

use ::windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use ::windows::Win32::System::Console::{
    CONSOLE_MODE, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT,
    ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
    GetConsoleOutputCP, GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW, STD_ERROR_HANDLE,
    SetConsoleMode, SetConsoleOutputCP, SetStdHandle,
};
use ::windows::Win32::System::Threading::WaitForSingleObject;
use crossterm::Command as _;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::WindowSize;
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::os::windows::io::{AsRawHandle as _, IntoRawHandle as _};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Instant;

use super::{Caps, PROBE_TIMEOUT, has_device_attributes};

/// Kitty graphics, whether mode 8452 is there to keep sixel images from
/// scrolling the screen, the cell size in pixels, and then the primary device
/// attributes, which also say whether there are sixel images.
const QUERIES: &str = concat!(
    "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\",
    "\x1b[?8452$p",
    "\x1b[16t",
    "\x1b[c",
);
/// Escape codes that set the terminal up, sent once the probe is done.
const ENTER: &str = concat!(
    "\x1b[?1049h", // alternate screen
    "\x1b[?25l",   // hide the text cursor
    "\x1b[22;0t",  // save the window title
    "\x1b[?80l",   // sixel scrolling mode: images start at the cursor
);
/// The cursor goes beside a sixel image, not below it; sent when supported.
const SIXEL_CURSOR: &str = "\x1b[?8452h";
/// Undoes [`ENTER`] and [`SIXEL_CURSOR`].
pub const LEAVE: &str = concat!(
    // Ends an image the writer may have been stopped in the middle of.
    "\x1b\\",
    "\x1b_Ga=d,d=A,q=2\x1b\\",
    "\x1b[?8452l",
    "\x1b[23;0t",
    "\x1b[?25h",
    "\x1b[?1049l",
);
/// Where stderr goes when `term.log` can't be opened.
pub const NULL_DEVICE: &str = "NUL";
/// The UTF-8 code page.
const UTF8: u32 = 65001;
/// The most written to the console at once. Very large writes to a console
/// have failed with `ERROR_NOT_ENOUGH_MEMORY`, so Rust's own stdout keeps to
/// this, and so do frames, which run to hundreds of kilobytes.
const WRITE_CHUNK: usize = 8 * 1024;

/// The console output's mode and code page before [`prepare`] changed them.
static OUTPUT_MODE: OnceLock<(CONSOLE_MODE, u32)> = OnceLock::new();
/// The original stderr while it's redirected to the log, or null.
static SAVED_STDERR: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// The console's screen, open for writing and for reading its mode.
///
/// # Errors
///
/// Fails when the process has no console.
pub fn open() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("CONOUT$")
}

/// Write all of `bytes` to the console, a piece at a time.
///
/// # Errors
///
/// Fails once the console is gone.
pub fn write_all(out: &mut File, bytes: &[u8]) -> io::Result<()> {
    bytes
        .chunks(WRITE_CHUNK)
        .try_for_each(|chunk| out.write_all(chunk))
}

/// Have the console pass escape codes through to the terminal as they are,
/// and read what's written as UTF-8, as the window title is.
///
/// # Errors
///
/// Fails when the console's mode or code page can't be read or set.
pub fn prepare(out: &File) -> io::Result<()> {
    let handle = HANDLE(out.as_raw_handle());
    let mut mode = CONSOLE_MODE::default();
    // SAFETY: `handle` is the console output `out` holds open.
    unsafe { GetConsoleMode(handle, &mut mode) }?;
    // SAFETY: reads this process's console's code page.
    let code_page = unsafe { GetConsoleOutputCP() };
    let _ = OUTPUT_MODE.set((mode, code_page));
    let wanted = mode
        | ENABLE_PROCESSED_OUTPUT
        | ENABLE_VIRTUAL_TERMINAL_PROCESSING
        | DISABLE_NEWLINE_AUTO_RETURN;
    // SAFETY: as above.
    unsafe { SetConsoleMode(handle, wanted) }?;
    // SAFETY: sets this process's console's code page.
    unsafe { SetConsoleOutputCP(UTF8) }?;
    Ok(())
}

/// Ask what the terminal can show, and read the answers, which arrive as
/// typed characters while VT input is on.
///
/// # Errors
///
/// Fails when the console can't be written to or read from.
pub fn probe(mut out: &File) -> io::Result<Vec<u8>> {
    let input = OpenOptions::new().read(true).write(true).open("CONIN$")?;
    let handle = HANDLE(input.as_raw_handle());
    let mut original = CONSOLE_MODE::default();
    // SAFETY: `handle` is the console input `input` holds open for the call.
    unsafe { GetConsoleMode(handle, &mut original) }?;
    // SAFETY: as above.
    unsafe { SetConsoleMode(handle, ENABLE_VIRTUAL_TERMINAL_INPUT) }?;
    let replies = out
        .write_all(QUERIES.as_bytes())
        .and_then(|()| out.flush())
        .and_then(|()| read_replies(handle));
    // SAFETY: as above.
    let _ = unsafe { SetConsoleMode(handle, original) };
    replies
}

/// Read typed characters until the device attributes are in, or the time is
/// up. Other input records, such as focus changes, are dropped.
fn read_replies(input: HANDLE) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut replies = Vec::new();
    let mut records = [INPUT_RECORD::default(); 64];
    while !has_device_attributes(&replies) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let millis = u32::try_from(left.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: waits on a handle the caller keeps open.
        let waited = unsafe { WaitForSingleObject(input, millis) };
        if waited == WAIT_TIMEOUT {
            break;
        }
        if waited != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut read = 0;
        // SAFETY: fills at most `records.len()` records of a buffer we own.
        unsafe { ReadConsoleInputW(input, &mut records, &mut read) }?;
        for record in records.iter().take(read as usize) {
            if u32::from(record.EventType) != KEY_EVENT {
                continue;
            }
            // SAFETY: `EventType` says the union holds a key event, and both
            // fields of its character union are 16 bits.
            let (down, unit) = unsafe {
                let key = record.Event.KeyEvent;
                (key.bKeyDown.as_bool(), key.uChar.UnicodeChar)
            };
            // The answers are plain ASCII.
            if let Ok(byte) = u8::try_from(unit)
                && down
                && byte.is_ascii()
            {
                replies.push(byte);
            }
        }
    }
    Ok(replies)
}

/// Frames go out as kitty graphics, or else as sixel images.
///
/// # Errors
///
/// Fails when the terminal can show neither.
pub fn check(caps: &Caps) -> anyhow::Result<()> {
    if !caps.kitty && !caps.sixel {
        anyhow::bail!(
            "this terminal can't show kitty graphics or sixel images; fffinder --term on Windows \
             works in Windows Terminal 1.22 or later and WezTerm"
        );
    }
    Ok(())
}

/// The codes that take the terminal over.
pub fn enter(caps: &Caps) -> String {
    let mut enter = String::from(ENTER);
    if !caps.kitty && caps.sixel_cursor {
        enter.push_str(SIXEL_CURSOR);
    }
    enter
}

/// Mouse events as console input records. This also turns off Quick Edit,
/// which would take the mouse for selecting text.
///
/// # Errors
///
/// Fails when the console input's mode can't be set.
pub fn start_input() -> io::Result<()> {
    EnableMouseCapture.execute_winapi()
}

/// Put the console's input and output modes, and its code page, back.
pub fn restore_modes() {
    let _ = DisableMouseCapture.execute_winapi();
    if let (Some((mode, code_page)), Ok(out)) = (OUTPUT_MODE.get(), open()) {
        // SAFETY: `out` holds the console output open for the call, and the
        // code page is this process's console's own.
        unsafe {
            let _ = SetConsoleMode(HANDLE(out.as_raw_handle()), *mode);
            let _ = SetConsoleOutputCP(*code_page);
        }
    }
}

/// The console's size in cells; it knows nothing of pixels.
///
/// # Errors
///
/// Fails when the size can't be read.
pub fn window_size() -> io::Result<WindowSize> {
    let (columns, rows) = crossterm::terminal::size()?;
    Ok(WindowSize {
        rows,
        columns,
        width: 0,
        height: 0,
    })
}

/// How frames reach the terminal, for the log.
pub fn frames_as(caps: &Caps) -> &'static str {
    match (caps.kitty, caps.sixel_cursor) {
        (true, _) => "kitty graphics escape codes",
        (false, true) => "sixel images, with the cursor beside them",
        (false, false) => "sixel images",
    }
}

/// Point stderr at `log`, keeping the original to put back. The log stays
/// open until the process ends.
pub fn redirect_stderr(log: File) {
    let log = HANDLE(log.into_raw_handle());
    // SAFETY: swaps this process's standard error handle for a file handle
    // that is never closed.
    unsafe {
        if let Ok(saved) = GetStdHandle(STD_ERROR_HANDLE)
            && SetStdHandle(STD_ERROR_HANDLE, log).is_ok()
        {
            SAVED_STDERR.store(saved.0, Ordering::SeqCst);
        }
    }
}

/// Put the original stderr back, if it was redirected.
pub fn restore_stderr() {
    let saved = SAVED_STDERR.swap(std::ptr::null_mut(), Ordering::SeqCst);
    if !saved.is_null() {
        // SAFETY: `saved` is the handle `redirect_stderr` replaced.
        let _ = unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(saved)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_or_sixel_and_the_cursor_mode_only_for_sixel() {
        let sixel = Caps {
            sixel: true,
            sixel_cursor: true,
            ..Caps::default()
        };
        assert!(enter(&sixel).ends_with(SIXEL_CURSOR));
        let kitty = Caps {
            kitty: true,
            ..sixel
        };
        assert!(!enter(&kitty).contains(SIXEL_CURSOR));
        let no_mode = Caps {
            sixel_cursor: false,
            ..sixel
        };
        assert!(!enter(&no_mode).contains(SIXEL_CURSOR));
        assert!(check(&sixel).is_ok() && check(&kitty).is_ok());
        assert!(check(&Caps::default()).is_err());
    }
}
