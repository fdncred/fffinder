//! One asset source for the app: GPUI Kit's default icons, the extra Lucide
//! icons used here, and fffinder's own colored file-type icons.

use gpui_kit::assets::{Assets as KitAssets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

icon_assets!(pub ExtraIcons, [
    AppWindow, ArrowUpDown, ClipboardCopy, Clock, Cloud, Code, Columns3, Download, Film,
    FolderCog, FolderPlus, FolderSearch, Funnel, GalleryHorizontalEnd, House, Image, Laptop,
    LayoutGrid, List, Monitor, Music, Pencil, Play, ReplaceAll, ScanSearch, SlidersHorizontal,
    SquarePen, Tag, Terminal, TextSearch, Trash, Usb, X, Zap,
]);

macro_rules! embedded {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../assets/", $path)) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = embedded![
    "filetypes/app.svg",
    "filetypes/drive.svg",
    "filetypes/file-archive.svg",
    "filetypes/file-audio.svg",
    "filetypes/file-code.svg",
    "filetypes/file-data.svg",
    "filetypes/file-exec.svg",
    "filetypes/file-image.svg",
    "filetypes/file-pdf.svg",
    "filetypes/file-text.svg",
    "filetypes/file-video.svg",
    "filetypes/file.svg",
    "filetypes/folder.svg",
    "app-icon.svg",
];

/// One of fffinder's own embedded files, such as `app-icon.svg`.
pub fn embedded(path: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, bytes)| *bytes)
}

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = embedded(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        // Last: the kit's source errors on unknown paths.
        KitAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out = KitAssets.list(path)?;
        out.extend(ExtraIcons.list(path)?);
        out.extend(
            FILES
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .map(|(p, _)| SharedString::from(*p)),
        );
        out.sort();
        out.dedup();
        Ok(out)
    }
}

pub const THEMES: &str = include_str!("../assets/themes/fffinder.json");
