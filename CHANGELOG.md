# Changelog

All notable changes to fffinder are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- On macOS, apps are no longer indexed. "This Computer" searches find them by reading `/Applications`, `/System/Applications`, `~/Applications`, and the folders directly inside them, so Calculator, Terminal, and the other apps in `/System/Applications` are found too. Leaving the files inside app bundles out of the index took memory once settled from 258 MB to 150 MB on the development Mac, and name searches from 65–100 ms to 25–35 ms. Existing settings drop `/Applications` from the search locations once; add it back with Add Folder… to search inside bundles. Settings › Search locations has an Applications switch to stop finding apps.
- Apps in the app folders show their sizes in the list view, added up in the background the first time you open the folder.
- Run from a terminal without a folder, fffinder opens the folder you ran it from on every system, as `--term` already did; before, it opened the last folder. When fffinder is already running, that window goes to the folder instead of only coming forward. Started from the Dock, Finder, Explorer, or a desktop menu, it still opens the last folder.

### Added

- The sidebar footer says "Finishing index…" while fff-search reads each file once after a scan to flag binaries, which takes minutes for a large folder and keeps the CPU busy (issue #3). `GET /v1/status` reports it as `finishing`, for each location and overall.

### Fixed

- The app icon was missing from the Dock and the window switcher on macOS when fffinder ran outside `fffinder.app`, as after `cargo install` or `scripts/install.nu`, which showed a generic executable instead. fffinder now sets it itself. X11 windows get the icon too, and on Linux `scripts/install.nu` installs the desktop entry and icon, which Wayland needs.

## [0.2.0] - 2026-10-28

fffinder can now manage files as well as find them. Unless a line says otherwise, each feature works on macOS, Linux, and Windows. Shortcuts are written as on macOS; on Linux and Windows, Cmd is Ctrl. [The manual](https://github.com/fdncred/fffinder/blob/main/docs/manual.md) has the details.

### Added

#### Working with files

- Cmd+C copies the selected items, Cmd+V pastes them into the folder shown, and Cmd+Alt+V moves them there. On macOS and Windows the clipboard holds them as files, so Finder and Explorer can paste them, and files copied there paste into fffinder.
- Dragging items onto a folder, a column, or a sidebar place moves them. Holding Option on macOS, or Ctrl on Linux and Windows, when you let go copies them, and so does a drag that includes any item from another volume. Files dropped from other apps are copied in.
- When names clash, a dialog offers Stop, Skip, Keep Both, or Replace. Replace removes the old item only once its replacement is complete.
- Copies, moves, Compress, and Extract run in the background with a progress row and a Stop button. The work starts at once while fffinder counts the bytes alongside it, and the bar stays at 0% until the count is done. When a move to another volume copies an item but can't remove the original, a warning says so, and Undo treats the item as a copy.
- Cmd+Z undoes renames, batch renames, moves, copies, new items, and moves to the Trash, and Cmd+Shift+Z redoes them. One list of the last 100 operations covers every window. When an Undo can't finish, what it reversed stays reversed and Cmd+Z tries the rest again. Undo never overwrites an item that has taken an old name since.
- A Trash row at the end of the sidebar lists what's in the Trash, with Put Back, Delete Immediately, and Empty Trash.
- Get Info (Cmd+I) shows an item's kind, size and size on disk, dates, link target, default app, owner, group, and the address a download came from. Clicking a permission box changes the item's permissions, and a folder's size is added up in the background.
- Open With in the item menu lists the apps that open the file's type. Always Open With makes the chosen app the default.
- Duplicate (Cmd+D) copies items beside themselves as "name copy", and New File (Cmd+Alt+N) makes an empty file.
- Make Alias makes Finder aliases on macOS and Create Shortcut makes shortcuts on Windows. Make Symbolic Link works everywhere.
- Compress puts the selection in a zip. Extract unpacks zip, tar, and tar.gz archives, and opening an archive extracts it.

#### Browsing

- Tabs. Cmd+T opens one, Ctrl+Tab and Cmd+Shift+] and [ move between them, and Cmd+W closes one. Drag a tab onto another to reorder them. Right-click a tab, or use Window > Move Tab to New Window, to give it a window of its own.
- Split View (Cmd+Backslash) adds a second pane beside the current tab. F5 copies the selection into the other pane's folder, and F6 moves it there.
- The list view opens folders in place with disclosure triangles. Right-click the header to choose columns from Date Modified, Date Created, Date Last Opened, Size, Kind, Permissions, and Git. Columns can be dragged to reorder or resize them, and the layout is saved.
- Calculate All Sizes shows folder sizes from the search index without reading the disk. The Git column shows each file's git status.
- Keep Folders on Top sorts folders first in every view.
- Group By shows the list and icon views in groups by name, kind, date, or size, like Finder's Use Groups. Click a group's header to collapse it.
- Removable disks, disk images, and servers in the sidebar have an eject button. The status bar shows the free space on the current folder's volume.
- Connect to Server (Cmd+K) mounts SMB and WebDAV servers, AFP, NFS, and FTP on macOS and Linux, and SFTP on Linux. It remembers them under Recent servers, leaving out any password typed in the address.

#### Previews

- Quick Look opens in a window of its own that you can move, resize, maximize, or make full screen. The arrow keys still step through items. In `--term` mode Quick Look is still a dialog, because the terminal shows one window.
- PDFs read as in a PDF viewer, in the gallery and in Quick Look. Pages scroll in one column and open fitted to the width. The toolbar has previous and next page buttons, "Page 2 of 12", zoom buttons, and a zoom menu with Fit Width, Fit Page, Actual Size, and sizes from 25% to 400%. Pinching, Cmd+scroll, Cmd+Plus, Cmd+Minus, Cmd+0, Page Up, and Page Down work too. [hayro](https://github.com/LaurenzV/hayro) draws the pages, so they look the same on every system.
- Code is colored with syntect, in a light or dark theme to match the window.
- Markdown is rendered.
- Zip and tar archives show a table of what's inside.
- Fonts show their alphabet and a sample line drawn in the font.
- MP4, MOV, M4A, M4V, WAV, and FLAC files show their running time.
- PDFs, videos, documents, fonts, and images show the system's thumbnails in the icon view, the gallery strip, and previews. They come from QuickLook on macOS, the shell's thumbnail handlers on Windows, and freedesktop.org thumbnailers on Linux, and are cached in the data folder, up to 256 MB, dropping the least recently used first.

#### Terminal mode

- `fffinder --term` works on Windows, with a build that has the `term` feature. Frames go out as kitty graphics in terminals that have them, such as WezTerm, Rio, and VS Code's terminal, and as sixel images, sending only the cells that changed, in Windows Terminal 1.22 or later.

#### Releases

- Release archives and shell and PowerShell installers for macOS, Linux, and Windows on x86_64 and ARM64, built with dist. Each archive includes the `fff` Nushell module.
- `cargo binstall --git https://github.com/fdncred/fffinder fffinder` installs the release binary for your system instead of building it.

### Changed

- A search made before the index is ready now shows results within 25 ms of a location finishing indexing. It used to retry every 600 ms. The index is rebuilt at every launch, so a search right after launch can still wait a few seconds.
- While an Undo or Redo runs, other file operations wait and say "Wait for Undo to finish".
- Updated GPUI Kit to 0.8.1, and every other dependency to its latest version.
- On macOS, Move to Trash no longer asks for permission to control Finder. fffinder moves items to the Trash itself, as most Mac apps do, so Finder's Put Back doesn't work for them; Put Back in fffinder's Trash view and Undo do.
- The sidebar's Locations load in the background, so a slow or offline drive no longer holds up opening a window. On Windows, fffinder lists drives from the system's own table and asks only local and removable ones for a name, so a mapped drive whose server is offline no longer stalls it.

### Fixed

- `fff files` without a query listed each file added since the index was built twice.

### Known limitations

- On Linux, GPUI can only put text on the clipboard, so files copied in fffinder paste as files only into fffinder.
- Dragging files out to other apps works on macOS and Wayland. GPUI doesn't support it on Windows or X11.
- On macOS, fffinder can put back only items it moved to the Trash, and Finder's own Put Back doesn't work for those.
- On Windows, `fffinder --term` clicks land on whole cells and emoji are one color. In Windows Terminal, which gets sixel images, frames are drawn at 10 by 20 pixels a character cell, so text looks softer than with kitty graphics, and images have at most 256 colors.
- Connect to Server can't mount SFTP on macOS. On Linux it can't ask for a password, so put the user name in the address and keep the password in your keyring.

[0.2.0]: https://github.com/fdncred/fffinder/releases/tag/v0.2.0
