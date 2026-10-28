//! Actions, key bindings, and the application menu bar.

use gpui_kit::component::input;
use gpui_kit::{App, DummyKeyboardMapper, KeyBinding, Menu, MenuItem, OsAction, actions};

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
    let finder = Some(FINDER);
    let files = Some(FILES);
    let bindings = [
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", ShowSettings, None),
        KeyBinding::new("secondary-n", NewWindow, None),
        KeyBinding::new("secondary-w", CloseWindow, finder),
        KeyBinding::new("secondary-t", NewTab, finder),
        KeyBinding::new("ctrl-tab", ShowNextTab, finder),
        KeyBinding::new("ctrl-shift-tab", ShowPreviousTab, finder),
        KeyBinding::new("secondary-shift-]", ShowNextTab, finder),
        KeyBinding::new("secondary-shift-[", ShowPreviousTab, finder),
        KeyBinding::new("secondary-\\", ToggleSplitView, finder),
        KeyBinding::new("secondary-f", FocusSearch, finder),
        KeyBinding::new("secondary-[", GoBack, finder),
        KeyBinding::new("secondary-]", GoForward, finder),
        KeyBinding::new("alt-left", GoBack, finder),
        KeyBinding::new("alt-right", GoForward, finder),
        KeyBinding::new("secondary-up", GoUp, finder),
        KeyBinding::new("secondary-shift-h", GoHome, finder),
        KeyBinding::new("secondary-shift-f", GoRecents, finder),
        KeyBinding::new("secondary-shift-g", GoToFolder, finder),
        KeyBinding::new("secondary-k", ConnectToServer, finder),
        KeyBinding::new("secondary-1", ViewIcons, finder),
        KeyBinding::new("secondary-2", ViewList, finder),
        KeyBinding::new("secondary-3", ViewColumns, finder),
        KeyBinding::new("secondary-4", ViewGallery, finder),
        KeyBinding::new("secondary-shift-.", ToggleHidden, finder),
        KeyBinding::new("secondary-r", Refresh, finder),
        KeyBinding::new("secondary-alt-s", ToggleSidebar, finder),
        KeyBinding::new("secondary-shift-n", NewFolder, finder),
        KeyBinding::new("secondary-i", GetInfo, finder),
        KeyBinding::new("secondary-alt-n", NewFile, finder),
        KeyBinding::new("secondary-shift-backspace", EmptyTrash, finder),
        KeyBinding::new("secondary-alt-c", CopyPath, finder),
        KeyBinding::new("secondary-shift-c", CopyAsNuon, finder),
        KeyBinding::new("secondary-alt-t", OpenInNushell, finder),
        KeyBinding::new("ctrl-`", ToggleNuPanel, finder),
        KeyBinding::new("secondary-shift-d", ToggleAppearance, finder),
        KeyBinding::new("secondary-/", SearchHelp, finder),
        KeyBinding::new("secondary-alt-f", ToggleFilter, finder),
        // File views.
        KeyBinding::new("up", MoveUp, files),
        KeyBinding::new("down", MoveDown, files),
        KeyBinding::new("left", MoveLeft, files),
        KeyBinding::new("right", MoveRight, files),
        KeyBinding::new("shift-up", ExtendUp, files),
        KeyBinding::new("shift-down", ExtendDown, files),
        KeyBinding::new("shift-left", ExtendLeft, files),
        KeyBinding::new("shift-right", ExtendRight, files),
        KeyBinding::new("home", MoveHome, files),
        KeyBinding::new("end", MoveEnd, files),
        KeyBinding::new("enter", OpenSelected, files),
        KeyBinding::new("secondary-down", OpenSelected, files),
        KeyBinding::new("secondary-o", OpenSelected, files),
        KeyBinding::new("space", QuickLook, files),
        KeyBinding::new("secondary-y", QuickLook, files),
        KeyBinding::new("space", QuickLook, Some(QUICK_LOOK)),
        KeyBinding::new("secondary-y", QuickLook, Some(QUICK_LOOK)),
        KeyBinding::new("escape", QuickLook, Some(QUICK_LOOK)),
        KeyBinding::new("up", MoveUp, Some(QUICK_LOOK)),
        KeyBinding::new("down", MoveDown, Some(QUICK_LOOK)),
        KeyBinding::new("left", MoveLeft, Some(QUICK_LOOK)),
        KeyBinding::new("right", MoveRight, Some(QUICK_LOOK)),
        KeyBinding::new("secondary-=", PreviewTextBigger, Some(QUICK_LOOK)),
        KeyBinding::new("secondary-+", PreviewTextBigger, Some(QUICK_LOOK)),
        KeyBinding::new("secondary--", PreviewTextSmaller, Some(QUICK_LOOK)),
        KeyBinding::new("secondary-0", PreviewTextReset, Some(QUICK_LOOK)),
        KeyBinding::new("pagedown", NextPdfPage, Some(QUICK_LOOK)),
        KeyBinding::new("pageup", PreviousPdfPage, Some(QUICK_LOOK)),
        // The same keys for the gallery's preview, from the files.
        KeyBinding::new("secondary-=", PreviewTextBigger, files),
        KeyBinding::new("secondary-+", PreviewTextBigger, files),
        KeyBinding::new("secondary--", PreviewTextSmaller, files),
        KeyBinding::new("secondary-0", PreviewTextReset, files),
        KeyBinding::new("pagedown", NextPdfPage, files),
        KeyBinding::new("pageup", PreviousPdfPage, files),
        KeyBinding::new("escape", Escape, files),
        KeyBinding::new("secondary-a", SelectAll, files),
        // Edit › Copy and Paste act on files when the files have focus.
        KeyBinding::new("secondary-c", input::Copy, files),
        KeyBinding::new("secondary-v", input::Paste, files),
        KeyBinding::new("secondary-alt-v", MoveItemsHere, files),
        KeyBinding::new("f2", Rename, files),
        KeyBinding::new("secondary-d", Duplicate, files),
        KeyBinding::new("f5", CopyToOtherPane, files),
        KeyBinding::new("f6", MoveToOtherPane, files),
        KeyBinding::new("secondary-backspace", MoveToTrash, files),
        KeyBinding::new("secondary-alt-backspace", DeleteImmediately, files),
        KeyBinding::new("secondary-z", input::Undo, files),
        KeyBinding::new("secondary-shift-z", input::Redo, files),
        KeyBinding::new("delete", MoveToTrash, files),
    ];
    // Terminals keep most Cmd shortcuts for themselves, so in one every Cmd
    // binding also works with Ctrl. Added last, menus show the Ctrl form.
    let aliases: Vec<KeyBinding> = if crate::term::active() {
        bindings.iter().filter_map(ctrl_alias).collect()
    } else {
        Vec::new()
    };
    cx.bind_keys(bindings);
    cx.bind_keys(aliases);
}

/// The binding with Cmd swapped for Ctrl, if it uses Cmd at all.
fn ctrl_alias(binding: &KeyBinding) -> Option<KeyBinding> {
    if !binding
        .keystrokes()
        .iter()
        .any(|k| k.inner().modifiers.platform)
    {
        return None;
    }
    let keystrokes: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|keystroke| {
            let mut keystroke = keystroke.inner().clone();
            if keystroke.modifiers.platform {
                keystroke.modifiers.platform = false;
                keystroke.modifiers.control = true;
            }
            keystroke.unparse()
        })
        .collect();
    KeyBinding::load(
        &keystrokes.join(" "),
        binding.action().boxed_clone(),
        binding.predicate(),
        false,
        None,
        &DummyKeyboardMapper,
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

    #[test]
    fn ctrl_alias_swaps_cmd_for_ctrl() {
        let binding = KeyBinding::new("cmd-shift-n", NewFolder, Some(FINDER));
        let alias = ctrl_alias(&binding).expect("a Cmd binding has an alias");
        assert_eq!(alias.keystrokes()[0].inner().unparse(), "ctrl-shift-n");
        assert!(alias.action().partial_eq(&NewFolder));
        assert_eq!(alias.predicate(), binding.predicate());
        assert!(ctrl_alias(&KeyBinding::new("alt-left", GoBack, None)).is_none());
    }
}
