//! How long audio and video files run, read from their headers without
//! decoding anything: MP4 and QuickTime (the `mvhd` box), WAV (the `fmt `
//! and `data` chunks), and FLAC (`STREAMINFO`).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// The running time of the audio or video file at `path`, when its format
/// is one of these and its header says.
pub fn duration(path: &Path) -> Option<Duration> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let mut file = File::open(path).ok()?;
    match ext.as_str() {
        "mp4" | "m4v" | "m4a" | "m4b" | "mov" | "3gp" => mp4(&mut file).ok().flatten(),
        "wav" | "wave" => wav(&mut file).ok().flatten(),
        "flac" => flac(&mut file).ok().flatten(),
        _ => None,
    }
}

/// `3:07`, or `1:02:03` for an hour or more.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn read_u32_be(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_u64_be(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

/// Walk ISO media boxes from the current position to `end`, looking for
/// `moov` and then `mvhd`.
fn mp4(file: &mut File) -> io::Result<Option<Duration>> {
    let end = file.metadata()?.len();
    find_mvhd(file, end, 0)
}

fn find_mvhd(file: &mut File, end: u64, depth: u8) -> io::Result<Option<Duration>> {
    while file.stream_position()? + 8 <= end {
        let start = file.stream_position()?;
        let size = read_u32_be(file)? as u64;
        let mut kind = [0u8; 4];
        file.read_exact(&mut kind)?;
        let size = match size {
            // A 64-bit size follows the type.
            1 => read_u64_be(file)?,
            // The box runs to the end.
            0 => end - start,
            n => n,
        };
        // A box can't be smaller than its header or run past the largest
        // offset, which a damaged or crafted file may claim.
        let Some(next) = start.checked_add(size).filter(|_| size >= 8) else {
            return Ok(None);
        };
        match &kind {
            b"moov" if depth == 0 => return find_mvhd(file, next.min(end), 1),
            b"mvhd" => {
                let mut version = [0u8; 4];
                file.read_exact(&mut version)?;
                let (timescale, units) = if version[0] == 1 {
                    file.seek(SeekFrom::Current(16))?;
                    (read_u32_be(file)?, read_u64_be(file)?)
                } else {
                    file.seek(SeekFrom::Current(8))?;
                    (read_u32_be(file)?, read_u32_be(file)? as u64)
                };
                return Ok((timescale > 0)
                    .then(|| Duration::from_secs_f64(units as f64 / timescale as f64)));
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(next))?;
    }
    Ok(None)
}

/// RIFF chunks: the byte rate in `fmt `, and the length of `data`.
fn wav(file: &mut File) -> io::Result<Option<Duration>> {
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Ok(None);
    }
    let mut byte_rate = None;
    loop {
        let mut chunk = [0u8; 8];
        if file.read_exact(&mut chunk).is_err() {
            return Ok(None);
        }
        let len = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        match &chunk[..4] {
            b"fmt " => {
                let mut fmt = [0u8; 12];
                file.read_exact(&mut fmt)?;
                byte_rate = Some(u32::from_le_bytes([fmt[8], fmt[9], fmt[10], fmt[11]]));
                file.seek(SeekFrom::Current(len as i64 - 12 + (len % 2) as i64))?;
            }
            b"data" => {
                return Ok(byte_rate
                    .filter(|&rate| rate > 0)
                    .map(|rate| Duration::from_secs_f64(len as f64 / rate as f64)));
            }
            // Chunks are padded to an even length.
            _ => {
                file.seek(SeekFrom::Current((len + len % 2) as i64))?;
            }
        }
    }
}

/// `STREAMINFO` packs a 20-bit sample rate and 36-bit sample count into
/// bytes 10 to 17.
fn flac(file: &mut File) -> io::Result<Option<Duration>> {
    let mut head = [0u8; 4 + 4 + 18];
    file.read_exact(&mut head)?;
    if &head[..4] != b"fLaC" || head[4] & 0x7F != 0 {
        return Ok(None);
    }
    let info = &head[8..];
    let rate =
        (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
    let samples = (u64::from(info[13] & 0x0F) << 32)
        | (u64::from(info[14]) << 24)
        | (u64::from(info[15]) << 16)
        | (u64::from(info[16]) << 8)
        | u64::from(info[17]);
    Ok((rate > 0 && samples > 0).then(|| Duration::from_secs_f64(samples as f64 / rate as f64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("fffinder-media-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_an_mp4_duration() {
        // mvhd version 0: flags, two dates, timescale 600, duration 90,000 (150 s).
        let mut mvhd = vec![0u8; 12];
        mvhd.extend_from_slice(&600u32.to_be_bytes());
        mvhd.extend_from_slice(&90_000u32.to_be_bytes());
        mvhd.extend_from_slice(&[0u8; 80]);
        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend(mp4_box(b"moov", &mp4_box(b"mvhd", &mvhd)));
        let path = temp("a.mp4", &file);
        assert_eq!(duration(&path), Some(Duration::from_secs(150)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_box_too_big_to_be_real_ends_the_walk() {
        // A 64-bit size near the largest offset, then a `moov` that's never reached.
        let mut file = 1u32.to_be_bytes().to_vec();
        file.extend_from_slice(b"free");
        file.extend_from_slice(&(u64::MAX - 4).to_be_bytes());
        file.extend(mp4_box(b"moov", &[0u8; 8]));
        let path = temp("huge.mp4", &file);
        assert_eq!(duration(&path), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_a_wav_duration() {
        let rate: u32 = 44_100 * 2 * 2;
        let mut file = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        file.extend_from_slice(&16u32.to_le_bytes());
        file.extend_from_slice(&[1, 0, 2, 0]);
        file.extend_from_slice(&44_100u32.to_le_bytes());
        file.extend_from_slice(&rate.to_le_bytes());
        file.extend_from_slice(&[4, 0, 16, 0]);
        file.extend_from_slice(b"data");
        file.extend_from_slice(&(rate * 3).to_le_bytes());
        let path = temp("a.wav", &file);
        assert_eq!(duration(&path), Some(Duration::from_secs(3)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_a_flac_duration() {
        let mut file = b"fLaC\x00\x00\x00\x22".to_vec();
        let mut info = [0u8; 18];
        // 48,000 Hz and 96,000 samples: two seconds.
        let (rate, samples): (u32, u64) = (48_000, 96_000);
        info[10] = (rate >> 12) as u8;
        info[11] = (rate >> 4) as u8;
        info[12] = ((rate & 0xF) << 4) as u8;
        info[14] = (samples >> 24) as u8;
        info[15] = (samples >> 16) as u8;
        info[16] = (samples >> 8) as u8;
        info[17] = samples as u8;
        file.extend_from_slice(&info);
        let path = temp("a.flac", &file);
        assert_eq!(duration(&path), Some(Duration::from_secs(2)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_secs(187)), "3:07");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
    }
}
