//! The sidebar's folder tree, driven with the mouse and keyboard the way
//! Explorer's navigation pane is, and the Full Disk Access prompt.

mod common;

use common::{Fixture, Ui, cmd, item_id};
use fffinder::actions::{SidebarAsList, SidebarAsTree};
use fffinder::disk_access::{Access, Checked};
use fffinder::finder::{NodeKey, Section};
use fffinder::model::Location;
use fffinder::system::SystemCall;
use fffinder_core::SidebarMode;
use gpui_kit::{Modifiers, MouseButton, TestAppContext, point, px};
use std::time::Duration;

/// A window on the fixture with the fixture folder as a favorite and the
/// tree on, and that favorite's node. fffinder has Full Disk Access, so the
/// tree looks inside folders to tell which open, whatever the test runner
/// may read.
fn tree_window<'a>(cx: &'a mut TestAppContext, fx: &Fixture) -> (Ui<'a>, NodeKey) {
    let mut ui = Ui::open(cx, &fx.dir);
    ui.cx.update(|cx| cx.set_global(Checked(Access::Granted)));
    let dir = fx.dir.clone();
    ui.update(|f, _, cx| {
        f.add_favorite(dir, cx);
        f.set_sidebar_mode(SidebarMode::Tree, cx);
    });
    ui.settle();
    (ui, NodeKey::place(Section::Favorites, fx.dir.clone()))
}

fn selected(ui: &Ui) -> Option<NodeKey> {
    ui.read(|f| f.tree_selection_for_test())
}

/// Where the fixture folder is among Favorites.
fn favorite_ix(ui: &Ui, fx: &Fixture) -> usize {
    ui.read(|f| f.favorites().iter().position(|p| p.path == fx.dir))
        .expect("the fixture folder is a favorite")
}

#[gpui_kit::test]
fn a_favorite_opens_in_place_and_shows_only_folders(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-open");
    let (mut ui, home) = tree_window(cx, &fx);
    let projects = home.under(fx.path("Projects"));
    assert_eq!(
        selected(&ui),
        Some(home.clone()),
        "the folder shown is selected"
    );
    assert!(!ui.exists(projects.row_id()), "a place starts closed");

    ui.click(home.disclosure_id());
    assert!(ui.exists(projects.row_id()));
    assert!(!ui.exists(home.under(fx.path("notes.txt")).row_id()));
    assert_eq!(
        ui.folder(),
        Some(fx.dir.clone()),
        "opening doesn't move the window"
    );

    ui.click(projects.row_id());
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
    assert_eq!(selected(&ui), Some(projects.clone()));

    // atlas holds a file and no folders, so nothing opens it.
    let atlas = projects.under(fx.path("Projects/atlas"));
    ui.click(projects.disclosure_id());
    assert!(ui.exists(atlas.row_id()));
    assert!(!ui.exists(atlas.disclosure_id()));

    ui.click(home.disclosure_id());
    assert!(!ui.exists(projects.row_id()), "closing hides its folders");
    assert_eq!(ui.folder(), Some(fx.path("Projects")));

    // Double-clicking a row opens and closes it too.
    ui.double_click(("fav", favorite_ix(&ui, &fx)));
    assert!(ui.exists(projects.row_id()));
}

#[gpui_kit::test]
fn the_tree_opens_down_to_the_folder_the_window_shows(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-follow");
    std::fs::create_dir_all(fx.path("Projects/atlas/docs")).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let docs = home.under(fx.path("Projects/atlas/docs"));
    ui.go_to(&fx.path("Projects/atlas/docs"));
    assert!(ui.exists(docs.row_id()));
    assert_eq!(selected(&ui), Some(docs));

    ui.press(&cmd("["));
    assert_eq!(ui.folder(), Some(fx.dir.clone()));
    assert_eq!(
        selected(&ui),
        Some(home.clone()),
        "Back goes back up the tree"
    );

    // Without following, a folder is selected only where it already shows.
    ui.update(|f, _, cx| f.set_expand_to_open_folder(false, cx));
    ui.click(home.disclosure_id());
    ui.go_to(&fx.path("Projects/atlas"));
    assert!(!ui.exists(home.under(fx.path("Projects")).row_id()));
    assert_eq!(selected(&ui), None);
}

#[gpui_kit::test]
fn arrows_move_through_the_tree_as_in_explorer(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-keys");
    std::fs::create_dir_all(fx.path("Archive")).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let (archive, projects) = (
        home.under(fx.path("Archive")),
        home.under(fx.path("Projects")),
    );
    let atlas = projects.under(fx.path("Projects/atlas"));
    ui.click(("fav", favorite_ix(&ui, &fx)));
    assert!(tree_focused(&mut ui), "a click gives the tree the keyboard");

    ui.press("right");
    assert!(ui.exists(archive.row_id()), "Right opens the place");
    ui.press("right");
    assert_eq!(selected(&ui), Some(archive.clone()), "then goes into it");
    assert_eq!(ui.folder(), Some(fx.path("Archive")));
    ui.press("down");
    assert_eq!(selected(&ui), Some(projects.clone()));
    ui.press("right");
    ui.press("right");
    assert_eq!(selected(&ui), Some(atlas.clone()));
    ui.press("left");
    assert_eq!(
        selected(&ui),
        Some(projects.clone()),
        "Left goes up from a closed folder"
    );
    ui.press("left");
    assert!(!ui.exists(atlas.row_id()), "and closes an open one");
    ui.press("up");
    assert_eq!(selected(&ui), Some(archive));
    ui.type_text("p");
    assert_eq!(selected(&ui), Some(projects), "typing goes to a name");
    ui.press("home");
    assert_eq!(ui.read(|f| f.location().clone()), Location::Recents);
    ui.press("end");
    assert_eq!(ui.read(|f| f.location().clone()), Location::Trash);

    // With no line active, Home still goes to the first line.
    ui.update(|f, window, cx| {
        let found = Location::Results {
            title: "Found".into(),
            paths: Default::default(),
        };
        f.navigate(found, window, cx)
    });
    ui.press("home");
    assert_eq!(ui.read(|f| f.location().clone()), Location::Recents);

    ui.click(("fav", favorite_ix(&ui, &fx)));
    ui.press("enter");
    assert!(ui.files_focused(), "Return takes the keyboard to the items");
}

/// Whether the browser's folder tree has the keyboard.
fn tree_focused(ui: &mut Ui) -> bool {
    ui.update(|f, window, _| f.tree_focused_for_test(window))
}

#[gpui_kit::test]
fn hidden_folders_show_with_hidden_files_or_on_the_way_down(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-hidden");
    std::fs::create_dir_all(fx.path(".cache/inner")).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let cache = home.under(fx.path(".cache"));
    ui.click(home.disclosure_id());
    assert!(!ui.exists(cache.row_id()));

    ui.update(|f, _, cx| f.set_show_hidden(true, cx));
    ui.settle();
    assert!(ui.exists(cache.row_id()));
    ui.update(|f, _, cx| f.set_show_hidden(false, cx));
    ui.settle();
    assert!(!ui.exists(cache.row_id()));

    ui.go_to(&fx.path(".cache/inner"));
    assert!(
        ui.exists(cache.row_id()),
        "a hidden folder shows on the way down"
    );
}

#[gpui_kit::test]
fn items_dropped_on_a_folder_in_the_tree_move_into_it(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-drop");
    std::fs::create_dir_all(fx.path("Archive")).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let (archive, projects) = (
        home.under(fx.path("Archive")),
        home.under(fx.path("Projects")),
    );
    ui.click("view-list");
    ui.click(home.disclosure_id());

    ui.drag(
        item_id("name-cell", &fx.path("notes.txt")),
        projects.row_id(),
        Modifiers::none(),
    );
    assert!(fx.path("Projects/notes.txt").exists());
    assert!(!fx.path("notes.txt").exists());

    // A folder in the tree drags like any item, and dropped on a folder it
    // goes in there, not into Favorites as well.
    let favorites = ui.read(|f| f.favorites().len());
    ui.drag(archive.row_id(), projects.row_id(), Modifiers::none());
    assert!(fx.path("Projects/Archive").is_dir());
    assert!(!ui.exists(archive.row_id()), "the tree shows the move");
    assert_eq!(ui.read(|f| f.favorites().len()), favorites);
}

#[gpui_kit::test]
fn holding_a_drag_over_a_closed_folder_opens_it_in_the_tree(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-spring");
    let (mut ui, home) = tree_window(cx, &fx);
    let projects = home.under(fx.path("Projects"));
    let atlas = projects.under(fx.path("Projects/atlas"));
    ui.click("view-list");
    ui.click(home.disclosure_id());
    let file = ui
        .snapshot(item_id("name-cell", &fx.path("notes.txt")))
        .bounds();
    let folder = ui.snapshot(projects.row_id()).bounds();

    ui.drag_and_hold(file.center(), folder.center());
    ui.settle();
    assert!(ui.exists(atlas.row_id()), "the node opened");
    assert_eq!(ui.folder(), Some(fx.dir.clone()), "the window stayed put");

    let into = ui.snapshot(atlas.row_id()).bounds().center();
    ui.move_while_held(into, Modifiers::none());
    ui.release(into);
    assert!(fx.path("Projects/atlas/notes.txt").exists());
}

#[gpui_kit::test]
fn new_folder_and_rename_happen_in_the_tree(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-name");
    let (mut ui, home) = tree_window(cx, &fx);
    let projects = home.under(fx.path("Projects"));
    ui.click(home.disclosure_id());

    ui.right_click(projects.row_id());
    assert!(
        ui.menu_labels()
            .contains(&"Paste Into “Projects”".to_string())
    );
    ui.click_menu_item("New Folder");
    assert!(fx.path("Projects/untitled folder").is_dir());
    assert!(ui.exists("rename-input"), "it's named in its row");
    ui.type_text("Gamma");
    ui.press("enter");
    assert!(fx.path("Projects/Gamma").is_dir());
    let gamma = projects.under(fx.path("Projects/Gamma"));
    assert!(ui.exists(gamma.row_id()));
    assert!(tree_focused(&mut ui), "the keyboard is back in the tree");

    // Renaming the folder the window shows takes the window along, and the
    // tree keeps what was open inside it.
    std::fs::create_dir(fx.path("Projects/Gamma/inner")).unwrap();
    ui.click(gamma.row_id());
    assert_eq!(ui.folder(), Some(fx.path("Projects/Gamma")));
    ui.click(gamma.disclosure_id());
    assert!(ui.exists(gamma.under(fx.path("Projects/Gamma/inner")).row_id()));
    ui.click(gamma.row_id());
    ui.press("f2");
    ui.type_text("Delta");
    ui.press("enter");
    assert_eq!(ui.folder(), Some(fx.path("Projects/Delta")));
    let delta = projects.under(fx.path("Projects/Delta"));
    assert_eq!(selected(&ui), Some(delta.clone()));
    assert!(ui.exists(delta.row_id()));
    assert!(!ui.exists(gamma.row_id()));
    assert!(ui.exists(delta.under(fx.path("Projects/Delta/inner")).row_id()));
    ui.press(&cmd("["));
    assert_eq!(
        ui.folder(),
        Some(fx.dir.clone()),
        "Back goes where the window was before, not to the old name"
    );
}

#[gpui_kit::test]
fn moving_the_folder_shown_takes_the_window_along(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-move-shown");
    std::fs::create_dir_all(fx.path("Archive/inner")).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let (archive, projects) = (
        home.under(fx.path("Archive")),
        home.under(fx.path("Projects")),
    );
    ui.click(home.disclosure_id());
    ui.click(archive.row_id());
    ui.click(archive.disclosure_id());
    assert_eq!(ui.folder(), Some(fx.path("Archive")));

    ui.drag(archive.row_id(), projects.row_id(), Modifiers::none());
    assert!(fx.path("Projects/Archive/inner").is_dir());
    assert_eq!(ui.folder(), Some(fx.path("Projects/Archive")));
    let moved = home.under(fx.path("Projects/Archive"));
    assert_eq!(selected(&ui), Some(moved.clone()));
    assert!(ui.exists(moved.row_id()), "the tree opens down to it");
    assert!(
        ui.exists(moved.under(fx.path("Projects/Archive/inner")).row_id()),
        "and keeps it open"
    );

    // Undo moves it back, and the window with it.
    ui.press(&cmd("z"));
    assert!(fx.path("Archive/inner").is_dir());
    assert_eq!(ui.folder(), Some(fx.path("Archive")));
    assert_eq!(selected(&ui), Some(archive.clone()));
    assert!(ui.exists(archive.row_id()));
}

#[gpui_kit::test]
fn a_tree_folder_let_go_off_a_folder_stays_out_of_favorites(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-drop-off");
    std::fs::create_dir_all(fx.path("Archive")).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let archive = home.under(fx.path("Archive"));
    ui.click(home.disclosure_id());
    let favorites = ui.read(|f| f.favorites().len());

    ui.drag(archive.row_id(), "recents", Modifiers::none());
    assert_eq!(ui.read(|f| f.favorites().len()), favorites);
    assert!(fx.path("Archive").is_dir(), "nothing moved");

    // A folder from the items, let go there, is added as in Finder.
    ui.click("view-list");
    ui.drag(
        item_id("name-cell", &fx.path("Archive")),
        "recents",
        Modifiers::none(),
    );
    assert_eq!(ui.read(|f| f.favorites().len()), favorites + 1);
}

#[gpui_kit::test]
fn trashing_the_folder_shown_from_the_tree_goes_to_its_parent(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-trash");
    let (mut ui, home) = tree_window(cx, &fx);
    let atlas = home.under(fx.path("Projects/atlas"));
    ui.go_to(&fx.path("Projects/atlas"));
    assert!(ui.exists(atlas.row_id()));

    ui.right_click(atlas.row_id());
    ui.click_menu_item("Move to Trash");
    assert!(!fx.path("Projects/atlas").exists());
    assert_eq!(ui.folder(), Some(fx.path("Projects")));
    assert!(!ui.exists(atlas.row_id()));

    // A place stays put: the tree's Delete leaves favorites and disks alone,
    // and so does Copy, so a paste can't copy a whole disk.
    ui.click(home.under(fx.path("Projects")).row_id());
    ui.press(&cmd("c"));
    let copied = ui.clipboard();
    assert_eq!(copied, fx.path("Projects").display().to_string());
    ui.click(("fav", favorite_ix(&ui, &fx)));
    ui.press("delete");
    assert!(fx.dir.exists());
    ui.press(&cmd("c"));
    assert_eq!(ui.clipboard(), copied);
}

#[gpui_kit::test]
fn folders_made_elsewhere_show_up_in_the_tree(cx: &mut TestAppContext) {
    // In an open folder the window isn't showing, so only the tree's own
    // look for changes can find it.
    let fx = Fixture::new("tree-live");
    let (mut ui, home) = tree_window(cx, &fx);
    let projects = home.under(fx.path("Projects"));
    ui.click(home.disclosure_id());
    ui.click(projects.disclosure_id());
    let later = projects.under(fx.path("Projects/Later"));
    assert!(!ui.exists(later.row_id()));

    std::fs::create_dir(fx.path("Projects/Later")).unwrap();
    ui.cx.executor().advance_clock(Duration::from_secs(3));
    ui.settle();
    assert!(ui.exists(later.row_id()));

    std::fs::remove_dir(fx.path("Projects/Later")).unwrap();
    ui.cx.executor().advance_clock(Duration::from_secs(3));
    ui.settle();
    assert!(!ui.exists(later.row_id()));

    // atlas holds no folders, so it can't open, until one is made in it.
    let atlas = projects.under(fx.path("Projects/atlas"));
    assert!(ui.exists(atlas.row_id()));
    assert!(!ui.exists(atlas.disclosure_id()));
    std::fs::create_dir(fx.path("Projects/atlas/maps")).unwrap();
    ui.cx.executor().advance_clock(Duration::from_secs(3));
    ui.settle();
    assert!(ui.exists(atlas.disclosure_id()), "now it opens");
}

#[cfg(unix)]
#[gpui_kit::test]
fn a_folder_that_cant_be_read_shows_a_lock(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;
    let fx = Fixture::new("tree-locked");
    let locked = fx.path("Locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (mut ui, home) = tree_window(cx, &fx);
    let row = home.under(locked.clone());
    ui.click(home.disclosure_id());
    let (shown, lock, opens) = (
        ui.exists(row.row_id()),
        ui.exists("unreadable"),
        ui.exists(row.disclosure_id()),
    );
    // Readable again before anything can fail, so the fixture can go.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(shown);
    assert!(lock, "a lock says it can't be read");
    assert!(!opens, "and it has no triangle to open it");
}

#[gpui_kit::test]
fn the_sidebar_switches_between_list_and_tree(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-mode");
    let (mut ui, home) = tree_window(cx, &fx);
    assert!(ui.exists(home.disclosure_id()));
    ui.dispatch(SidebarAsList);
    assert_eq!(ui.read(|f| f.config().sidebar_mode), SidebarMode::List);
    assert!(!ui.exists(home.disclosure_id()));
    // The list works as Finder's sidebar does.
    ui.click(("fav", favorite_ix(&ui, &fx)));
    assert_eq!(ui.folder(), Some(fx.dir.clone()));
    assert!(!tree_focused(&mut ui));
    ui.dispatch(SidebarAsTree);
    assert_eq!(ui.read(|f| f.config().sidebar_mode), SidebarMode::Tree);
    assert!(ui.exists(home.disclosure_id()));

    // Settings has the same choice.
    ui.press(&cmd(","));
    ui.scroll("use-nu-env", point(px(0.), px(-2000.)));
    ui.click("sidebar-list");
    assert_eq!(ui.read(|f| f.config().sidebar_mode), SidebarMode::List);
    ui.click("sidebar-tree");
    assert_eq!(ui.read(|f| f.config().sidebar_mode), SidebarMode::Tree);
}

#[gpui_kit::test]
fn a_middle_click_opens_a_tree_folder_in_a_new_tab(cx: &mut TestAppContext) {
    let fx = Fixture::new("tree-tab");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        common::Open {
            workspace: true,
            ..Default::default()
        },
    );
    let dir = fx.dir.clone();
    ui.update(|f, _, cx| {
        f.add_favorite(dir, cx);
        f.set_sidebar_mode(SidebarMode::Tree, cx);
    });
    ui.settle();
    let home = NodeKey::place(Section::Favorites, fx.dir.clone());
    ui.click(home.disclosure_id());
    ui.click_with(
        MouseButton::Middle,
        home.under(fx.path("Projects")).row_id(),
    );
    let tabs = ui
        .workspace
        .clone()
        .expect("opened with tabs")
        .read_with(ui.cx, |w, _| w.tabs().to_vec());
    assert_eq!(tabs.len(), 2);
    let last = tabs[1].read_with(ui.cx, |f, _| f.current_folder().map(|p| p.to_path_buf()));
    assert_eq!(last, Some(fx.path("Projects")));
    assert_eq!(ui.folder(), Some(fx.dir.clone()), "this tab stays put");
}

#[gpui_kit::test]
fn missing_full_disk_access_opens_system_settings(cx: &mut TestAppContext) {
    let fx = Fixture::new("disk-access");
    let mut ui = Ui::open(cx, &fx.dir);
    let opened = SystemCall::OpenUrl(fffinder::disk_access::SETTINGS_URL.into());
    ui.update(|f, window, cx| f.ask_for_full_disk_access(window, cx));
    ui.settle();
    assert!(ui.dialog_open());
    assert!(
        ui.system_calls().is_empty(),
        "System Settings doesn't come up in front of the dialog by itself"
    );

    ui.click("disk-access-open");
    assert_eq!(ui.system_calls(), std::slice::from_ref(&opened));
    assert!(ui.dialog_open(), "opening Settings leaves the dialog up");
    // The click that brings the window back from System Settings lands
    // outside the dialog, and mustn't close it.
    ui.click_point(point(px(10.), px(300.)));
    assert!(ui.dialog_open());

    ui.click("disk-access-never");
    assert!(!ui.dialog_open());
    assert!(!ui.read(|f| f.config().ask_for_full_disk_access));
}

#[gpui_kit::test]
fn indexing_waits_for_full_disk_access(cx: &mut TestAppContext) {
    let fx = Fixture::new("disk-access-held");
    let mut ui = Ui::open(cx, &fx.dir);
    let roots = |ui: &mut Ui| {
        ui.cx
            .read(|cx| cx.global::<fffinder::finder::AppState>().engine.roots())
    };
    let held = |ui: &mut Ui| ui.cx.read(fffinder::disk_access::index_held);
    // As startup leaves things when access is off and asking is on.
    ui.cx.update(|cx| {
        cx.global::<fffinder::finder::AppState>()
            .engine
            .set_roots(&[]);
        cx.set_global(fffinder::disk_access::IndexHeld(true));
    });
    ui.update(|f, _, cx| f.set_search_roots(vec![fx.dir.clone()], cx));
    assert!(roots(&mut ui).is_empty(), "new folders to index wait too");
    assert!(ui.exists("index-status"));

    ui.click("index-status");
    assert!(ui.dialog_open(), "the sidebar says why, and asks again");
    assert!(ui.system_calls().is_empty());
    ui.click("disk-access-index");
    assert!(!ui.dialog_open());
    assert!(!held(&mut ui));
    assert_eq!(roots(&mut ui).len(), 1, "indexing starts");

    // Turning asking off starts it as well.
    ui.cx.update(|cx| {
        cx.global::<fffinder::finder::AppState>()
            .engine
            .set_roots(&[]);
        cx.set_global(fffinder::disk_access::IndexHeld(true));
    });
    ui.update(|f, _, cx| f.set_ask_for_full_disk_access(false, cx));
    assert!(!held(&mut ui));
    assert_eq!(roots(&mut ui).len(), 1);
}
