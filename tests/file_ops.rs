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
