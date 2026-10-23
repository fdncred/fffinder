//! Shared harness for the UI tests: a fixture folder, a window driven through
//! real mouse and keyboard events, and a watchdog that fails the run when a
//! step hangs.

#![allow(dead_code)]

use fffinder::finder::{AppState, Finder, PickerOptions};
use fffinder::model::Location;
use fffinder::system::{Recorder, SystemCall};
use fffinder_core::terminals::Terminal;
use fffinder_core::{Appearance, Engine, EngineOptions};
use gpui_kit::base::test_support::{self, ElementSnapshot};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, ElementId, Entity, InputEvent as _,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, TestAppContext,
    Window, WindowOptions, point, px,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// A UI step that runs longer than this is treated as a hang.
const HANG_LIMIT: Duration = Duration::from_secs(20);

pub struct Fixture {
    pub dir: PathBuf,
}

impl Fixture {
    /// ```text
    /// Budget.csv  Projects/atlas/Project Atlas.txt  Roadmap.md  notes.txt  .secret
    /// ```
    pub fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("fffinder-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Projects/atlas")).unwrap();
        std::fs::write(dir.join("Budget.csv"), "a,b\n1,2\n").unwrap();
        std::fs::write(dir.join("Roadmap.md"), "# Roadmap\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "plain notes\n").unwrap();
        std::fs::write(dir.join(".secret"), "hidden").unwrap();
        std::fs::write(dir.join("Projects/atlas/Project Atlas.txt"), "atlas").unwrap();
        isolate_settings();
        Fixture { dir }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Point settings at one empty temp folder, once, before any window opens.
fn isolate_settings() {
    static CONFIG: OnceLock<PathBuf> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let config =
            std::env::temp_dir().join(format!("fffinder-ui-config-{}", std::process::id()));
        std::fs::create_dir_all(&config).unwrap();
        // SAFETY: runs once, inside the OnceLock, before any test reads the variable.
        unsafe {
            std::env::set_var("FFFINDER_CONFIG_DIR", &config);
            std::env::set_var("FFFINDER_DATA_DIR", config.join("data"));
        }
        config
    });
}

// ----- hang watchdog -----------------------------------------------------------

fn active_steps() -> &'static Mutex<HashMap<ThreadId, (String, Instant)>> {
    static STEPS: OnceLock<Mutex<HashMap<ThreadId, (String, Instant)>>> = OnceLock::new();
    STEPS.get_or_init(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let steps = active_steps().lock().unwrap();
                if let Some((step, started)) =
                    steps.values().find(|(_, t)| t.elapsed() > HANG_LIMIT)
                {
                    eprintln!(
                        "UI step `{step}` has run for {:.0?}; the window looks hung",
                        started.elapsed()
                    );
                    std::process::exit(101);
                }
            }
        });
        Mutex::new(HashMap::new())
    })
}

struct StepGuard;

impl StepGuard {
    fn new(label: String) -> StepGuard {
        active_steps()
            .lock()
            .unwrap()
            .insert(std::thread::current().id(), (label, Instant::now()));
        StepGuard
    }
}

impl Drop for StepGuard {
    fn drop(&mut self) {
        active_steps()
            .lock()
            .unwrap()
            .remove(&std::thread::current().id());
    }
}

// ----- the window ----------------------------------------------------------------

pub struct Ui<'a> {
    pub cx: &'a mut TestAppContext,
    pub handle: AnyWindowHandle,
    pub view: Entity<Finder>,
    pub dir: PathBuf,
}

#[derive(Default)]
pub struct Open {
    pub nu: Option<PathBuf>,
    pub picker: Option<PickerOptions>,
    /// Terminals for "Open Nushell Here"; none by default, so tests never
    /// depend on what is installed.
    pub terminals: Vec<Terminal>,
    /// Frames per second in the status bar, as with `--fps`.
    pub fps: bool,
}

impl<'a> Ui<'a> {
    pub fn open(cx: &'a mut TestAppContext, dir: &Path) -> Ui<'a> {
        Ui::open_with(cx, dir, Open::default())
    }

    pub fn open_with(cx: &'a mut TestAppContext, dir: &Path, options: Open) -> Ui<'a> {
        let _guard = StepGuard::new(format!("open {}", dir.display()));
        let engine = Engine::new(EngineOptions {
            roots: vec![dir.to_path_buf()],
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(20)));
        let nu = options.nu;
        let terminals = Some(options.terminals);
        cx.update(|cx| {
            fffinder::init(
                cx,
                AppState {
                    engine,
                    nu,
                    terminals,
                },
                Appearance::Light,
            );
            cx.set_global(Recorder::default());
            if options.fps {
                cx.set_global(fffinder::finder::ShowFps);
            }
            // Dialogs slide in on a wall-clock animation; under load a click
            // could land while one is still moving.
            cx.set_reduce_motion(true);
        });
        let start = Location::Folder(dir.to_path_buf());
        let picker = options.picker;
        let (handle, view) = cx
            .update(|cx| {
                gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
                    let view = cx.new(|cx| Finder::new(start, None, picker, window, cx));
                    let focus = view.read(cx).files_focus_handle();
                    focus.focus(window, cx);
                    view
                })
            })
            .unwrap();
        let root = dir.to_path_buf();
        view.update(cx, |f, cx| {
            f.set_persist(false);
            // Settings shows these; the engine already indexes them.
            f.set_search_roots(vec![root], cx);
        });
        let mut ui = Ui {
            cx,
            handle,
            view,
            dir: dir.to_path_buf(),
        };
        ui.settle();
        ui
    }

    /// Let background work and timers run, then draw.
    pub fn settle(&mut self) {
        for _ in 0..6 {
            self.cx.run_until_parked();
            self.cx.executor().advance_clock(Duration::from_millis(100));
        }
        self.cx.run_until_parked();
        self.frame();
    }

    pub fn frame(&mut self) {
        // The window may have closed itself (Cmd-W).
        self.cx
            .update_window(self.handle, |_, window, cx| window.render_frame(cx))
            .ok();
    }

    pub fn window<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        self.cx
            .update_window(self.handle, |_, window, cx| f(window, cx))
            .expect("window is open")
    }

    pub fn read<R>(&self, f: impl FnOnce(&Finder) -> R) -> R {
        self.view.read_with(self.cx, |finder, _| f(finder))
    }

    pub fn update<R>(
        &mut self,
        f: impl FnOnce(&mut Finder, &mut Window, &mut Context<Finder>) -> R,
    ) -> R {
        let view = self.view.clone();
        let result = self.window(|window, cx| view.update(cx, |finder, cx| f(finder, window, cx)));
        self.settle();
        result
    }

    fn step(&mut self, label: String, f: impl FnOnce(&mut Window, &mut App)) {
        let _guard = StepGuard::new(label);
        self.window(f);
        self.settle();
    }

    pub fn click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.step(format!("click {id:?}"), |window, cx| window.click(id, cx));
    }

    /// Click `id` inside the element `scope`, for IDs that repeat (a
    /// dialog's close button and a notification's share the name `close`).
    pub fn click_in(&mut self, scope: impl Into<ElementId>, id: impl Into<ElementId>) {
        let (scope, id) = (scope.into(), id.into());
        self.step(format!("click {id:?} in {scope:?}"), |window, cx| {
            window.within(scope).click(id, cx)
        });
    }

    /// Click the open dialog's close (×) button.
    pub fn close_dialog(&mut self) {
        self.click_in("dialog", "close");
    }

    /// Click without letting spawned work run afterwards, so a test can
    /// replace that work before it starts.
    pub fn click_without_settling(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        let _guard = StepGuard::new(format!("click {id:?}"));
        self.window(|window, cx| window.click(id, cx));
    }

    pub fn click_at(&mut self, id: impl Into<ElementId>, offset: Point<Pixels>) {
        let id = id.into();
        self.step(format!("click {id:?} at {offset:?}"), |window, cx| {
            window.click_at(id, offset, cx)
        });
    }

    pub fn double_click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.step(format!("double-click {id:?}"), |window, cx| {
            window.double_click(id, cx)
        });
    }

    pub fn right_click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.step(format!("right-click {id:?}"), |window, cx| {
            window.right_click(id, cx)
        });
    }

    /// Right-click a point inside `id`, `offset` from its top-left corner.
    pub fn right_click_at(&mut self, id: impl Into<ElementId>, offset: Point<Pixels>) {
        let id = id.into();
        let origin = self.snapshot(id.clone()).bounds().origin;
        self.step(format!("right-click {id:?} at {offset:?}"), |window, cx| {
            click_point(window, origin + offset, MouseButton::Right, cx)
        });
    }

    pub fn press(&mut self, key: &str) {
        self.step(format!("press {key}"), |window, cx| window.press(key, cx));
    }

    pub fn type_text(&mut self, text: &str) {
        self.step(format!("type {text:?}"), |window, cx| {
            window.input(text, cx)
        });
    }

    pub fn dispatch(&mut self, action: impl gpui_kit::Action) {
        let label = format!("dispatch {}", action.name());
        let _guard = StepGuard::new(label);
        self.cx.dispatch_action(self.handle, action);
        self.settle();
    }

    pub fn snapshot(&mut self, id: impl Into<ElementId>) -> ElementSnapshot {
        let id = id.into();
        self.window(|window, cx| {
            window.render_frame(cx);
            window.find(id)
        })
    }

    pub fn exists(&mut self, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        self.window(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    pub fn menu_open(&mut self) -> bool {
        self.exists("popup-menu")
    }

    pub fn dialog_open(&mut self) -> bool {
        self.window(|window, cx| window.has_active_dialog(cx))
    }

    /// The labels of the open menu's items, top to bottom.
    pub fn menu_labels(&mut self) -> Vec<String> {
        let mut items: Vec<(Pixels, String)> = self
            .snapshots()
            .into_iter()
            .filter(|s| s.role() == Some(gpui_kit::Role::MenuItem))
            .filter_map(|s| Some((s.bounds().origin.y, s.label()?.to_string())))
            .collect();
        items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        items.into_iter().map(|(_, label)| label).collect()
    }

    /// Click the open menu's item labelled `label`.
    pub fn click_menu_item(&mut self, label: &str) {
        let item = self
            .snapshots()
            .into_iter()
            .find(|s| s.role() == Some(gpui_kit::Role::MenuItem) && s.label() == Some(label))
            .unwrap_or_else(|| panic!("no menu item {label:?}; menu has {:?}", self.menu_labels()));
        assert!(item.visible(), "menu item {label:?} is not visible");
        let center = item.bounds().center();
        self.step(format!("click menu item {label:?}"), |window, cx| {
            click_point(window, center, MouseButton::Left, cx)
        });
    }

    pub fn snapshots(&mut self) -> Vec<ElementSnapshot> {
        self.window(|window, cx| {
            window.render_frame(cx);
            test_support::snapshots(window)
        })
    }

    /// Files handed to the system (open, reveal) so far.
    /// Act as if no app is registered to open `path`.
    pub fn set_no_app(&mut self, path: &Path) {
        self.cx.read(|cx| cx.global::<Recorder>().set_no_app(path));
    }

    pub fn system_calls(&self) -> Vec<SystemCall> {
        self.cx.read(|cx| cx.global::<Recorder>().calls())
    }

    pub fn names(&self) -> Vec<String> {
        self.read(|f| {
            f.rows_for_test()
                .iter()
                .map(|r| r.entry.name.clone())
                .collect()
        })
    }

    pub fn selected(&self) -> Vec<String> {
        self.read(|f| f.selected_entries().into_iter().map(|e| e.name).collect())
    }

    pub fn folder(&self) -> Option<PathBuf> {
        self.read(|f| f.current_folder().map(Path::to_path_buf))
    }

    pub fn files_focused(&mut self) -> bool {
        let view = self.view.clone();
        self.window(|window, cx| view.read(cx).files_focused(window))
    }

    pub fn clipboard(&self) -> String {
        self.cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default()
    }

    /// Navigate to `dir` (inside the fixture) the way the sidebar does.
    pub fn go_to(&mut self, dir: &Path) {
        let dir = dir.to_path_buf();
        self.update(|f, window, cx| f.navigate(Location::Folder(dir), window, cx));
    }

    /// Select the item named `name` with a plain click on its row or tile.
    pub fn select(&mut self, name: &str) {
        let ix = self.index_of(name);
        self.update(|f, window, cx| f.click_row(ix, Default::default(), 1, window, cx));
    }

    /// Add the item named `name` to the selection, as Cmd-click does.
    pub fn select_also(&mut self, name: &str) {
        let ix = self.index_of(name);
        let toggle = if cfg!(target_os = "macos") {
            gpui_kit::Modifiers::command()
        } else {
            gpui_kit::Modifiers::control()
        };
        self.update(|f, window, cx| f.click_row(ix, toggle, 1, window, cx));
    }

    fn index_of(&self, name: &str) -> usize {
        self.names()
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no item {name:?} in {:?}", self.names()))
    }
}

/// A real mouse click at a window position, through native hit testing.
fn click_point(window: &mut Window, position: Point<Pixels>, button: MouseButton, cx: &mut App) {
    window.render_frame(cx);
    window.dispatch_event(
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Default::default(),
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
    window.dispatch_event(
        MouseDownEvent {
            button,
            position,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
    window.dispatch_event(
        MouseUpEvent {
            button,
            position,
            modifiers: Default::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

/// `key` with the platform's command modifier: Cmd on macOS, Ctrl elsewhere.
pub fn cmd(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-{key}")
    } else {
        format!("ctrl-{key}")
    }
}

/// An offset from an element's top-left corner.
pub fn at(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

pub fn item_id(kind: &'static str, path: &Path) -> ElementId {
    (kind, fffinder::finder::path_id(path)).into()
}
