//! Browsing, searching, and every keyboard shortcut, driven through real key
//! presses in a headless window.

mod common;

use common::{Fixture, Ui, cmd};
use fffinder::model::Location;
use fffinder_core::ViewMode;
use gpui_kit::TestAppContext;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};

#[gpui_kit::test]
fn lists_navigates_and_goes_back(cx: &mut TestAppContext) {
    let fx = Fixture::new("nav");
    let mut ui = Ui::open(cx, &fx.dir);
    assert_eq!(
        ui.names(),
        ["Budget.csv", "notes.txt", "Projects", "Roadmap.md"]
    );

    ui.update(|f, window, cx| f.click_row(2, Default::default(), 2, window, cx));
    assert_eq!(ui.names(), ["atlas"]);

    ui.press(&cmd("["));
    assert_eq!(ui.folder().as_deref(), Some(fx.dir.as_path()));
    assert_eq!(
        ui.selected(),
        ["Projects"],
        "back selects the folder we left"
    );
    ui.press(&cmd("]"));
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
    ui.press("alt-left");
    assert_eq!(ui.folder().as_deref(), Some(fx.dir.as_path()));
    ui.press("alt-right");
    ui.press(&cmd("up"));
    assert_eq!(
        ui.folder().as_deref(),
        Some(fx.dir.as_path()),
        "Cmd-Up goes to the parent"
    );
    assert_eq!(ui.selected(), ["Projects"]);

    // Cmd-Down, Cmd-O, and Return open the selection.
    ui.press(&cmd("down"));
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
    ui.press(&cmd("up"));
    ui.press(&cmd("o"));
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
    ui.press(&cmd("up"));
    ui.press("enter");
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
}

#[gpui_kit::test]
fn go_shortcuts(cx: &mut TestAppContext) {
    let fx = Fixture::new("go-keys");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd("shift-f"));
    assert_eq!(ui.read(|f| f.location().clone()), Location::Recents);
    ui.press(&cmd("shift-h"));
    assert_eq!(ui.folder(), dirs::home_dir());
}

#[gpui_kit::test]
fn searches_by_name_and_returns_to_browsing(cx: &mut TestAppContext) {
    let fx = Fixture::new("search");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd("f"));
    assert!(
        ui.window(|window, cx| window.has_focused_input(cx)),
        "Cmd-F focuses search"
    );
    ui.type_text("atlas");
    let found = ui.names();
    assert!(
        found.contains(&"Project Atlas.txt".to_string()),
        "{found:?}"
    );
    assert!(
        found.contains(&"atlas".to_string()),
        "folders match too: {found:?}"
    );

    // Return moves into the results with the first one selected.
    ui.press("enter");
    assert!(ui.files_focused());
    assert_eq!(ui.selected().len(), 1);

    // Escape clears the selection, then the search.
    ui.press("escape");
    assert!(ui.selected().is_empty());
    ui.press("escape");
    assert_eq!(
        ui.names(),
        ["Budget.csv", "notes.txt", "Projects", "Roadmap.md"]
    );
}

#[gpui_kit::test]
fn arrow_keys_select_in_each_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("arrows");
    let mut ui = Ui::open(cx, &fx.dir);

    ui.press(&cmd("2"));
    assert_eq!(ui.read(|f| f.effective_view()), ViewMode::List);
    ui.press("down");
    ui.press("shift-down");
    assert_eq!(ui.selected(), ["Budget.csv", "notes.txt"]);
    ui.press("up");
    assert_eq!(ui.selected(), ["Budget.csv"]);
    ui.press("end");
    assert_eq!(ui.selected(), ["Roadmap.md"]);
    ui.press("home");
    assert_eq!(ui.selected(), ["Budget.csv"]);
    ui.press("shift-down");
    ui.press("shift-down");
    ui.press("shift-up");
    assert_eq!(ui.selected(), ["Budget.csv", "notes.txt"]);
    ui.press(&cmd("a"));
    assert_eq!(ui.selected().len(), 4, "Cmd-A selects everything");
    ui.press("escape");
    assert!(ui.selected().is_empty());

    // Icons and the gallery move with Left and Right too.
    ui.press(&cmd("1"));
    assert_eq!(ui.read(|f| f.effective_view()), ViewMode::Icons);
    ui.press("right");
    ui.press("right");
    assert_eq!(ui.selected(), ["notes.txt"]);
    ui.press("shift-left");
    assert_eq!(ui.selected(), ["Budget.csv", "notes.txt"]);
    ui.press("shift-right");
    assert_eq!(ui.selected(), ["notes.txt"]);

    ui.press(&cmd("4"));
    assert_eq!(ui.read(|f| f.effective_view()), ViewMode::Gallery);
    ui.press("right");
    assert_eq!(ui.selected().len(), 1);

    // Columns: Right enters a folder, Left comes back.
    ui.press(&cmd("3"));
    assert_eq!(ui.read(|f| f.effective_view()), ViewMode::Columns);
    ui.update(|f, window, cx| f.select_in_column(0, 2, window, cx));
    assert_eq!(ui.read(|f| f.column_names_for_test()).len(), 2);
    ui.press("right");
    assert_eq!(ui.selected(), ["atlas"]);
    ui.press("left");
    assert_eq!(ui.selected(), ["Projects"]);
}

#[gpui_kit::test]
fn clipboard_shortcuts(cx: &mut TestAppContext) {
    let fx = Fixture::new("clipboard");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd("2"));
    ui.press("down");
    ui.press("shift-down");
    let both = format!(
        "{}\n{}",
        fx.path("Budget.csv").display(),
        fx.path("notes.txt").display()
    );
    ui.press(&cmd("c"));
    assert_eq!(ui.clipboard(), both);
    ui.cx
        .write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
    ui.press(&cmd("alt-c"));
    assert_eq!(ui.clipboard(), both);
    ui.press(&cmd("shift-c"));
    assert!(
        ui.clipboard().starts_with('['),
        "NUON list: {}",
        ui.clipboard()
    );
    assert!(ui.clipboard().contains("notes.txt"));
}

#[gpui_kit::test]
fn window_shortcuts(cx: &mut TestAppContext) {
    let fx = Fixture::new("window-keys");
    let mut ui = Ui::open(cx, &fx.dir);

    ui.press(&cmd("shift-."));
    assert!(ui.read(|f| f.config().show_hidden));
    assert!(ui.names().contains(&".secret".to_string()));
    ui.press(&cmd("shift-."));
    assert!(!ui.names().contains(&".secret".to_string()));

    ui.press(&cmd("alt-s"));
    assert!(!ui.read(|f| f.config().show_sidebar));
    ui.press(&cmd("alt-s"));
    assert!(ui.read(|f| f.config().show_sidebar));

    let dark = ui.cx.update(|cx| cx.theme().is_dark());
    ui.press(&cmd("shift-d"));
    assert_ne!(ui.cx.update(|cx| cx.theme().is_dark()), dark);

    std::fs::write(fx.path("new.txt"), "x").unwrap();
    ui.press(&cmd("r"));
    assert!(
        ui.names().contains(&"new.txt".to_string()),
        "Cmd-R refreshes"
    );

    ui.press("ctrl-`");
    assert!(ui.read(|f| f.nu_panel_open()));
    ui.press("ctrl-`");
    assert!(!ui.read(|f| f.nu_panel_open()));

    ui.press(&cmd("/"));
    assert!(ui.dialog_open(), "Cmd-/ shows the search syntax");
    ui.press("escape");

    // Without nu this warns instead of opening a terminal.
    ui.press(&cmd("alt-t"));
    assert!(!ui.dialog_open());
}

#[gpui_kit::test]
fn file_shortcuts(cx: &mut TestAppContext) {
    let fx = Fixture::new("file-keys");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd("shift-n"));
    assert!(fx.path("untitled folder").is_dir());
    assert!(ui.dialog_open(), "a new folder opens Rename");
    ui.type_text("Archive");
    ui.press("enter");
    assert!(!ui.dialog_open());
    assert!(fx.path("Archive").is_dir());
    assert_eq!(ui.selected(), ["Archive"]);

    ui.select("notes.txt");
    ui.press("f2");
    assert!(ui.dialog_open());
    ui.press("escape");
    for key in [cmd("backspace"), "delete".to_string()] {
        ui.press(&key);
        assert!(ui.dialog_open(), "{key} asks before trashing");
        ui.press("escape");
    }
    ui.press("space");
    assert!(ui.read(|f| f.quick_look_is_open()));
    ui.press(&cmd("y"));
    assert!(!ui.read(|f| f.quick_look_is_open()));

    ui.press(&cmd("w"));
    let open = ui.cx.update(|cx| cx.windows().len());
    assert_eq!(open, 0, "Cmd-W closes the window");
}

#[gpui_kit::test]
fn type_into_the_search_field_and_down_arrow(cx: &mut TestAppContext) {
    let fx = Fixture::new("search-down");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("search");
    ui.type_text("roadmap");
    ui.press("down");
    assert!(ui.files_focused());
    assert_eq!(ui.selected(), ["Roadmap.md"]);
    // Keys typed in the field edit text instead of moving the selection.
    ui.click("search");
    ui.press("left");
    ui.press("backspace");
    assert_eq!(ui.read(|f| f.query().to_string()), "roadmp");
}

#[gpui_kit::test]
fn results_sent_from_nushell(cx: &mut TestAppContext) {
    let fx = Fixture::new("results");
    let mut ui = Ui::open(cx, &fx.dir);
    let sent = vec![
        fx.path("Roadmap.md"),
        fx.path("Projects/atlas/Project Atlas.txt"),
        fx.path("gone.txt"),
    ];
    let (shown, missing) =
        ui.update(|f, window, cx| f.show_results("Big files".into(), sent, window, cx));
    assert_eq!((shown, missing), (2, vec![fx.path("gone.txt")]));
    assert_eq!(
        ui.names(),
        ["Roadmap.md", "Project Atlas.txt"],
        "the order is kept"
    );
    assert_eq!(ui.read(|f| f.title()), "Big files");
    assert!(ui.read(|f| f.shows_parent_column()));

    ui.select("Project Atlas.txt");
    let report = ui.read(|f| f.selection_json(false));
    assert_eq!(report["location"], "results");
    assert_eq!(report["items"].as_array().unwrap().len(), 1);
    assert_eq!(report["items"][0]["name"], "Project Atlas.txt");
    assert_eq!(report["items"][0]["type"], "file");
    let everything = ui.read(|f| f.selection_json(true));
    assert_eq!(everything["items"].as_array().unwrap().len(), 2);

    ui.press(&cmd("["));
    assert_eq!(
        ui.folder().as_deref(),
        Some(fx.dir.as_path()),
        "Back leaves the results"
    );

    // `fff gui` and a second launch: a folder and a search.
    let projects = fx.path("Projects");
    ui.update(|f, window, cx| f.open_request(Some(&projects), Some("atlas"), window, cx));
    assert_eq!(ui.folder(), Some(projects));
    assert_eq!(ui.read(|f| f.query().to_string()), "atlas");
    let report = ui.read(|f| f.selection_json(true));
    assert_eq!(report["location"], "search");
    assert_eq!(report["query"], "atlas");

    // A file opens its folder with the file selected.
    let roadmap = fx.path("Roadmap.md");
    ui.update(|f, window, cx| f.open_request(Some(&roadmap), None, window, cx));
    assert_eq!(ui.folder().as_deref(), Some(fx.dir.as_path()));
    assert_eq!(ui.selected(), ["Roadmap.md"]);
}

#[gpui_kit::test]
fn typing_jumps_to_matching_names(cx: &mut TestAppContext) {
    let fx = Fixture::new("type-select");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd("2"));
    ui.type_text("r");
    assert_eq!(ui.selected(), ["Roadmap.md"]);
    // Letters typed within a second add to the prefix.
    ui.type_text("o");
    assert_eq!(ui.selected(), ["Roadmap.md"]);

    ui.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    ui.type_text("n");
    assert_eq!(ui.selected(), ["notes.txt"]);
    // No name starts with "np", so the next one after it is chosen.
    ui.type_text("p");
    assert_eq!(ui.selected(), ["Projects"]);

    ui.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    ui.type_text("zz");
    assert_eq!(
        ui.selected(),
        ["Roadmap.md"],
        "past the end picks the last item"
    );

    // Typing in the search field searches instead.
    ui.click("search");
    ui.type_text("b");
    assert_eq!(ui.read(|f| f.query().to_string()), "b");
    ui.press("escape");

    // The column view selects in the active column.
    ui.press(&cmd("3"));
    ui.update(|f, window, cx| f.focus_files(window, cx));
    ui.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    ui.type_text("p");
    assert_eq!(ui.selected(), ["Projects"]);
    assert_eq!(
        ui.read(|f| f.column_names_for_test()).len(),
        2,
        "the folder opens to the right"
    );
}

#[gpui_kit::test]
fn pdfs_preview_page_by_page(cx: &mut TestAppContext) {
    let fx = Fixture::new("pdf-pages");
    let pdf = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/fffinder-core/testdata/pages.pdf"
    );
    std::fs::copy(pdf, fx.path("Manual.pdf")).unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    let shown = |ui: &Ui| ui.read(|f| f.shown_pdf_page());

    ui.click("view-gallery");
    ui.select("Manual.pdf");
    assert_eq!(shown(&ui), Some((1, 2)), "a PDF opens at its first page");
    ui.click("pdf-next");
    assert_eq!(shown(&ui), Some((2, 2)));
    ui.click("pdf-next");
    assert_eq!(shown(&ui), Some((2, 2)), "the last page has no next");

    // Zooming: the buttons, the gallery's keys, and Fit.
    let zoom = |ui: &Ui| {
        ui.read(|f| f.shown_pdf_zoom())
            .expect("the page is laid out")
    };
    let fit = zoom(&ui);
    ui.click("pdf-zoom-in");
    let once = zoom(&ui);
    assert!(once > fit, "Zoom In makes the page bigger");
    ui.press(&cmd("="));
    assert!(zoom(&ui) > once, "Cmd+= zooms in the gallery");
    ui.press(&cmd("0"));
    assert_eq!(zoom(&ui), fit, "Cmd+0 fits the page again");

    // The zoom stays as the pages change.
    ui.press(&cmd("="));
    ui.press("pageup");
    assert_eq!(shown(&ui), Some((1, 2)), "Page Up in the gallery");
    let zoomed = zoom(&ui);
    ui.click("pdf-zoom-fit");
    assert!(zoom(&ui) < zoomed, "the zoom button fits the page");
    ui.press("pagedown");
    assert_eq!(zoom(&ui), fit);

    ui.press("space");
    assert!(ui.read(|f| f.quick_look_is_open()));
    ui.in_quick_look(|ui| ui.press("pageup"));
    assert_eq!(shown(&ui), Some((1, 2)), "Page Up in Quick Look");
    ui.in_quick_look(|ui| ui.press("pageup"));
    assert_eq!(shown(&ui), Some((1, 2)));
    ui.in_quick_look(|ui| ui.press("pagedown"));
    assert_eq!(shown(&ui), Some((2, 2)), "Page Down in Quick Look");
    let in_quick_look = ui.in_quick_look(|ui| zoom(ui));
    ui.in_quick_look(|ui| ui.press(&cmd("=")));
    assert!(
        ui.in_quick_look(|ui| zoom(ui)) > in_quick_look,
        "Cmd+= zooms in Quick Look"
    );
}
