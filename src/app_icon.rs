//! The app icon where it can only be given at run time: the Dock on macOS
//! when fffinder runs outside its app bundle (as `cargo install` and
//! `scripts/install.nu` put it), and window icons on X11. Windows takes the
//! icon embedded in the executable (see `build.rs`), and Wayland the one
//! named by the desktop entry that `scripts/install.nu` installs.

use resvg::{tiny_skia, usvg};

/// `assets/app-icon.svg` drawn `size` pixels square, with premultiplied
/// alpha.
fn render(size: u32) -> Option<tiny_skia::Pixmap> {
    let svg = crate::assets::embedded("app-icon.svg")?;
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// Show the icon in the Dock unless an app bundle already does. Call it on
/// the main thread once the app is running, or AppKit puts its own back.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    // A bundle's icon is in the Dock before the app starts, drawn in the
    // system's style.
    let bundled = std::env::current_exe().is_ok_and(|exe| {
        exe.ancestors()
            .any(|dir| dir.extension().is_some_and(|ext| ext == "app"))
    });
    let Some(mtm) = MainThreadMarker::new().filter(|_| !bundled) else {
        return;
    };
    // 512 pixels covers the largest Dock on a Retina screen.
    let Some(png) = render(512).and_then(|pixmap| pixmap.encode_png().ok()) else {
        return;
    };
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png)) else {
        return;
    };
    // SAFETY: the only thing to uphold is passing an image, not `None`.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
}

/// The window icon for `WindowOptions::icon`, which GPUI uses on X11 only,
/// drawn once. `None` on other systems, which take the icon elsewhere.
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    static ICON: std::sync::OnceLock<Option<std::sync::Arc<image::RgbaImage>>> =
        std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        const SIZE: u32 = 128;
        let pixmap = render(SIZE)?;
        // X11 takes straight alpha.
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|px| {
                let px = px.demultiply();
                [px.red(), px.green(), px.blue(), px.alpha()]
            })
            .collect();
        image::RgbaImage::from_raw(SIZE, SIZE, rgba).map(std::sync::Arc::new)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_the_icon() {
        let pixmap = render(64).expect("the bundled icon draws");
        assert_eq!((pixmap.width(), pixmap.height()), (64, 64));
        assert!(pixmap.pixels().iter().any(|px| px.alpha() > 0));
        assert!(
            pixmap
                .encode_png()
                .is_ok_and(|png| png.starts_with(b"\x89PNG"))
        );
    }

    #[test]
    fn has_a_window_icon_only_for_x11() {
        let icon = window_icon();
        assert_eq!(icon.is_some(), cfg!(all(unix, not(target_os = "macos"))));
        if let Some(icon) = icon {
            assert_eq!(icon.dimensions(), (128, 128));
        }
    }
}
