//! Copying, moving, and the other things done to files, driven through key
//! presses, menus, and drags in a headless window.

mod common;

use common::{Fixture, Ui, at, cmd, item_id};
use fffinder::actions::*;
use gpui_kit::Action as _;
use gpui_kit::{Modifiers, TestAppContext};

#[gpui_kit::test]
fn copy_and_paste_files(cx: &mut TestAppContext) {
    let fx = Fixture::new("paste");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("notes.txt");
    ui.press(&cmd("c"));
    assert_eq!(
        ui.clipboard(),
        fx.path("notes.txt").display().to_string(),
        "the path goes on the clipboard as text too"
    );

    ui.go_to(&fx.path("Projects"));
    ui.press(&cmd("v"));
    assert_eq!(
        std::fs::read_to_string(fx.path("Projects/notes.txt")).unwrap(),
        "plain notes\n"
    );
    assert!(fx.path("notes.txt").exists(), "a paste copies");
    assert_eq!(ui.selected(), ["notes.txt"], "the pasted item is selected");

    // The name is taken now: keep both, then skip.
    ui.press(&cmd("v"));
    assert!(ui.dialog_open(), "a name clash asks first");
    ui.click("clash-keep");
    assert!(fx.path("Projects/notes 2.txt").exists());
    ui.press(&cmd("v"));
    ui.click("clash-skip");
    assert!(!fx.path("Projects/notes 3.txt").exists());

    // Cmd+Alt+V moves.
    ui.go_to(&fx.dir);
    ui.select("Roadmap.md");
    ui.press(&cmd("c"));
    ui.go_to(&fx.path("Projects"));
    ui.press(&cmd("alt-v"));
    assert!(fx.path("Projects/Roadmap.md").exists());
    assert!(!fx.path("Roadmap.md").exists(), "moving takes the original");

    // A path copied as text, from a terminal or another file manager, pastes the item.
    ui.cx
        .write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            fx.path("Budget.csv").display().to_string(),
        ));
    ui.press(&cmd("v"));
    assert!(fx.path("Projects/Budget.csv").exists());

    // Text that isn't a fffinder copy pastes nothing.
    ui.cx
        .write_to_clipboard(gpui_kit::ClipboardItem::new_string("hello".into()));
    let before = ui.names();
    ui.press(&cmd("v"));
    assert!(!ui.dialog_open());
    assert_eq!(ui.names(), before);
}

#[gpui_kit::test]
fn replacing_on_paste(cx: &mut TestAppContext) {
    let fx = Fixture::new("replace");
    std::fs::write(fx.path("Projects/notes.txt"), "older").unwrap();
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("notes.txt");
    ui.press(&cmd("c"));
    ui.go_to(&fx.path("Projects"));
    ui.press(&cmd("v"));
    ui.click("clash-stop");
    assert_eq!(
        std::fs::read_to_string(fx.path("Projects/notes.txt")).unwrap(),
        "older"
    );
    ui.press(&cmd("v"));
    ui.click("clash-replace");
    assert_eq!(
        std::fs::read_to_string(fx.path("Projects/notes.txt")).unwrap(),
        "plain notes\n"
    );
}

#[gpui_kit::test]
fn copy_and_paste_from_the_context_menus(cx: &mut TestAppContext) {
    let fx = Fixture::new("paste-menus");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.right_click(item_id("row", &fx.path("Budget.csv")));
    ui.click_menu_item("Copy “Budget.csv”");

    // Pasting where the item already is makes a copy beside it.
    let content = ui.snapshot("content").bounds();
    let below = at(40., f32::from(content.size.height) - 30.);
    ui.right_click_at("content", below);
    ui.click_menu_item("Paste Item");
    assert!(fx.path("Budget copy.csv").exists());
    assert_eq!(ui.selected(), ["Budget copy.csv"]);

    ui.right_click_at("content", below);
    assert!(ui.menu_labels().contains(&"Move Item Here".to_string()));
}

#[gpui_kit::test]
fn dragging_onto_a_folder_moves_or_copies(cx: &mut TestAppContext) {
    let fx = Fixture::new("drag");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    let projects = item_id("row", &fx.path("Projects"));
    ui.drag(
        item_id("row", &fx.path("notes.txt")),
        projects.clone(),
        Modifiers::none(),
    );
    assert!(fx.path("Projects/notes.txt").exists());
    assert!(
        !fx.path("notes.txt").exists(),
        "a drag within a volume moves"
    );

    // Option copies on macOS, Ctrl elsewhere.
    let copy = Modifiers {
        alt: cfg!(target_os = "macos"),
        control: !cfg!(target_os = "macos"),
        ..Modifiers::none()
    };
    ui.drag(item_id("row", &fx.path("Budget.csv")), projects, copy);
    assert!(fx.path("Projects/Budget.csv").exists());
    assert!(fx.path("Budget.csv").exists(), "the copy key copies");
}

#[gpui_kit::test]
fn undo_reverses_file_operations(cx: &mut TestAppContext) {
    let fx = Fixture::new("undo");
    let mut ui = Ui::open(cx, &fx.dir);

    ui.select("Roadmap.md");
    ui.press("f2");
    ui.type_text("Plan");
    ui.press("enter");
    assert!(fx.path("Plan.md").exists());
    ui.press(&cmd("z"));
    assert!(fx.path("Roadmap.md").exists() && !fx.path("Plan.md").exists());
    assert_eq!(ui.selected(), ["Roadmap.md"], "the item is selected again");

    ui.press(&cmd("shift-n"));
    ui.click("rename-cancel");
    assert!(fx.path("untitled folder").is_dir());
    ui.press(&cmd("z"));
    assert!(
        !fx.path("untitled folder").exists(),
        "a new folder goes to the Trash"
    );

    ui.select("notes.txt");
    ui.press(&cmd("c"));
    ui.press(&cmd("v"));
    assert!(fx.path("notes copy.txt").exists());
    ui.press(&cmd("z"));
    assert!(
        !fx.path("notes copy.txt").exists(),
        "a copy goes to the Trash"
    );

    ui.select("notes.txt");
    ui.press(&cmd("c"));
    ui.go_to(&fx.path("Projects"));
    ui.press(&cmd("alt-v"));
    assert!(fx.path("Projects/notes.txt").exists());
    ui.press(&cmd("z"));
    assert!(fx.path("notes.txt").exists() && !fx.path("Projects/notes.txt").exists());

    // Undo with nothing left to undo changes nothing.
    ui.press(&cmd("z"));
    ui.press(&cmd("z"));
    assert!(fx.path("notes.txt").exists() && fx.path("Roadmap.md").exists());
}

#[gpui_kit::test]
fn file_operations_wait_for_undo(cx: &mut TestAppContext) {
    let fx = Fixture::new("undo-busy");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("notes.txt");
    ui.press(&cmd("d"));
    assert!(fx.path("notes copy.txt").exists());

    // Duplicate arrives while the Undo is still running, and is refused.
    ui.select("Budget.csv");
    ui.window(|window, cx| {
        window.dispatch_action(gpui_kit::component::input::Undo.boxed_clone(), cx);
        window.dispatch_action(Duplicate.boxed_clone(), cx);
    });
    ui.settle();
    assert!(!fx.path("notes copy.txt").exists(), "the Undo finished");
    assert!(
        !fx.path("Budget copy.csv").exists(),
        "nothing else starts while Undo runs"
    );

    // Once it's done, file operations work again.
    ui.press(&cmd("d"));
    assert!(fx.path("Budget copy.csv").exists());
}

#[gpui_kit::test]
fn redo_reverses_undo(cx: &mut TestAppContext) {
    let fx = Fixture::new("redo");
    let mut ui = Ui::open(cx, &fx.dir);

    ui.select("Roadmap.md");
    ui.press("f2");
    ui.type_text("Plan");
    ui.press("enter");
    ui.select("notes.txt");
    ui.press(&cmd("c"));
    ui.press(&cmd("v"));
    assert!(fx.path("notes copy.txt").exists());

    // Undo both, newest first, then Redo both, oldest first.
    ui.press(&cmd("z"));
    ui.press(&cmd("z"));
    assert!(fx.path("Roadmap.md").exists() && !fx.path("notes copy.txt").exists());
    ui.press(&cmd("shift-z"));
    assert!(fx.path("Plan.md").exists(), "Redo renames it again");
    assert!(!fx.path("notes copy.txt").exists());
    ui.press(&cmd("shift-z"));
    assert!(
        fx.path("notes copy.txt").exists(),
        "a redone copy comes back out of the Trash"
    );

    // Undo after Redo works, and a new operation ends what Redo can do.
    ui.press(&cmd("z"));
    assert!(!fx.path("notes copy.txt").exists());
    ui.select("Budget.csv");
    ui.press(&cmd("d"));
    assert!(fx.path("Budget copy.csv").exists());
    ui.press(&cmd("shift-z"));
    assert!(
        !fx.path("notes copy.txt").exists(),
        "nothing to redo after a new operation"
    );
    ui.press(&cmd("z"));
    assert!(!fx.path("Budget copy.csv").exists());
}

#[gpui_kit::test]
fn trash_puts_back_and_empties(cx: &mut TestAppContext) {
    let fx = Fixture::new("trash-view");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");
    ui.select("notes.txt");
    ui.press(&cmd("backspace"));
    ui.click("ok");
    assert!(!fx.path("notes.txt").exists());

    ui.click("trash");
    assert_eq!(ui.names(), ["notes.txt"]);
    ui.select("notes.txt");
    ui.right_click(item_id(
        "row",
        &common::trash_dir(&fx.dir).join("notes.txt"),
    ));
    ui.click_menu_item("Put Back");
    assert!(fx.path("notes.txt").exists(), "Put Back restores it");
    assert!(ui.names().is_empty());

    // Trash by dragging onto the sidebar, then Cmd+Z puts it back.
    ui.go_to(&fx.dir);
    ui.drag(
        item_id("row", &fx.path("Budget.csv")),
        "trash",
        Modifiers::none(),
    );
    assert!(!fx.path("Budget.csv").exists());
    ui.press(&cmd("z"));
    assert!(fx.path("Budget.csv").exists());

    ui.select("Roadmap.md");
    ui.press(&cmd("backspace"));
    ui.click("ok");
    ui.click("trash");
    ui.click("empty-trash");
    ui.click("ok");
    assert!(ui.names().is_empty());
    assert!(
        !common::trash_dir(&fx.dir).join("Roadmap.md").exists(),
        "emptied for good"
    );
}

#[gpui_kit::test]
fn delete_immediately_asks_first(cx: &mut TestAppContext) {
    let fx = Fixture::new("delete-now");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.select("notes.txt");
    ui.press(&cmd("alt-backspace"));
    assert!(ui.dialog_open());
    ui.click("cancel");
    assert!(fx.path("notes.txt").exists());
    ui.press(&cmd("alt-backspace"));
    ui.click("ok");
    assert!(!fx.path("notes.txt").exists());
    assert!(
        !common::trash_dir(&fx.dir).join("notes.txt").exists(),
        "not in the Trash"
    );
}

#[gpui_kit::test]
fn get_info_shows_details_and_changes_permissions(cx: &mut TestAppContext) {
    let fx = Fixture::new("info");
    let mut ui = Ui::open(cx, &fx.dir);
    let label = |ui: &mut Ui, id: &'static str| {
        ui.snapshot(id)
            .label()
            .map(|l| l.to_string())
            .unwrap_or_default()
    };
    ui.select("notes.txt");
    ui.press(&cmd("i"));
    assert!(ui.dialog_open());
    assert!(
        label(&mut ui, "info-size").starts_with("12 bytes, "),
        "{}",
        label(&mut ui, "info-size")
    );
    assert_eq!(label(&mut ui, "info-where"), fx.dir.display().to_string());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ui.click("perm-everyone-write");
        let mode = std::fs::metadata(fx.path("notes.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o002, 0, "the box sets the bit on disk");
        assert!(label(&mut ui, "info-mode").contains("rw-"));
    }
    ui.click("info-done");
    assert!(!ui.dialog_open());

    // A folder's size fills in once it's added up.
    ui.select("Projects");
    ui.press(&cmd("i"));
    assert!(label(&mut ui, "info-size").ends_with("for 2 items"));
    ui.click("info-done");

    ui.select("notes.txt");
    ui.select_also("Budget.csv");
    ui.press(&cmd("i"));
    assert!(label(&mut ui, "info-size").starts_with("20 bytes, "));
}

#[gpui_kit::test]
fn open_with_lists_apps_and_sets_the_default(cx: &mut TestAppContext) {
    use fffinder::apps::OpenApp;
    use fffinder::system::{Recorder, SystemCall};
    let fx = Fixture::new("open-with");
    let mut ui = Ui::open(cx, &fx.dir);
    let app = |name: &str, is_default: bool| OpenApp {
        name: name.into(),
        path: format!("/Apps/{name}.app").into(),
        is_default,
    };
    ui.cx.read(|cx| {
        cx.global::<Recorder>()
            .set_apps(vec![app("Editor", true), app("Viewer", false)])
    });
    ui.click("view-list");
    let notes = item_id("row", &fx.path("notes.txt"));

    ui.right_click(notes.clone());
    // On Windows, Explorer's Open with dialog (Other…) sets the default.
    assert_eq!(
        ui.menu_labels().contains(&"Always Open With".to_string()),
        !cfg!(windows)
    );
    ui.click_menu_item("Open With");
    let labels = ui.menu_labels();
    for label in ["Editor (default)", "Viewer", "Other…"] {
        assert!(labels.contains(&label.to_string()), "{label} in {labels:?}");
    }
    ui.click_menu_item("Viewer");
    let opened = SystemCall::OpenWith {
        app: "/Apps/Viewer.app".into(),
        paths: vec![fx.path("notes.txt")],
    };
    assert_eq!(ui.system_calls(), std::slice::from_ref(&opened));

    if !cfg!(windows) {
        ui.right_click(notes);
        ui.click_menu_item("Always Open With");
        ui.click_menu_item("Viewer");
        let calls = ui.system_calls();
        assert!(calls.contains(&SystemCall::SetDefaultApp {
            app: "/Apps/Viewer.app".into(),
            file: fx.path("notes.txt"),
        }));
        assert_eq!(calls.last(), Some(&opened), "and opens the file with it");
    }

    ui.right_click(item_id("row", &fx.path("Projects")));
    assert!(
        !ui.menu_labels().contains(&"Open With".to_string()),
        "folders have none"
    );
}

#[gpui_kit::test]
fn duplicate_new_file_links_and_archives(cx: &mut TestAppContext) {
    let fx = Fixture::new("make");
    let mut ui = Ui::open(cx, &fx.dir);

    ui.select("notes.txt");
    ui.press(&cmd("d"));
    assert!(fx.path("notes copy.txt").exists());
    assert_eq!(ui.selected(), ["notes copy.txt"]);

    // New File selects the name before the extension.
    ui.press(&cmd("alt-n"));
    assert!(ui.dialog_open());
    ui.type_text("todo");
    ui.click("new-file-create");
    assert!(fx.path("todo.txt").is_file());
    assert_eq!(ui.selected(), ["todo.txt"]);
    ui.press(&cmd("alt-n"));
    ui.press(&cmd("a"));
    ui.type_text("notes.txt");
    ui.press("enter");
    assert!(ui.dialog_open(), "a taken name is refused");
    ui.click("new-file-cancel");

    ui.select("Roadmap.md");
    ui.dispatch(MakeSymlink);
    let link = fx.path("Roadmap.md link");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::Path::new("Roadmap.md")
    );
    assert_eq!(std::fs::read_to_string(&link).unwrap(), "# Roadmap\n");
    #[cfg(target_os = "macos")]
    {
        ui.select("Roadmap.md");
        ui.dispatch(MakeAlias);
        assert!(fx.path("Roadmap.md alias").is_file(), "a Finder alias file");
    }
    #[cfg(windows)]
    {
        ui.select("Roadmap.md");
        ui.dispatch(MakeAlias);
        assert!(
            fx.path("Roadmap.md - Shortcut.lnk").is_file(),
            "an Explorer shortcut"
        );
    }

    // Compress, then opening the zip extracts it beside itself.
    ui.select("Projects");
    ui.dispatch(Compress);
    assert!(fx.path("Projects.zip").is_file());
    assert_eq!(ui.selected(), ["Projects.zip"]);
    let ix = ui.names().iter().position(|n| n == "Projects.zip").unwrap();
    ui.update(|f, window, cx| f.click_row(ix, Default::default(), 2, window, cx));
    let atlas = fx.path("Projects 2/atlas/Project Atlas.txt");
    assert_eq!(std::fs::read_to_string(&atlas).unwrap(), "atlas");
    assert_eq!(ui.selected(), ["Projects 2"]);
    ui.press(&cmd("z"));
    assert!(
        !fx.path("Projects 2").exists(),
        "Undo moves the extracted folder to the Trash"
    );
}

#[gpui_kit::test]
fn list_view_columns_folders_and_tree(cx: &mut TestAppContext) {
    let fx = Fixture::new("list-view");
    let mut ui = Ui::open(cx, &fx.dir);
    ui.click("view-list");

    // Folder sizes come from the index: Projects holds "atlas" (5 bytes).
    let projects = ui.read(|f| {
        f.rows_for_test()
            .iter()
            .find(|r| r.entry.name == "Projects")
            .map(|r| r.size.to_string())
    });
    assert_eq!(projects.as_deref(), Some("5 bytes"));

    // The column chooser adds a column after the others; it sorts.
    ui.right_click("Size");
    ui.click_menu_item("Date Created");
    assert!(ui.exists("Date Created"));
    ui.click("Date Created");
    assert_eq!(
        ui.read(|f| f.current_sort().key),
        fffinder_core::SortKey::Created
    );
    ui.right_click("Size");
    ui.click_menu_item("Permissions");
    assert!(ui.exists("Permissions"));
    let columns = |ui: &Ui| {
        ui.read(|f| {
            f.config()
                .list_columns
                .iter()
                .map(|c| c.column)
                .collect::<Vec<_>>()
        })
    };
    use fffinder_core::ListColumn::*;
    assert_eq!(columns(&ui), [Modified, Size, Kind, Created, Permissions]);

    // Drag a header onto another to move it, and a header's edge to resize it.
    ui.drag("Kind", "Date Modified", Modifiers::none());
    assert_eq!(columns(&ui), [Kind, Modified, Size, Created, Permissions]);
    let width = ui.read(|f| f.config().list_columns[2].width);
    ui.drag("resize-Size", "Kind", Modifiers::none());
    assert_ne!(
        ui.read(|f| f.config().list_columns[2].width),
        width,
        "Size was resized"
    );

    // Folders open in place, and Right and Left open and close them.
    ui.click(item_id("disclosure", &fx.path("Projects")));
    assert!(ui.names().contains(&"atlas".to_string()));
    let depth = ui.read(|f| {
        f.rows_for_test()
            .iter()
            .find(|r| r.entry.name == "atlas")
            .map(|r| r.depth)
    });
    assert_eq!(depth, Some(1));
    ui.select("atlas");
    ui.press("right");
    assert!(ui.names().contains(&"Project Atlas.txt".to_string()));
    ui.press("left");
    assert!(!ui.names().contains(&"Project Atlas.txt".to_string()));
    ui.press("left");
    assert_eq!(
        ui.selected(),
        ["Projects"],
        "Left goes up to the open folder"
    );
    ui.press("left");
    assert!(!ui.names().contains(&"atlas".to_string()));
}

#[gpui_kit::test]
fn volumes_eject_free_space_and_servers(cx: &mut TestAppContext) {
    use fffinder::system::{Recorder, SystemCall};
    use fffinder_core::places::{Place, PlaceIcon};
    let fx = Fixture::new("volumes");
    let mut ui = Ui::open(cx, &fx.dir);

    let status = ui
        .snapshot("status-text")
        .label()
        .unwrap_or_default()
        .to_string();
    assert!(status.ends_with(" available"), "{status}");

    // An ejectable place has an eject button and an Eject menu item.
    let disk = fx.path("Projects");
    let place = Place {
        name: "Backup".into(),
        path: disk.clone(),
        icon: PlaceIcon::External,
        ejectable: true,
    };
    let show = |ui: &mut Ui| {
        let place = place.clone();
        ui.update(|f, _, cx| {
            f.set_locations_for_test(vec![place]);
            cx.notify();
        })
    };
    show(&mut ui);
    ui.click(item_id("eject", &disk));
    assert_eq!(ui.system_calls(), [SystemCall::Eject(disk.clone())]);
    // Ejecting rereads the machine's volumes.
    show(&mut ui);
    ui.right_click(("loc", 0usize));
    ui.click_menu_item("Eject");
    assert_eq!(ui.system_calls().len(), 2);

    // Connect to Server opens where the server mounted, and remembers it.
    ui.cx.read(|cx| {
        cx.global::<Recorder>()
            .set_mount_point(fx.path("Projects/atlas"))
    });
    ui.press(&cmd("k"));
    assert!(ui.dialog_open());
    ui.press(&cmd("a"));
    ui.type_text("gopher://old");
    ui.click("server-connect");
    assert!(ui.dialog_open(), "an unsupported address is refused");
    ui.press(&cmd("a"));
    ui.type_text("smb://nas/share");
    ui.click("server-connect");
    assert!(!ui.dialog_open());
    assert_eq!(
        ui.system_calls().last(),
        Some(&SystemCall::Connect("smb://nas/share".into()))
    );
    assert_eq!(ui.folder(), Some(fx.path("Projects/atlas")));
    assert_eq!(
        ui.read(|f| f.config().recent_servers.clone()),
        ["smb://nas/share"]
    );
}

#[gpui_kit::test]
fn tabs_and_split_view(cx: &mut TestAppContext) {
    use common::Open;
    use fffinder::model::Location;
    let fx = Fixture::new("tabs");
    let mut ui = Ui::open_with(
        cx,
        &fx.dir,
        Open {
            workspace: true,
            ..Default::default()
        },
    );
    let workspace = ui.workspace.clone().expect("opened with tabs");
    let tabs = |ui: &Ui| workspace.read_with(ui.cx, |w, _| (w.tabs().len(), w.active_index()));
    assert!(!ui.exists("tab-bar"), "one tab has no tab bar");
    // Dialogs and notifications still work with tabs around the browser.
    ui.select("notes.txt");
    ui.press(&cmd("i"));
    assert!(ui.dialog_open());
    ui.click("info-done");
    assert!(!ui.dialog_open());
    ui.press(&cmd("shift-g"));
    assert!(ui.dialog_open());
    ui.click("goto-cancel");

    ui.press(&cmd("t"));
    assert_eq!(tabs(&ui), (2, 1));
    assert!(ui.exists("tab-bar"));
    // Each tab keeps its own place.
    let second = workspace.read_with(ui.cx, |w, _| w.active().clone());
    let projects = fx.path("Projects");
    ui.window(|window, cx| {
        second.update(cx, |f, cx| {
            f.navigate(Location::Folder(projects), window, cx)
        })
    });
    ui.settle();
    // Drag the second tab in front of the first; it stays the active one.
    ui.drag(("tab", 1usize), ("tab", 0usize), Modifiers::none());
    assert_eq!(tabs(&ui), (2, 0));
    assert_eq!(
        workspace.read_with(ui.cx, |w, _| w.active().entity_id()),
        second.entity_id()
    );
    ui.drag(("tab", 0usize), ("tab", 1usize), Modifiers::none());
    assert_eq!(tabs(&ui), (2, 1));
    ui.press("ctrl-tab");
    assert_eq!(tabs(&ui), (2, 0));
    assert_eq!(
        ui.folder(),
        Some(fx.dir.clone()),
        "the first tab didn't move"
    );
    ui.click(("tab", 1usize));
    assert_eq!(tabs(&ui).1, 1);
    ui.press(&cmd("w"));
    assert_eq!(tabs(&ui), (1, 0), "Cmd+W closes the tab, not the window");
    // The toolbar's Actions menu has them too, for systems without a menu bar.
    ui.click("actions");
    ui.click_menu_item("New Tab");
    assert_eq!(tabs(&ui), (2, 1));
    ui.press(&cmd("w"));

    // Split view: F5 copies and F6 moves into the other pane's folder.
    ui.press(&cmd("\\"));
    let pane = workspace
        .read_with(ui.cx, |w, _| w.split_pane().cloned())
        .expect("a second pane");
    let projects = fx.path("Projects");
    ui.window(|window, cx| {
        pane.update(cx, |f, cx| {
            f.navigate(Location::Folder(projects), window, cx)
        })
    });
    ui.settle();
    ui.select("notes.txt");
    ui.press("f5");
    assert!(fx.path("Projects/notes.txt").exists() && fx.path("notes.txt").exists());
    ui.select("Budget.csv");
    ui.press("f6");
    assert!(fx.path("Projects/Budget.csv").exists() && !fx.path("Budget.csv").exists());
    let shown: Vec<String> = pane.read_with(ui.cx, |f, _| {
        f.rows_for_test()
            .iter()
            .map(|r| r.entry.name.clone())
            .collect()
    });
    assert!(
        shown.contains(&"Budget.csv".to_string()),
        "the other pane reloads: {shown:?}"
    );
    ui.press(&cmd("\\"));
    assert!(workspace.read_with(ui.cx, |w, _| w.split_pane().is_none()));
}
