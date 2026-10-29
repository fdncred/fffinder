//! Frames to the terminal on a thread of their own, so a slow terminal never
//! holds up the UI. When frames arrive faster than the terminal takes them,
//! only the newest is sent.
//!
//! The encoder turns a frame into escape codes: kitty graphics wherever the
//! terminal has them, and on Windows sixel images where it doesn't.

use std::sync::atomic::Ordering;
use std::sync::mpsc;

use super::kitty;
#[cfg(windows)]
use super::sixel;
use super::tty::{self, Caps};

/// One rendered window, and the cells it covers.
#[derive(Clone)]
pub struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub cols: u16,
    pub rows: u16,
}

enum Message {
    Frame(Frame),
    /// Other escape codes: the title, the pointer shape.
    Codes(String),
}

/// Sends frames and escape codes to the writer thread.
#[derive(Clone)]
pub struct Writer(mpsc::Sender<Message>);

impl Writer {
    /// Start the writer thread, encoding frames the way `caps` allow.
    pub fn spawn(caps: &Caps) -> Writer {
        let (tx, rx) = mpsc::channel();
        let encoder = Encoder::new(caps);
        let spawned = std::thread::Builder::new()
            .name("fffinder-term-writer".into())
            .spawn(move || run(&rx, encoder));
        if let Err(err) = spawned {
            log::error!("could not start the terminal writer: {err}");
        }
        Writer(tx)
    }

    pub fn frame(&self, frame: Frame) {
        let _ = self.0.send(Message::Frame(frame));
    }

    pub fn codes(&self, codes: String) {
        let _ = self.0.send(Message::Codes(codes));
    }
}

/// Turns frames into escape codes the terminal can show.
enum Encoder {
    Kitty(kitty::Encoder),
    #[cfg(windows)]
    Sixel(sixel::Encoder),
}

impl Encoder {
    fn new(caps: &Caps) -> Encoder {
        #[cfg(windows)]
        if !caps.kitty {
            return Encoder::Sixel(sixel::Encoder::new(caps));
        }
        Encoder::Kitty(kitty::Encoder::new(caps))
    }

    fn encode(&mut self, frame: Frame, out: &mut Vec<u8>) {
        match self {
            Encoder::Kitty(encoder) => encoder.encode(frame, out),
            #[cfg(windows)]
            Encoder::Sixel(encoder) => encoder.encode(frame, out),
        }
    }
}

fn run(rx: &mpsc::Receiver<Message>, mut encoder: Encoder) {
    let mut out = Vec::new();
    while let Ok(first) = rx.recv() {
        out.clear();
        let mut newest = None;
        for message in std::iter::once(first).chain(rx.try_iter()) {
            match message {
                Message::Frame(frame) => newest = Some(frame),
                Message::Codes(codes) => out.extend_from_slice(codes.as_bytes()),
            }
        }
        let has_frame = newest.is_some();
        if let Some(frame) = newest {
            encoder.encode(frame, &mut out);
        }
        if let Err(err) = tty::write(&out) {
            log::error!("writing to the terminal: {err}");
            return;
        }
        if has_frame {
            super::FRAMES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn kitty_comes_before_sixel() {
        let both = Caps {
            kitty: true,
            sixel: true,
            ..Caps::default()
        };
        assert!(matches!(Encoder::new(&both), Encoder::Kitty(_)));
        let sixel = Caps {
            kitty: false,
            ..both
        };
        assert!(matches!(Encoder::new(&sixel), Encoder::Sixel(_)));
    }
}
