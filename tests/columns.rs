//! Making things wider and narrower, and seeing text that doesn't fit: list
//! columns, column view panes, and the sidebar, driven with the mouse.

mod common;

use common::{Fixture, Open, Ui, at, cmd, item_id};
use fffinder_core::config::{
    DEFAULT_COLUMN_VIEW_WIDTH, DEFAULT_PARENT_COLUMN_WIDTH, DEFAULT_SIDEBAR_WIDTH,
    MIN_COLUMN_WIDTH, MIN_NAME_WIDTH, SIDEBAR_WIDTH_RANGE,
};
use fffinder_core::{ListColumn, SortKey};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, point, px};

const LONG_FILE: &str = "a very long file name that will never fit in a narrow column.txt";

/// Search results in the list, for a name in a deep folder.
fn search_results(ui: &mut Ui) {
    ui.click("view-list");
    ui.click("search");
    ui.type_text("roadmap");
    ui.settle();
    assert!(ui.read(|f| f.is_searching()));
    assert!(ui.names().contains(&"Roadmap.md".to_string()));
}

#[gpui_kit::test]
fn name_and_folder_columns_resize_in_search_results(cx: &mut TestAppContext) {
    let fx = Fixture::new("resize-search");
    let mut ui = Ui::open(cx, &fx.dir);
    search_results(&mut ui);

    // The Folder column takes its width from its header, and so do its cells.
    assert_eq!(
        ui.read(|f| f.config().parent_column_width),
        DEFAULT_PARENT_COLUMN_WIDTH
    );
    let before = ui.snapshot("Folder").bounds().size.width;
    ui.drag_by("resize-Folder", 60.);
    assert_eq!(
        ui.read(|f| f.config().parent_column_width),
        DEFAULT_PARENT_COLUMN_WIDTH + 60.
    );
    assert!(ui.snapshot("Folder").bounds().size.width > before);

    // Name fills the room until its edge is dragged, from where it was drawn.
    assert_eq!(ui.read(|f| f.config().name_column_width), None);
    let drawn = f32::from(ui.snapshot("Name").bounds().size.width);
    ui.drag_by("resize-Name", 40.);
    let dragged = ui.read(|f| f.config().name_column_width).unwrap();
    assert!(
        (dragged - (drawn + 40.)).abs() < 8.,
        "Name went from about {drawn} to {dragged}"
    );

    // Neither goes below its smallest size.
    ui.drag_by("resize-Folder", -2000.);
    assert_eq!(
        ui.read(|f| f.config().parent_column_width),
        MIN_COLUMN_WIDTH
    );
    ui.drag_by("resize-Name", -2000.);
    assert_eq!(
        ui.read(|f| f.config().name_column_width),
        Some(MIN_NAME_WIDTH)
    );

    // Reset Columns brings both back.
    ui.right_click("Size");
    ui.click_menu_item("Reset Columns");
    assert_eq!(ui.read(|f| f.config().name_column_width), None);
    assert_eq!(
        ui.read(|f| f.config().parent_column_width),
        DEFAULT_PARENT_COLUMN_WIDTH
    );
}

#[gpui_kit::test]
fn dragging_a_columns_edge_leaves_the_next_column_alone(cx: &mut TestAppContext) {
    let fx = Fixture::new("resize-alone");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let order = |ui: &Ui| {
        ui.read(|f| {
            f.config()
                .list_columns
                .iter()
                .map(|c| c.column)
                .collect::<Vec<_>>()
        })
    };
    let columns = order(&ui);
    let sort = ui.read(|f| f.current_sort());

    ui.drag_by("resize-Date Modified", 30.);

    assert_eq!(order(&ui), columns, "the drag didn't move a column");
    assert_eq!(ui.read(|f| f.current_sort()), sort, "or sort by one");
    assert_eq!(
        ui.read(|f| f.config().list_columns[0].width),
        ListColumn::Modified.default_width() + 30.
    );
}

#[gpui_kit::test]
fn wide_columns_scroll_sideways_together(cx: &mut TestAppContext) {
    let fx = Fixture::new("sideways");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.drag_by("resize-Kind", 3000.);
    let row = item_id("row", &fx.path("notes.txt"));
    let header_x = ui.snapshot("Date Modified").bounds().origin.x;
    let row_x = ui.snapshot(row.clone()).bounds().origin.x;

    ui.scroll("list-columns", point(px(-300.), px(0.)));

    let moved = ui.snapshot("Date Modified").bounds().origin.x - header_x;
    assert_eq!(moved, px(-300.), "the header moved with the swipe");
    assert_eq!(
        ui.snapshot(row.clone()).bounds().origin.x - row_x,
        moved,
        "and the rows moved with the header"
    );

    // A swipe up and down moves the rows and not the columns.
    let top = ui.snapshot(row.clone()).bounds().origin.y;
    ui.scroll("list-columns", point(px(0.), px(-4.)));
    assert_eq!(
        ui.snapshot("Date Modified").bounds().origin.x - header_x,
        moved
    );
    assert!(ui.snapshot(row).bounds().origin.y <= top);
}

#[gpui_kit::test]
fn cut_off_text_shows_in_a_tooltip(cx: &mut TestAppContext) {
    let fx = Fixture::new("tooltip");
    std::fs::write(fx.path(LONG_FILE), "long").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.drag_by("resize-Name", -2000.);

    // A name that doesn't fit shows whole when the mouse rests on it.
    let long_row = item_id("row", &fx.path(LONG_FILE));
    let long_name = ui.window(|window, _| window.within(long_row.clone()).find("name").bounds());
    ui.hover_point(long_name.center());
    let tip = ui.snapshot("cut-off-text");
    assert_eq!(tip.label(), Some(LONG_FILE));

    // A name that fits has no tooltip.
    ui.hover_point(at(0., 0.));
    assert!(!ui.exists("cut-off-text"));
    ui.drag_by("resize-Name", 200.);
    let short_row = item_id("row", &fx.path("notes.txt"));
    let short_name = ui.window(|window, _| window.within(short_row.clone()).find("name").bounds());
    ui.hover_point(short_name.center());
    assert!(!ui.exists("cut-off-text"));

    // Nor does a column wide enough for its text; a narrow one does.
    ui.hover_point(at(0., 0.));
    let date = ui.read(|f| f.rows_for_test()[0].date.to_string());
    let first = ui.read(|f| f.rows_for_test()[0].entry.path.clone());
    let cell = |ui: &mut Ui| {
        ui.window(|window, _| {
            window
                .within(item_id("row", &first))
                .find(("cell", 0usize))
                .bounds()
        })
    };
    let wide = cell(&mut ui);
    ui.hover_point(wide.center());
    assert!(!ui.exists("cut-off-text"));
    ui.hover_point(at(0., 0.));
    ui.drag_by("resize-Date Modified", -2000.);
    let narrow = cell(&mut ui);
    ui.hover_point(narrow.center());
    assert_eq!(ui.snapshot("cut-off-text").label(), Some(date.as_str()));
}

#[gpui_kit::test]
fn a_cut_off_header_shows_in_a_tooltip(cx: &mut TestAppContext) {
    let fx = Fixture::new("tooltip-header");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.drag_by("resize-Date Modified", -2000.);
    ui.hover("Date Modified");
    assert_eq!(ui.snapshot("cut-off-text").label(), Some("Date Modified"));
}

#[gpui_kit::test]
fn a_cut_off_name_shows_in_a_tooltip_in_the_icon_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("tooltip-icons");
    std::fs::write(fx.path(LONG_FILE), "long").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-icons");

    // Anywhere on the tile will do, its icon as well as its name.
    ui.hover(item_id("tile", &fx.path(LONG_FILE)));
    assert_eq!(ui.snapshot("cut-off-text").label(), Some(LONG_FILE));

    ui.hover_point(at(0., 0.));
    ui.hover(item_id("tile", &fx.path("notes.txt")));
    assert!(!ui.exists("cut-off-text"));
}

#[gpui_kit::test]
fn a_long_tooltip_wraps(cx: &mut TestAppContext) {
    let fx = Fixture::new("tooltip-wrap");
    let name = "a name that goes on and on ".repeat(8) + ".txt";
    std::fs::write(fx.path(&name), "long").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    let row = item_id("row", &fx.path(&name));
    let shown = ui.window(|window, _| window.within(row).find("name").bounds());
    ui.hover_point(shown.center());

    let tip = ui.snapshot("cut-off-text").bounds();
    assert!(tip.size.width <= px(560.), "{:?} wide", tip.size.width);
    assert!(tip.size.height > px(30.), "{:?} tall", tip.size.height);
}

#[gpui_kit::test]
fn double_clicking_an_edge_fits_the_column(cx: &mut TestAppContext) {
    let fx = Fixture::new("fit");
    std::fs::write(fx.path(LONG_FILE), "long").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    // Far wider than any date, then back to just fit one.
    ui.drag_by("resize-Date Modified", 500.);
    let wide = ui.read(|f| f.config().list_columns[0].width);
    ui.double_click("resize-Date Modified");
    let fitted = ui.read(|f| f.config().list_columns[0].width);
    assert!(fitted < wide, "{fitted} isn't narrower than {wide}");
    assert!(fitted >= MIN_COLUMN_WIDTH);

    // Fitting Name gives it the width of the longest one, and it shows whole.
    ui.double_click("resize-Name");
    let name = ui.read(|f| f.config().name_column_width).unwrap();
    assert!(name > 300., "{name} points is too narrow for the long name");
    let long_row = item_id("row", &fx.path(LONG_FILE));
    let bounds = ui.window(|window, _| window.within(long_row).find("name").bounds());
    ui.hover_point(bounds.center());
    assert!(!ui.exists("cut-off-text"), "the name fits now");
}

#[gpui_kit::test]
fn search_results_sort_by_folder(cx: &mut TestAppContext) {
    let fx = Fixture::new("sort-folder");
    std::fs::write(fx.path("Projects/atlas/atlas.md"), "a").unwrap();
    std::fs::write(fx.path("Projects/atlas.md"), "b").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.click("search");
    ui.type_text("atlas.md");
    ui.settle();
    let folders = |ui: &Ui| {
        ui.read(|f| {
            f.rows_for_test()
                .iter()
                .map(|r| r.entry.parent().map(|p| p.to_path_buf()))
                .collect::<Vec<_>>()
        })
    };

    ui.click("Folder");
    assert_eq!(ui.read(|f| f.current_sort().key), SortKey::Folder);
    let ascending = folders(&ui);
    assert!(ascending.windows(2).all(|pair| pair[0] <= pair[1]));

    ui.click("Folder");
    let descending = folders(&ui);
    assert!(descending.windows(2).all(|pair| pair[0] >= pair[1]));
}

#[gpui_kit::test]
fn a_sort_by_folder_stays_out_of_folders(cx: &mut TestAppContext) {
    let fx = Fixture::new("sort-folder-leak");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let paths = vec![
        fx.path("notes.txt"),
        fx.path("Projects/atlas/Project Atlas.txt"),
    ];
    ui.update(|f, window, cx| f.show_results("Sent".into(), paths, window, cx));

    ui.click("Folder");
    assert_eq!(ui.read(|f| f.current_sort().key), SortKey::Folder);

    ui.go_to(&fx.dir);
    assert_eq!(ui.read(|f| f.current_sort().key), SortKey::Name);
}

#[gpui_kit::test]
fn the_trash_has_no_folder_sort(cx: &mut TestAppContext) {
    let fx = Fixture::new("trash-sort");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.select("notes.txt");
    ui.press(&cmd("backspace"));
    ui.click("ok");
    ui.click("trash");
    let sort = ui.read(|f| f.current_sort());

    ui.click("Original Folder");

    assert_eq!(ui.read(|f| f.current_sort()), sort);
}

#[gpui_kit::test]
fn column_view_panes_resize(cx: &mut TestAppContext) {
    let fx = Fixture::new("panes");
    std::fs::write(fx.path(LONG_FILE), "long").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-columns");
    ui.settle();
    let width =
        |ui: &mut Ui, col: usize| f32::from(ui.snapshot(("column-pane", col)).bounds().size.width);
    assert_eq!(width(&mut ui, 0), DEFAULT_COLUMN_VIEW_WIDTH);

    ui.drag_by(("resize-pane", 0usize), 50.);
    assert_eq!(width(&mut ui, 0), DEFAULT_COLUMN_VIEW_WIDTH + 50.);

    // The next pane to open starts at that width too.
    ui.click(item_id("crow", &fx.path("Projects")));
    ui.settle();
    assert_eq!(width(&mut ui, 1), DEFAULT_COLUMN_VIEW_WIDTH + 50.);

    // Dragging a pane leaves the others alone; double-clicking fits it.
    ui.drag_by(("resize-pane", 1usize), -60.);
    assert_eq!(width(&mut ui, 0), DEFAULT_COLUMN_VIEW_WIDTH + 50.);
    assert_eq!(width(&mut ui, 1), DEFAULT_COLUMN_VIEW_WIDTH - 10.);
    ui.double_click(("resize-pane", 0usize));
    assert!(width(&mut ui, 0) > 300., "the long name fits now");
}

#[gpui_kit::test]
fn the_sidebar_resizes(cx: &mut TestAppContext) {
    let fx = Fixture::new("sidebar-width");
    let mut ui = Ui::open(cx, &fx.dir);
    let width = |ui: &Ui| ui.read(|f| f.config().sidebar_width);
    assert_eq!(width(&ui), DEFAULT_SIDEBAR_WIDTH);

    ui.drag_by("resize-sidebar", 40.);
    assert_eq!(width(&ui), DEFAULT_SIDEBAR_WIDTH + 40.);
    ui.drag_by("resize-sidebar", 2000.);
    assert_eq!(width(&ui), SIDEBAR_WIDTH_RANGE.1);
    ui.double_click("resize-sidebar");
    assert_eq!(width(&ui), DEFAULT_SIDEBAR_WIDTH);
}

#[gpui_kit::test]
fn every_tab_shows_the_same_widths(cx: &mut TestAppContext) {
    let fx = Fixture::new("shared-widths");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            workspace: true,
            ..Default::default()
        },
    );
    ui.click("view-list");
    ui.press(&cmd("t"));
    let tabs = ui
        .workspace
        .clone()
        .unwrap()
        .read_with(ui.cx, |w, _| w.tabs().to_vec());
    assert_eq!(tabs.len(), 2);
    // Tests in this process share a settings folder; keep the new tab out of it.
    for tab in &tabs {
        tab.update(ui.cx, |f, _| f.set_persist(false));
    }

    // A tab's copy of the settings can't go stale and overwrite the widths.
    ui.drag_by("resize-sidebar", 30.);
    ui.settle();
    for tab in &tabs {
        let sidebar = tab.read_with(ui.cx, |f, _| f.config().sidebar_width);
        assert_eq!(sidebar, DEFAULT_SIDEBAR_WIDTH + 30.);
    }
}
