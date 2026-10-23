//! The platform: the regular one for this OS, started without windows, with
//! the one window it opens shown in the terminal instead.
//!
//! Executors, text shaping, the clipboard, and opening or revealing files all
//! go to the regular platform. Menus, native dialogs, and other windows have
//! nowhere to appear in a terminal.

use anyhow::{Context as _, Result, anyhow};
use crossterm::event::Event;
use futures::channel::oneshot;
use gpui_kit::{
    Action, ActivityGuard, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle,
    ForegroundExecutor, Keymap, Menu, MenuItem, PathPromptOptions, Platform, PlatformDisplay,
    PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Task,
    ThermalState, WindowAppearance, WindowParams,
};
use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use super::input;
use super::kitty::Writer;
use super::tty::{self, Session};
use super::window::{TerminalWindow, WindowConfig};

/// GPUI's quit handler, which runs the app's shutdown.
type QuitHandler = Rc<RefCell<Option<Box<dyn FnMut() -> bool>>>>;

pub struct TerminalPlatform {
    inner: Rc<dyn Platform>,
    session: Session,
    writer: Writer,
    /// Terminal input, until the window takes it.
    input: RefCell<Option<async_channel::Receiver<Event>>>,
    window: Cell<Option<AnyWindowHandle>>,
    on_quit: QuitHandler,
    cursor: Cell<CursorStyle>,
}

impl TerminalPlatform {
    pub fn new(session: Session) -> TerminalPlatform {
        TerminalPlatform {
            inner: gpui_kit::platform::current_platform(true),
            writer: Writer::spawn(session.caps.shared_memory),
            input: RefCell::new(Some(input::spawn_reader())),
            session,
            window: Cell::new(None),
            on_quit: Rc::default(),
            cursor: Cell::new(CursorStyle::Arrow),
        }
    }
}

/// Shut the app down, put the terminal back, and exit. Runs as its own task,
/// since GPUI asks to quit from inside an update.
fn quit_later(foreground: &ForegroundExecutor, on_quit: QuitHandler) {
    foreground
        .spawn(async move {
            let handler = on_quit.borrow_mut().take();
            if let Some(mut handler) = handler {
                handler();
            }
            tty::restore();
            std::process::exit(0);
        })
        .detach();
}

/// The CSS cursor name for a pointer shape, which terminals take in OSC 22.
fn cursor_name(style: CursorStyle) -> &'static str {
    match style {
        CursorStyle::Arrow => "default",
        CursorStyle::IBeam => "text",
        CursorStyle::Crosshair => "crosshair",
        CursorStyle::ClosedHand => "grabbing",
        CursorStyle::OpenHand => "grab",
        CursorStyle::PointingHand => "pointer",
        CursorStyle::ResizeLeft => "w-resize",
        CursorStyle::ResizeRight => "e-resize",
        CursorStyle::ResizeLeftRight => "ew-resize",
        CursorStyle::ResizeUp => "n-resize",
        CursorStyle::ResizeDown => "s-resize",
        CursorStyle::ResizeUpDown => "ns-resize",
        CursorStyle::ResizeUpLeftDownRight => "nwse-resize",
        CursorStyle::ResizeUpRightDownLeft => "nesw-resize",
        CursorStyle::ResizeColumn => "col-resize",
        CursorStyle::ResizeRow => "row-resize",
        CursorStyle::IBeamCursorForVerticalLayout => "vertical-text",
        CursorStyle::OperationNotAllowed => "not-allowed",
        CursorStyle::DragLink => "alias",
        CursorStyle::DragCopy => "copy",
        CursorStyle::ContextualMenu => "context-menu",
    }
}

/// A file dialog's answer: there is no dialog to show.
fn no_dialog<T>() -> oneshot::Receiver<Result<T>> {
    let (tx, rx) = oneshot::channel();
    let _ = tx.send(Err(anyhow!(
        "file dialogs aren't available in the terminal"
    )));
    rx
}

impl Platform for TerminalPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.inner.background_executor()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.inner.foreground_executor()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.inner.text_system()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        self.inner.run(on_finish_launching);
    }

    fn quit(&self) {
        quit_later(&self.inner.foreground_executor(), self.on_quit.clone());
    }

    fn restart(&self, _binary_path: Option<PathBuf>, _arguments: Vec<OsString>) {}

    fn activate(&self, _ignoring_other_apps: bool) {}

    fn hide(&self) {}

    fn hide_other_apps(&self) {}

    fn unhide_other_apps(&self) {}

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.inner.displays()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.inner.primary_display()
    }

    fn on_displays_changed(&self, _callback: Box<dyn FnMut()>) {}

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.window.get()
    }

    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        Some(self.window.get().into_iter().collect())
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        _options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let input = self
            .input
            .borrow_mut()
            .take()
            .context("the terminal shows only one window")?;
        let renderer = gpui_kit::platform::current_headless_renderer()?
            .context("this platform has no offscreen renderer")?;
        let foreground = self.inner.foreground_executor();
        let on_quit = self.on_quit.clone();
        let hangup_executor = foreground.clone();
        let window = TerminalWindow::new(WindowConfig {
            session: self.session,
            writer: self.writer.clone(),
            input,
            renderer,
            background: self.inner.background_executor(),
            foreground,
            display: self.inner.primary_display(),
            appearance: self.inner.window_appearance(),
            on_hangup: Box::new(move || quit_later(&hangup_executor, on_quit)),
        });
        self.window.set(Some(handle));
        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        self.inner.window_appearance()
    }

    fn open_url(&self, url: &str) {
        self.inner.open_url(url);
    }

    fn on_open_urls(&self, _callback: Box<dyn FnMut(Vec<String>)>) {}

    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!("no URL schemes in the terminal")))
    }

    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        no_dialog()
    }

    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        no_dialog()
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    fn reveal_path(&self, path: &Path) {
        self.inner.reveal_path(path);
    }

    fn open_with_system(&self, path: &Path) {
        self.inner.open_with_system(path);
    }

    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        *self.on_quit.borrow_mut() = Some(callback);
    }

    fn on_reopen(&self, _callback: Box<dyn FnMut()>) {}

    fn on_system_sleep(&self, _callback: Box<dyn FnMut()>) {}

    fn on_system_wake(&self, _callback: Box<dyn FnMut()>) {}

    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}

    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn Action)>) {}

    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {}

    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn Action) -> bool>) {}

    fn thermal_state(&self) -> ThermalState {
        self.inner.thermal_state()
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_thermal_state_change(callback);
    }

    fn prevent_idle_sleep(&self, reason: &str) -> Task<Result<ActivityGuard>> {
        self.inner.prevent_idle_sleep(reason)
    }

    fn app_path(&self) -> Result<PathBuf> {
        self.inner.app_path()
    }

    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.inner.path_for_auxiliary_executable(name)
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        if self.cursor.replace(style) != style {
            self.writer
                .codes(format!("\x1b]22;{}\x1b\\", cursor_name(style)));
        }
    }

    fn hide_cursor_until_mouse_moves(&self) {}

    fn is_cursor_visible(&self) -> bool {
        true
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        self.inner.should_auto_hide_scrollbars()
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_clipboard()
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        self.inner.write_to_clipboard(item);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.inner.read_from_primary()
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write_to_primary(&self, item: ClipboardItem) {
        self.inner.write_to_primary(item);
    }

    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_find_pasteboard()
    }

    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, item: ClipboardItem) {
        self.inner.write_to_find_pasteboard(item);
    }

    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        self.inner.write_credentials(url, username, password)
    }

    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        self.inner.read_credentials(url)
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        self.inner.delete_credentials(url)
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        self.inner.keyboard_layout()
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.inner.keyboard_mapper()
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_keyboard_layout_change(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pointer_shape_has_a_css_name() {
        assert_eq!(cursor_name(CursorStyle::Arrow), "default");
        assert_eq!(cursor_name(CursorStyle::PointingHand), "pointer");
        assert_eq!(cursor_name(CursorStyle::IBeam), "text");
        assert_eq!(cursor_name(CursorStyle::ResizeColumn), "col-resize");
    }
}
