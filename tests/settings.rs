//! One copy of the settings: tabs share what's shared, keep their own view of
//! the files, and can't write an old value over a newer one.

mod common;

use common::{Fixture, Open, Ui, cmd};
use fffinder::finder::Finder;
use fffinder::settings;
use fffinder_core::{ListColumn, SortKey, ViewMode};
use gpui_kit::{Entity, TestAppContext};

fn with_tabs<'a>(cx: &'a mut TestAppContext, dir: &'a std::path::Path) -> Ui<'a> {
    Ui::open_with(
        cx,
        dir,
        Open {
            workspace: true,
            ..Default::default()
        },
    )
}

fn tabs(ui: &Ui) -> Vec<Entity<Finder>> {
    ui.workspace
        .clone()
        .expect("opened with tabs")
        .read_with(ui.cx, |w, _| w.tabs().to_vec())
}

/// Show tab `ix` and drive the window through it.
fn show_tab<'a>(ui: &mut Ui<'a>, ix: usize) {
    let workspace = ui.workspace.clone().expect("opened with tabs");
    ui.window(|window, cx| workspace.update(cx, |w, cx| w.select_tab(ix, window, cx)));
    ui.view = tabs(ui)[ix].clone();
    ui.settle();
}

#[gpui_kit::test]
fn changes_in_two_tabs_both_stay(cx: &mut TestAppContext) {
    let fx = Fixture::new("two-tabs");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.click("view-list");
    ui.press(&cmd("t"));

    // The second tab adds a column, then the first changes its sort. The
    // first tab's older copy must not put the old columns back.
    show_tab(&mut ui, 1);
    ui.click("view-list");
    ui.right_click("Size");
    ui.click_menu_item("Permissions");
    show_tab(&mut ui, 0);
    ui.click("Size");

    let saved = ui.cx.update(settings::current);
    assert!(
        saved
            .list_columns
            .iter()
            .any(|c| c.column == ListColumn::Permissions),
        "the second tab's column"
    );
    assert_eq!(saved.sort.key, SortKey::Size, "and the first tab's sort");
}

#[gpui_kit::test]
fn shared_settings_show_in_every_tab_at_once(cx: &mut TestAppContext) {
    let fx = Fixture::new("shared");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.press(&cmd("t"));
    let all = tabs(&ui);
    let hidden = |ui: &Ui, tab: &Entity<Finder>| {
        tab.read_with(ui.cx, |f, _| {
            f.rows_for_test().iter().any(|r| r.entry.name == ".secret")
        })
    };
    assert!(!hidden(&ui, &all[0]));

    // Hidden files, from the tab that's showing.
    ui.view = all[1].clone();
    ui.press(&cmd("shift-."));
    assert!(hidden(&ui, &all[0]), "the other tab lists them too");
    assert!(hidden(&ui, &all[1]));

    // A favorite, in the other tab's sidebar without a reload.
    let added = fx.path("Projects");
    all[1].update(ui.cx, |f, cx| f.add_favorite(added.clone(), cx));
    ui.settle();
    let shown = all[0].read_with(ui.cx, |f, _| {
        f.favorites().iter().any(|place| place.path == added)
    });
    assert!(shown);
}

#[gpui_kit::test]
fn each_tab_keeps_its_own_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("own-view");
    let mut ui = with_tabs(cx, &fx.dir);
    let before = ui.read(|f| f.config().view_mode);
    ui.press(&cmd("t"));
    show_tab(&mut ui, 1);
    ui.click("view-gallery");

    let view = |ui: &Ui, ix: usize| tabs(ui)[ix].read_with(ui.cx, |f, _| f.config().view_mode);
    assert_eq!(view(&ui, 1), ViewMode::Gallery);
    assert_eq!(view(&ui, 0), before, "the first tab didn't move");

    // The next tab starts with the view chosen last.
    ui.press(&cmd("t"));
    assert_eq!(view(&ui, 2), ViewMode::Gallery);
}

#[gpui_kit::test]
fn the_split_pane_keeps_its_view_to_itself(cx: &mut TestAppContext) {
    let fx = Fixture::new("split-own");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.click("view-icons");
    ui.press(&cmd("\\"));
    let workspace = ui.workspace.clone().unwrap();
    let pane = workspace
        .read_with(ui.cx, |w, _| w.split_pane().cloned())
        .expect("split view is on");
    let main = ui.view.clone();

    pane.update(ui.cx, |f, cx| f.set_view_mode(ViewMode::List, cx));
    ui.settle();

    assert_eq!(
        main.read_with(ui.cx, |f, _| f.config().view_mode),
        ViewMode::Icons
    );
    let saved = ui.cx.update(settings::current);
    assert_eq!(
        saved.view_mode,
        ViewMode::Icons,
        "the pane's view isn't saved"
    );
    assert!(
        main.read_with(ui.cx, |f, _| f.config().show_sidebar),
        "and the pane having no sidebar doesn't hide the main one"
    );

    // What's shared still reaches everyone from there.
    pane.update(ui.cx, |f, cx| f.set_show_hidden(true, cx));
    ui.settle();
    assert!(main.read_with(ui.cx, |f, _| f.config().show_hidden));
}

#[gpui_kit::test]
fn a_new_window_starts_from_the_latest_settings(cx: &mut TestAppContext) {
    let fx = Fixture::new("new-window");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.press(&cmd("shift-."));

    // The file hasn't been written; the next browser still sees the change.
    ui.press(&cmd("t"));

    let last = tabs(&ui).pop().unwrap();
    assert!(last.read_with(ui.cx, |f, _| f.config().show_hidden));
}

#[gpui_kit::test]
fn a_folder_opens_in_the_view_chosen_in_it(cx: &mut TestAppContext) {
    let fx = Fixture::new("folder-view");
    let mut ui = Ui::open(cx, &fx.dir);
    let view = |ui: &Ui| ui.read(|f| f.config().view_mode);
    let sort = |ui: &Ui| ui.read(|f| f.current_sort().key);

    // Projects gets the list, sorted by size.
    ui.go_to(&fx.path("Projects"));
    ui.click("view-list");
    ui.click("Size");
    assert_eq!(sort(&ui), SortKey::Size);

    // The folder above gets icons of its own.
    ui.go_to(&fx.dir);
    ui.click("view-icons");

    // Each opens with what was chosen in it.
    ui.go_to(&fx.path("Projects"));
    assert_eq!(view(&ui), ViewMode::List);
    assert_eq!(sort(&ui), SortKey::Size);
    ui.press(&cmd("["));
    assert_eq!(view(&ui), ViewMode::Icons, "Back too");

    // A folder with no choice of its own keeps the view it was opened from.
    ui.go_to(&fx.path("Projects"));
    ui.go_to(&fx.path("Projects/atlas"));
    assert_eq!(view(&ui), ViewMode::List);
}

#[gpui_kit::test]
fn another_tabs_choice_leaves_a_new_folder_alone(cx: &mut TestAppContext) {
    let fx = Fixture::new("folder-view-tabs");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.click("view-list");
    ui.press(&cmd("t"));

    // The second tab picks icons, and the first goes somewhere new.
    show_tab(&mut ui, 1);
    ui.click("view-icons");
    show_tab(&mut ui, 0);
    ui.go_to(&fx.path("Projects"));

    assert_eq!(ui.read(|f| f.config().view_mode), ViewMode::List);
}

#[gpui_kit::test]
fn a_new_tab_opens_a_folder_in_its_own_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("folder-view-tab");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.go_to(&fx.path("Projects"));
    ui.click("view-gallery");
    ui.go_to(&fx.dir);
    ui.click("view-list");

    ui.right_click(common::item_id("row", &fx.path("Projects")));
    ui.click_menu_item("Open in New Tab");

    let opened = tabs(&ui)[1].read_with(ui.cx, |f, _| f.config().view_mode);
    assert_eq!(opened, ViewMode::Gallery);
}

#[gpui_kit::test]
fn folders_can_share_one_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("folder-view-off");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.update(|f, _, cx| f.set_remember_folder_views(false, cx));
    let view = |ui: &Ui| ui.read(|f| f.config().view_mode);

    ui.go_to(&fx.path("Projects"));
    ui.click("view-list");
    ui.go_to(&fx.dir);
    assert_eq!(view(&ui), ViewMode::List, "the view stays as it is");
    ui.click("view-icons");
    ui.go_to(&fx.path("Projects"));
    assert_eq!(view(&ui), ViewMode::Icons, "nothing was remembered");
}

#[gpui_kit::test]
fn the_split_pane_chooses_nothing_for_a_folder(cx: &mut TestAppContext) {
    let fx = Fixture::new("folder-view-split");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.click("view-icons");
    ui.press(&cmd("\\"));
    let pane = ui
        .workspace
        .clone()
        .unwrap()
        .read_with(ui.cx, |w, _| w.split_pane().cloned())
        .expect("split view is on");

    pane.update(ui.cx, |f, cx| f.set_view_mode(ViewMode::List, cx));
    ui.settle();

    ui.go_to(&fx.path("Projects"));
    ui.go_to(&fx.dir);
    assert_eq!(ui.read(|f| f.config().view_mode), ViewMode::Icons);
}
