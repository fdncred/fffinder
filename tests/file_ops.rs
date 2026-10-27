//! Copying, moving, and the other things done to files, driven through key
//! presses, menus, and drags in a headless window.

mod common;

use common::{Fixture, Ui, at, cmd, item_id};
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

    ui.right_click(notes);
    ui.click_menu_item("Always Open With");
    ui.click_menu_item("Viewer");
    let calls = ui.system_calls();
    assert!(calls.contains(&SystemCall::SetDefaultApp {
        app: "/Apps/Viewer.app".into(),
        file: fx.path("notes.txt"),
    }));
    assert_eq!(calls.last(), Some(&opened), "and opens the file with it");

    ui.right_click(item_id("row", &fx.path("Projects")));
    assert!(
        !ui.menu_labels().contains(&"Open With".to_string()),
        "folders have none"
    );
}
