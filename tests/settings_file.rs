//! What the app reads from and writes to its files: `settings.json` and
//! `window.json`. These tests turn writing on, which the other UI tests never
//! do. Each test program has a settings folder of its own, so only the tests
//! in this file share one, and they take turns.

mod common;

use common::{Fixture, Open, Ui, cmd};
use fffinder::finder::PickerOptions;
use fffinder::model::Location;
use fffinder::settings;
use fffinder_core::Config;
use fffinder_core::config::{WindowPlacement, config_path};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Pixels, TestAppContext, VisualTestContext, px, size,
};
use std::sync::{Mutex, MutexGuard};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn take_turn() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A window with tabs that writes its changes to the files, from a known start.
fn open_writing<'a>(cx: &'a mut TestAppContext, dir: &std::path::Path) -> Ui<'a> {
    let mut ui = Ui::open_with(
        cx,
        dir,
        Open {
            workspace: true,
            ..Default::default()
        },
    );
    ui.cx.update(|cx| settings::set_persist(true, cx));
    // Opening changed the settings before writing was on. This writes all of
    // that, so the file and the app agree.
    toggle_hidden(&mut ui);
    ui
}

fn toggle_hidden(ui: &mut Ui) {
    ui.press(&cmd("shift-."));
    ui.settle();
}

fn settings_file() -> Config {
    Config::read_from(&config_path()).expect("a settings file")
}

#[gpui_kit::test]
fn a_new_tab_picks_up_a_hand_edit(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let fx = Fixture::new("hand-edit");
    let mut ui = open_writing(cx, &fx.dir);

    let edited = Config {
        nu_font_size: 19.,
        ..settings_file()
    };
    edited.save_to(&config_path()).unwrap();
    ui.press(&cmd("t"));

    assert_eq!(ui.cx.update(settings::current).nu_font_size, 19.);
    assert_eq!(
        ui.read(|f| f.config().nu_font_size),
        19.,
        "and in the tabs already open"
    );
}

#[gpui_kit::test]
fn a_new_tab_ignores_a_file_it_cant_read(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let fx = Fixture::new("hand-typo");
    let mut ui = open_writing(cx, &fx.dir);
    let favorite = fx.path("Projects");
    ui.update(|f, _, cx| f.add_favorite(favorite.clone(), cx));
    let before = ui.cx.update(settings::current);
    assert!(before.favorites.contains(&favorite));

    std::fs::write(config_path(), "{ a typo").unwrap();
    ui.press(&cmd("t"));

    assert_eq!(ui.cx.update(settings::current), before);

    // The next save puts all of it back, not only what changed.
    toggle_hidden(&mut ui);
    assert!(settings_file().favorites.contains(&favorite));
    assert_eq!(settings_file().show_hidden, !before.show_hidden);
}

#[cfg(unix)]
#[gpui_kit::test]
fn a_change_that_could_not_be_written_is_written_with_the_next(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;
    let _turn = take_turn();
    let fx = Fixture::new("write-fails");
    let mut ui = open_writing(cx, &fx.dir);
    let folder = fffinder_core::config::config_dir();
    let hidden = ui.cx.update(settings::current).show_hidden;
    let on_top = ui.cx.update(settings::current).folders_first;
    let set_mode = |mode: u32| {
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(mode)).unwrap();
    };

    // No temporary file can be made in a folder that can't be written.
    set_mode(0o500);
    toggle_hidden(&mut ui);
    set_mode(0o700);
    assert_eq!(settings_file().show_hidden, hidden, "the write failed");

    ui.dispatch(fffinder::actions::ToggleFoldersFirst);
    ui.settle();

    let file = settings_file();
    assert_eq!(file.folders_first, !on_top, "the new change");
    assert_eq!(file.show_hidden, !hidden, "and the one before it");
}

/// Move the window without giving the wait before saving time to end.
fn resize_quickly(ui: &mut Ui, width: f32, height: f32) {
    VisualTestContext::from_window(ui.handle, ui.cx).simulate_resize(size(px(width), px(height)));
}

#[gpui_kit::test]
fn closing_a_window_right_after_resizing_it_saves_its_place(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let fx = Fixture::new("place-close");
    let mut ui = open_writing(cx, &fx.dir);
    let _ = std::fs::remove_file(fffinder_core::config::config_dir().join("window.json"));

    resize_quickly(&mut ui, 913., 611.);
    assert_eq!(WindowPlacement::load(), None, "not saved yet");

    // The window's views go once nothing else holds on to them.
    ui.workspace = None;
    let handle = ui.handle;
    ui.cx
        .update_window(handle, |_, window, _| window.remove_window())
        .unwrap();
    ui.cx.run_until_parked();

    let saved = WindowPlacement::load().expect("saved on the way out");
    assert_eq!((saved.width, saved.height), (913., 611.));
}

#[gpui_kit::test]
fn quitting_right_after_resizing_saves_the_windows_place(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let fx = Fixture::new("place-quit");
    let mut ui = open_writing(cx, &fx.dir);
    let _ = std::fs::remove_file(fffinder_core::config::config_dir().join("window.json"));

    resize_quickly(&mut ui, 947., 633.);
    assert_eq!(WindowPlacement::load(), None, "not saved yet");
    ui.cx.quit();

    let saved = WindowPlacement::load().expect("saved on the way out");
    assert_eq!((saved.width, saved.height), (947., 633.));
}

#[gpui_kit::test]
fn a_file_chooser_opens_in_the_middle_and_a_browser_where_the_last_was(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let fx = Fixture::new("place-picker");
    let mut ui = Ui::open(cx, &fx.dir);
    let (x, y, width, height) = (31., 47., 907., 601.);
    WindowPlacement {
        x,
        y,
        width,
        height,
        maximized: false,
        display: None,
    }
    .save()
    .unwrap();
    let open = |ui: &mut Ui, picker: Option<PickerOptions>| -> AnyWindowHandle {
        let start = Location::Folder(ui.dir.clone());
        let (handle, _) = ui
            .cx
            .update(|cx| fffinder::open_window(start, None, picker, cx))
            .expect("a window");
        ui.settle();
        handle
    };
    let bounds = |ui: &mut Ui, handle: AnyWindowHandle| -> Bounds<Pixels> {
        ui.cx
            .update_window(handle, |_, window, _| window.bounds())
            .unwrap()
    };

    let browser = open(&mut ui, None);
    assert_eq!(bounds(&mut ui, browser).size, size(px(width), px(height)));

    let chooser = open(&mut ui, Some(PickerOptions { multiple: false }));
    assert_eq!(bounds(&mut ui, chooser).size, size(px(1100.), px(700.)));
    let _ = std::fs::remove_file(fffinder_core::config::config_dir().join("window.json"));
}
