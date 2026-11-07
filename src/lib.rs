//! fffinder: a Finder-style file search app on fff-search and GPUI Kit.
//! The binary is a thin wrapper around [`run`]; the library also backs the
//! headless screenshot tool in `examples/screenshot.rs`.

pub mod actions;
pub mod ansi;
pub mod app_icon;
pub mod apps;
pub mod args;
pub mod assets;
pub mod finder;
pub mod graphics;
pub mod highlight;
pub mod macos;
pub mod model;
pub mod nu_env;
pub mod nushell;
pub mod picker;
pub mod rename;
pub mod server;
pub mod settings;
pub mod system;
pub mod term;
pub mod theme;
pub mod thumbnails;
pub mod trash_bin;
pub mod undo;
pub mod user_actions;
pub mod watchdog;
#[cfg(windows)]
pub mod win;
pub mod workspace;

use crate::finder::{AppState, Finder, PickerOptions};
use crate::model::Location;
use fffinder_core::config::WindowPlacement;
use fffinder_core::{Config, Engine, EngineOptions, config};
use gpui_kit::*;
use std::path::PathBuf;

/// Start indexing, open the first window, and run until it closes.
pub fn run(args: args::Args) {
    config::migrate_legacy_dirs();
    let config = Config::load();
    let folder = args.path.clone().or_else(|| shell_folder(args.term));
    let start = start_location(folder.as_ref(), &config);
    let picker = args.pick.then_some(PickerOptions {
        multiple: args.multiple,
    });
    // One app, one index: a second launch hands its folder and search to the
    // window that is already open. A window in a terminal is its own.
    if picker.is_none() && !args.term {
        let path = folder
            .as_ref()
            .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()));
        if server::client::forward_open(path.as_deref(), args.query.as_deref()) {
            return;
        }
    }
    // Checked before indexing starts, so a terminal that can't show the
    // window fails right away.
    let app = if args.term {
        term::application().unwrap_or_else(|err| {
            eprintln!("fffinder: {err:#}");
            std::process::exit(2);
        })
    } else {
        gpui_kit::application()
    };
    // Indexing starts right away, in the background.
    let engine = Engine::new(EngineOptions {
        roots: config.search_roots.clone(),
        app_dirs: config.app_dirs(),
        db_dir: Some(config::data_dir().join("gui")),
        watch: true,
        search: config.search.clone(),
    });
    let nu = nushell::find_nu(config.nu_path.as_deref());

    // macOS keeps running with no windows open, like Finder, so the index
    // stays warm for Nushell; elsewhere, and in a terminal, closing the last
    // window quits.
    let app = app
        .with_assets(assets::AppAssets)
        .with_quit_mode(if picker.is_some() || args.term {
            QuitMode::LastWindowClosed
        } else {
            QuitMode::Default
        });
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            let config = settings::current(cx);
            open_window(start_location(None, &config), None, None, cx);
        }
    });
    app.run(move |cx: &mut App| {
        let server_engine = engine.clone();
        init(
            cx,
            AppState {
                engine,
                nu,
                terminals: None,
            },
            config.appearance,
        );
        watchdog::start_if_enabled(cx);
        #[cfg(target_os = "macos")]
        if !args.term {
            app_icon::set_dock_icon();
        }
        if args.fps {
            cx.set_global(finder::ShowFps);
        }
        // The control socket stays with the GUI.
        if picker.is_none() && !args.term {
            serve(server_engine, cx);
        }
        cx.on_action(move |_: &actions::Quit, cx: &mut App| {
            if picker.is_some() {
                picker::cancel();
            }
            cx.quit();
        });
        cx.on_action(|_: &actions::NewWindow, cx: &mut App| {
            let config = settings::current(cx);
            open_window(start_location(None, &config), None, None, cx);
        });
        open_window(start, args.query.clone(), picker, cx);
        cx.activate(true);
    });
}

/// Browser windows, oldest first, for requests that need "the window".
#[derive(Default)]
struct Windows(Vec<(AnyWindowHandle, WeakEntity<workspace::Workspace>)>);

impl Global for Windows {}

/// Keeps the control server alive for the life of the app.
struct ServerGuard(#[allow(dead_code)] server::Server);

impl Global for ServerGuard {}

/// The window a request should go to (the active one, else the newest),
/// and the tab it shows.
fn frontmost(cx: &mut App) -> Option<(AnyWindowHandle, Entity<Finder>)> {
    let active = cx.active_window();
    let windows = &mut cx.default_global::<Windows>().0;
    windows.retain(|(_, view)| view.upgrade().is_some());
    let pick = windows
        .iter()
        .rev()
        .find(|(handle, _)| Some(*handle) == active)
        .or_else(|| windows.last())?;
    let (handle, workspace) = (pick.0, pick.1.upgrade()?);
    let finder = workspace.read(cx).active().clone();
    Some((handle, finder))
}

/// Answer the control socket (see `docs/protocol.md`).
fn serve(engine: std::sync::Arc<Engine>, cx: &mut App) {
    let (tx, rx) = async_channel::unbounded();
    match server::start(engine, Some(tx)) {
        Ok(server) => {
            cx.set_global(ServerGuard(server));
            // Remove the socket and server.json on a normal quit.
            cx.on_app_quit(|cx| {
                cx.remove_global::<ServerGuard>();
                async {}
            })
            .detach();
        }
        Err(err) => {
            log::warn!("control socket: {err}");
            return;
        }
    }
    cx.spawn(async move |cx| {
        while let Ok(request) = rx.recv().await {
            cx.update(|cx| handle_request(request, cx));
        }
    })
    .detach();
}

fn handle_request(request: server::UiRequest, cx: &mut App) {
    use serde_json::json;
    use server::UiRequest;
    match request {
        UiRequest::Selection { all, reply } => {
            let answer = frontmost(cx)
                .map(|(_, view)| view.read(cx).selection_json(all))
                .ok_or_else(|| "no window is open".to_string());
            let _ = reply.send(answer);
        }
        UiRequest::Show {
            paths,
            title,
            reply,
        } => {
            let title = title.unwrap_or_else(|| "From Nushell".into());
            let (missing, found): (Vec<PathBuf>, Vec<PathBuf>) =
                paths.into_iter().partition(|p| !p.exists());
            if found.is_empty() {
                let _ = reply.send(Err("none of the paths exist".into()));
                return;
            }
            let shown = found.len();
            match frontmost(cx) {
                Some((handle, view)) => {
                    let _ = handle.update(cx, |_, window, cx| {
                        view.update(cx, |f, cx| f.show_results(title, found, window, cx));
                        window.activate_window();
                    });
                }
                None => {
                    let location = Location::Results {
                        title,
                        paths: std::sync::Arc::new(found),
                    };
                    open_window(location, None, None, cx);
                }
            }
            cx.activate(true);
            let _ = reply.send(Ok(json!({ "shown": shown, "missing": missing })));
        }
        UiRequest::Open { path, query, reply } => {
            match frontmost(cx) {
                Some((handle, view)) => {
                    let _ = handle.update(cx, |_, window, cx| {
                        view.update(cx, |f, cx| {
                            f.open_request(path.as_deref(), query.as_deref(), window, cx)
                        });
                        window.activate_window();
                    });
                }
                None => {
                    let start = start_location(path.as_ref(), &settings::current(cx));
                    open_window(start, query, None, cx);
                }
            }
            cx.activate(true);
            let _ = reply.send(Ok(json!({ "ok": true })));
        }
    }
}

/// Kit, theme, shared state, key bindings, and menus.
pub fn init(cx: &mut App, state: AppState, appearance: fffinder_core::Appearance) {
    gpui_kit::init(cx);
    theme::install(cx);
    theme::apply(appearance, None, cx);
    cx.set_global(state);
    actions::bind_keys(cx);
    actions::set_menus(cx);
}

/// The folder to open: the argument, the last folder, or home.
/// The folder to open when none is given: where the shell that started
/// fffinder is, like any command run from a terminal. A launch from the
/// Dock, Finder, Explorer, or a desktop menu has none, so it opens the last
/// folder. `--term` always draws in a terminal, so it always has one.
pub fn shell_folder(term: bool) -> Option<PathBuf> {
    // `bool::then` gives `Some(current_dir())` only when the test passes.
    (term || started_from_shell())
        .then(std::env::current_dir)
        .and_then(Result::ok)
}

#[cfg(not(windows))]
fn started_from_shell() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdin().is_terminal()
}

/// A console program started from Explorer or a shortcut gets a console of
/// its own, so its input is a terminal there too. One started from a shell
/// shares the shell's console with it.
#[cfg(windows)]
fn started_from_shell() -> bool {
    let mut ids = [0u32; 2];
    // SAFETY: the call writes at most `ids.len()` process ids into `ids`.
    let attached = unsafe { ::windows::Win32::System::Console::GetConsoleProcessList(&mut ids) };
    attached > 1
}

pub fn start_location(arg: Option<&PathBuf>, config: &Config) -> Location {
    if let Some(path) = arg {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        if path.is_dir() {
            return Location::Folder(path);
        }
        if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
            return Location::Folder(parent.to_path_buf());
        }
    }
    config
        .last_folder
        .clone()
        .filter(|p| p.is_dir())
        .or_else(dirs::home_dir)
        .map(Location::Folder)
        .unwrap_or(Location::Recents)
}

/// How far each open window moves the next one along, so none is hidden.
const WINDOW_CASCADE: f32 = 28.;

/// Where a window saved at `saved` goes on a display covering `area`, given
/// how many windows are `open`, or `None` if it would be off that display
/// (one that's been rearranged or replaced). `area` is in the same
/// coordinates as the saved position.
fn placed_bounds(
    saved: &WindowPlacement,
    area: Bounds<Pixels>,
    open: usize,
) -> Option<Bounds<Pixels>> {
    let size = size(px(saved.width), px(saved.height)).min(&area.size);
    let bounds = Bounds::new(point(px(saved.x), px(saved.y)), size);
    // Enough of it to see and to drag back.
    let overlap = bounds.intersect(&area).size;
    if overlap.width < px(100.) || overlap.height < px(40.) {
        return None;
    }
    let step = px(WINDOW_CASCADE * open as f32);
    let moved = Bounds::new(bounds.origin + point(step, step), size);
    Some(if area.contains(&moved.bottom_right()) {
        moved
    } else {
        bounds
    })
}

/// Where a new window goes: where the last one was, or the middle of the
/// screen when that's unknown or no longer on any display.
fn new_window_place(cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let centered = || (WindowBounds::centered(size(px(1100.), px(700.)), cx), None);
    if term::active() {
        return centered();
    }
    let Some(saved) = WindowPlacement::load() else {
        return centered();
    };
    let display = cx
        .displays()
        .into_iter()
        .find(|display| {
            let name = display.uuid().ok().map(|uuid| uuid.to_string());
            saved.display.is_some() && name == saved.display
        })
        .or_else(|| cx.primary_display());
    let Some(display) = display else {
        return centered();
    };
    // macOS places a window relative to its display, the others on the
    // whole desktop.
    let mut area = display.bounds();
    if cfg!(target_os = "macos") {
        area.origin = Point::default();
    }
    match placed_bounds(&saved, area, cx.windows().len()) {
        Some(bounds) if saved.maximized => (WindowBounds::Maximized(bounds), Some(display.id())),
        Some(bounds) => (WindowBounds::Windowed(bounds), Some(display.id())),
        None => centered(),
    }
}

/// Options for a browser window: on macOS a transparent titlebar with the
/// traffic lights inset into the toolbar, which also handles window drags.
pub fn window_options(title: SharedString, cx: &App) -> WindowOptions {
    let mac = cfg!(target_os = "macos");
    let (window_bounds, display_id) = new_window_place(cx);
    WindowOptions {
        window_bounds: Some(window_bounds),
        display_id,
        window_min_size: Some(size(px(850.), px(470.))),
        titlebar: Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: mac,
            traffic_light_position: mac.then(|| point(px(18.), px(18.))),
        }),
        app_owns_titlebar_drag: mac,
        app_id: Some("fffinder".into()),
        icon: app_icon::window_icon(),
        ..Default::default()
    }
}

pub fn open_window(
    start: Location,
    query: Option<String>,
    picker: Option<PickerOptions>,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<Finder>)> {
    let title: SharedString = match picker {
        Some(PickerOptions { multiple: true }) => "Choose Files".into(),
        Some(_) => "Choose a File".into(),
        None => "fffinder".into(),
    };
    let options = window_options(title.clone(), cx);
    let result = gpui_kit::open_window(options, cx, move |window, cx| {
        window.set_window_title(&title);
        graphics::report(window);
        if picker.is_some() {
            window.on_window_should_close(cx, |_, _| picker::cancel());
        }
        let view = cx.new(|cx| workspace::Workspace::new(start, query, picker, window, cx));
        let focus = view.read(cx).active().read(cx).files_focus_handle();
        focus.focus(window, cx);
        view
    });
    match result {
        Ok((handle, view)) => {
            cx.default_global::<Windows>()
                .0
                .push((handle, view.downgrade()));
            let finder = view.read(cx).active().clone();
            Some((handle, finder))
        }
        Err(err) => {
            eprintln!("fffinder: could not open a window: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{WINDOW_CASCADE, placed_bounds};
    use fffinder_core::config::WindowPlacement;
    use gpui_kit::{Bounds, point, px, size};

    fn saved(x: f32, y: f32, width: f32, height: f32) -> WindowPlacement {
        WindowPlacement {
            x,
            y,
            width,
            height,
            maximized: false,
            display: None,
        }
    }

    fn display() -> Bounds<gpui_kit::Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)))
    }

    #[test]
    fn a_window_goes_back_where_it_was() {
        let bounds = placed_bounds(&saved(100., 80., 1300., 800.), display(), 0).unwrap();

        assert_eq!(bounds.origin, point(px(100.), px(80.)));
        assert_eq!(bounds.size, size(px(1300.), px(800.)));
    }

    #[test]
    fn more_windows_step_along_until_they_would_leave_the_display() {
        let first = saved(100., 80., 1300., 800.);
        let stepped = placed_bounds(&first, display(), 2).unwrap();
        let step = px(WINDOW_CASCADE * 2.);
        assert_eq!(stepped.origin, point(px(100.) + step, px(80.) + step));

        let at_the_edge = saved(600., 270., 1300., 800.);
        let stayed = placed_bounds(&at_the_edge, display(), 2).unwrap();
        assert_eq!(stayed.origin, point(px(600.), px(270.)));
    }

    #[test]
    fn a_window_off_the_display_is_placed_afresh() {
        assert_eq!(
            placed_bounds(&saved(5000., 80., 1300., 800.), display(), 0),
            None
        );
        assert_eq!(
            placed_bounds(&saved(100., -795., 1300., 800.), display(), 0),
            None
        );
    }

    #[test]
    fn a_window_too_big_for_the_display_shrinks_to_it() {
        let bounds = placed_bounds(&saved(0., 0., 4000., 3000.), display(), 0).unwrap();

        assert_eq!(bounds.size, display().size);
    }
}
