//! Render the fffinder window headlessly (Metal, macOS) and save a PNG.
//!
//! ```text
//! cargo run --example screenshot -- out.png [--dir PATH] [--view icons|list|columns|gallery]
//!     [--light|--dark] [--query TEXT] [--scope-folder] [--select N] [--quicklook]
//!     [--nu PIPELINE] [--size 1100x700] [--no-sidebar] [--settings] [--info]
//!     [--tabs N] [--split]
//! ```

use fffinder::finder::AppState;
use fffinder::model::Location;
use fffinder::workspace::Workspace;
use fffinder::{assets, nushell};
use fffinder_core::{Appearance, Config, Engine, EngineOptions, ViewMode};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, HeadlessAppContext, WindowBounds, WindowOptions, px, size,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct Options {
    out: PathBuf,
    dir: PathBuf,
    view: ViewMode,
    appearance: Appearance,
    query: Option<String>,
    select: Option<usize>,
    nu: Option<String>,
    quick_look: bool,
    settings: bool,
    info: bool,
    tabs: usize,
    split: bool,
    scope_folder: bool,
    width: f32,
    height: f32,
    sidebar: bool,
}

fn parse() -> Options {
    let mut args = std::env::args().skip(1);
    let mut o = Options {
        out: PathBuf::from("screenshot.png"),
        dir: dirs::home_dir().unwrap_or_default(),
        view: ViewMode::Icons,
        appearance: Appearance::Dark,
        query: None,
        select: None,
        nu: None,
        quick_look: false,
        settings: false,
        info: false,
        tabs: 1,
        split: false,
        scope_folder: false,
        width: 1100.,
        height: 700.,
        sidebar: true,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => o.dir = PathBuf::from(args.next().expect("--dir PATH")),
            "--view" => {
                o.view = match args.next().as_deref() {
                    Some("list") => ViewMode::List,
                    Some("columns") => ViewMode::Columns,
                    Some("gallery") => ViewMode::Gallery,
                    _ => ViewMode::Icons,
                }
            }
            "--light" => o.appearance = Appearance::Light,
            "--dark" => o.appearance = Appearance::Dark,
            "--query" => o.query = args.next(),
            "--select" => o.select = args.next().and_then(|s| s.parse().ok()),
            "--nu" => o.nu = args.next(),
            "--no-sidebar" => o.sidebar = false,
            "--quicklook" => o.quick_look = true,
            "--settings" => o.settings = true,
            "--info" => o.info = true,
            "--tabs" => o.tabs = args.next().and_then(|n| n.parse().ok()).unwrap_or(1),
            "--split" => o.split = true,
            "--scope-folder" => o.scope_folder = true,
            "--size" => {
                let s = args.next().expect("--size WxH");
                let (w, h) = s.split_once('x').expect("WxH");
                o.width = w.parse().expect("width");
                o.height = h.parse().expect("height");
            }
            other => o.out = PathBuf::from(other),
        }
    }
    o
}

fn main() {
    let o = parse();
    let config = Config::load();
    let engine = Engine::new(EngineOptions {
        roots: config.search_roots.clone(),
        db_dir: None,
        watch: false,
        search: Default::default(),
    });
    if o.query.is_some() {
        engine.wait_ready(Duration::from_secs(120));
    }
    let nu = nushell::find_nu(config.nu_path.as_deref());

    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(assets::AppAssets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.allow_parking();
    cx.update(|cx| {
        fffinder::init(
            cx,
            AppState {
                engine,
                nu,
                terminals: None,
            },
            o.appearance,
        )
    });

    let dir = o.dir.clone();
    let (handle, workspace) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(o.width), px(o.height)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    cx.new(|cx| Workspace::new(Location::Folder(dir), None, None, window, cx))
                },
            )
        })
        .expect("open window");
    let view = cx.update(|cx| workspace.read(cx).active().clone());

    // Quick Look draws in a window of its own, so every window is drawn.
    let settle = |cx: &mut HeadlessAppContext| {
        for _ in 0..30 {
            cx.run_until_parked();
            cx.advance_clock(Duration::from_millis(100));
            std::thread::sleep(Duration::from_millis(20));
            for window in cx.update(|cx| cx.windows()) {
                cx.update_window(window, |_, window, cx| window.render_frame(cx))
                    .ok();
            }
        }
    };

    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |finder, cx| {
            finder.set_persist(false);
            finder.set_appearance(o.appearance, window, cx);
            finder.set_sidebar_visible(o.sidebar, cx);
            finder.set_view_mode(o.view, cx);
            if o.scope_folder {
                finder.set_scope(fffinder::finder::SearchScope::Folder, cx);
            }
            if let Some(query) = o.query.clone() {
                finder.set_query(query, true, cx);
            }
        })
    })
    .ok();
    settle(&mut cx);
    let extra: Vec<Box<dyn gpui_kit::Action>> = (1..o.tabs)
        .map(|_| Box::new(fffinder::actions::NewTab) as Box<dyn gpui_kit::Action>)
        .chain(
            o.split
                .then(|| Box::new(fffinder::actions::ToggleSplitView) as _),
        )
        .collect();
    for action in extra {
        cx.update_window(handle, |_, window, cx| {
            // A new tab takes over the window; send the next action to it.
            let active = workspace.read(cx).active().clone();
            active.update(cx, |finder, cx| finder.focus_files(window, cx));
            window.dispatch_action(action, cx);
        })
        .ok();
        settle(&mut cx);
    }
    // The rest acts on the tab that shows.
    let view = cx.update(|cx| workspace.read(cx).active().clone());
    if let Some(ix) = o.select {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |finder, cx| {
                finder.click_row(ix, Default::default(), 1, window, cx)
            });
        })
        .ok();
        settle(&mut cx);
    }
    if o.quick_look {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |finder, cx| {
                finder.quick_look(&fffinder::actions::QuickLook, window, cx)
            });
        })
        .ok();
        settle(&mut cx);
        settle(&mut cx);
    }
    let dialogs: [(bool, Box<dyn gpui_kit::Action>); 2] = [
        (o.settings, Box::new(fffinder::actions::ShowSettings)),
        (o.info, Box::new(fffinder::actions::GetInfo)),
    ];
    for (_, action) in dialogs.into_iter().filter(|(on, _)| *on) {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |finder, cx| finder.focus_files(window, cx));
            window.dispatch_action(action, cx);
        })
        .ok();
        settle(&mut cx);
        settle(&mut cx);
    }
    if let Some(pipeline) = o.nu {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |finder, cx| finder.open_nu_with(&pipeline, window, cx));
        })
        .ok();
        // The pipeline runs in a real `nu` process.
        for _ in 0..40 {
            settle(&mut cx);
            if !view.read_with(&cx, |f, _| f.nu_busy()) {
                break;
            }
        }
    }
    settle(&mut cx);

    let shown = match o.quick_look {
        true => view
            .read_with(&cx, |f, _| f.quick_look_window())
            .expect("Quick Look opened its window"),
        false => handle,
    };
    let image = cx.capture_screenshot(shown).expect("Metal rendering");
    image.save(&o.out).expect("save png");
    println!("{}", o.out.display());
}
