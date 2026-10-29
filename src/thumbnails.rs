//! Thumbnails the system draws for files: PDF pages, video frames, office
//! documents, fonts, and images GPUI can't decode. macOS makes them with
//! QuickLook, Windows with the shell's thumbnail handlers (Explorer's), and
//! Linux with the freedesktop.org thumbnailers its desktop installed. They're
//! cached as PNGs in the data folder and loaded as GPUI assets, so a view
//! shows the file's icon until its thumbnail is ready and then redraws. A
//! type no thumbnailer draws keeps its icon.

use fffinder_core::{FileEntry, FileKind};
use gpui_kit::{
    App, AppContext as _, Asset, ImageCacheError, ImageSource, ImgResourceLoader, Resource,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

/// The most the cached thumbnails may take; the least recently used go
/// first when they take more.
const CACHE_LIMIT: u64 = 256 * 1024 * 1024;

/// What to draw and how big: the file, its modification time (so an edited
/// file gets a new thumbnail), and the size in pixels.
type Key = (PathBuf, Option<SystemTime>, u32);

/// Types with a useful picture: everything that isn't plain text, code, a
/// folder, or an app, which keep their icons.
pub fn wanted(entry: &FileEntry) -> bool {
    if entry.is_dir {
        return false;
    }
    let ext = entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match entry.kind {
        FileKind::Pdf | FileKind::Video | FileKind::Image => true,
        FileKind::Text | FileKind::Data | FileKind::Other => matches!(
            ext.as_str(),
            "doc"
                | "docx"
                | "xls"
                | "xlsx"
                | "ppt"
                | "pptx"
                | "pages"
                | "numbers"
                | "key"
                | "rtf"
                | "odt"
                | "ods"
                | "odp"
                | "epub"
                | "ttf"
                | "otf"
                | "ttc"
                | "psd"
                | "ai"
                | "eps"
                | "usdz"
                | "stl"
                | "obj"
        ),
        FileKind::Folder
        | FileKind::Application
        | FileKind::Audio
        | FileKind::Archive
        | FileKind::Code
        | FileKind::Executable => false,
    }
}

/// An image of `entry`'s thumbnail at `px` pixels across, for `img()`. It
/// loads in the background; until then, and when there is none, the
/// image's loading and fallback elements show.
pub fn image(entry: &FileEntry, px: u32) -> ImageSource {
    let key: Key = (entry.path.clone(), entry.modified, px);
    ImageSource::Custom(Arc::new(move |window, cx| {
        // UI tests run on a deterministic executor that our worker threads
        // would wake from outside, so they keep the icons.
        if cx.has_global::<crate::system::Recorder>() {
            return Some(Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
                "no thumbnails in tests"
            )))));
        }
        match window.use_asset::<Thumbnail>(&key, cx)? {
            Some(png) => window.use_asset::<ImgResourceLoader>(&Resource::Path(png.into()), cx),
            None => Some(Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
                "no thumbnail"
            ))))),
        }
    }))
}

/// The thumbnail PNG for a [`Key`], made once and then read from disk.
enum Thumbnail {}

impl Asset for Thumbnail {
    type Source = Key;
    type Output = Option<PathBuf>;

    fn load(
        key: Key,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Option<PathBuf>> + Send + 'static {
        let (reply, answer) = async_channel::bounded(1);
        let sent = queue().send_blocking(Job { key, reply }).is_ok();
        cx.background_spawn(async move {
            if sent {
                answer.recv().await.ok().flatten()
            } else {
                None
            }
        })
    }
}

struct Job {
    key: Key,
    reply: async_channel::Sender<Option<PathBuf>>,
}

/// QuickLook does its work elsewhere and calls back, so a few threads of
/// our own wait on it instead of tying up GPUI's background threads. The
/// first time, another thread trims the cache.
fn queue() -> &'static async_channel::Sender<Job> {
    static QUEUE: OnceLock<async_channel::Sender<Job>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("thumbnails-prune".into())
            .spawn(|| prune(&cache_dir(), CACHE_LIMIT));
        let (tx, rx) = async_channel::unbounded::<Job>();
        for n in 0..3 {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("thumbnails-{n}"))
                .spawn(move || {
                    while let Ok(job) = rx.recv_blocking() {
                        let _ = job.reply.send_blocking(cached(&job.key));
                    }
                });
        }
        tx
    })
}

fn cache_dir() -> PathBuf {
    fffinder_core::config::data_dir().join("thumbnails")
}

/// A name for `key`'s PNG that stays the same from one build of fffinder
/// to the next: FNV-1a over the path's bytes, the modification time, and
/// the size, unlike std's hasher, which may change with Rust.
fn file_name(key: &Key) -> String {
    let (path, modified, px) = key;
    let nanos = modified
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let (nanos, px) = (nanos.to_le_bytes(), px.to_le_bytes());
    let bytes = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .chain(&nanos)
        .chain(&px);
    let hash = bytes.fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}.png")
}

/// The PNG for `key`, made now if it isn't cached yet.
fn cached(key: &Key) -> Option<PathBuf> {
    let png = cache_dir().join(file_name(key));
    if png.is_file() {
        // Marked as used, so trimming the cache keeps it.
        let _ = std::fs::File::options()
            .write(true)
            .open(&png)
            .and_then(|file| file.set_modified(SystemTime::now()));
        return Some(png);
    }
    std::fs::create_dir_all(cache_dir()).ok()?;
    let partial = png.with_extension("partial.png");
    make(&key.0, key.2, &partial)?;
    std::fs::rename(&partial, &png).ok()?;
    Some(png)
}

/// Remove unfinished PNGs left by a crash, and the least recently used
/// ones while the folder holds more than `limit` bytes of them.
fn prune(dir: &Path, limit: u64) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut pngs: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".partial.png") {
            let _ = std::fs::remove_file(&path);
        } else if name.ends_with(".png")
            && let Ok(meta) = entry.metadata()
        {
            let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            pngs.push((used, meta.len(), path));
        }
    }
    let mut total: u64 = pngs.iter().map(|(_, len, _)| len).sum();
    if total <= limit {
        return;
    }
    pngs.sort_unstable_by_key(|(used, _, _)| *used);
    for (_, len, path) in pngs {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(target_os = "macos")]
fn make(file: &Path, px: u32, out: &Path) -> Option<()> {
    crate::macos::save_thumbnail(file, px, out).ok()
}

#[cfg(windows)]
fn make(file: &Path, px: u32, out: &Path) -> Option<()> {
    crate::win::save_thumbnail(file, px, out).ok()
}

/// Run the desktop's thumbnailer for the file's type, which writes the PNG.
/// One that takes longer than [`THUMBNAILER_TIMEOUT`] is stopped, so a hung
/// one can't hold a thumbnail thread for good.
#[cfg(not(any(target_os = "macos", windows)))]
fn make(file: &Path, px: u32, out: &Path) -> Option<()> {
    use fffinder_core::thumbnailers::{self, Thumbnailer};
    use wait_timeout::ChildExt as _;
    const THUMBNAILER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    static INSTALLED: OnceLock<Vec<Thumbnailer>> = OnceLock::new();
    let installed = INSTALLED.get_or_init(|| thumbnailers::installed(&thumbnailers::data_dirs()));
    let mime = crate::system::mime_type(file)?;
    let thumbnailer = thumbnailers::for_mime(installed, &mime)?;
    let words = thumbnailers::command(&thumbnailer.exec, file, out, px);
    let (program, args) = words.split_first()?;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let status = match child.wait_timeout(THUMBNAILER_TIMEOUT).ok()? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    (status.success() && out.is_file()).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_the_same() {
        let key: Key = (
            PathBuf::from("/a/b.pdf"),
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1)),
            128,
        );
        assert_eq!(file_name(&key), file_name(&key.clone()));
        assert_eq!(file_name(&key), "94ce6be0f59c52a1.png");
        assert_ne!(file_name(&key), file_name(&(key.0.clone(), key.1, 256)));
    }

    #[test]
    fn prunes_the_least_recently_used() {
        let dir = std::env::temp_dir().join(format!("fffinder-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let at = |secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        for (name, used) in [("old.png", 1), ("mid.png", 2), ("new.png", 3)] {
            std::fs::write(dir.join(name), [0u8; 10]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(dir.join(name))
                .unwrap()
                .set_modified(at(used))
                .unwrap();
        }
        std::fs::write(dir.join("x.partial.png"), [0u8; 1]).unwrap();
        prune(&dir, 20);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["mid.png", "new.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quicklook_draws_a_pdf() {
        let dir = std::env::temp_dir().join(format!("fffinder-thumbs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("page.pdf");
        std::fs::write(
            &pdf,
            "%PDF-1.1\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF\n",
        )
        .unwrap();
        let out = dir.join("thumb.png");
        make(&pdf, 128, &out).expect("QuickLook made a thumbnail");
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"), "a PNG");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
