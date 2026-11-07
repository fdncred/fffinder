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

// ----- the sidebar ---------------------------------------------------------------------

#[gpui_kit::test]
fn favorites_drag_into_another_order(cx: &mut TestAppContext) {
    let fx = Fixture::new("fav-order");
    let mut ui = Ui::open(cx, &fx.dir);
    let (projects, atlas) = (fx.path("Projects"), fx.path("Projects/atlas"));
    ui.update(|f, _, cx| {
        f.add_favorite(projects.clone(), cx);
        f.add_favorite(atlas.clone(), cx);
    });
    let places = |ui: &Ui| {
        ui.read(|f| {
            f.favorites()
                .iter()
                .map(|place| place.path.clone())
                .collect::<Vec<_>>()
        })
    };
    let order = places(&ui);
    let last = order.len() - 1;
    assert_eq!(order[last], atlas);

    // The last one goes where the one before it is.
    ui.drag(("fav", last), ("fav", last - 1), Modifiers::none());

    let order = places(&ui);
    assert_eq!(order[last - 1], atlas);
    assert_eq!(order[last], projects);
    assert_eq!(
        ui.read(|f| f.config().favorites.clone()),
        order,
        "and it's saved"
    );

    // A click still opens it.
    ui.click(("fav", last));
    assert_eq!(ui.folder(), Some(projects));
}

// ----- holding a drag over a folder ------------------------------------------------------

#[gpui_kit::test]
fn holding_a_drag_over_a_folder_opens_it(cx: &mut TestAppContext) {
    let fx = Fixture::new("spring");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let file = ui
        .snapshot(item_id("name-cell", &fx.path("notes.txt")))
        .bounds();
    let folder = ui.snapshot(item_id("row", &fx.path("Projects"))).bounds();

    ui.drag_and_hold(file.center(), folder.center());
    ui.settle();
    assert_eq!(ui.folder(), Some(fx.path("Projects")), "the folder opened");

    // The drag is still going, and it can go on into the next one down.
    let atlas = ui
        .snapshot(item_id("row", &fx.path("Projects/atlas")))
        .bounds();
    let moved = atlas.center();
    ui.window(|window, cx| {
        use gpui_kit::InputEvent as _;
        window.dispatch_event(
            gpui_kit::MouseMoveEvent {
                position: moved,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::none(),
            }
            .to_platform_input(),
            cx,
        );
    });
    ui.settle();
    ui.settle();
    assert_eq!(ui.folder(), Some(fx.path("Projects/atlas")));

    // Letting go puts the item in the folder that opened.
    let content = ui.snapshot("content").bounds();
    ui.release(content.center());
    assert!(fx.path("Projects/atlas/notes.txt").exists());
    assert!(!fx.path("notes.txt").exists());
}

#[gpui_kit::test]
fn a_quick_drag_past_a_folder_leaves_it_closed(cx: &mut TestAppContext) {
    let fx = Fixture::new("spring-past");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let file = ui
        .snapshot(item_id("name-cell", &fx.path("notes.txt")))
        .bounds();
    let folder = ui.snapshot(item_id("row", &fx.path("Projects"))).bounds();

    // Letting go before the delay drops it on the folder, without opening it.
    ui.drag_points(file.center(), folder.center(), Modifiers::none());

    assert_eq!(ui.folder(), Some(fx.dir.clone()));
    assert!(fx.path("Projects/notes.txt").exists());

    // And it stays closed, even if another drag starts before the delay ends.
    let budget = ui
        .snapshot(item_id("name-cell", &fx.path("Budget.csv")))
        .bounds();
    let empty = point(budget.center().x, budget.bottom() + px(200.));
    ui.drag_and_hold(budget.center(), empty);
    ui.settle();
    assert_eq!(ui.folder(), Some(fx.dir.clone()));
    ui.release(empty);
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

// ----- the toolbar ------------------------------------------------------------------

#[gpui_kit::test]
fn the_toolbar_folds_what_does_not_fit(cx: &mut TestAppContext) {
    let fx = Fixture::new("toolbar-fold");
    let mut ui = Ui::open(cx, &fx.dir);
    let width = |ui: &mut Ui, points: f32| {
        let handle = ui.handle;
        gpui_kit::VisualTestContext::from_window(handle, ui.cx)
            .simulate_resize(gpui_kit::size(px(points), px(600.)));
        ui.settle();
        ui.frame();
    };

    width(&mut ui, 1400.);
    assert!(ui.exists("search-scope"));
    assert!(ui.exists("view-list"));
    assert!(ui.exists("sort"));
    assert!(ui.exists("nushell"));

    // The sidebar takes 212 of these, and the rest isn't quite enough.
    width(&mut ui, 850.);
    assert!(!ui.exists("search-scope"), "the first to go");
    assert!(ui.exists("view-list"));

    width(&mut ui, 800.);
    assert!(!ui.exists("view-list"), "the views become one menu");
    assert!(ui.exists("view-menu"));
    assert!(ui.exists("nushell"));

    width(&mut ui, 700.);
    assert!(!ui.exists("nushell"), "into the Actions menu");
    assert!(ui.exists("sort"));
    ui.click("actions");
    assert!(ui.menu_labels().contains(&"Nushell".to_string()));
    ui.press("escape");

    width(&mut ui, 640.);
    assert!(!ui.exists("sort"), "into the Actions menu too");
    ui.click("actions");
    assert!(
        ui.menu_labels()
            .contains(&"Sort and View Options".to_string())
    );
    ui.press("escape");

    // The search field stays inside the window all the way down.
    let search = ui.snapshot("search").bounds();
    assert!(search.right() <= px(640.), "{:?}", search.right());

    // And everything comes back.
    width(&mut ui, 1400.);
    assert!(ui.exists("search-scope"));
    assert!(ui.exists("view-list"));
}

#[gpui_kit::test]
fn the_scope_bar_wraps_rather_than_losing_its_controls(cx: &mut TestAppContext) {
    let fx = Fixture::new("scope-wrap");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("search");
    ui.type_text("notes");
    ui.settle();
    let handle = ui.handle;
    gpui_kit::VisualTestContext::from_window(handle, ui.cx)
        .simulate_resize(gpui_kit::size(px(640.), px(600.)));
    ui.settle();

    let kind = ui.snapshot("kind").bounds();

    assert!(kind.right() <= px(640.), "{:?}", kind.right());
}

// ----- renaming in place ----------------------------------------------------------------

#[gpui_kit::test]
fn f2_renames_in_place(cx: &mut TestAppContext) {
    let fx = Fixture::new("rename-inline");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    // Only the part before the extension is selected, so typing replaces it.
    ui.select("Roadmap.md");
    ui.press("f2");
    assert!(ui.exists("rename-input"));
    assert!(!ui.dialog_open());
    ui.type_text("Plan B");
    ui.press("enter");
    assert!(!ui.exists("rename-input"));
    assert!(
        fx.path("Plan B.md").exists(),
        "a space is text, not Quick Look"
    );
    assert!(!ui.read(|f| f.quick_look_is_open()));
    assert_eq!(ui.selected(), ["Plan B.md"]);
    assert!(ui.files_focused(), "the keys go back to the files");

    // Letters are text too; they don't select other items.
    ui.select("Plan B.md");
    ui.press("f2");
    ui.type_text("n");
    assert_eq!(ui.selected(), ["Plan B.md"]);

    // Escape leaves the name alone.
    ui.press("escape");
    assert!(!ui.exists("rename-input"));
    assert!(fx.path("Plan B.md").exists());
    assert!(ui.files_focused());

    // Clicking somewhere else keeps what was typed.
    ui.press("f2");
    ui.type_text("Plan C");
    ui.click(item_id("row", &fx.path("notes.txt")));
    assert!(fx.path("Plan C.md").exists());
    assert!(!ui.exists("rename-input"));

    // Undo puts the old name back, as it does for any rename.
    ui.press(&cmd("z"));
    assert!(fx.path("Plan B.md").exists());
}

#[gpui_kit::test]
fn a_taken_name_keeps_editing(cx: &mut TestAppContext) {
    let fx = Fixture::new("rename-taken");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    ui.select("Roadmap.md");
    ui.press("f2");
    ui.press(&cmd("a"));
    ui.type_text("notes.txt");
    ui.press("enter");

    assert!(ui.exists("rename-input"), "still editing, to fix the name");
    assert!(fx.path("Roadmap.md").exists());
    ui.press("escape");
    assert!(fx.path("Roadmap.md").exists());
}

#[gpui_kit::test]
fn renaming_in_place_works_in_every_view_but_the_gallery(cx: &mut TestAppContext) {
    let fx = Fixture::new("rename-views");
    let mut ui = Ui::open(cx, &fx.dir);

    ui.click("view-icons");
    ui.select("notes.txt");
    ui.press("f2");
    ui.type_text("icons");
    ui.press("enter");
    assert!(fx.path("icons.txt").exists());

    ui.click("view-columns");
    ui.settle();
    ui.click(item_id("crow", &fx.path("icons.txt")));
    ui.press("f2");
    ui.type_text("columns");
    ui.press("enter");
    assert!(fx.path("columns.txt").exists());

    ui.click("view-gallery");
    ui.press("f2");
    assert!(ui.dialog_open());
    assert!(!ui.exists("rename-field"));
    ui.press("escape");
}

#[gpui_kit::test]
fn a_new_folder_is_named_in_place(cx: &mut TestAppContext) {
    let fx = Fixture::new("rename-new");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    ui.press(&cmd("shift-n"));

    assert!(!ui.dialog_open());
    assert_eq!(ui.selected(), ["untitled folder"]);
    ui.type_text("Archive");
    ui.press("enter");
    assert!(fx.path("Archive").is_dir());
    assert_eq!(ui.selected(), ["Archive"]);
}

// ----- the preview pane -----------------------------------------------------------------

#[gpui_kit::test]
fn the_preview_pane_follows_the_selection(cx: &mut TestAppContext) {
    let fx = Fixture::new("pane");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    assert!(!ui.exists("preview-pane"));

    ui.press(&cmd("shift-p"));
    assert!(ui.exists("preview-pane"));
    assert!(ui.exists("preview-pane-empty"), "nothing's selected yet");

    ui.select("Roadmap.md");
    assert!(!ui.exists("preview-pane-empty"));
    assert!(ui.read(|f| f.has_preview_for_test(&fx.path("Roadmap.md"))));

    ui.select_also("notes.txt");
    assert_eq!(ui.snapshot("preview-pane-many").label(), Some("2 items"));

    // It's for the views without a preview of their own.
    ui.click("view-gallery");
    assert!(!ui.exists("preview-pane"));
    ui.click("view-columns");
    assert!(!ui.exists("preview-pane"));
    ui.click("view-icons");
    assert!(ui.exists("preview-pane"));

    ui.press(&cmd("shift-p"));
    assert!(!ui.exists("preview-pane"));
}

#[gpui_kit::test]
fn the_preview_pane_resizes_from_its_left_edge(cx: &mut TestAppContext) {
    let fx = Fixture::new("pane-width");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.press(&cmd("shift-p"));
    let width = |ui: &Ui| ui.read(|f| f.config().preview_pane_width);
    let start = width(&ui);

    ui.drag_by("resize-preview", -40.);
    assert_eq!(width(&ui), start + 40., "dragging left widens it");

    ui.double_click("resize-preview");
    assert_eq!(width(&ui), start);
}

// ----- the status bar ---------------------------------------------------------------------

#[gpui_kit::test]
fn the_status_bar_adds_up_the_selection(cx: &mut TestAppContext) {
    let fx = Fixture::new("status-size");
    std::fs::write(fx.path("one.txt"), vec![b'x'; 2000]).unwrap();
    std::fs::write(fx.path("two.txt"), vec![b'x'; 3000]).unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    ui.select("one.txt");
    ui.select_also("two.txt");

    let status = ui.snapshot("status-text");
    let text = status.label().unwrap();
    assert!(text.contains("2 of"), "{text}");
    assert!(text.contains("(5.0 KB)"), "{text}");
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

    assert_eq!(ui.selected(), [names[at_notes + 1].as_str()]);

    // The last item has none after it, so the one before it is next.
    let last = ui.names().pop().unwrap();
    let before = ui.names()[ui.names().len() - 2].clone();
    ui.select(&last);
    ui.press(&cmd("backspace"));
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
