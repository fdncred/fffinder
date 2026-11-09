//! What `--term` mode does differently, driven in a headless window. The
//! terminal shows a single window, so Quick Look is a dialog inside it, and
//! every Cmd shortcut also works with Ctrl. Term mode is a process-wide flag,
//! so these tests live in a binary of their own.

mod common;

use common::{Fixture, Open, Ui, cmd};
use fffinder::finder::QuickLookMode;
use gpui_kit::TestAppContext;

/// A browser in term mode on `fx`, with `nu` for explore mode.
fn open_term<'a>(cx: &'a mut TestAppContext, fx: &Fixture) -> Option<Ui<'a>> {
    let Some(nu) = fffinder::nushell::find_nu(None) else {
        eprintln!("nu not installed; skipping");
        return None;
    };
    fffinder::term::pretend_active_for_tests();
    Some(Ui::open_with(
        cx,
        &fx.dir,
        Open {
            nu: Some(nu),
            ..Default::default()
        },
    ))
}

#[gpui_kit::test]
fn quick_look_explores_in_a_dialog(cx: &mut TestAppContext) {
    let fx = Fixture::new("term-explore");
    std::fs::write(
        fx.path("app.json"),
        r#"{"name": "fff", "deps": {"serde": {"version": "1"}}}"#,
    )
    .unwrap();
    let Some(mut ui) = open_term(cx, &fx) else {
        return;
    };
    let crumb = |depth: usize| ("explore-crumb", depth);

    ui.select("app.json");
    ui.press(&cmd("space"));
    assert!(ui.dialog_open(), "Quick Look is a dialog in the terminal");
    assert_eq!(ui.read(|f| f.quick_look_mode()), QuickLookMode::Explore);
    assert!(ui.exists(crumb(0)), "Cmd-Space explores the data");
    assert!(
        ui.read(|f| f.quick_look_window()).is_none(),
        "no second window"
    );

    ui.press("down");
    ui.press("enter");
    assert!(ui.exists(crumb(1)), "Return goes into deps");
    ui.press("enter");
    assert!(ui.exists(crumb(2)), "and into serde");
    ui.click(crumb(0));
    assert!(!ui.exists(crumb(1)), "the path goes back to the top");
    ui.press("enter");
    ui.press("escape");
    assert!(!ui.exists(crumb(1)), "Escape goes back out");
    assert!(ui.dialog_open(), "and keeps Quick Look open");

    ui.press("space");
    assert!(ui.dialog_open(), "Space switches to text");
    assert_eq!(ui.read(|f| f.quick_look_mode()), QuickLookMode::Text);
    assert!(!ui.exists(crumb(0)));
    ui.click("ql-explore");
    assert!(ui.exists(crumb(0)), "the Explore button goes back");
    ui.click("ql-text");
    assert!(!ui.exists(crumb(0)), "and the Text button to text");
    ui.press("ctrl-space");
    assert_eq!(
        ui.read(|f| f.quick_look_mode()),
        QuickLookMode::Explore,
        "Ctrl-Space does what Cmd-Space does, for terminals that keep Cmd"
    );

    ui.press("escape");
    assert!(!ui.dialog_open(), "Escape at the top closes Quick Look");
    assert!(!ui.read(|f| f.quick_look_is_open()));

    // Opening it again starts at the top, not where the last visit ended.
    ui.press(&cmd("space"));
    assert!(ui.dialog_open());
    assert!(ui.exists(crumb(0)) && !ui.exists(crumb(1)));
    ui.press(&cmd("space"));
    assert!(!ui.dialog_open(), "Cmd-Space in explore mode closes it");
}

#[gpui_kit::test]
fn a_broken_file_explains_itself_in_the_dialog(cx: &mut TestAppContext) {
    let fx = Fixture::new("term-broken");
    std::fs::write(fx.path("broken.toml"), "a = [\n").unwrap();
    let Some(mut ui) = open_term(cx, &fx) else {
        return;
    };
    ui.select("broken.toml");
    ui.press(&cmd("space"));
    assert!(ui.dialog_open());
    assert!(ui.exists("explore-failure"), "the reason, not a grid");
    ui.press("space");
    assert!(ui.dialog_open());
    assert!(!ui.exists("explore-failure"), "Space shows the text");
    ui.press("space");
    assert!(!ui.dialog_open(), "and Space again closes it");
}

#[gpui_kit::test]
fn arrows_step_through_items_after_quick_look_closes(cx: &mut TestAppContext) {
    let fx = Fixture::new("term-arrows");
    let Some(mut ui) = open_term(cx, &fx) else {
        return;
    };
    ui.select("notes.txt");
    let before = ui.selected();
    ui.press("space");
    assert!(ui.dialog_open());
    ui.press("escape");
    assert!(!ui.dialog_open());
    ui.press("right");
    assert_ne!(ui.selected(), before, "the browser has the keys back");
}
