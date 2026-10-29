//! A Unix terminal through `/dev/tty`: kitty graphics, SGR mouse reports,
//! and the kitty keyboard protocol.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crossterm::terminal::WindowSize;
use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;

use super::super::kitty;
use super::{Caps, PROBE_TIMEOUT, has_device_attributes};

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
pub const LEAVE: &str = concat!(
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
/// Where stderr goes when `term.log` can't be opened.
pub const NULL_DEVICE: &str = "/dev/null";

/// The original stderr while it's redirected to the log, or -1.
static SAVED_STDERR: AtomicI32 = AtomicI32::new(-1);

/// The terminal, open for writing.
///
/// # Errors
///
/// Fails when the process has no controlling terminal.
pub fn open() -> io::Result<File> {
    OpenOptions::new().write(true).open("/dev/tty")
}

/// Write all of `bytes` to the terminal.
///
/// # Errors
///
/// Fails once the terminal is gone.
pub fn write_all(out: &mut File, bytes: &[u8]) -> io::Result<()> {
    out.write_all(bytes)
}

/// Nothing to set up before the probe: a terminal takes escape codes as is.
///
/// # Errors
///
/// Never.
pub fn prepare(_out: &File) -> io::Result<()> {
    Ok(())
}

/// Ask about kitty graphics, shared memory, pixel mouse reports, and the cell
/// size, then the primary device attributes, and read the answers.
///
/// # Errors
///
/// Fails when the terminal can't be written to or read from.
pub fn probe(mut out: &File) -> io::Result<Vec<u8>> {
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
    let replies = out
        .write_all(query.as_bytes())
        .and_then(|()| out.flush())
        .and_then(|()| read_replies());
    if shm {
        // The terminal removes it after reading it, if it did.
        kitty::unlink_shm(&shm_name);
    }
    replies
}

/// Read from the terminal until it has sent its device attributes, or the
/// time is up. Uses `select`, since macOS can't `poll` a terminal.
fn read_replies() -> io::Result<Vec<u8>> {
    let mut tty = File::open("/dev/tty")?;
    let fd = tty.as_raw_fd();
    let deadline = Instant::now() + PROBE_TIMEOUT;
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

/// Frames go out as kitty graphics.
///
/// # Errors
///
/// Fails when the terminal has no kitty graphics.
pub fn check(caps: &Caps) -> anyhow::Result<()> {
    if !caps.kitty {
        anyhow::bail!(
            "this terminal can't show kitty graphics; fffinder --term works in kitty, Ghostty, \
             WezTerm, and Konsole (not inside tmux)"
        );
    }
    Ok(())
}

/// The codes that take the terminal over.
pub fn enter(caps: &Caps) -> String {
    let mut enter = String::from(ENTER);
    if caps.pixel_mouse {
        enter.push_str(PIXEL_MOUSE);
    }
    enter
}

/// Mouse reports were turned on by [`enter`]'s codes.
///
/// # Errors
///
/// Never.
pub fn start_input() -> io::Result<()> {
    Ok(())
}

/// [`LEAVE`] puts everything back; there are no modes besides raw mode.
pub fn restore_modes() {}

/// The terminal's size in cells, and in pixels when the kernel knows it.
///
/// # Errors
///
/// Fails when the size can't be read.
pub fn window_size() -> io::Result<WindowSize> {
    crossterm::terminal::window_size()
}

/// How frames reach the terminal, for the log.
pub fn frames_as(caps: &Caps) -> &'static str {
    if caps.shared_memory {
        "kitty graphics in shared memory"
    } else {
        "kitty graphics escape codes"
    }
}

/// Point stderr at `log`, keeping the original to put back.
pub fn redirect_stderr(log: File) {
    // SAFETY: duplicates and replaces standard descriptors this process owns.
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved >= 0 && libc::dup2(log.as_raw_fd(), libc::STDERR_FILENO) >= 0 {
            SAVED_STDERR.store(saved, Ordering::SeqCst);
        }
    }
}

/// Put the original stderr back, if it was redirected.
pub fn restore_stderr() {
    let saved = SAVED_STDERR.swap(-1, Ordering::SeqCst);
    if saved >= 0 {
        // SAFETY: `saved` is the descriptor `redirect_stderr` duplicated.
        unsafe {
            libc::dup2(saved, libc::STDERR_FILENO);
            libc::close(saved);
        }
    }
}
