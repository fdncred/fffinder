//! `fffinder --term`: the same window, drawn inside the terminal.
//!
//! GPUI lays out and paints the window as usual, the frame is rendered
//! offscreen (Metal on macOS, wgpu on Linux), and the pixels go to the
//! terminal through the kitty graphics protocol. Keys and the mouse come back
//! as escape codes and reach GPUI as the input events a native window sends.
//! Everything else, such as the clipboard and opening files, is the regular
//! platform running without windows.
//!
//! Needs the `term` cargo feature, macOS or Linux, and a terminal with kitty
//! graphics.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(all(feature = "term", unix))]
mod input;
#[cfg(all(feature = "term", unix))]
mod kitty;
#[cfg(all(feature = "term", unix))]
mod platform;
#[cfg(all(feature = "term", unix))]
mod tty;
#[cfg(all(feature = "term", unix))]
mod window;

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Frames written to the terminal; the writer thread counts them.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// Whether this process draws its window in the terminal.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// How many frames have reached the terminal so far. Frames that arrived
/// while the terminal was still taking an earlier one, and were replaced by
/// a newer one, don't count. Always 0 outside the terminal.
pub fn frames_shown() -> u64 {
    FRAMES.load(Ordering::Relaxed)
}

/// Take over the terminal and return an app whose window is drawn there.
/// The terminal is put back when the process exits.
///
/// # Errors
///
/// Fails, leaving the terminal alone, when there is no terminal, it can't
/// show kitty graphics or report its size in pixels, or this build lacks the
/// `term` feature.
#[cfg(all(feature = "term", unix))]
pub fn application() -> anyhow::Result<gpui_kit::Application> {
    let session = tty::Session::start()?;
    ACTIVE.store(true, Ordering::Relaxed);
    Ok(gpui_kit::Application::with_platform(std::rc::Rc::new(
        platform::TerminalPlatform::new(session),
    )))
}

/// Without the `term` feature there is no terminal mode.
///
/// # Errors
///
/// Always.
#[cfg(not(all(feature = "term", unix)))]
pub fn application() -> anyhow::Result<gpui_kit::Application> {
    anyhow::bail!(
        "this build has no terminal mode; build it with `cargo build --release --features term` \
         on macOS or Linux"
    )
}

/// Put the terminal back the way it was, before printing to it. Does nothing
/// when the terminal wasn't taken over, or was already put back.
pub fn restore() {
    #[cfg(all(feature = "term", unix))]
    tty::restore();
}
