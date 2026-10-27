//! Every control in the window, driven with real clicks and key presses, with
//! an observable effect checked for each. A step that hangs fails the run
//! (see `common::HANG_LIMIT`).

mod common;

use common::{Fixture, Open, Ui, at, cmd, item_id};
use fffinder::actions::*;
use fffinder::finder::{PickerOptions, SearchScope};
use fffinder::model::Location;
use fffinder::system::SystemCall;
use fffinder_core::terminals::{Launch, Terminal};
use fffinder_core::{Appearance, KindFilter, SortKey, ViewMode};
use gpui_kit::Action as _;
use gpui_kit::TestAppContext;
use gpui_kit::component::ActiveTheme as _;
use serde_json::json;
use std::path::PathBuf;

// ----- toolbar -------------------------------------------------------------------

#[gpui_kit::test]
fn view_buttons_switch_views(cx: &mut TestAppContext) {
    let fx = Fixture::new("view-buttons");
    let mut ui = Ui::open(cx, &fx.dir);
    for (id, mode) in [
        ("view-list", ViewMode::List),
        ("view-columns", ViewMode::Columns),
        ("view-gallery", ViewMode::Gallery),
        ("view-icons", ViewMode::Icons),
    ] {
        ui.click(id);
        assert_eq!(ui.read(|f| f.effective_view()), mode, "after clicking {id}");
    }
}

#[gpui_kit::test]
fn back_and_forward_buttons(cx: &mut TestAppContext) {
    let fx = Fixture::new("back-forward");
    let mut ui = Ui::open(cx, &fx.dir);
    // Buttons don't report a disabled state to tests; check that clicking
    // them does nothing instead.
    ui.click("back");
    ui.click("forward");
    assert_eq!(ui.folder().as_deref(), Some(fx.dir.as_path()));

    ui.double_click(item_id("tile", &fx.path("Projects")));
    assert_eq!(ui.names(), ["atlas"]);
    assert!(ui.read(|f| f.can_go_back()));

    ui.click("back");
    assert_eq!(ui.folder().as_deref(), Some(fx.dir.as_path()));
    assert_eq!(
        ui.selected(),
        ["Projects"],
        "back selects the folder we left"
    );
    ui.click("forward");
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
}

#[gpui_kit::test]
fn sidebar_hide_and_show_buttons(cx: &mut TestAppContext) {
    let fx = Fixture::new("sidebar-toggle");
    let mut ui = Ui::open(cx, &fx.dir);
    assert!(!ui.exists("show-sidebar"));
    ui.click("hide-sidebar");
    assert!(!ui.read(|f| f.config().show_sidebar));
    assert!(!ui.exists("hide-sidebar"));
    ui.click("show-sidebar");
    assert!(ui.read(|f| f.config().show_sidebar));
}

#[gpui_kit::test]
fn sort_menu_items(cx: &mut TestAppContext) {
    let fx = Fixture::new("sort-menu");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("sort");
    assert_eq!(
        ui.menu_labels(),
        [
            "Sort By",
            "Name",
            "Date Modified",
            "Size",
            "Kind",
            "Ascending",
            "Descending",
            "Show Hidden Files"
        ]
    );
    ui.click_menu_item("Size");
    assert!(!ui.menu_open(), "choosing an item closes the menu");
    assert_eq!(ui.read(|f| f.current_sort().key), SortKey::Size);

    ui.click("sort");
    ui.click_menu_item("Ascending");
    assert!(ui.read(|f| f.current_sort().ascending));
    ui.click("sort");
    ui.click_menu_item("Descending");
    assert!(!ui.read(|f| f.current_sort().ascending));

    assert!(!ui.names().contains(&".secret".to_string()));
    ui.click("sort");
    ui.click_menu_item("Show Hidden Files");
    assert!(ui.read(|f| f.config().show_hidden));
    assert!(ui.names().contains(&".secret".to_string()));
}

#[gpui_kit::test]
fn nushell_menu_items(cx: &mut TestAppContext) {
    let fx = Fixture::new("nushell-menu");
    let mut ui = Ui::open(cx, &fx.dir);
    // Disabled without a selection: clicking it copies nothing.
    ui.click("nushell");
    ui.click_menu_item("Copy as Nushell List");
    assert_eq!(ui.clipboard(), "");
    ui.press("escape");

    ui.click("nushell");
    ui.click_menu_item("Run a Pipeline…");
    assert!(ui.read(|f| f.nu_panel_open()));
    ui.click("nushell");
    ui.click_menu_item("Hide Pipeline Panel");
    assert!(!ui.read(|f| f.nu_panel_open()));

    ui.select("Roadmap.md");
    ui.click("nushell");
    ui.click_menu_item("Copy as Nushell List");
    assert!(ui.clipboard().contains("Roadmap.md"), "{}", ui.clipboard());

    // Without nu this reports a problem instead of opening a terminal.
    ui.click("nushell");
    ui.click_menu_item("Open Nushell Here");
    assert!(!ui.menu_open());
}

#[gpui_kit::test]
fn actions_menu_items(cx: &mut TestAppContext) {
    let fx = Fixture::new("actions-menu");
    let mut ui = Ui::open(cx, &fx.dir);
    // These need a selection, so they're disabled and do nothing yet.
    for needs_selection in ["Quick Look", "Rename…", "Move to Trash"] {
        ui.click("actions");
        ui.click_menu_item(needs_selection);
        assert!(!ui.dialog_open(), "{needs_selection} without a selection");
        ui.press("escape");
    }

    ui.click("actions");
    ui.click_menu_item("Add Folder to Sidebar");
    assert!(ui.read(|f| f.favorites().iter().any(|p| p.path == fx.dir)));

    ui.select("Roadmap.md");
    ui.click("actions");
    ui.click_menu_item("Copy Path");
    assert_eq!(ui.clipboard(), fx.path("Roadmap.md").display().to_string());

    ui.click("actions");
    ui.click_menu_item(fffinder::finder::reveal_label());
    assert_eq!(
        ui.system_calls(),
        [SystemCall::Reveal(fx.path("Roadmap.md"))]
    );

    for (item, close) in [
        ("Quick Look", "dialog-close"),
        ("Rename…", "rename-cancel"),
        ("Move to Trash", "cancel"),
        ("Go to Folder…", "goto-cancel"),
        ("Search Syntax…", "help-done"),
        ("Settings…", "settings-done"),
    ] {
        ui.click("actions");
        ui.click_menu_item(item);
        assert!(ui.dialog_open(), "{item} opens a dialog");
        if close == "dialog-close" {
            ui.close_dialog();
        } else {
            ui.click(close);
        }
        assert!(!ui.dialog_open(), "{close} closes the {item} dialog");
        assert!(
            ui.files_focused(),
            "focus returns to the files after {item}"
        );
    }
    assert!(fx.path("Roadmap.md").exists(), "cancel kept the file");

    ui.click("actions");
    ui.click_menu_item("New Folder");
    assert!(fx.path("untitled folder").is_dir());
    assert!(ui.dialog_open(), "a new folder opens Rename");
    ui.click("rename-cancel");
    assert!(ui.names().contains(&"untitled folder".to_string()));
}

#[gpui_kit::test]
fn every_menu_bar_action_has_a_handler(cx: &mut TestAppContext) {
    let fx = Fixture::new("menu-bar");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("Roadmap.md");
    let actions: Vec<Box<dyn gpui_kit::Action>> = vec![
        ShowSettings.boxed_clone(),
        Rescan.boxed_clone(),
        NewFolder.boxed_clone(),
        OpenSelected.boxed_clone(),
        QuickLook.boxed_clone(),
        Rename.boxed_clone(),
        MoveToTrash.boxed_clone(),
        CloseWindow.boxed_clone(),
        CopyPath.boxed_clone(),
        CopyAsNuon.boxed_clone(),
        SelectAll.boxed_clone(),
        FocusSearch.boxed_clone(),
        SearchHelp.boxed_clone(),
        ViewIcons.boxed_clone(),
        ViewList.boxed_clone(),
        ViewColumns.boxed_clone(),
        ViewGallery.boxed_clone(),
        ToggleSidebar.boxed_clone(),
        ToggleHidden.boxed_clone(),
        ToggleAppearance.boxed_clone(),
        Refresh.boxed_clone(),
        GoBack.boxed_clone(),
        GoForward.boxed_clone(),
        GoUp.boxed_clone(),
        GoRecents.boxed_clone(),
        GoHome.boxed_clone(),
        GoToFolder.boxed_clone(),
        ToggleNuPanel.boxed_clone(),
        OpenInNushell.boxed_clone(),
        ToggleFilter.boxed_clone(),
        OpenInEditor.boxed_clone(),
        MoveItemsHere.boxed_clone(),
        PutBack.boxed_clone(),
        DeleteImmediately.boxed_clone(),
        EmptyTrash.boxed_clone(),
        GoTrash.boxed_clone(),
        GetInfo.boxed_clone(),
        gpui_kit::component::input::Undo.boxed_clone(),
    ];
    // Menus send actions to whatever has focus: the files, or a text field.
    for focus in ["files", "search"] {
        if focus == "search" {
            ui.click("search");
        }
        let missing: Vec<&str> = ui.window(|window, cx| {
            actions
                .iter()
                .filter(|a| !window.is_action_available(a.as_ref(), cx))
                .map(|a| a.name())
                .collect()
        });
        assert!(
            missing.is_empty(),
            "with focus in {focus}, no handler for {missing:?}"
        );
    }
}

// ----- search ----------------------------------------------------------------------

#[gpui_kit::test]
fn search_field_typing_escape_and_down_arrow(cx: &mut TestAppContext) {
    let fx = Fixture::new("search-field");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("search");
    ui.type_text("atlas");
    assert_eq!(ui.read(|f| f.query().to_string()), "atlas");
    assert!(
        ui.names().contains(&"Project Atlas.txt".to_string()),
        "{:?}",
        ui.names()
    );
    assert!(ui.exists("scope-computer"), "the scope bar appears");

    ui.press("down");
    assert!(ui.files_focused(), "Down moves into the results");
    assert_eq!(ui.selected().len(), 1);

    ui.click("search");
    ui.press("escape");
    assert_eq!(ui.read(|f| f.query().to_string()), "");
    assert!(!ui.exists("scope-computer"));
    assert_eq!(ui.names().len(), 4);
}

#[gpui_kit::test]
fn search_scope_button(cx: &mut TestAppContext) {
    let fx = Fixture::new("scope-button");
    let mut ui = Ui::open(cx, &fx.dir);
    let scope = |ui: &Ui| ui.read(|f| f.search_scope());
    ui.go_to(&fx.path("Projects"));
    ui.click("search-scope");
    assert_eq!(scope(&ui), SearchScope::Folder, "set before searching");
    ui.update(|f, _, cx| f.set_query("atlas".into(), true, cx));
    let names = ui.names();
    assert!(names.iter().any(|n| n == "Project Atlas.txt"), "{names:?}");
    ui.update(|f, _, cx| f.set_query("budget".into(), true, cx));
    assert!(ui.names().is_empty(), "Budget.csv is outside Projects");
    assert!(ui.exists("scope-folder"));

    ui.click("search-scope");
    assert_eq!(scope(&ui), SearchScope::Computer);
    assert_eq!(ui.names(), ["Budget.csv"], "the search runs again");
    ui.click("scope-folder");
    ui.click("search-scope");
    assert_eq!(
        scope(&ui),
        SearchScope::Computer,
        "it follows the scope bar"
    );

    // Recents isn't a folder, so the button does nothing there.
    ui.update(|f, window, cx| f.navigate(Location::Recents, window, cx));
    ui.click("search-scope");
    assert_eq!(scope(&ui), SearchScope::Computer);
}

#[gpui_kit::test]
fn scope_bar_controls(cx: &mut TestAppContext) {
    let fx = Fixture::new("scope-bar");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.update(|f, _, cx| f.set_query("atlas".into(), true, cx));

    ui.click("scope-folder");
    assert_eq!(ui.read(|f| f.search_scope()), SearchScope::Folder);
    ui.click("scope-computer");
    assert_eq!(ui.read(|f| f.search_scope()), SearchScope::Computer);

    ui.click("by-contents");
    assert!(ui.read(|f| f.searches_contents()));
    ui.click("kind");
    assert!(!ui.menu_open(), "Kind is off for a contents search");
    ui.click("by-name");
    assert!(!ui.read(|f| f.searches_contents()));

    ui.click("kind");
    let labels: Vec<String> = KindFilter::ALL
        .iter()
        .map(|k| k.label().to_string())
        .collect();
    assert_eq!(ui.menu_labels(), labels);
    ui.click_menu_item("Folder");
    assert_eq!(ui.read(|f| f.config().kind_filter), KindFilter::Folder);
    assert_eq!(ui.names(), ["atlas"], "only folders match");
    ui.click("kind");
    ui.click_menu_item("Any Kind");
    assert_eq!(ui.read(|f| f.config().kind_filter), KindFilter::Any);

    ui.click("search-help");
    assert!(ui.dialog_open());
    ui.click("help-done");
    assert!(!ui.dialog_open());
}

// ----- sidebar, path bar, status bar ---------------------------------------------

#[gpui_kit::test]
fn sidebar_rows_and_menus(cx: &mut TestAppContext) {
    let fx = Fixture::new("sidebar");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("recents");
    assert_eq!(ui.read(|f| f.location().clone()), Location::Recents);

    let projects = fx.path("Projects");
    ui.update(|f, _, cx| f.add_favorite(projects.clone(), cx));
    let ix = ui.read(|f| {
        f.favorites()
            .iter()
            .position(|p| p.path == projects)
            .unwrap()
    });
    ui.click(("fav", ix));
    assert_eq!(ui.folder(), Some(projects.clone()));

    if ui.exists(("loc", 0usize)) {
        ui.click(("loc", 0usize));
        assert!(ui.folder().is_some(), "a location opens a folder");
    }

    ui.right_click(("fav", ix));
    assert!(ui.menu_open());
    assert!(ui.menu_labels().contains(&"Open Nushell Here".to_string()));
    ui.click_menu_item("Remove from Sidebar");
    assert!(!ui.read(|f| f.favorites().iter().any(|p| p.path == projects)));

    ui.click("index-settings");
    assert!(ui.dialog_open(), "the gear opens Settings");
}

#[gpui_kit::test]
fn path_bar_crumbs_navigate(cx: &mut TestAppContext) {
    let fx = Fixture::new("crumbs");
    let mut ui = Ui::open(cx, &fx.dir);
    let atlas = fx.path("Projects/atlas");
    ui.go_to(&atlas);
    let crumbs = fffinder_core::places::breadcrumbs(&atlas).len();
    ui.click(("crumb", crumbs - 2));
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
}

#[gpui_kit::test]
fn status_bar_slider_resizes_icons(cx: &mut TestAppContext) {
    let fx = Fixture::new("slider");
    let mut ui = Ui::open(cx, &fx.dir);
    let before = ui.read(|f| f.icon_size_for_test());
    let bar = ui.snapshot("slider-bar-container").bounds();
    ui.click_at(
        "slider-bar-container",
        at(
            f32::from(bar.size.width) - 1.,
            f32::from(bar.size.height) / 2.,
        ),
    );
    let after = ui.read(|f| f.icon_size_for_test());
    assert!(after > before, "icon size {before} -> {after}");
    ui.click("view-list");
    assert!(
        !ui.exists("slider-bar-container"),
        "the slider is only for icons"
    );
}

#[gpui_kit::test]
fn status_bar_has_no_fps_by_default(cx: &mut TestAppContext) {
    let fx = Fixture::new("fps-off");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    ui.settle();
    assert!(!ui.exists("fps"));
}

#[gpui_kit::test]
fn status_bar_shows_fps_when_asked(cx: &mut TestAppContext) {
    let fx = Fixture::new("fps-on");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            fps: true,
            ..Default::default()
        },
    );
    ui.cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    ui.settle();
    assert!(ui.exists("fps"), "shown once the first second has passed");
}

// ----- views -----------------------------------------------------------------------

#[gpui_kit::test]
fn list_headers_sort(cx: &mut TestAppContext) {
    let fx = Fixture::new("headers");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    for (header, key) in [
        ("Date Modified", SortKey::Modified),
        ("Size", SortKey::Size),
        ("Kind", SortKey::Kind),
        ("Name", SortKey::Name),
    ] {
        ui.click(header);
        assert_eq!(ui.read(|f| f.current_sort().key), key, "{header}");
    }
    let ascending = ui.read(|f| f.current_sort().ascending);
    ui.click("Name");
    assert_ne!(
        ui.read(|f| f.current_sort().ascending),
        ascending,
        "a second click flips"
    );
}

#[gpui_kit::test]
fn items_select_and_open_in_every_view(cx: &mut TestAppContext) {
    let fx = Fixture::new("items");
    let mut ui = Ui::open(cx, &fx.dir);
    let roadmap = fx.path("Roadmap.md");

    ui.click(item_id("tile", &roadmap));
    assert_eq!(ui.selected(), ["Roadmap.md"], "icons");

    ui.click("view-list");
    ui.click(item_id("row", &fx.path("notes.txt")));
    assert_eq!(ui.selected(), ["notes.txt"], "list");

    ui.click("view-gallery");
    ui.click(item_id("strip", &roadmap));
    assert_eq!(ui.selected(), ["Roadmap.md"], "gallery");
    assert!(
        ui.read(|f| f.has_preview_for_test(&roadmap)),
        "gallery loads the preview"
    );

    ui.click("view-columns");
    ui.click(item_id("crow", &fx.path("Projects")));
    assert_eq!(
        ui.read(|f| f.column_names_for_test()),
        vec![
            vec!["Budget.csv", "notes.txt", "Projects", "Roadmap.md"],
            vec!["atlas"]
        ]
    );
    ui.click(item_id("crow", &fx.path("Projects/atlas")));
    assert_eq!(ui.read(|f| f.column_names_for_test()).len(), 3);

    ui.click("view-list");
    ui.double_click(item_id("row", &fx.path("Projects")));
    assert_eq!(
        ui.folder(),
        Some(fx.path("Projects")),
        "double-click opens a folder"
    );
}

#[gpui_kit::test]
fn gallery_previews_an_item_with_nothing_selected(cx: &mut TestAppContext) {
    let fx = Fixture::new("gallery");
    let mut ui = Ui::open(cx, &fx.dir);
    assert!(ui.selected().is_empty());
    ui.click("view-gallery");
    assert_eq!(
        ui.selected(),
        ["Budget.csv"],
        "the gallery selects the first item"
    );
    assert!(
        ui.read(|f| f.has_preview_for_test(&fx.path("Budget.csv"))),
        "the preview finished loading instead of spinning"
    );
    // Escape clears the selection; the shown item still gets its preview.
    ui.press("escape");
    assert!(ui.read(|f| f.has_preview_for_test(&fx.path("Budget.csv"))));
}

#[gpui_kit::test]
fn context_menus_on_items_and_empty_space(cx: &mut TestAppContext) {
    let fx = Fixture::new("context-menus");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.right_click(item_id("row", &fx.path("notes.txt")));
    assert_eq!(ui.selected(), ["notes.txt"]);
    assert!(
        ui.menu_labels()
            .contains(&"Copy as Nushell List".to_string())
    );
    ui.click_menu_item("Copy Path");
    assert_eq!(ui.clipboard(), fx.path("notes.txt").display().to_string());

    ui.right_click(item_id("row", &fx.path("notes.txt")));
    ui.click_menu_item("Run Nushell Pipeline…");
    assert!(ui.read(|f| f.nu_panel_open()));
    ui.dispatch(ToggleNuPanel);

    // Empty space below the rows.
    let content = ui.snapshot("content").bounds();
    let below = at(40., f32::from(content.size.height) - 30.);
    ui.right_click_at("content", below);
    assert!(ui.menu_open(), "the background menu opens");
    assert!(!ui.menu_labels().contains(&"Copy Path".to_string()));
    ui.click_menu_item("Show Hidden Files");
    assert!(ui.read(|f| f.config().show_hidden));

    ui.right_click_at("content", below);
    ui.click_menu_item("Refresh");
    ui.right_click_at("content", below);
    ui.click_menu_item("New Folder");
    assert!(fx.path("untitled folder").is_dir());
    ui.click("rename-cancel");
}

// ----- dialogs ---------------------------------------------------------------------

#[gpui_kit::test]
fn settings_dialog_controls(cx: &mut TestAppContext) {
    let fx = Fixture::new("settings");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd(","));
    assert!(ui.dialog_open());

    // The switch used to dispatch an action the dialog's focus could not
    // deliver, so clicking it did nothing.
    assert_eq!(ui.snapshot("show-hidden").checked(), Some(false));
    ui.click("show-hidden");
    assert!(ui.read(|f| f.config().show_hidden));
    assert_eq!(ui.snapshot("show-hidden").checked(), Some(true));
    assert!(ui.names().contains(&".secret".to_string()));
    ui.click("show-hidden");
    assert!(!ui.read(|f| f.config().show_hidden));

    for (id, appearance, dark) in [
        ("ap-dark", Appearance::Dark, true),
        ("ap-light", Appearance::Light, false),
        ("ap-system", Appearance::System, false),
    ] {
        ui.click(id);
        assert_eq!(ui.read(|f| f.config().appearance), appearance, "{id}");
        if appearance != Appearance::System {
            assert_eq!(ui.cx.update(|cx| cx.theme().is_dark()), dark, "{id}");
        }
    }

    ui.click("rescan");
    assert!(ui.dialog_open(), "Rescan keeps Settings open");

    // Add Folder… asks for folders with the system picker.
    ui.click("add-root");
    assert!(ui.cx.did_prompt_for_paths());
    let projects = fx.path("Projects");
    ui.cx
        .simulate_path_prompt_response(|_| Some(vec![projects.clone()]));
    ui.settle();
    assert_eq!(
        ui.read(|f| f.config().search_roots.clone()),
        vec![fx.dir.clone(), projects.clone()]
    );
    ui.click(("remove-root", 1usize));
    assert_eq!(
        ui.read(|f| f.config().search_roots.clone()),
        vec![fx.dir.clone()]
    );

    // These would index the whole disk or the home folder; check the
    // setting, then put the fixture back before the engine starts on it.
    let disk = if cfg!(windows) { "C:\\" } else { "/" };
    ui.click_without_settling("add-disk");
    assert_eq!(
        ui.read(|f| f.config().search_roots.clone()),
        vec![std::path::PathBuf::from(disk)]
    );
    let root = fx.dir.clone();
    ui.update(|f, _, cx| f.set_search_roots(vec![root.clone()], cx));
    ui.click_without_settling("reset-roots");
    assert_eq!(
        ui.read(|f| f.config().search_roots.clone()),
        fffinder_core::config::default_search_roots()
    );
    ui.update(|f, _, cx| f.set_search_roots(vec![root.clone()], cx));

    ui.click("settings-done");
    assert!(!ui.dialog_open());
    assert!(ui.files_focused(), "Done returns focus to the files");

    ui.press(&cmd(","));
    ui.press("escape");
    assert!(!ui.dialog_open(), "Escape closes Settings");
    ui.press(&cmd(","));
    ui.close_dialog();
    assert!(!ui.dialog_open(), "the close button closes Settings");
}

#[gpui_kit::test]
fn settings_text_sizes(cx: &mut TestAppContext) {
    let fx = Fixture::new("text-sizes");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let row = item_id("row", &fx.path("notes.txt"));
    let row_height = |ui: &mut Ui| ui.snapshot(row.clone()).bounds().size.height;
    let font = |ui: &mut Ui| ui.window(|_, cx| cx.theme().font_size);
    let (height, size) = (row_height(&mut ui), font(&mut ui));
    ui.press(&cmd(","));

    // Text size scales the window's own layout and the kit's controls.
    assert_eq!(ui.snapshot("ui-text-reset").label(), Some("100%"));
    ui.click("ui-text-bigger");
    ui.click("ui-text-bigger");
    assert_eq!(ui.read(|f| f.config().ui_scale), 1.2);
    assert_eq!(ui.snapshot("ui-text-reset").label(), Some("120%"));
    assert_eq!(font(&mut ui), size * 1.2);
    // Switching light and dark reloads the theme; the scale applies once.
    ui.close_dialog();
    ui.press(&cmd("shift-d"));
    ui.press(&cmd("shift-d"));
    assert_eq!(font(&mut ui), size * 1.2, "not scaled twice");
    ui.press(&cmd(","));
    ui.click("ui-text-smaller");
    assert_eq!(ui.read(|f| f.config().ui_scale), 1.1);
    ui.close_dialog();
    // Layout snaps to half pixels.
    let grown = f32::from(row_height(&mut ui)) / f32::from(height);
    assert!((grown - 1.1).abs() < 0.03, "rows grow with it: {grown}");

    // Nushell text size has its own row; both reset from their labels.
    ui.press(&cmd(","));
    ui.click("nu-text-bigger");
    assert_eq!(ui.read(|f| f.config().nu_font_size), 13.);
    assert_eq!(ui.snapshot("nu-text-reset").label(), Some("13 pt"));
    ui.click("nu-text-reset");
    ui.click("ui-text-reset");
    assert_eq!(ui.read(|f| f.config().nu_font_size), 12.);
    assert_eq!(ui.read(|f| f.config().ui_scale), 1.0);
    assert_eq!(font(&mut ui), size, "the theme's font size comes back");
    ui.close_dialog();
    assert_eq!(row_height(&mut ui), height);
}

#[gpui_kit::test]
fn settings_search_engine(cx: &mut TestAppContext) {
    use fffinder::finder::AppState;
    use fffinder_core::SearchCase;
    let fx = Fixture::new("search-engine");
    let mut ui = Ui::open(cx, &fx.dir);
    let engine = ui.window(|_, cx| cx.global::<AppState>().engine.clone());
    let search = |ui: &Ui| ui.read(|f| f.config().search.clone());
    ui.press(&cmd(","));

    // A switch that shapes the index rebuilds it; searching still works.
    ui.click("follow-symlinks");
    assert!(search(&ui).follow_symlinks);
    assert!(engine.wait_ready(std::time::Duration::from_secs(20)));
    ui.settle();
    assert!(
        engine.search_settings().follow_symlinks,
        "the engine has it"
    );
    ui.click("git-recency");
    assert!(!search(&ui).git_recency);

    ui.click("case-sensitive");
    assert_eq!(search(&ui).case, SearchCase::Sensitive);
    ui.click("time-limit-less");
    assert_eq!(search(&ui).time_limit_secs, 2);
    assert_eq!(ui.snapshot("time-limit-value").label(), Some("2 s"));
    ui.click("threads-more");
    assert_eq!(search(&ui).threads, 1);
    ui.click("max-file-size-more");
    assert_eq!(search(&ui).max_file_size_mb, 20);
    ui.click("result-limit-more");
    assert_eq!(ui.read(|f| f.config().result_limit), 1000);
    assert_eq!(engine.search_settings(), search(&ui));
    ui.close_dialog();

    assert!(engine.wait_ready(std::time::Duration::from_secs(20)));
    ui.click("search");
    ui.type_text("notes");
    assert!(
        ui.names().contains(&"notes.txt".to_string()),
        "{:?}",
        ui.names()
    );
}

#[gpui_kit::test]
fn settings_terminal_choice(cx: &mut TestAppContext) {
    let fx = Fixture::new("terminal-choice");
    let ghostty = Terminal {
        id: "ghostty".into(),
        name: "Ghostty".into(),
        location: Some("/Applications/Ghostty.app".into()),
        launch: Launch::Ghostty {
            bundle: "/Applications/Ghostty.app".into(),
        },
    };
    let other = Terminal {
        id: "terminal".into(),
        name: "Terminal".into(),
        location: Some("/usr/bin/term".into()),
        launch: Launch::Exec {
            program: "/usr/bin/term".into(),
            args: vec!["--cwd".into(), "{dir}".into(), "{nu}".into()],
        },
    };
    let nu = PathBuf::from("/opt/nu/bin/nu");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            nu: Some(nu.clone()),
            terminals: vec![ghostty, other],
            ..Default::default()
        },
    );
    let dir = fx.dir.display().to_string();
    let last_launch = |ui: &Ui| match ui.system_calls().last() {
        Some(SystemCall::Terminal { program, args }) => {
            (program.display().to_string(), args.clone())
        }
        other => panic!("no terminal was started: {other:?}"),
    };
    let label = |ui: &mut Ui| ui.snapshot("terminal-app").label().map(str::to_string);

    ui.press(&cmd(","));
    assert_eq!(label(&mut ui).as_deref(), Some("Automatic (Ghostty)"));
    ui.click("terminal-app");
    assert_eq!(
        ui.menu_labels(),
        [
            "Automatic (Ghostty)",
            "Ghostty",
            "Terminal",
            "Custom Command",
            "Detect Again"
        ]
    );
    ui.click_menu_item("Terminal");
    assert_eq!(
        ui.read(|f| f.terminal_setting().app).as_deref(),
        Some("terminal")
    );
    assert_eq!(label(&mut ui).as_deref(), Some("Terminal"));
    ui.click("try-terminal");
    assert_eq!(
        last_launch(&ui),
        (
            "/usr/bin/term".into(),
            vec!["--cwd".into(), dir.clone(), nu.display().to_string()]
        )
    );

    // A custom command shows its field; words are split like a shell would.
    assert!(!ui.exists("terminal-input"));
    ui.click("terminal-app");
    ui.click_menu_item("Custom Command");
    ui.click("terminal-input");
    ui.type_text("foot -D {dir} {nu}");
    assert_eq!(
        ui.read(|f| f.terminal_setting().command).as_deref(),
        Some("foot -D {dir} {nu}")
    );
    ui.click("try-terminal");
    assert_eq!(
        last_launch(&ui),
        (
            "foot".into(),
            vec!["-D".into(), dir.clone(), nu.display().to_string()]
        )
    );

    // Automatic picks Ghostty, which goes through AppleScript; the custom
    // command is kept for later.
    ui.click("terminal-app");
    ui.click_menu_item("Automatic (Ghostty)");
    assert!(!ui.exists("terminal-input"));
    assert_eq!(ui.read(|f| f.terminal_setting().app), None);
    assert!(ui.read(|f| f.terminal_setting().command).is_some());
    ui.click("try-terminal");
    assert_eq!(last_launch(&ui).0, "/usr/bin/osascript");

    // A terminal that isn't installed any more falls back to Automatic.
    ui.update(|f, _, cx| f.set_terminal_app(Some("kitty".into()), cx));
    assert_eq!(label(&mut ui).as_deref(), Some("kitty (not installed)"));
    ui.click("try-terminal");
    assert_eq!(last_launch(&ui).0, "/usr/bin/osascript");

    ui.click("terminal-app");
    ui.click_menu_item("Detect Again");
    assert_eq!(ui.read(|f| f.terminals().map(<[Terminal]>::len)), Some(2));

    // The menu items and the shortcut use the same choice.
    ui.click("settings-done");
    ui.update(|f, _, cx| f.set_terminal_app(Some("terminal".into()), cx));
    ui.press(&cmd("alt-t"));
    assert_eq!(last_launch(&ui).1[1], dir);
    ui.click("nushell");
    ui.click_menu_item("Open Nushell Here");
    assert_eq!(last_launch(&ui).1[1], dir);
    let calls = ui.system_calls().len();
    assert_eq!(calls, 6, "one launch per request: {:?}", ui.system_calls());
}

#[gpui_kit::test]
fn go_to_folder_dialog(cx: &mut TestAppContext) {
    let fx = Fixture::new("goto");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.press(&cmd("shift-g"));
    assert!(ui.dialog_open());
    // The field starts selected, so typing replaces it; Return goes.
    ui.type_text("Projects");
    ui.press("enter");
    assert!(!ui.dialog_open());
    assert_eq!(ui.folder(), Some(fx.path("Projects")));

    ui.press(&cmd("shift-g"));
    ui.type_text("does-not-exist");
    ui.click("goto-go");
    assert!(ui.dialog_open(), "a missing folder keeps the dialog open");
    ui.click("goto-cancel");
    assert!(!ui.dialog_open());

    ui.press(&cmd("shift-g"));
    ui.type_text(&fx.path("Roadmap.md").display().to_string());
    ui.click("goto-go");
    assert_eq!(ui.folder().as_deref(), Some(fx.dir.as_path()));
    assert_eq!(
        ui.selected(),
        ["Roadmap.md"],
        "a file opens its folder and selects it"
    );
}

#[gpui_kit::test]
fn rename_dialog(cx: &mut TestAppContext) {
    let fx = Fixture::new("rename");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("Roadmap.md");
    ui.press("f2");
    assert!(ui.dialog_open());
    // Only the name before the extension is selected.
    ui.type_text("Plan");
    ui.click("rename-ok");
    assert!(!ui.dialog_open());
    assert!(fx.path("Plan.md").exists() && !fx.path("Roadmap.md").exists());
    assert!(ui.names().contains(&"Plan.md".to_string()));

    ui.select("Plan.md");
    ui.press("f2");
    ui.type_text("Later");
    ui.click("rename-cancel");
    assert!(fx.path("Plan.md").exists(), "Cancel leaves the name alone");

    ui.select("Plan.md");
    ui.press("f2");
    ui.press(&cmd("a"));
    ui.type_text("notes.txt");
    ui.press("enter");
    assert!(
        ui.dialog_open(),
        "renaming onto an existing name is refused"
    );
    ui.press("escape");
    assert!(fx.path("Plan.md").exists());
}

#[gpui_kit::test]
fn trash_confirmation_cancel(cx: &mut TestAppContext) {
    // Confirming is covered in file_ops.rs, with a stand-in for the Trash.
    let fx = Fixture::new("trash");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("notes.txt");
    ui.press(&cmd("backspace"));
    assert!(ui.dialog_open());
    ui.click("cancel");
    assert!(!ui.dialog_open());
    ui.press(&cmd("backspace"));
    ui.press("escape");
    assert!(!ui.dialog_open());
    assert!(fx.path("notes.txt").exists());
}

#[gpui_kit::test]
fn quick_look_buttons_and_keys(cx: &mut TestAppContext) {
    let fx = Fixture::new("quick-look");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("Roadmap.md");
    ui.press("space");
    assert!(ui.read(|f| f.quick_look_is_open()) && ui.dialog_open());
    assert!(ui.read(|f| f.has_preview_for_test(&fx.path("Roadmap.md"))));
    ui.press("space");
    assert!(!ui.dialog_open(), "Space closes it again");
    assert!(!ui.read(|f| f.quick_look_is_open()));

    ui.press("space");
    ui.click("ql-reveal");
    assert!(ui.dialog_open(), "Show in Finder keeps the preview open");
    ui.click("ql-open");
    assert!(!ui.dialog_open() && !ui.read(|f| f.quick_look_is_open()));
    let roadmap = fx.path("Roadmap.md");
    assert_eq!(
        ui.system_calls(),
        [
            SystemCall::Reveal(roadmap.clone()),
            SystemCall::Open(roadmap)
        ]
    );
    ui.press("space");
    assert!(ui.dialog_open(), "one Space reopens it after Open");
    ui.press("space");

    ui.press(&cmd("y"));
    ui.close_dialog();
    assert!(
        !ui.read(|f| f.quick_look_is_open()),
        "the close button resets the state"
    );
    ui.press("space");
    assert!(ui.dialog_open(), "and Space opens it again");
}

#[gpui_kit::test]
fn files_no_app_claims_open_in_an_editor(cx: &mut TestAppContext) {
    use fffinder::nu_env::NuEnv;
    let fx = Fixture::new("no-app");
    std::fs::write(fx.path("blob.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    let (roadmap, notes, blob) = (
        fx.path("Roadmap.md"),
        fx.path("notes.txt"),
        fx.path("blob.bin"),
    );
    ui.set_no_app(&roadmap);
    ui.set_no_app(&blob);

    // Quick Look's Open hands an unclaimed text file to the text editor.
    ui.select("Roadmap.md");
    ui.press("space");
    ui.click("ql-open");
    assert!(!ui.dialog_open());
    assert_eq!(ui.system_calls(), [SystemCall::EditText(roadmap.clone())]);

    // A file with an app opens in it; a binary no app claims doesn't open.
    ui.select("notes.txt");
    ui.press("enter");
    ui.select("blob.bin");
    ui.press("enter");
    ui.dispatch(OpenInEditor);
    assert_eq!(
        ui.system_calls(),
        [
            SystemCall::EditText(roadmap.clone()),
            SystemCall::Open(notes)
        ]
    );

    // "Open in Editor" without Nushell's editor does the same as Open.
    ui.select("Roadmap.md");
    ui.dispatch(OpenInEditor);
    assert_eq!(
        ui.system_calls().last(),
        Some(&SystemCall::EditText(roadmap.clone()))
    );

    // With Nushell's editor on, Return opens the file in that editor.
    let env = NuEnv {
        buffer_editor: Some(json!(["code", "--wait"])),
        ..Default::default()
    };
    ui.update(|f, _, cx| f.set_nu_env_for_test(env, cx));
    ui.press("enter");
    assert_eq!(
        ui.system_calls().last(),
        Some(&SystemCall::Terminal {
            program: "code".into(),
            args: vec!["--wait".into(), roadmap.display().to_string()],
        })
    );
}

#[gpui_kit::test]
fn quick_look_text_size(cx: &mut TestAppContext) {
    let fx = Fixture::new("quick-look-text");
    let mut ui = Ui::open(cx, &fx.dir);
    let size = |ui: &Ui| ui.read(|f| f.config().preview_font_size);
    ui.select("Roadmap.md");
    ui.press("space");
    assert_eq!(size(&ui), 11.5);
    assert_eq!(ui.snapshot("ql-text-reset").label(), Some("11.5 pt"));

    ui.click("ql-text-bigger");
    assert_eq!(size(&ui), 12.5);
    assert_eq!(ui.snapshot("ql-text-reset").label(), Some("12.5 pt"));
    ui.press(&cmd("="));
    assert_eq!(size(&ui), 13.5);
    ui.press(&cmd("-"));
    ui.click("ql-text-smaller");
    assert_eq!(size(&ui), 11.5);
    ui.press(&cmd("="));
    ui.click("ql-text-reset");
    assert_eq!(size(&ui), 11.5, "the size label resets it");
    ui.press(&cmd("="));
    ui.press(&cmd("0"));
    assert_eq!(size(&ui), 11.5, "and so does Cmd-0");

    ui.update(|f, _, cx| f.set_preview_font_size(Some(31.5), cx));
    ui.press(&cmd("="));
    ui.press(&cmd("="));
    assert_eq!(size(&ui), 32., "it stops at the largest size");

    assert!(ui.dialog_open());
    ui.press("space");
    assert!(!ui.dialog_open(), "Space still closes it after the clicks");

    ui.select("Projects");
    ui.press("space");
    assert!(ui.dialog_open());
    assert!(!ui.exists("ql-text-bigger"), "a folder has no text to size");
    ui.press("space");
    assert_eq!(size(&ui), 32., "the size stays for the next preview");
}

#[gpui_kit::test]
fn quick_look_arrows_move_through_items(cx: &mut TestAppContext) {
    let fx = Fixture::new("quick-look-arrows");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let names = ui.names();
    let shown = |ui: &Ui| ui.read(|f| f.primary_entry().map(|e| e.name));
    ui.select(&names[0]);
    ui.press("space");
    ui.press("down");
    assert!(ui.dialog_open(), "the arrows keep Quick Look open");
    assert_eq!(ui.selected(), [names[1].as_str()]);
    assert!(ui.read(|f| f.has_preview_for_test(&fx.path(&names[1]))));
    ui.press("right");
    assert_eq!(
        ui.selected(),
        [names[2].as_str()],
        "Right is next in the list"
    );
    ui.press("left");
    ui.press("up");
    assert_eq!(ui.selected(), [names[0].as_str()]);
    ui.press("space");
    assert!(!ui.dialog_open(), "Space still closes it");

    // With several items selected, the arrows step through just those.
    ui.select(&names[0]);
    ui.select_also(&names[2]);
    ui.press("space");
    assert_eq!(shown(&ui).as_deref(), Some(names[2].as_str()));
    ui.press("right");
    assert_eq!(
        shown(&ui).as_deref(),
        Some(names[0].as_str()),
        "wraps around"
    );
    ui.press("up");
    assert_eq!(shown(&ui).as_deref(), Some(names[2].as_str()));
    assert_eq!(ui.selected(), [names[0].as_str(), names[2].as_str()]);
    assert!(ui.read(|f| f.has_preview_for_test(&fx.path(&names[0]))));
    ui.press("space");

    // In columns, the arrows stay in the active column, even after opening
    // a folder, which moves the focus to the files.
    ui.click("view-columns");
    ui.click(item_id("crow", &fx.path("notes.txt")));
    ui.press("space");
    ui.press("down");
    assert_eq!(ui.selected(), ["Projects"]);
    assert!(ui.read(|f| f.has_preview_for_test(&fx.path("Projects"))));
    ui.press("right");
    assert_eq!(ui.selected(), ["Roadmap.md"]);
    ui.press("left");
    assert_eq!(shown(&ui).as_deref(), Some("Projects"));
    assert!(ui.dialog_open());
}

// ----- Nushell panel -----------------------------------------------------------------

#[gpui_kit::test]
fn nushell_panel_controls(cx: &mut TestAppContext) {
    let fx = Fixture::new("nu-panel");
    let nu = fffinder::nushell::find_nu(None);
    let have_nu = nu.is_some();
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            nu,
            ..Default::default()
        },
    );
    ui.press("ctrl-`");
    assert!(ui.read(|f| f.nu_panel_open()));

    // An example fills the field and runs it.
    ui.click(("nu-example", 0usize));
    assert!(!ui.read(|f| f.nu_busy()));
    assert_eq!(
        ui.exists("nu-error"),
        !have_nu,
        "an error shows only without nu"
    );

    ui.click("nu-input");
    ui.press(&cmd("a"));
    ui.type_text("length");
    ui.press("enter");
    assert!(!ui.read(|f| f.nu_busy()));
    assert_eq!(ui.exists("nu-error"), !have_nu);
    if have_nu {
        assert!(ui.exists("nu-text"), "`length` prints a number");
    }

    ui.click("nu-run");
    assert!(!ui.read(|f| f.nu_busy()));
    assert!(
        !ui.exists(("nu-example", 0usize)),
        "a result replaces the examples"
    );

    // Clear empties the field and brings the examples back.
    ui.click("nu-clear");
    assert!(ui.exists(("nu-example", 0usize)));
    assert_eq!(ui.update(|f, _, cx| f.nu_pipeline(cx)), "");
    ui.click(("nu-example", 1usize));
    assert!(!ui.exists(("nu-example", 0usize)));
    assert!(!ui.update(|f, _, cx| f.nu_pipeline(cx)).is_empty());

    ui.click("nu-close");
    assert!(!ui.read(|f| f.nu_panel_open()));
    assert!(ui.files_focused());
}

// ----- picker ------------------------------------------------------------------------

#[gpui_kit::test]
fn picker_bar_enables_choose_with_a_selection(cx: &mut TestAppContext) {
    // Choose and Cancel end the process, so only their state is checked.
    let fx = Fixture::new("picker");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            picker: Some(PickerOptions { multiple: true }),
            ..Default::default()
        },
    );
    assert!(ui.exists("picker-cancel"));
    // With nothing selected Choose does nothing (with a selection it would
    // print the paths and exit).
    ui.click("picker-choose");
    ui.select("Roadmap.md");
    assert_eq!(ui.snapshot("picker-choose").label(), Some("Choose"));
    ui.select_also("notes.txt");
    assert_eq!(ui.snapshot("picker-choose").label(), Some("Choose 2 Items"));
}

#[gpui_kit::test]
fn nushell_filter_and_smart_folders(cx: &mut TestAppContext) {
    let Some(nu) = fffinder::nushell::find_nu(None) else {
        eprintln!("nu not installed; skipping");
        return;
    };
    let fx = Fixture::new("filter");
    std::fs::write(fx.path("big.log"), "x".repeat(20_000)).unwrap();
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            nu: Some(nu),
            ..Default::default()
        },
    );
    ui.press(&cmd("alt-f"));
    assert!(ui.exists("filter-input"), "Cmd-Alt-F opens the filter bar");
    ui.type_text("size > 10kb");
    ui.press("enter");
    assert_eq!(ui.names(), ["big.log"]);

    // A mistake shows Nushell's message and leaves the items alone.
    ui.press(&cmd("a"));
    ui.type_text("sise > 10kb");
    ui.press("enter");
    assert!(ui.exists("filter-error"));
    assert_eq!(ui.names().len(), 5);
    ui.press(&cmd("a"));
    ui.type_text("| sort-by size --reverse | first 2");
    ui.press("enter");
    assert!(!ui.exists("filter-error"));
    assert_eq!(ui.names().len(), 2, "a pipeline works too");
    ui.press(&cmd("a"));
    ui.type_text("size > 10kb");
    ui.press("enter");

    // Searches are filtered too, and can be saved.
    ui.click("search");
    ui.type_text("log");
    assert_eq!(ui.names(), ["big.log"]);
    ui.click("save-smart-folder");
    assert!(ui.dialog_open());
    ui.type_text("Big logs");
    ui.click("smart-save");
    assert!(!ui.dialog_open());
    let saved = ui.read(|f| f.smart_folders().to_vec());
    assert_eq!(saved.len(), 1);
    assert_eq!(
        (
            saved[0].name.as_str(),
            saved[0].query.as_str(),
            saved[0].filter.as_deref()
        ),
        ("Big logs", "log", Some("size > 10kb"))
    );

    ui.click("close-filter");
    assert!(!ui.exists("filter-input"));
    assert_eq!(ui.read(|f| f.filter_text().to_string()), "");
    ui.click("search");
    ui.press("escape");
    assert!(!ui.read(|f| f.is_searching()));

    // The sidebar runs it again: search, filter, and all.
    ui.click(("smart", 0usize));
    assert_eq!(ui.read(|f| f.query().to_string()), "log");
    assert_eq!(ui.read(|f| f.filter_text().to_string()), "size > 10kb");
    assert_eq!(ui.names(), ["big.log"]);
    ui.right_click(("smart", 0usize));
    ui.click_menu_item("Remove from Sidebar");
    assert!(ui.read(|f| f.smart_folders().is_empty()));
}

#[gpui_kit::test]
fn batch_rename_with_a_closure(cx: &mut TestAppContext) {
    use fffinder::rename::Outcome;
    let Some(nu) = fffinder::nushell::find_nu(None) else {
        eprintln!("nu not installed; skipping");
        return;
    };
    let fx = Fixture::new("batch-rename");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            nu: Some(nu),
            ..Default::default()
        },
    );
    ui.click("view-list");
    ui.select("Budget.csv");
    ui.select_also("notes.txt");
    ui.right_click(item_id("row", &fx.path("notes.txt")));
    assert!(ui.menu_labels().contains(&"Rename 2 Items…".to_string()));
    ui.press("escape");

    ui.press("f2");
    assert!(ui.read(|f| f.batch_rename_open()));
    let outcomes = |ui: &Ui| -> Vec<(String, Outcome)> {
        match ui.read(|f| f.batch_preview_for_test()) {
            Some(Ok(rows)) => rows.into_iter().map(|(_, new, o)| (new, o)).collect(),
            other => panic!("no preview: {other:?}"),
        }
    };
    // It starts as "keep every name": nothing to rename, so Rename does nothing.
    assert!(outcomes(&ui).iter().all(|(_, o)| *o == Outcome::Unchanged));
    ui.click("batch-apply");
    assert!(ui.dialog_open());

    ui.click(("batch-example", 0usize));
    assert_eq!(
        outcomes(&ui),
        [
            ("budget.csv".to_string(), Outcome::Rename),
            ("notes.txt".to_string(), Outcome::Unchanged)
        ]
    );
    assert!(ui.exists("batch-summary"));

    ui.click("batch-closure");
    ui.press(&cmd("a"));
    ui.type_text("{|f| 'same.txt' }");
    ui.press("enter");
    assert!(ui.dialog_open(), "Return previews instead of renaming");
    assert!(
        outcomes(&ui)
            .iter()
            .all(|(_, o)| matches!(o, Outcome::Problem(_)))
    );
    ui.click("batch-apply");
    assert!(fx.path("Budget.csv").exists(), "problems block the rename");

    ui.press(&cmd("a"));
    ui.type_text("{|f| $f.nope }");
    ui.press("enter");
    assert!(ui.exists("batch-error"));

    ui.press(&cmd("a"));
    ui.type_text("{|f| $\"($f.index)-($f.name)\" }");
    ui.press("enter");
    ui.click("batch-apply");
    assert!(!ui.dialog_open());
    assert!(fx.path("1-Budget.csv").exists() && fx.path("2-notes.txt").exists());
    assert!(!fx.path("Budget.csv").exists());
    assert!(ui.names().contains(&"1-Budget.csv".to_string()));

    ui.select("1-Budget.csv");
    ui.select_also("2-notes.txt");
    ui.press("f2");
    ui.click("batch-cancel");
    assert!(
        !ui.read(|f| f.batch_rename_open()),
        "Cancel forgets the batch"
    );
    assert!(fx.path("1-Budget.csv").exists());
}

#[gpui_kit::test]
fn nushell_actions_from_the_context_menu(cx: &mut TestAppContext) {
    let Some(nu) = fffinder::nushell::find_nu(None) else {
        eprintln!("nu not installed; skipping");
        return;
    };
    let fx = Fixture::new("user-actions");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            nu: Some(nu),
            ..Default::default()
        },
    );
    let labels: Vec<String> =
        ui.read(|f| f.user_actions().iter().map(|a| a.label.clone()).collect());
    assert!(
        labels.contains(&"Count Lines".to_string()),
        "starters load: {labels:?}"
    );
    assert!(fffinder::user_actions::actions_path().exists());

    ui.click("view-list");
    ui.right_click(item_id("row", &fx.path("notes.txt")));
    ui.click_menu_item("Nushell Actions");
    // The items are drawn at the Nushell text size, so they're found by id.
    let count_lines = labels.iter().position(|l| l == "Count Lines").unwrap();
    let item = ui.snapshot(("nu-action", count_lines));
    assert_eq!(item.label(), Some("Count Lines"));
    assert!(ui.exists("nu-actions-edit"));
    let height = item.bounds().size.height;
    ui.click(("nu-action", count_lines));
    assert!(ui.read(|f| f.nu_panel_open()));
    assert!(!ui.read(|f| f.nu_busy()));
    assert!(!ui.exists("nu-error"), "the action ran cleanly");

    // The toolbar's Nushell menu has them too, bigger after Settings changes.
    ui.update(|f, _, cx| f.set_nu_font_size(Some(20.), cx));
    ui.click("nushell");
    ui.click_menu_item("Nushell Actions");
    let total = labels.iter().position(|l| l == "Total Size").unwrap();
    assert_eq!(
        ui.snapshot(("nu-action", total)).label(),
        Some("Total Size")
    );
    let bigger = ui.snapshot(("nu-action", total)).bounds().size.height;
    assert!(bigger > height, "{height:?} -> {bigger:?}");
}

#[gpui_kit::test]
fn nushell_colors_and_editor(cx: &mut TestAppContext) {
    use fffinder::nu_env::NuEnv;
    let fx = Fixture::new("nu-env");
    let term = Terminal {
        id: "term".into(),
        name: "Term".into(),
        location: Some("/usr/bin/term".into()),
        launch: Launch::Exec {
            program: "/usr/bin/term".into(),
            args: vec!["--cwd".into(), "{dir}".into(), "-e".into(), "{nu}".into()],
        },
    };
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            terminals: vec![term],
            ..Default::default()
        },
    );
    let notes = fx.path("notes.txt");

    // Off: files open in their default app, names use the theme.
    ui.select("notes.txt");
    ui.dispatch(OpenInEditor);
    assert_eq!(ui.system_calls(), [SystemCall::Open(notes.clone())]);
    assert!(ui.read(|f| f.rows_for_test().iter().all(|r| r.name_style.is_none())));

    // On, with nvim: names colored like `ls`, the editor in the terminal.
    let env = NuEnv {
        ls_colors: Some("di=38;2;10;20;30:*.md=38;2;200;0;0".into()),
        buffer_editor: Some(json!("nvim")),
        ..Default::default()
    };
    ui.update(|f, _, cx| f.set_nu_env_for_test(env, cx));
    let color = |ui: &Ui, name: &str| {
        ui.read(|f| {
            f.rows_for_test()
                .iter()
                .find(|r| r.entry.name == name)
                .and_then(|r| r.name_style)
                .and_then(|s| s.color)
                .map(|c| {
                    let c = gpui_kit::Rgba::from(c);
                    (
                        (c.r * 255.).round() as u8,
                        (c.g * 255.).round() as u8,
                        (c.b * 255.).round() as u8,
                    )
                })
        })
    };
    assert_eq!(color(&ui, "Projects"), Some((10, 20, 30)));
    assert_eq!(color(&ui, "Roadmap.md"), Some((200, 0, 0)));
    assert_eq!(color(&ui, "Budget.csv"), None);

    ui.select("notes.txt");
    ui.dispatch(OpenInEditor);
    let dir = fx.dir.display().to_string();
    assert_eq!(
        ui.system_calls().last(),
        Some(&SystemCall::Terminal {
            program: "/usr/bin/term".into(),
            args: vec![
                "--cwd".into(),
                dir.clone(),
                "-e".into(),
                "nvim".into(),
                notes.display().to_string()
            ],
        })
    );

    // A windowed editor starts directly, with its own arguments.
    let env = NuEnv {
        buffer_editor: Some(json!(["code", "--wait"])),
        ..Default::default()
    };
    ui.update(|f, _, cx| f.set_nu_env_for_test(env, cx));
    ui.right_click(item_id("tile", &notes));
    ui.click_menu_item("Open in Editor");
    assert_eq!(
        ui.system_calls().last(),
        Some(&SystemCall::Terminal {
            program: "code".into(),
            args: vec!["--wait".into(), notes.display().to_string()],
        })
    );

    // The Settings switch turns it off again.
    ui.press(&cmd(","));
    assert!(ui.read(|f| f.config().use_nu_environment));
    ui.click("use-nu-env");
    assert!(!ui.read(|f| f.config().use_nu_environment));
    assert_eq!(color(&ui, "Projects"), None);
}
