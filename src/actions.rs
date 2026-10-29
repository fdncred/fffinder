//! Actions, key bindings, and the application menu bar.

use gpui_kit::component::input;
use gpui_kit::{
    Action, App, KeyBinding, KeyBindingContextPredicate, Menu, MenuItem, OsAction, actions,
};
use std::rc::Rc;

actions!(
    finder,
    [
        Quit,
        NewWindow,
        NewTab,
        ShowNextTab,
        ShowPreviousTab,
        MoveTabToNewWindow,
        ToggleSplitView,
        CopyToOtherPane,
        MoveToOtherPane,
        CloseWindow,
        ShowSettings,
        FocusSearch,
        GoBack,
        GoForward,
        GoUp,
        GoHome,
        GoRecents,
        GoToFolder,
        ConnectToServer,
        OpenSelected,
        QuickLook,
        GetInfo,
        ToggleSidebar,
        ViewIcons,
        ViewList,
        ViewColumns,
        ViewGallery,
        ToggleHidden,
        ToggleFoldersFirst,
        ToggleGroups,
        Refresh,
        SelectAll,
        Escape,
        MoveUp,
        MoveDown,
        MoveLeft,
        MoveRight,
        ExtendUp,
        ExtendDown,
        ExtendLeft,
        ExtendRight,
        MoveHome,
        MoveEnd,
        CopyPath,
        CopyAsNuon,
        MoveItemsHere,
        RevealInFileManager,
        MoveToTrash,
        PutBack,
        DeleteImmediately,
        EmptyTrash,
        GoTrash,
        NewFolder,
        NewFile,
        Duplicate,
        MakeAlias,
        MakeSymlink,
        Compress,
        Extract,
        Rename,
        OpenInNushell,
        ToggleNuPanel,
        SearchHelp,
        Rescan,
        Choose,
        ToggleAppearance,
        ToggleFilter,
        OpenInEditor,
        PreviewTextBigger,
        PreviewTextSmaller,
        PreviewTextReset,
        NextPdfPage,
        PreviousPdfPage,
    ]
);

/// Key context of the browser window.
pub const FINDER: &str = "Finder";
/// Key context of the file views; arrows, Space and Return only apply here
/// so they keep working normally in text fields.
pub const FILES: &str = "FinderFiles";
/// Key context of the Quick Look preview, so Space closes it and the arrows
/// move through the items.
pub const QUICK_LOOK: &str = "QuickLook";

pub fn bind_keys(cx: &mut App) {
    let finder = context(FINDER);
    let files = context(FILES);
    let quick_look = context(QUICK_LOOK);
    let bindings = [
        key("secondary-q", Quit, &None, cx),
        key("secondary-,", ShowSettings, &None, cx),
        key("secondary-n", NewWindow, &None, cx),
        key("secondary-w", CloseWindow, &finder, cx),
        key("secondary-t", NewTab, &finder, cx),
        key("ctrl-tab", ShowNextTab, &finder, cx),
        key("ctrl-shift-tab", ShowPreviousTab, &finder, cx),
        key("secondary-shift-]", ShowNextTab, &finder, cx),
        key("secondary-shift-[", ShowPreviousTab, &finder, cx),
        key("secondary-\\", ToggleSplitView, &finder, cx),
        key("secondary-f", FocusSearch, &finder, cx),
        key("secondary-[", GoBack, &finder, cx),
        key("secondary-]", GoForward, &finder, cx),
        key("alt-left", GoBack, &finder, cx),
        key("alt-right", GoForward, &finder, cx),
        key("secondary-up", GoUp, &finder, cx),
        key("secondary-shift-h", GoHome, &finder, cx),
        key("secondary-shift-f", GoRecents, &finder, cx),
        key("secondary-shift-g", GoToFolder, &finder, cx),
        key("secondary-k", ConnectToServer, &finder, cx),
        key("secondary-1", ViewIcons, &finder, cx),
        key("secondary-2", ViewList, &finder, cx),
        key("secondary-3", ViewColumns, &finder, cx),
        key("secondary-4", ViewGallery, &finder, cx),
        key("secondary-shift-.", ToggleHidden, &finder, cx),
        key("secondary-r", Refresh, &finder, cx),
        key("secondary-alt-s", ToggleSidebar, &finder, cx),
        key("secondary-shift-n", NewFolder, &finder, cx),
        key("secondary-i", GetInfo, &finder, cx),
        key("secondary-alt-n", NewFile, &finder, cx),
        key("secondary-shift-backspace", EmptyTrash, &finder, cx),
        key("secondary-alt-c", CopyPath, &finder, cx),
        key("secondary-shift-c", CopyAsNuon, &finder, cx),
        key("secondary-alt-t", OpenInNushell, &finder, cx),
        key("ctrl-`", ToggleNuPanel, &finder, cx),
        key("secondary-shift-d", ToggleAppearance, &finder, cx),
        key("secondary-/", SearchHelp, &finder, cx),
        key("secondary-alt-f", ToggleFilter, &finder, cx),
        // File views.
        key("up", MoveUp, &files, cx),
        key("down", MoveDown, &files, cx),
        key("left", MoveLeft, &files, cx),
        key("right", MoveRight, &files, cx),
        key("shift-up", ExtendUp, &files, cx),
        key("shift-down", ExtendDown, &files, cx),
        key("shift-left", ExtendLeft, &files, cx),
        key("shift-right", ExtendRight, &files, cx),
        key("home", MoveHome, &files, cx),
        key("end", MoveEnd, &files, cx),
        key("enter", OpenSelected, &files, cx),
        key("secondary-down", OpenSelected, &files, cx),
        key("secondary-o", OpenSelected, &files, cx),
        key("space", QuickLook, &files, cx),
        key("secondary-y", QuickLook, &files, cx),
        key("space", QuickLook, &quick_look, cx),
        key("secondary-y", QuickLook, &quick_look, cx),
        key("escape", QuickLook, &quick_look, cx),
        key("up", MoveUp, &quick_look, cx),
        key("down", MoveDown, &quick_look, cx),
        key("left", MoveLeft, &quick_look, cx),
        key("right", MoveRight, &quick_look, cx),
        key("secondary-=", PreviewTextBigger, &quick_look, cx),
        key("secondary-+", PreviewTextBigger, &quick_look, cx),
        key("secondary--", PreviewTextSmaller, &quick_look, cx),
        key("secondary-0", PreviewTextReset, &quick_look, cx),
        key("pagedown", NextPdfPage, &quick_look, cx),
        key("pageup", PreviousPdfPage, &quick_look, cx),
        // The same keys for the gallery's preview, from the files.
        key("secondary-=", PreviewTextBigger, &files, cx),
        key("secondary-+", PreviewTextBigger, &files, cx),
        key("secondary--", PreviewTextSmaller, &files, cx),
        key("secondary-0", PreviewTextReset, &files, cx),
        key("pagedown", NextPdfPage, &files, cx),
        key("pageup", PreviousPdfPage, &files, cx),
        key("escape", Escape, &files, cx),
        key("secondary-a", SelectAll, &files, cx),
        // Edit › Copy and Paste act on files when the files have focus.
        key("secondary-c", input::Copy, &files, cx),
        key("secondary-v", input::Paste, &files, cx),
        key("secondary-alt-v", MoveItemsHere, &files, cx),
        key("f2", Rename, &files, cx),
        key("secondary-d", Duplicate, &files, cx),
        key("f5", CopyToOtherPane, &files, cx),
        key("f6", MoveToOtherPane, &files, cx),
        key("secondary-backspace", MoveToTrash, &files, cx),
        key("secondary-alt-backspace", DeleteImmediately, &files, cx),
        key("secondary-z", input::Undo, &files, cx),
        key("secondary-shift-z", input::Redo, &files, cx),
        key("delete", MoveToTrash, &files, cx),
    ];
    // Terminals keep most Cmd shortcuts for themselves, so in one every Cmd
    // binding also works with Ctrl. Added last, menus show the Ctrl form.
    let aliases: Vec<KeyBinding> = if crate::term::active() {
        bindings
            .iter()
            .filter_map(|binding| ctrl_alias(binding, cx))
            .collect()
    } else {
        Vec::new()
    };
    cx.bind_keys(bindings);
    cx.bind_keys(aliases);
}

/// A key context's predicate, parsed once for all of its bindings.
fn context(name: &str) -> Option<Rc<KeyBindingContextPredicate>> {
    let predicate = KeyBindingContextPredicate::parse(name).expect("key contexts are plain names");
    Some(Rc::new(predicate))
}

/// `action` bound to `keys` in `context`. `secondary` in `keys` is Cmd on
/// macOS and Ctrl on Linux and Windows, which only the running app knows.
fn key(
    keys: &str,
    action: impl Action,
    context: &Option<Rc<KeyBindingContextPredicate>>,
    cx: &App,
) -> KeyBinding {
    KeyBinding::load(keys, Box::new(action), context.clone(), false, None, cx)
        .expect("the key table's keystrokes are valid")
}

/// The binding with Cmd swapped for Ctrl, if it uses Cmd at all.
fn ctrl_alias(binding: &KeyBinding, cx: &App) -> Option<KeyBinding> {
    if !binding
        .keystrokes()
        .iter()
        .any(|k| k.inner().modifiers.platform)
    {
        return None;
    }
    let os = cx.platform_info().os;
    let keystrokes: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|keystroke| {
            let mut keystroke = keystroke.inner().clone();
            if keystroke.modifiers.platform {
                keystroke.modifiers.platform = false;
                keystroke.modifiers.control = true;
            }
            keystroke.unparse(os)
        })
        .collect();
    KeyBinding::load(
        &keystrokes.join(" "),
        binding.action().boxed_clone(),
        binding.predicate(),
        false,
        None,
        cx,
    )
    .ok()
}

/// What Make Alias is called: Finder's "Make Alias", Explorer's "Create
/// Shortcut", or nothing on Linux, where links are the aliases.
pub fn alias_label() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("Make Alias")
    } else if cfg!(windows) {
        Some("Create Shortcut")
    } else {
        None
    }
}

pub fn set_menus(cx: &mut App) {
    // Finder aliases are macOS only; symbolic links work everywhere.
    let mut file = vec![
        MenuItem::action("New Window", NewWindow),
        MenuItem::action("New Tab", NewTab),
        MenuItem::action("New Folder", NewFolder),
        MenuItem::action("New File…", NewFile),
        MenuItem::separator(),
        MenuItem::action("Open", OpenSelected),
        MenuItem::action("Open in Editor", OpenInEditor),
        MenuItem::action("Quick Look", QuickLook),
        MenuItem::action("Get Info", GetInfo),
        MenuItem::action("Rename…", Rename),
        MenuItem::action("Duplicate", Duplicate),
    ];
    // Finder aliases on macOS, shortcuts on Windows; Linux has symbolic links.
    if let Some(label) = alias_label() {
        file.push(MenuItem::action(label, MakeAlias));
    }
    file.extend([
        MenuItem::action("Make Symbolic Link", MakeSymlink),
        MenuItem::action("Compress", Compress),
        MenuItem::action("Extract", Extract),
        MenuItem::separator(),
        MenuItem::action("Move to Trash", MoveToTrash),
        MenuItem::action("Put Back", PutBack),
        MenuItem::action("Delete Immediately…", DeleteImmediately),
        MenuItem::separator(),
        MenuItem::action("Close Window", CloseWindow),
    ]);
    cx.set_menus([
        Menu::new("fffinder").items([
            MenuItem::action("Settings…", ShowSettings),
            MenuItem::action("Rescan Index", Rescan),
            MenuItem::action("Empty Trash…", EmptyTrash),
            MenuItem::separator(),
            MenuItem::action("Quit fffinder", Quit),
        ]),
        Menu::new("File").items(file),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::action("Move Items Here", MoveItemsHere),
            MenuItem::separator(),
            MenuItem::action("Copy Path", CopyPath),
            MenuItem::action("Copy as Nushell List", CopyAsNuon),
            MenuItem::action("Select All Items", SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find", FocusSearch),
            MenuItem::action("Filter with Nushell", ToggleFilter),
            MenuItem::action("Search Syntax…", SearchHelp),
        ]),
        Menu::new("View").items([
            MenuItem::action("as Icons", ViewIcons),
            MenuItem::action("as List", ViewList),
            MenuItem::action("as Columns", ViewColumns),
            MenuItem::action("as Gallery", ViewGallery),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Show/Hide Hidden Files", ToggleHidden),
            MenuItem::action("Keep Folders on Top", ToggleFoldersFirst),
            MenuItem::action("Use Groups", ToggleGroups),
            MenuItem::action("Toggle Light/Dark", ToggleAppearance),
            MenuItem::action("Refresh", Refresh),
        ]),
        Menu::new("Go").items([
            MenuItem::action("Back", GoBack),
            MenuItem::action("Forward", GoForward),
            MenuItem::action("Enclosing Folder", GoUp),
            MenuItem::separator(),
            MenuItem::action("Recents", GoRecents),
            MenuItem::action("Home", GoHome),
            MenuItem::action("Trash", GoTrash),
            MenuItem::action("Go to Folder…", GoToFolder),
            MenuItem::action("Connect to Server…", ConnectToServer),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Show Previous Tab", ShowPreviousTab),
            MenuItem::action("Show Next Tab", ShowNextTab),
            MenuItem::action("Move Tab to New Window", MoveTabToNewWindow),
            MenuItem::separator(),
            MenuItem::action("Split View", ToggleSplitView),
            MenuItem::action("Copy to Other Pane", CopyToOtherPane),
            MenuItem::action("Move to Other Pane", MoveToOtherPane),
        ]),
        Menu::new("Nushell").items([
            MenuItem::action("Pipeline Panel", ToggleNuPanel),
            MenuItem::action("Open in Nushell", OpenInNushell),
            MenuItem::action("Copy as Nushell List", CopyAsNuon),
        ]),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Modifiers, TestAppContext};

    #[gpui_kit::test]
    fn secondary_is_the_systems_shortcut_key(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let binding = key("secondary-n", NewWindow, &context(FINDER), cx);
            let keystroke = binding.keystrokes()[0].inner();
            assert_eq!(keystroke.modifiers, Modifiers::secondary_key(cx));
            assert_eq!(keystroke.key, "n");
            assert_eq!(binding.predicate(), context(FINDER));
        });
    }

    #[gpui_kit::test]
    fn ctrl_alias_swaps_cmd_for_ctrl(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let binding = key("cmd-shift-n", NewFolder, &context(FINDER), cx);
            let alias = ctrl_alias(&binding, cx).expect("a Cmd binding has an alias");
            let os = cx.platform_info().os;
            assert_eq!(alias.keystrokes()[0].inner().unparse(os), "ctrl-shift-n");
            assert!(alias.action().partial_eq(&NewFolder));
            assert_eq!(alias.predicate(), binding.predicate());
            assert!(ctrl_alias(&key("alt-left", GoBack, &None, cx), cx).is_none());
        });
    }
}
