//! The everyday things people reach for without thinking: dragging a box
//! around items, the mouse's back and forward buttons, opening a folder in a
//! new tab, paging through a long list, and where the selection goes.

mod common;

use common::{Fixture, Open, Ui, cmd, item_id};
use gpui_kit::{
    Modifiers, MouseButton, NavigationDirection, Pixels, Point, TestAppContext, point, px,
};

/// A folder with more items than fit in the window.
fn long_folder(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    for ix in 0..200 {
        std::fs::write(fx.path(&format!("item {ix:03}.txt")), "x").unwrap();
    }
    fx
}

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

/// How many tabs the window has, and the folder the last one shows.
fn tabs(ui: &Ui) -> (usize, Option<std::path::PathBuf>) {
    let tabs = ui
        .workspace
        .clone()
        .expect("opened with tabs")
        .read_with(ui.cx, |w, _| w.tabs().to_vec());
    let last = tabs
        .last()
        .and_then(|tab| tab.read_with(ui.cx, |f, _| f.current_folder().map(|p| p.to_path_buf())));
    (tabs.len(), last)
}

/// A point in the rows' empty space, under the last row of a short list.
fn empty_list_space(ui: &mut Ui, fx: &Fixture) -> Point<Pixels> {
    let row = ui.snapshot(item_id("row", &fx.path("Roadmap.md"))).bounds();
    point(row.center().x, row.bottom() + px(60.))
}

/// Cmd on macOS, Ctrl elsewhere.
fn ui_secondary() -> Modifiers {
    Modifiers {
        platform: cfg!(target_os = "macos"),
        control: !cfg!(target_os = "macos"),
        ..Modifiers::none()
    }
}

// ----- dragging a box ------------------------------------------------------------

#[gpui_kit::test]
fn dragging_in_empty_space_selects_the_rows_it_covers(cx: &mut TestAppContext) {
    let fx = Fixture::new("box-list");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let empty = empty_list_space(&mut ui, &fx);
    let notes = ui.snapshot(item_id("row", &fx.path("notes.txt"))).bounds();

    ui.drag_points(empty, notes.center(), Modifiers::none());

    assert_eq!(ui.selected(), ["notes.txt", "Projects", "Roadmap.md"]);
    assert!(ui.files_focused());

    // A press and let go in the same place clears, as a click there always has.
    ui.drag_points(empty, empty, Modifiers::none());
    assert!(ui.selected().is_empty());
}

#[gpui_kit::test]
fn dragging_from_a_rows_other_columns_draws_a_box(cx: &mut TestAppContext) {
    let fx = Fixture::new("box-row");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let budget = item_id("row", &fx.path("Budget.csv"));
    let date = ui.window(|window, _| {
        use gpui_kit::test::TestWindowExt as _;
        window.within(budget).find(("cell", 0usize)).bounds()
    });
    let notes = ui.snapshot(item_id("row", &fx.path("notes.txt"))).bounds();

    ui.drag_points(
        date.center(),
        point(date.center().x, notes.center().y),
        Modifiers::none(),
    );

    assert_eq!(ui.selected(), ["Budget.csv", "notes.txt"]);
    assert!(fx.path("Budget.csv").exists(), "nothing was moved");
    assert!(fx.path("notes.txt").exists());
}

#[gpui_kit::test]
fn cmd_or_shift_adds_a_box_to_the_selection(cx: &mut TestAppContext) {
    let fx = Fixture::new("box-add");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.select("Budget.csv");
    let empty = empty_list_space(&mut ui, &fx);
    let roadmap = ui.snapshot(item_id("row", &fx.path("Roadmap.md"))).bounds();

    ui.drag_points(
        empty,
        roadmap.center(),
        Modifiers {
            shift: true,
            ..Modifiers::none()
        },
    );

    assert_eq!(ui.selected(), ["Budget.csv", "Roadmap.md"]);
}

#[gpui_kit::test]
fn a_box_selects_tiles_in_the_icon_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("box-icons");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-icons");
    let content = ui.snapshot("content").bounds();
    let budget = ui
        .snapshot(item_id("tile", &fx.path("Budget.csv")))
        .bounds();
    let notes = ui.snapshot(item_id("tile", &fx.path("notes.txt"))).bounds();

    // From under the tiles, up to the middle of the second one.
    let start = point(budget.left() + px(4.), content.bottom() - px(20.));
    ui.drag_points(start, notes.center(), Modifiers::none());

    assert_eq!(ui.selected(), ["Budget.csv", "notes.txt"]);

    // A box that stops in the blank margin beside an icon doesn't take it.
    let third = ui.read(|f| f.rows_for_test()[2].entry.path.clone());
    let next = ui.snapshot(item_id("tile", &third)).bounds();
    let into_margin = point(next.left() + px(6.), notes.center().y);
    ui.drag_points(start, into_margin, Modifiers::none());
    assert_eq!(ui.selected(), ["Budget.csv", "notes.txt"]);
}

#[gpui_kit::test]
fn a_box_past_the_edge_scrolls_the_list(cx: &mut TestAppContext) {
    let fx = long_folder("box-scroll");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let content = ui.snapshot("content").bounds();
    let first = ui.snapshot(item_id("row", &fx.path("Budget.csv"))).bounds();
    let date = point(first.right() - px(40.), first.center().y);
    let below = point(date.x, content.bottom() + px(50.));

    ui.drag_and_hold(date, below);
    let before = ui.selected().len();
    ui.cx
        .executor()
        .advance_clock(std::time::Duration::from_millis(500));
    ui.settle();

    let after = ui.selected().len();
    assert!(after > before, "{before} selected, then {after}");
}

#[gpui_kit::test]
fn dragging_a_name_still_moves_the_item(cx: &mut TestAppContext) {
    let fx = Fixture::new("box-name");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    ui.drag(
        item_id("name-cell", &fx.path("notes.txt")),
        item_id("row", &fx.path("Projects")),
        Modifiers::none(),
    );

    assert!(fx.path("Projects/notes.txt").exists());
}

// ----- the mouse's other buttons ---------------------------------------------------

#[gpui_kit::test]
fn the_mouse_back_and_forward_buttons_navigate(cx: &mut TestAppContext) {
    let fx = Fixture::new("mouse-nav");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.go_to(&fx.path("Projects"));

    ui.click_with(MouseButton::Navigate(NavigationDirection::Back), "content");
    assert_eq!(ui.folder(), Some(fx.dir.clone()));

    ui.click_with(
        MouseButton::Navigate(NavigationDirection::Forward),
        "content",
    );
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
}

// ----- new tabs ---------------------------------------------------------------------

#[gpui_kit::test]
fn folders_open_in_new_tabs(cx: &mut TestAppContext) {
    let fx = Fixture::new("new-tab");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.click("view-list");
    let projects = fx.path("Projects");

    // From the item's menu.
    ui.right_click(item_id("row", &projects));
    ui.click_menu_item("Open in New Tab");
    assert_eq!(tabs(&ui), (2, Some(projects.clone())));
    assert_eq!(ui.folder(), Some(fx.dir.clone()), "this tab stays put");

    // With the middle button, and with Cmd+double-click.
    let workspace = ui.workspace.clone().unwrap();
    workspace.update(ui.cx, |w, _| assert_eq!(w.active_index(), 1));
    ui.window(|window, cx| {
        workspace.update(cx, |w, cx| w.select_tab(0, window, cx));
    });
    ui.settle();
    ui.click_with(MouseButton::Middle, item_id("row", &projects));
    assert_eq!(tabs(&ui).0, 3);
    ui.window(|window, cx| {
        workspace.update(cx, |w, cx| w.select_tab(0, window, cx));
    });
    ui.settle();
    let row = item_id("row", &projects);
    ui.double_click_with(row, ui_secondary());
    assert_eq!(tabs(&ui).0, 4);
}

#[gpui_kit::test]
fn files_have_no_open_in_new_tab(cx: &mut TestAppContext) {
    let fx = Fixture::new("new-tab-file");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.click("view-list");

    ui.right_click(item_id("row", &fx.path("notes.txt")));

    assert!(!ui.menu_labels().contains(&"Open in New Tab".to_string()));
}

// ----- the path bar --------------------------------------------------------------------

#[gpui_kit::test]
fn items_drop_on_the_path_bar(cx: &mut TestAppContext) {
    let fx = Fixture::new("crumb-drop");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.go_to(&fx.path("Projects/atlas"));
    let crumbs = fffinder_core::places::breadcrumbs(&fx.path("Projects/atlas")).len();

    // The crumb before the last is the folder this one is in.
    ui.drag(
        item_id("name-cell", &fx.path("Projects/atlas/Project Atlas.txt")),
        ("crumb", crumbs - 2),
        Modifiers::none(),
    );

    assert!(fx.path("Projects/Project Atlas.txt").exists());
}

#[gpui_kit::test]
fn the_current_folder_stays_in_a_long_path_bar(cx: &mut TestAppContext) {
    let fx = Fixture::new("crumb-long");
    let deep = "a folder with quite a long name/".repeat(7) + "the folder shown";
    let deep = fx.path(&deep);
    std::fs::create_dir_all(&deep).unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.go_to(&deep);

    let crumbs = fffinder_core::places::breadcrumbs(&deep).len();
    let shown = ui.snapshot(("crumb", crumbs - 1)).bounds();
    let window = ui.snapshot("content").bounds();

    assert!(
        shown.right() <= window.right(),
        "the last crumb ends at {:?}, past the window at {:?}",
        shown.right(),
        window.right()
    );
}

// ----- keys ----------------------------------------------------------------------------

#[gpui_kit::test]
fn page_up_and_down_move_through_a_long_list(cx: &mut TestAppContext) {
    let fx = long_folder("paging");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let cursor = |ui: &Ui| ui.read(|f| f.selected_entries().first().map(|e| e.name.clone()));

    ui.press("home");
    let first = cursor(&ui);
    ui.press("pagedown");
    let paged = cursor(&ui);
    assert_ne!(paged, first);
    let row = ui.read(|f| {
        f.rows_for_test()
            .iter()
            .position(|r| Some(&r.entry.name) == paged.as_ref())
    });
    assert!(row.unwrap() > 5, "a page is more than a few rows: {row:?}");

    ui.press("pageup");
    assert_eq!(cursor(&ui), first);

    // Shift extends, and so it does for Home and End.
    ui.press("shift-pagedown");
    assert!(ui.selected().len() > 5);
    ui.press("home");
    ui.press("shift-end");
    assert_eq!(ui.selected().len(), ui.names().len());
    ui.press("end");
    assert_eq!(ui.selected().len(), 1);
    ui.press("shift-home");
    assert_eq!(ui.selected().len(), ui.names().len());
}

#[gpui_kit::test]
fn f5_refreshes_without_split_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("f5");
    let mut ui = Ui::open(cx, &fx.dir);
    std::fs::write(fx.path("arrived.txt"), "x").unwrap();
    assert!(!ui.names().contains(&"arrived.txt".to_string()));

    ui.press("f5");

    assert!(ui.names().contains(&"arrived.txt".to_string()));
}

#[gpui_kit::test]
fn the_menu_still_warns_without_a_second_pane(cx: &mut TestAppContext) {
    let fx = Fixture::new("f5-menu");
    let mut ui = with_tabs(cx, &fx.dir);
    ui.select("notes.txt");
    std::fs::write(fx.path("arrived.txt"), "x").unwrap();

    ui.dispatch(fffinder::actions::CopyToOtherPane);

    assert!(
        !ui.names().contains(&"arrived.txt".to_string()),
        "the menu item doesn't refresh"
    );
}

// ----- the selection after a removal ---------------------------------------------------

#[gpui_kit::test]
fn the_next_item_is_selected_after_moving_one_to_the_trash(cx: &mut TestAppContext) {
    let fx = Fixture::new("trash-next");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let names = ui.names();
    let at_notes = names.iter().position(|n| n == "notes.txt").unwrap();

    ui.select("notes.txt");
    ui.press(&cmd("backspace"));
    ui.click("ok");

    assert_eq!(ui.selected(), [names[at_notes + 1].as_str()]);

    // The last item has none after it, so the one before it is next.
    let last = ui.names().pop().unwrap();
    let before = ui.names()[ui.names().len() - 2].clone();
    ui.select(&last);
    ui.press(&cmd("backspace"));
    ui.click("ok");
    assert_eq!(ui.selected(), [before.as_str()]);
}

// ----- the empty-space menu --------------------------------------------------------------

#[gpui_kit::test]
fn the_empty_space_menu_changes_the_view_and_sort(cx: &mut TestAppContext) {
    let fx = Fixture::new("space-menu");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let empty = empty_list_space(&mut ui, &fx);

    ui.right_click_point(empty);

    let labels = ui.menu_labels();
    for label in ["View", "Sort By", "Select All"] {
        assert!(labels.contains(&label.to_string()), "{label} in {labels:?}");
    }
    assert_eq!(
        labels.iter().filter(|l| *l == "Show Hidden Files").count(),
        1,
        "the submenus don't repeat it: {labels:?}"
    );
    ui.click_menu_item("Select All");
    assert_eq!(ui.selected().len(), ui.names().len());
}
