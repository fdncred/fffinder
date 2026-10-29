//! The window: the whole terminal grid. GPUI lays it out and paints it as it
//! would a native window; each frame is rendered offscreen and sent to the
//! terminal as one image.

use crossterm::event::Event;
use futures::channel::oneshot;
use gpui_kit::{
    BackgroundExecutor, Bounds, Capslock, DispatchEventResult, ForegroundExecutor, GpuSpecs,
    Modifiers, Pixels, PlatformAtlas, PlatformDisplay, PlatformHeadlessRenderer, PlatformInput,
    PlatformInputHandler, PlatformWindow, Point, PromptButton, PromptLevel, RequestFrameOptions,
    Scene, Size, WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
    WindowVisibility, px, size,
};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::frames::{Frame, Writer};
use super::input::{self, Mapping, Translator};
use super::tty::{self, Geometry, Session};

/// Frames are drawn at most this often, about 60 a second.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Everything a window needs from the platform that opens it.
pub struct WindowConfig {
    pub session: Session,
    pub writer: Writer,
    pub input: async_channel::Receiver<Event>,
    pub renderer: Box<dyn PlatformHeadlessRenderer>,
    pub foreground: ForegroundExecutor,
    pub background: BackgroundExecutor,
    pub display: Option<Rc<dyn PlatformDisplay>>,
    pub appearance: WindowAppearance,
    /// Called when the terminal goes away.
    pub on_hangup: Box<dyn FnOnce()>,
}

/// GPUI owns the window; it and the input task share its state.
pub struct TerminalWindow(Rc<Inner>);

struct Inner {
    state: RefCell<State>,
    callbacks: RefCell<Callbacks>,
    renderer: RefCell<Box<dyn PlatformHeadlessRenderer>>,
    atlas: Arc<dyn PlatformAtlas>,
    writer: Writer,
    foreground: ForegroundExecutor,
    background: BackgroundExecutor,
    display: Option<Rc<dyn PlatformDisplay>>,
    appearance: WindowAppearance,
    frame_scheduled: Cell<bool>,
    last_frame: Cell<Option<Instant>>,
}

struct State {
    session: Session,
    scale: f32,
    translator: Translator,
    input_handler: Option<PlatformInputHandler>,
    title: String,
    active: bool,
}

impl State {
    fn logical_size(&self) -> Size<Pixels> {
        let geometry = &self.session.geometry;
        size(
            px(geometry.width as f32 / self.scale),
            px(geometry.height as f32 / self.scale),
        )
    }

    fn mapping(&self) -> Mapping {
        Mapping {
            scale: self.scale,
            cell: self.session.geometry.cell(),
            pixels: self.session.caps.pixel_mouse,
        }
    }
}

type ResizeCallback = Box<dyn FnMut(Size<Pixels>, f32)>;

/// GPUI's callbacks. Each is taken out while it runs, since it may call back
/// into the window.
#[derive(Default)]
struct Callbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status: Option<Box<dyn FnMut(bool)>>,
    resize: Option<ResizeCallback>,
}

impl TerminalWindow {
    pub fn new(config: WindowConfig) -> TerminalWindow {
        let WindowConfig {
            session,
            writer,
            input,
            renderer,
            foreground,
            background,
            display,
            appearance,
            on_hangup,
        } = config;
        let inner = Rc::new(Inner {
            state: RefCell::new(State {
                session,
                scale: tty::scale(&session.geometry, session.requested_scale),
                translator: Translator::default(),
                input_handler: None,
                title: String::new(),
                active: true,
            }),
            callbacks: RefCell::default(),
            atlas: renderer.sprite_atlas(),
            renderer: RefCell::new(renderer),
            writer,
            foreground,
            background,
            display,
            appearance,
            frame_scheduled: Cell::new(false),
            last_frame: Cell::new(None),
        });
        let weak = Rc::downgrade(&inner);
        inner
            .foreground
            .spawn(async move {
                while let Ok(event) = input.recv().await {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    inner.handle(event);
                }
                on_hangup();
            })
            .detach();
        TerminalWindow(inner)
    }
}

impl Inner {
    fn handle(&self, event: Event) {
        match event {
            Event::Key(key) => {
                let input = self.state.borrow_mut().translator.key(key);
                if let Some(input) = input {
                    let text = input::typed_text(&input).map(str::to_owned);
                    if self.dispatch(input)
                        && let Some(text) = text
                    {
                        self.insert_text(&text);
                    }
                }
            }
            Event::Mouse(mouse) => {
                let input = {
                    let mut state = self.state.borrow_mut();
                    let mapping = state.mapping();
                    state.translator.mouse(mouse, &mapping, Instant::now())
                };
                self.dispatch(input);
            }
            Event::Paste(text) => self.insert_text(&text),
            Event::FocusGained | Event::FocusLost => {
                let active = matches!(event, Event::FocusGained);
                self.state.borrow_mut().active = active;
                let callback = self.callbacks.borrow_mut().active_status.take();
                if let Some(mut callback) = callback {
                    callback(active);
                    self.callbacks.borrow_mut().active_status = Some(callback);
                }
            }
            Event::Resize(..) => self.resize(),
        }
    }

    /// Hand an event to GPUI. True when nothing claimed it.
    fn dispatch(&self, input: PlatformInput) -> bool {
        let callback = self.callbacks.borrow_mut().input.take();
        let Some(mut callback) = callback else {
            return true;
        };
        let result = callback(input);
        self.callbacks.borrow_mut().input = Some(callback);
        result.propagate
    }

    /// Type into the focused text field, if there is one.
    fn insert_text(&self, text: &str) {
        let handler = self.state.borrow_mut().input_handler.take();
        if let Some(mut handler) = handler {
            handler.replace_text_in_range(None, text);
            self.state.borrow_mut().input_handler = Some(handler);
        }
    }

    fn resize(&self) {
        let cell = self.state.borrow().session.caps.cell;
        let geometry = match tty::geometry(cell) {
            Ok(geometry) => geometry,
            Err(err) => {
                log::error!("terminal resized: {err:#}");
                return;
            }
        };
        let (size, scale) = {
            let mut state = self.state.borrow_mut();
            state.session.geometry = geometry;
            state.scale = tty::scale(&geometry, state.session.requested_scale);
            (state.logical_size(), state.scale)
        };
        let callback = self.callbacks.borrow_mut().resize.take();
        if let Some(mut callback) = callback {
            callback(size, scale);
            self.callbacks.borrow_mut().resize = Some(callback);
        }
    }

    /// Ask GPUI for a frame soon, unless one is already on the way. GPUI
    /// draws only when something changed, so asking costs little.
    fn schedule_frame(self: &Rc<Self>) {
        if self.frame_scheduled.replace(true) {
            return;
        }
        let wait = self.last_frame.get().map_or(Duration::ZERO, |at| {
            FRAME_INTERVAL.saturating_sub(at.elapsed())
        });
        let timer = (!wait.is_zero()).then(|| self.background.timer(wait));
        let weak = Rc::downgrade(self);
        self.foreground
            .spawn(async move {
                if let Some(timer) = timer {
                    timer.await;
                }
                if let Some(inner) = weak.upgrade() {
                    inner.frame_scheduled.set(false);
                    inner.request_frame();
                }
            })
            .detach();
    }

    fn request_frame(&self) {
        self.last_frame.set(Some(Instant::now()));
        let callback = self.callbacks.borrow_mut().request_frame.take();
        if let Some(mut callback) = callback {
            callback(RequestFrameOptions::default());
            self.callbacks.borrow_mut().request_frame = Some(callback);
        }
    }
}

impl raw_window_handle::HasWindowHandle for TerminalWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl raw_window_handle::HasDisplayHandle for TerminalWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl PlatformWindow for TerminalWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), self.content_size())
    }

    fn is_maximized(&self) -> bool {
        true
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.0.state.borrow().logical_size()
    }

    /// The terminal decides the size.
    fn resize(&mut self, _size: Size<Pixels>) {}

    fn scale_factor(&self) -> f32 {
        self.0.state.borrow().scale
    }

    fn appearance(&self) -> WindowAppearance {
        self.0.appearance
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.0.display.clone()
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.0.state.borrow().translator.position()
    }

    fn modifiers(&self) -> Modifiers {
        self.0.state.borrow().translator.modifiers()
    }

    fn capslock(&self) -> Capslock {
        Capslock::default()
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.state.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.state.borrow_mut().input_handler.take()
    }

    /// GPUI draws its own prompts when the platform has none.
    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        None
    }

    fn activate(&self) {}

    fn is_active(&self) -> bool {
        self.0.state.borrow().active
    }

    fn visibility(&self) -> WindowVisibility {
        WindowVisibility::Visible
    }

    fn is_hovered(&self) -> bool {
        true
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }

    fn set_title(&mut self, title: &str) {
        let title: String = title.chars().filter(|c| !c.is_control()).collect();
        self.0.writer.codes(format!("\x1b]2;{title}\x1b\\"));
        self.0.state.borrow_mut().title = title;
    }

    fn get_title(&self) -> String {
        self.0.state.borrow().title.clone()
    }

    fn set_background_appearance(&self, _background_appearance: WindowBackgroundAppearance) {}

    fn minimize(&self) {}

    fn zoom(&self) {}

    fn toggle_fullscreen(&self) {}

    fn is_fullscreen(&self) -> bool {
        false
    }

    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        let weak: Weak<Inner> = Rc::downgrade(&self.0);
        Some(Rc::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.schedule_frame();
            }
        }))
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.callbacks.borrow_mut().request_frame = Some(callback);
    }

    fn schedule_frame(&self) {
        self.0.schedule_frame();
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0.callbacks.borrow_mut().input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().active_status = Some(callback);
    }

    fn on_visibility_change(&self, _callback: Box<dyn FnMut(WindowVisibility)>) {}

    fn on_hover_status_change(&self, _callback: Box<dyn FnMut(bool)>) {}

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.callbacks.borrow_mut().resize = Some(callback);
    }

    fn on_moved(&self, _callback: Box<dyn FnMut()>) {}

    fn on_display_changed(&self, _callback: Box<dyn FnMut()>) {}

    fn on_should_close(&self, _callback: Box<dyn FnMut() -> bool>) {}

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_close(&self, _callback: Box<dyn FnOnce()>) {}

    fn on_appearance_changed(&self, _callback: Box<dyn FnMut()>) {}

    fn draw(&self, scene: &Scene) {
        let (device, cols, rows) = {
            let state = self.0.state.borrow();
            let Geometry { cols, rows, .. } = state.session.geometry;
            (
                state.logical_size().to_device_pixels(state.scale),
                cols,
                rows,
            )
        };
        let image = self
            .0
            .renderer
            .borrow_mut()
            .render_scene_to_image(scene, device);
        match image {
            Ok(image) => {
                let (width, height) = image.dimensions();
                self.0.writer.frame(Frame {
                    rgba: image.into_raw(),
                    width,
                    height,
                    cols,
                    rows,
                });
            }
            Err(err) => log::error!("drawing a frame: {err:#}"),
        }
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.atlas.clone()
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}

    /// The terminal's window isn't this process's.
    #[cfg(windows)]
    fn get_raw_handle(&self) -> ::windows::Win32::Foundation::HWND {
        ::windows::Win32::Foundation::HWND::default()
    }
}
