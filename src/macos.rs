//! Finder's look, taken from macOS itself: the accent and selection colors,
//! the icons Finder shows for files and folders, and its sidebar icons.
//! Icons are drawn once into PNGs cached in the data folder. Elsewhere, and
//! off the main thread (AppKit drawing belongs there; UI tests run on other
//! threads), there are no system icons or colors and the bundled ones are
//! used.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fffinder_core::FileEntry;

/// Colors as 0xRRGGBBAA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemColors {
    pub accent: u32,
    pub selection_light: u32,
    pub selection_dark: u32,
}

/// A Finder icon, looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconLookup {
    /// A cached PNG of the icon.
    Ready(PathBuf),
    /// Not drawn yet because this frame's drawing time is used up; draw
    /// another frame to get it.
    Later,
    /// No system icon: another platform, off the main thread, or drawing
    /// failed. Use the bundled icon.
    Unavailable,
}

/// Where cached icon images live.
fn icon_dir() -> PathBuf {
    fffinder_core::config::data_dir().join("icons").join("v2")
}

/// Drawing an app's icon the first time takes about 10ms, so a window of
/// new icons is drawn over several frames: at most this long in each
/// stretch of `FRAME`, with the bundled icons standing in until then.
const BUDGET: Duration = Duration::from_millis(10);
const FRAME: Duration = Duration::from_millis(16);

thread_local! {
    /// Icons already looked up, so redraws don't touch the disk.
    static MEMO: RefCell<HashMap<String, Option<PathBuf>>> = Default::default();
    /// When the current stretch started, and the time spent drawing in it.
    static SPENT: Cell<(Option<Instant>, Duration)> = const { Cell::new((None, Duration::ZERO)) };
}

fn over_budget() -> bool {
    match SPENT.get() {
        (Some(start), spent) if start.elapsed() < FRAME => spent >= BUDGET,
        _ => {
            SPENT.set((Some(Instant::now()), Duration::ZERO));
            false
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn memo(key: String, make: impl FnOnce() -> Option<PathBuf>) -> IconLookup {
    let found = |value: Option<PathBuf>| value.map_or(IconLookup::Unavailable, IconLookup::Ready);
    if let Some(hit) = MEMO.with(|m| m.borrow().get(&key).cloned()) {
        return found(hit);
    }
    if over_budget() {
        return IconLookup::Later;
    }
    let start = Instant::now();
    let value = make();
    let (window, spent) = SPENT.get();
    SPENT.set((window, spent + start.elapsed()));
    MEMO.with(|m| m.borrow_mut().insert(key, value.clone()));
    found(value)
}

/// Folders Finder gives their own icon.
fn special_folder(path: &Path) -> bool {
    if path == Path::new("/")
        || path == Path::new("/Applications")
        || path.parent() == Some(Path::new("/Volumes"))
    {
        return true;
    }
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    if path == home {
        return true;
    }
    path.parent() == Some(home.as_path())
        && matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some(
                "Applications"
                    | "Desktop"
                    | "Documents"
                    | "Downloads"
                    | "Library"
                    | "Movies"
                    | "Music"
                    | "Pictures"
                    | "Public"
                    | "Sites"
            )
        )
}

fn hash(value: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// The cache key for `entry`'s icon: one per extension for documents, one
/// per name for files without an extension (`Makefile`, `.gitignore`, and a
/// program get different icons), and one per path and modification time for
/// apps, packages, volumes, and folders with their own icon.
pub fn icon_key(entry: &FileEntry) -> String {
    let ext = entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let per_path = if entry.is_dir {
        !ext.is_empty() || special_folder(&entry.path) || entry.path.join("Icon\r").exists()
    } else {
        false
    };
    if per_path {
        return format!("path-{:016x}", hash((&entry.path, entry.modified)));
    }
    if entry.is_dir {
        "folder".into()
    } else if ext.is_empty() {
        let program = entry.kind == fffinder_core::FileKind::Executable;
        format!("name-{:016x}", hash((entry.name.to_lowercase(), program)))
    } else {
        format!(
            "ext-{}",
            ext.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        )
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc2::AnyThread as _;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBitmapImageFileType,
        NSBitmapImageRep, NSColor, NSColorSpace, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSImage, NSRectFillUsingOperation, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString, NSThread, NSURL};

    pub fn on_main_thread() -> bool {
        NSThread::isMainThread_class()
    }

    pub fn has_default_app(path: &Path) -> bool {
        autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            NSWorkspace::sharedWorkspace()
                .URLForApplicationToOpenURL(&url)
                .is_some()
        })
    }

    pub fn default_app(path: &Path) -> Option<PathBuf> {
        autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
            app.path().map(|p| PathBuf::from(p.to_string()))
        })
    }

    /// A Finder alias is a bookmark written to a file.
    pub fn make_alias(target: &Path, alias: &Path) -> Result<(), String> {
        use objc2_foundation::NSURLBookmarkCreationOptions;
        let url = |p: &Path| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
        autoreleasepool(|_| {
            let data = url(target)
                .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                    NSURLBookmarkCreationOptions::SuitableForBookmarkFile,
                    None,
                    None,
                )
                .map_err(|e| e.localizedDescription().to_string())?;
            NSURL::writeBookmarkData_toURL_options_error(&data, &url(alias), 0)
                .map_err(|e| e.localizedDescription().to_string())
        })
    }

    pub fn apps_for(path: &Path) -> Vec<PathBuf> {
        autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            NSWorkspace::sharedWorkspace()
                .URLsForApplicationsToOpenURL(&url)
                .iter()
                .filter_map(|app| app.path().map(|p| PathBuf::from(p.to_string())))
                .collect()
        })
    }

    /// Launch Services answers on another thread, after the user confirms
    /// the change, so this waits for it.
    pub fn set_default_app(app: &Path, file: &Path) -> Result<(), String> {
        use objc2_foundation::NSError;
        let (tx, rx) = std::sync::mpsc::channel();
        autoreleasepool(|_| {
            let url = |p: &Path| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
            let done = block2::RcBlock::new(move |error: *mut NSError| {
                // SAFETY: Launch Services passes a valid error or null.
                let message =
                    unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                let _ = tx.send(message);
            });
            NSWorkspace::sharedWorkspace()
                .setDefaultApplicationAtURL_toOpenContentTypeOfFileAtURL_completionHandler(
                    &url(app),
                    &url(file),
                    Some(&done),
                );
        });
        match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(None) => Ok(()),
            Ok(Some(message)) => Err(message),
            Err(_) => Err("macOS didn’t answer".into()),
        }
    }

    fn rgba(color: &NSColor) -> Option<u32> {
        let c = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
        let byte = |v: f64| (v.clamp(0., 1.) * 255.).round() as u32;
        Some(
            byte(c.redComponent()) << 24
                | byte(c.greenComponent()) << 16
                | byte(c.blueComponent()) << 8
                | byte(c.alphaComponent()),
        )
    }

    /// `color` resolved under the light or dark appearance.
    fn resolved(dark: bool, color: fn() -> Retained<NSColor>) -> Option<u32> {
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        let appearance = NSAppearance::appearanceNamed(name)?;
        let out = std::cell::Cell::new(None);
        let block = block2::RcBlock::new(|| out.set(rgba(&color())));
        appearance.performAsCurrentDrawingAppearance(&block);
        out.get()
    }

    pub fn system_colors() -> Option<SystemColors> {
        if !on_main_thread() {
            return None;
        }
        Some(SystemColors {
            accent: resolved(false, NSColor::controlAccentColor)?,
            selection_light: resolved(false, NSColor::selectedContentBackgroundColor)?,
            selection_dark: resolved(true, NSColor::selectedContentBackgroundColor)?,
        })
    }

    /// Draw `image` into a `px`-square PNG, filled with `tint` when given
    /// (for template icons).
    fn png(image: &NSImage, px: usize, tint: Option<u32>) -> Option<Vec<u8>> {
        autoreleasepool(|_| unsafe {
            let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                px as isize,
                px as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )?;
            rep.setSize(NSSize::new(px as f64, px as f64));
            let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&context));
            let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(px as f64, px as f64));
            image.drawInRect_fromRect_operation_fraction(
                rect,
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                1.,
            );
            if let Some(tint) = tint {
                let channel = |shift: u32| ((tint >> shift) & 0xFF) as f64 / 255.;
                NSColor::colorWithSRGBRed_green_blue_alpha(
                    channel(24),
                    channel(16),
                    channel(8),
                    channel(0),
                )
                .set();
                NSRectFillUsingOperation(rect, NSCompositingOperation::SourceIn);
            }
            context.flushGraphics();
            NSGraphicsContext::restoreGraphicsState_class();
            let data = rep.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )?;
            Some(data.to_vec())
        })
    }

    pub fn file_icon_png(entry: &FileEntry, px: usize) -> Option<Vec<u8>> {
        let workspace = NSWorkspace::sharedWorkspace();
        let ext = entry
            .path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        let image = if entry.is_dir || ext.is_empty() {
            workspace.iconForFile(&NSString::from_str(&entry.path.to_string_lossy()))
        } else {
            // The replacement takes a UTType, which needs another binding crate;
            // this still returns the same icon.
            #[allow(deprecated)]
            workspace.iconForFileType(&NSString::from_str(&ext))
        };
        png(&image, px, None)
    }

    /// One pasteboard item per file URL, with `text` on the first, so
    /// Finder pastes the files and a terminal pastes the paths.
    pub fn write_file_clipboard(paths: &[PathBuf], text: &str) -> bool {
        use objc2::runtime::ProtocolObject;
        use objc2_app_kit::{
            NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString,
            NSPasteboardWriting,
        };
        use objc2_foundation::NSArray;
        autoreleasepool(|_| {
            let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
                .iter()
                .enumerate()
                .filter_map(|(ix, path)| {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
                        .absoluteString()?;
                    let item = NSPasteboardItem::new();
                    // SAFETY: the type constants are AppKit's own statics.
                    unsafe {
                        item.setString_forType(&url, NSPasteboardTypeFileURL);
                        if ix == 0 {
                            item.setString_forType(
                                &NSString::from_str(text),
                                NSPasteboardTypeString,
                            );
                        }
                    }
                    Some(ProtocolObject::from_retained(item))
                })
                .collect();
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.writeObjects(&NSArray::from_retained_slice(&items))
        })
    }

    pub fn template_png(file: &Path, px: usize, tint: u32) -> Option<Vec<u8>> {
        let image = NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&file.to_string_lossy()),
        )?;
        png(&image, px, Some(tint))
    }
}

/// Cache `bytes` produced by `make` at `name` in the icon folder.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn cached(name: &str, make: impl FnOnce() -> Option<Vec<u8>>) -> Option<PathBuf> {
    let path = icon_dir().join(name);
    if path.is_file() {
        return Some(path);
    }
    let bytes = make()?;
    std::fs::create_dir_all(icon_dir()).ok()?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// The accent and selection colors the user chose in System Settings.
pub fn system_colors() -> Option<SystemColors> {
    #[cfg(target_os = "macos")]
    {
        imp::system_colors()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Put `paths` on the general pasteboard as files, and `text` as a string.
/// Returns whether that worked.
#[cfg(target_os = "macos")]
pub fn write_file_clipboard(paths: &[PathBuf], text: &str) -> bool {
    imp::write_file_clipboard(paths, text)
}

/// The app bundle Launch Services opens the file at `path` with.
#[cfg(target_os = "macos")]
pub fn default_app(path: &Path) -> Option<PathBuf> {
    imp::default_app(path)
}

/// Make a Finder alias to `target` at `alias`.
#[cfg(target_os = "macos")]
pub fn make_alias(target: &Path, alias: &Path) -> Result<(), String> {
    imp::make_alias(target, alias)
}

/// Every app Launch Services says can open the file at `path`.
#[cfg(target_os = "macos")]
pub fn apps_for(path: &Path) -> Vec<PathBuf> {
    imp::apps_for(path)
}

/// Make `app` the default for files of `file`'s type.
#[cfg(target_os = "macos")]
pub fn set_default_app(app: &Path, file: &Path) -> Result<(), String> {
    imp::set_default_app(app, file)
}

/// Whether Launch Services knows an app that opens the file at `path`.
#[cfg(target_os = "macos")]
pub fn has_default_app(path: &Path) -> bool {
    imp::has_default_app(path)
}

/// The icon Finder shows for `entry`, as a cached PNG, sized for display
/// at `points` (rendered at a size that stays sharp on Retina screens).
pub fn file_icon(entry: &FileEntry, points: f32) -> IconLookup {
    #[cfg(target_os = "macos")]
    {
        if !imp::on_main_thread() {
            return IconLookup::Unavailable;
        }
        // Twice the size for Retina screens, rounded up to a few sizes.
        let px = match points {
            p if p <= 24. => 64,
            p if p <= 128. => 256,
            _ => 512,
        };
        // Documents share an icon per extension, so skip the folder checks.
        let memo_key = if entry.is_dir {
            format!("{}\0{:?}\0{px}", entry.path.display(), entry.modified)
        } else {
            format!("{}\0{px}", icon_key(entry))
        };
        memo(memo_key, || {
            let key = icon_key(entry);
            cached(&format!("{key}-{px}.png"), || imp::file_icon_png(entry, px))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (entry, points);
        IconLookup::Unavailable
    }
}

/// One of Finder's sidebar icons (`SidebarDownloadsFolder`, …) tinted with
/// `rgba`, as a cached PNG.
pub fn sidebar_icon(name: &str, rgba: u32) -> IconLookup {
    #[cfg(target_os = "macos")]
    {
        if !imp::on_main_thread() {
            return IconLookup::Unavailable;
        }
        let source = Path::new("/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources")
            .join(format!("{name}.icns"));
        let file = format!("sidebar-{name}-{rgba:08x}.png");
        memo(file.clone(), || {
            cached(&file, || imp::template_png(&source, 48, rgba))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (name, rgba);
        IconLookup::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            is_dir,
            size: None,
            modified: None,
            kind: fffinder_core::FileKind::Other,
            hidden: false,
            is_symlink: false,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn knows_which_files_have_an_app() {
        let dir = std::env::temp_dir().join(format!("fffinder-apps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("notes.txt");
        std::fs::write(&text, "hi").unwrap();
        assert!(super::has_default_app(&text), "TextEdit opens .txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn icons_are_shared_by_type() {
        assert_eq!(icon_key(&entry("/x/a.PDF", false)), "ext-pdf");
        assert_eq!(icon_key(&entry("/x/b.pdf", false)), "ext-pdf");
        assert_eq!(icon_key(&entry("/x/plain", true)), "folder");
        // Files without an extension get an icon per name.
        let readme = icon_key(&entry("/x/README", false));
        assert!(readme.starts_with("name-"), "{readme}");
        assert_eq!(readme, icon_key(&entry("/y/readme", false)));
        assert_ne!(readme, icon_key(&entry("/x/.gitignore", false)));
        let app = icon_key(&entry("/Applications/Safari.app", true));
        assert!(app.starts_with("path-"), "{app}");
        assert_ne!(app, icon_key(&entry("/Applications/Mail.app", true)));
        if let Some(home) = dirs::home_dir() {
            let downloads = home.join("Downloads");
            assert!(icon_key(&entry(&downloads.to_string_lossy(), true)).starts_with("path-"));
        }
        assert!(icon_key(&entry("/Volumes/Backup", true)).starts_with("path-"));
    }
}
