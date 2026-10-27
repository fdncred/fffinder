//! fffinder: a Finder-style file search app on fff-search and GPUI Kit.
//! The binary is a thin wrapper around [`run`]; the library also backs the
//! headless screenshot tool in `examples/screenshot.rs`.

pub mod actions;
pub mod ansi;
pub mod args;
pub mod assets;
pub mod finder;
pub mod graphics;
pub mod macos;
pub mod model;
pub mod nu_env;
pub mod nushell;
pub mod picker;
pub mod rename;
pub mod server;
pub mod system;
pub mod term;
pub mod theme;
pub mod user_actions;
pub mod watchdog;
#[cfg(windows)]
pub mod win;

use crate::finder::{AppState, Finder, PickerOptions};
use crate::model::Location;
use fffinder_core::{Config, Engine, EngineOptions, config};
use gpui_kit::*;
use std::path::PathBuf;

/// Start indexing, open the first window, and run until it closes.
pub fn run(args: args::Args) {
    config::migrate_legacy_dirs();
    let config = Config::load();
    // A window in a terminal opens where it was started, like a shell command.
    // `bool::then` gives `Some(current_dir())` only with `--term`.
    let cwd = args.term.then(std::env::current_dir).and_then(Result::ok);
    let start = start_location(args.path.as_ref().or(cwd.as_ref()), &config);
    let picker = args.pick.then_some(PickerOptions {
        multiple: args.multiple,
    });
    // One app, one index: a second launch hands its folder and search to the
    // window that is already open. A window in a terminal is its own.
    if picker.is_none() && !args.term {
        let path = args
            .path
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
            open_window(start_location(None, &Config::load()), None, None, cx);
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
            let config = Config::load();
            open_window(start_location(None, &config), None, None, cx);
        });
        open_window(start, args.query.clone(), picker, cx);
        cx.activate(true);
    });
}

/// Browser windows, oldest first, for requests that need "the window".
#[derive(Default)]
struct Windows(Vec<(AnyWindowHandle, WeakEntity<Finder>)>);

impl Global for Windows {}

/// Keeps the control server alive for the life of the app.
struct ServerGuard(#[allow(dead_code)] server::Server);

impl Global for ServerGuard {}

/// The window a request should go to: the active one, else the newest.
fn frontmost(cx: &mut App) -> Option<(AnyWindowHandle, Entity<Finder>)> {
    let active = cx.active_window();
    let windows = &mut cx.default_global::<Windows>().0;
    windows.retain(|(_, view)| view.upgrade().is_some());
    let pick = windows
        .iter()
        .rev()
        .find(|(handle, _)| Some(*handle) == active)
        .or_else(|| windows.last())?;
    Some((pick.0, pick.1.upgrade()?))
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
                    let start = start_location(path.as_ref(), &Config::load());
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

/// Options for a browser window: on macOS a transparent titlebar with the
/// traffic lights inset into the toolbar, which also handles window drags.
pub fn window_options(title: SharedString, cx: &App) -> WindowOptions {
    let mac = cfg!(target_os = "macos");
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1100.), px(700.)), cx)),
        window_min_size: Some(size(px(850.), px(470.))),
        titlebar: Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: mac,
            traffic_light_position: mac.then(|| point(px(18.), px(18.))),
        }),
        app_owns_titlebar_drag: mac,
        app_id: Some("fffinder".into()),
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
        let view = cx.new(|cx| Finder::new(start, query, picker, window, cx));
        let focus = view.read(cx).files_focus_handle();
        focus.focus(window, cx);
        view
    });
    match result {
        Ok((handle, view)) => {
            cx.default_global::<Windows>()
                .0
                .push((handle, view.downgrade()));
            Some((handle, view))
        }
        Err(err) => {
            eprintln!("fffinder: could not open a window: {err}");
            None
        }
    }
}
