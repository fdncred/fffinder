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
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

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
/// our own wait on it instead of tying up GPUI's background threads.
fn queue() -> &'static async_channel::Sender<Job> {
    static QUEUE: OnceLock<async_channel::Sender<Job>> = OnceLock::new();
    QUEUE.get_or_init(|| {
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

/// The PNG for `key`, made now if it isn't cached yet.
fn cached(key: &Key) -> Option<PathBuf> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let png = cache_dir().join(format!("{:016x}.png", hasher.finish()));
    if png.is_file() {
        return Some(png);
    }
    std::fs::create_dir_all(cache_dir()).ok()?;
    let partial = png.with_extension("partial.png");
    make(&key.0, key.2, &partial)?;
    std::fs::rename(&partial, &png).ok()?;
    Some(png)
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
#[cfg(not(any(target_os = "macos", windows)))]
fn make(file: &Path, px: u32, out: &Path) -> Option<()> {
    use fffinder_core::thumbnailers::{self, Thumbnailer};
    static INSTALLED: OnceLock<Vec<Thumbnailer>> = OnceLock::new();
    let installed = INSTALLED.get_or_init(|| thumbnailers::installed(&thumbnailers::data_dirs()));
    let mime = crate::system::xdg_query(&["filetype".as_ref(), file.as_os_str()])?;
    let thumbnailer = thumbnailers::for_mime(installed, &mime)?;
    let words = thumbnailers::command(&thumbnailer.exec, file, out, px);
    let (program, args) = words.split_first()?;
    let status = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    (status.success() && out.is_file()).then_some(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

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
