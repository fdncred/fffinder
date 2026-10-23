//! Frames to the terminal through the kitty graphics protocol, on a thread of
//! their own so a slow terminal never holds up the UI.
//!
//! Each frame is one RGBA image covering the whole grid. Frames alternate
//! between two image ids, so the new one is up before the old one is deleted
//! and nothing flickers. When frames arrive faster than the terminal takes
//! them, only the newest is sent.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::ffi::CString;
use std::io::{self, Write as _};
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use super::tty;

/// Input bytes per escape code: 3072 bytes encode to 4096 base64 characters,
/// the protocol's limit for one chunk.
const CHUNK: usize = 3072;

/// One rendered window, and the cells it covers.
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
    /// Start the writer thread. `shared_memory` hands frames over in POSIX
    /// shared memory, which only a terminal on this machine can read.
    pub fn spawn(shared_memory: bool) -> Writer {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("fffinder-term-writer".into())
            .spawn(move || run(&rx, shared_memory));
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

fn run(rx: &mpsc::Receiver<Message>, shared_memory: bool) {
    let mut encoder = Encoder::new(shared_memory);
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
            encoder.encode(&frame, &mut out);
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

/// Turns frames into escape codes.
struct Encoder {
    shared_memory: bool,
    frames: u64,
}

impl Encoder {
    fn new(shared_memory: bool) -> Encoder {
        Encoder {
            shared_memory,
            frames: 0,
        }
    }

    /// Append the codes that show `frame` at the top left and delete the
    /// frame before it.
    fn encode(&mut self, frame: &Frame, out: &mut Vec<u8>) {
        self.frames += 1;
        let (id, old) = if self.frames.is_multiple_of(2) {
            (2, 1)
        } else {
            (1, 2)
        };
        let keys = format!(
            "a=T,f=32,s={},v={},i={id},c={},r={},C=1,q=2",
            frame.width, frame.height, frame.cols, frame.rows
        );
        out.extend_from_slice(b"\x1b[H");
        let shared = self.shared_memory && {
            let name = format!("/fff-{}-{}", std::process::id(), self.frames);
            match write_shm(&name, &frame.rgba) {
                Ok(()) => {
                    let _ = write!(
                        out,
                        "\x1b_G{keys},t=s,S={};{}\x1b\\",
                        frame.rgba.len(),
                        BASE64.encode(&name)
                    );
                    true
                }
                Err(err) => {
                    log::warn!("shared memory for a frame: {err}; sending it inline");
                    false
                }
            }
        };
        if !shared {
            let mut zlib = ZlibEncoder::new(
                Vec::with_capacity(frame.rgba.len() / 16),
                Compression::fast(),
            );
            let compressed = zlib.write_all(&frame.rgba).and_then(|()| zlib.finish());
            match compressed {
                Ok(data) => chunked(&data, &format!("{keys},o=z"), out),
                Err(err) => {
                    log::warn!("compressing a frame: {err}; sending it uncompressed");
                    chunked(&frame.rgba, &keys, out);
                }
            }
        }
        let _ = write!(out, "\x1b_Ga=d,d=I,i={old},q=2\x1b\\");
    }
}

/// Direct transmission: the data in base64 over as many escape codes as it
/// takes, with `m=1` on all but the last.
fn chunked(data: &[u8], keys: &str, out: &mut Vec<u8>) {
    let chunks = data.chunks(CHUNK);
    let last = chunks.len().saturating_sub(1);
    for (ix, chunk) in chunks.enumerate() {
        let more = u8::from(ix != last);
        let _ = if ix == 0 {
            write!(out, "\x1b_G{keys},m={more};")
        } else {
            write!(out, "\x1b_Gm={more};")
        };
        let start = out.len();
        out.resize(start + chunk.len().div_ceil(3) * 4, 0);
        BASE64
            .encode_slice(chunk, &mut out[start..])
            .expect("the buffer was sized for this chunk");
        out.extend_from_slice(b"\x1b\\");
    }
}

/// Copy `data` into a new POSIX shared-memory object named `name`, for the
/// terminal to read and unlink.
///
/// # Errors
///
/// Fails when the object exists or can't be created, sized, or mapped.
pub fn write_shm(name: &str, data: &[u8]) -> io::Result<()> {
    let cname = CString::new(name).map_err(io::Error::other)?;
    // SAFETY: plain POSIX calls on a new object sized to `data`. The mapping
    // and the descriptor are released before returning.
    unsafe {
        let fd = libc::shm_open(
            cname.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            0o600 as libc::c_uint,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut result = Ok(());
        if libc::ftruncate(fd, data.len() as libc::off_t) != 0 {
            result = Err(io::Error::last_os_error());
        } else {
            let map = libc::mmap(
                std::ptr::null_mut(),
                data.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if map == libc::MAP_FAILED {
                result = Err(io::Error::last_os_error());
            } else {
                std::ptr::copy_nonoverlapping(data.as_ptr(), map.cast::<u8>(), data.len());
                libc::munmap(map, data.len());
            }
        }
        libc::close(fd);
        if result.is_err() {
            libc::shm_unlink(cname.as_ptr());
        }
        result
    }
}

/// Remove a shared-memory object; a missing one is fine.
pub fn unlink_shm(name: &str) {
    if let Ok(cname) = CString::new(name) {
        // SAFETY: unlinks a name; it touches no memory of ours.
        unsafe { libc::shm_unlink(cname.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read as _;

    fn frame(width: u32, height: u32) -> Frame {
        Frame {
            rgba: (0..width * height * 4).map(|ix| (ix % 251) as u8).collect(),
            width,
            height,
            cols: 80,
            rows: 24,
        }
    }

    /// The control keys and joined payload of each image escape code.
    fn images(out: &[u8]) -> Vec<(String, Vec<u8>)> {
        let text = String::from_utf8(out.to_vec()).expect("escape codes are ASCII");
        let mut found: Vec<(String, Vec<u8>)> = Vec::new();
        for code in text.split("\x1b_G").skip(1) {
            let body = code.split("\x1b\\").next().unwrap_or_default();
            let (keys, payload) = body.split_once(';').unwrap_or((body, ""));
            let data = BASE64.decode(payload).unwrap_or_default();
            match found.last_mut() {
                Some(last) if keys.starts_with("m=") => last.1.extend(data),
                _ => found.push((keys.to_string(), data)),
            }
        }
        found
    }

    #[test]
    fn inline_frames_decompress_to_the_pixels() {
        let frame = frame(300, 40);
        let mut out = Vec::new();
        Encoder::new(false).encode(&frame, &mut out);
        assert!(out.starts_with(b"\x1b[H"));
        let images = images(&out);
        assert_eq!(images.len(), 2, "one image and one delete");
        let (keys, data) = &images[0];
        assert!(keys.starts_with("a=T,f=32,s=300,v=40,i=1,c=80,r=24,C=1,q=2,o=z,m="));
        let mut pixels = Vec::new();
        ZlibDecoder::new(&data[..])
            .read_to_end(&mut pixels)
            .unwrap();
        assert_eq!(pixels, frame.rgba);
        assert_eq!(images[1].0, "a=d,d=I,i=2,q=2");
    }

    #[test]
    fn chunks_stay_within_the_protocol_limit() {
        let data: Vec<u8> = (0..10_000).map(|ix| (ix % 7) as u8).collect();
        let mut out = Vec::new();
        chunked(&data, "a=T", &mut out);
        let text = String::from_utf8(out).unwrap();
        let payloads: Vec<&str> = text
            .split("\x1b_G")
            .skip(1)
            .map(|code| code.split_once(';').unwrap().1.trim_end_matches("\x1b\\"))
            .collect();
        assert_eq!(payloads.len(), 4);
        assert!(payloads.iter().all(|payload| payload.len() <= 4096));
        assert!(text.contains("a=T,m=1;"));
        assert!(text.ends_with(&format!("\x1b_Gm=0;{}\x1b\\", payloads[3])));
    }

    #[test]
    fn ids_alternate_so_the_old_frame_outlives_the_new_one() {
        let mut encoder = Encoder::new(false);
        let frame = frame(4, 4);
        let ids: Vec<(String, String)> = (0..3)
            .map(|_| {
                let mut out = Vec::new();
                encoder.encode(&frame, &mut out);
                let images = images(&out);
                (images[0].0.clone(), images[1].0.clone())
            })
            .collect();
        assert!(ids[0].0.contains(",i=1,") && ids[0].1.ends_with("i=2,q=2"));
        assert!(ids[1].0.contains(",i=2,") && ids[1].1.ends_with("i=1,q=2"));
        assert!(ids[2].0.contains(",i=1,"));
    }

    #[test]
    fn shared_memory_frames_send_only_a_name() {
        let frame = frame(16, 8);
        let mut out = Vec::new();
        let mut encoder = Encoder::new(true);
        encoder.encode(&frame, &mut out);
        let images = images(&out);
        let (keys, name) = &images[0];
        assert!(keys.ends_with(&format!("t=s,S={}", frame.rgba.len())));
        let name = String::from_utf8(name.clone()).unwrap();
        assert_eq!(name, format!("/fff-{}-1", std::process::id()));
        unlink_shm(&name);
    }
}
