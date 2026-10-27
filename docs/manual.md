# fffinder manual

This manual describes everything fffinder does, control by control. The [README](../README.md) covers installing it, [Building](#building) lists what each platform needs to build it, [Development](#development) covers testing it, and [protocol.md](protocol.md) documents the local socket that the `fff` Nushell module and other tools use.

Keyboard shortcuts are written with Cmd. On Linux and Windows, use Ctrl instead.

## Contents

- [Starting fffinder](#starting-fffinder)
- [The window](#the-window)
- [Toolbar](#toolbar)
- [Sidebar](#sidebar)
- [Views](#views)
- [Selecting items](#selecting-items)
- [Sorting and hidden files](#sorting-and-hidden-files)
- [Searching](#searching)
- [Recents](#recents)
- [Previews and Quick Look](#previews-and-quick-look)
- [Opening files](#opening-files)
- [Working with files](#working-with-files)
- [Copying and moving](#copying-and-moving)
- [Get Info](#get-info)
- [Undo](#undo)
- [The Trash](#the-trash)
- [Dragging and dropping](#dragging-and-dropping)
- [The Nushell panel](#the-nushell-panel)
- [Filters and Smart Folders](#filters-and-smart-folders)
- [Renaming several items](#renaming-several-items)
- [Nushell actions](#nushell-actions)
- [Using your Nushell environment](#using-your-nushell-environment)
- [Open Nushell Here and terminals](#open-nushell-here-and-terminals)
- [Choosing files for scripts](#choosing-files-for-scripts)
- [In a terminal](#in-a-terminal)
- [Menus](#menus)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Settings](#settings)
- [settings.json](#settingsjson)
- [The fff Nushell module](#the-fff-nushell-module)
- [The control socket](#the-control-socket)
- [Files, folders, and environment variables](#files-folders-and-environment-variables)
- [Appearance](#appearance)
- [Names](#names)
- [Building](#building)
- [Development](#development)

## Starting fffinder

Run `fffinder`, open the app bundle, or use `fff gui` from Nushell.

```text
fffinder [PATH] [OPTIONS]

  [PATH]             Folder to open (default: the current folder with --term,
                     else the last folder or home)
  -s, --startup-path PATH
                     Folder to open, same as [PATH] (also --startup-path=PATH)
  -q, --query TEXT   Start with this search (also --query=TEXT)
      --pick         Choose files and print them as JSON to stdout
      --multiple     With --pick, allow choosing several items (also -m)
      --detach       Start in the background and return right away
      --term         Show the window inside this terminal (kitty graphics)
      --fps          With --term, show frames per second in the status bar
      --software-rendering
                     Draw without a GPU (Linux)
  -h, --help         Show this help
  -V, --version      Show the version
```

- `--startup-path` (`-s`) is another way to give PATH. Give one or the other, not both.
- A PATH that is a file opens its folder.
- With no PATH, the window opens the folder you last had open, or your home folder if that folder is gone. With `--term`, it opens the folder you ran it from.
- If fffinder is already running, a second launch hands its PATH and `--query` to the running copy, which opens a window, and then exits. There is always one app and one index. `--pick` windows are separate and never hand off.
- On macOS, fffinder keeps running after you close its last window, like Finder, so the index stays warm for Nushell. Clicking the Dock icon opens a new window. On Linux and Windows, closing the last window quits.
- `--detach` starts fffinder in the background and returns at once. `fff gui` uses it.
- `--pick` turns the window into a file chooser. See [Choosing files for scripts](#choosing-files-for-scripts).
- `--term` shows the window inside the terminal instead. See [In a terminal](#in-a-terminal).
- `--fps`, with `--term`, shows in the status bar how many frames a second reach the terminal.
- `--software-rendering` draws with Mesa's software renderers instead of a GPU, on Linux. It only applies when it starts the app, not when it hands off to a running copy. See [Without a GPU](#without-a-gpu-wsl2-and-virtual-machines).

Indexing starts as soon as the app starts. The first scan has to finish before "This Computer" searches return results.

macOS asks before an app reads Desktop, Documents, or Downloads. To index those and other protected folders without prompts, give fffinder Full Disk Access in System Settings > Privacy & Security, then reopen it.

On Linux the index watches folders with inotify, one watch per folder, and a big home folder can exceed the default limit. Raise it with `sudo sysctl fs.inotify.max_user_watches=1048576`.

## The window

From top to bottom, the window has:

- The toolbar, with the window title, navigation, view and menu buttons, and the search field.
- The scope bar, only while a search is active.
- The filter bar, while a Nushell filter is open.
- The file area, showing the current view.
- The Nushell panel, while it is open.
- The path bar.
- The status bar, or the picker bar in `--pick` mode.

The sidebar runs down the left side.

The file area shows a message when there is nothing to list:

| Message | When |
| --- | --- |
| Can't open this folder | The folder can't be read. The reason is shown. |
| Indexing… | A search ran before the index was ready. It runs again when indexing finishes. |
| No Results | The search found nothing. |
| No Recent Files | Recents is empty. |
| No Items | The folder is empty. |

Open folders refresh on their own. fffinder checks the folder every two seconds and reloads it quietly when it changed on disk.

### Path bar

The path bar shows the current folder as a row of crumbs, from the volume down. Click a crumb to go to that folder. While searching, in Recents, or when showing results sent from Nushell, the path bar shows the selected item's path instead. Clicking the item's own crumb opens its folder with the item selected.

### Status bar

The status bar shows:

- How many items are shown, or "N of M selected".
- With a filter on, "N of M items pass the filter".
- A `+` after the count when a search stopped at the result limit.
- The time the search took, in milliseconds.
- A spinner while a search or filter runs.
- "Indexing N files…" while the index is being built, when the sidebar is hidden. The sidebar footer shows it otherwise.
- In icon view, a slider for the icon size, from 32 to 128 points.

## Toolbar

From left to right:

| Control | What it does |
| --- | --- |
| Show Sidebar | Appears only when the sidebar is hidden. Shows it again. |
| Title | The folder's name, or `Searching "text"` during a search. On macOS and Linux, drag empty toolbar space to move the window, and double-click it to zoom (maximize on Linux). |
| Back, Forward | Move through the window's history. |
| as Icons, as List, as Columns, as Gallery | Switch the view. |
| Sort and View Options | The sort menu. See [Sorting and hidden files](#sorting-and-hidden-files). |
| Nushell | The Nushell menu. See [Menus](#menus). |
| Actions (…) | File commands and dialogs. See [Menus](#menus). |
| Search Only in "Folder name" | Searches only the folder the window shows, and everything inside it, instead of every search location. Click it again to search This Computer. It's the same choice as the scope bar's, so you can make it before you type. It's off in Recents and other results that aren't one folder. |
| Search field | Search as you type. The clear button ends the search. |
| Minimize, Maximize, Close | Linux only, when the window manager doesn't draw a title bar, as on GNOME with Wayland. The toolbar is then the title bar, and right-clicking empty toolbar space shows the window menu. |

## Sidebar

The sidebar has four sections:

- **Recents** lists files you used recently. See [Recents](#recents).
- **Favorites** starts with the standard folders that exist on the computer (Applications, Desktop, Documents, Downloads, and so on). Add folders with "Add to Sidebar" or "Add Folder to Sidebar", or by dragging folders onto the sidebar.
- **Smart Folders** lists saved searches. It appears once you save one. See [Filters and Smart Folders](#filters-and-smart-folders).
- **Locations** lists your home folder, iCloud Drive or other cloud drives, and mounted volumes, and ends with the **Trash**. See [The Trash](#the-trash).

Click a row to open it. The current folder's row is highlighted.

Right-click a Favorites or Locations row for:

- **Open Nushell Here**: open `nu` in that folder in your terminal.
- **Show in Finder** (Show in Explorer on Windows, Show in File Manager on Linux).
- **Remove from Sidebar**: Favorites only.

Right-click a Smart Folder for **Remove from Sidebar**.

The footer shows "N files indexed", or "Indexing… N" with a spinner while scanning. Its gear button opens Settings. The Hide Sidebar button, or Cmd+Alt+S, hides the sidebar.

## Views

Switch views with the toolbar, the View menu, or Cmd+1 to Cmd+4. The choice is saved.

- **Icons** (Cmd+1) shows a grid. Image files show thumbnails, and other documents show their icon with an extension badge. Change the icon size with the slider in the status bar.
- **List** (Cmd+2) shows Name, Date Modified, Size, and Kind columns. Click a header to sort by it, and click it again to reverse. Search results add a Folder column, or a Match column showing the matching line for a contents search. Rows alternate in color, as in Finder.
- **Columns** (Cmd+3) shows the folder hierarchy as side-by-side columns. Selecting a folder opens it in a new column to the right. Selecting a file shows a preview card with its kind, size, modification date, and location in the last column. Search results and Recents always use the list in this view, as Finder does.
- **Gallery** (Cmd+4) shows a large preview of the selected item above a strip of everything in the folder. Left and Right arrows move along the strip. The gallery always shows an item, so it selects the first one when nothing is selected.

Thumbnails cover PNG, JPEG, GIF, WebP, BMP, ICO, TIFF, and SVG files up to 40 MB.

## Selecting items

| Action | Result |
| --- | --- |
| Click | Select one item. |
| Cmd+click | Add the item to the selection, or remove it. |
| Shift+click | Select the range from the last item clicked. |
| Click one item of a multiple selection | Narrow the selection to it. |
| Double-click | Open the item. |
| Right-click | Select the item, unless it is already selected, and show its menu. |
| Arrow keys | Move the selection. In icon view, all four arrows move in the grid. In columns, Left and Right move between columns. |
| Shift+arrow keys | Extend the selection. |
| Home, End | Go to the first or last item. |
| Cmd+A | Select all. |
| Escape | Clear the selection. With nothing selected, end the search. In `--pick` mode, cancel. |

### Type to select

As in Finder, typing letters while the files have focus selects the first item whose name starts with them. Letters typed within a second of each other add to the prefix. When no name starts with the prefix, the next name after it in alphabetical order is selected, and the last item when the prefix is past every name. In column view, typing selects in the active column. Typing in the search field searches instead.

## Sorting and hidden files

The Sort and View Options menu has:

- **Sort By**: Relevance (only while searching), Name, Date Modified, Size, and Kind. Choosing the current key again reverses the order.
- **Ascending** and **Descending**. Both are disabled for Relevance.
- **Show Hidden Files**.

Name sorting is natural, so `file2` comes before `file10`. Date Modified and Size start with the newest and largest items. Kind sorts by kind, then name.

The folder sort is saved. A search has its own sort, which starts at Relevance and isn't saved.

**Show Hidden Files** (Cmd+Shift+.) shows dot files and other hidden items in folders. It is also in the empty-space menu and the View menu, and in Settings. Searches find hidden files only when the search location is itself a git repository, because that is what fff-search indexes. See [What the index skips](#what-the-index-skips).

## Searching

Type in the search field (Cmd+F) to search. Searches start 50 ms after you stop typing. Return searches right away and moves into the results, selecting the first one. Down Arrow in the search field also moves into the results.

To search one part of the tree, open its folder and turn on **Search Only in "Folder name"**, the folder button left of the search field, before or while you search.

While a search is active, the scope bar shows:

| Control | What it does |
| --- | --- |
| This Computer | Search every search location (set in Settings). |
| "Folder name" | Search only the folder you were in. Folders outside the search locations get a temporary index the first time. |
| Name | Match file and folder names. |
| Contents | Search inside files. Results show the first matching line. Letter case, the largest file searched, and the time limit are set in [Settings › Search engine](#search-engine). |
| Funnel | Open the Nushell filter bar (Cmd+Alt+F). |
| (i) | Show the Search Syntax dialog (Cmd+/). |
| Kind menu | Any Kind, Folder, Document, Image, Movie, Audio, Code, or Archive. Disabled for contents searches. |

### Search syntax

| Type | Meaning |
| --- | --- |
| `budget report` | Fuzzy words, typo tolerant. Every word must match. |
| `*.pdf *.md` | Extensions. Several are ORed. |
| `src/**/*.rs` | A glob against the path inside the search location. |
| `/notes/ todo` | Only paths inside a folder named `notes`. |
| `main src/` | A folder filter written at the end. |
| `git:modified` | Git status: `modified`, `untracked`, `staged`, `clean`. |
| `!*.log !test` | Exclude an extension, glob, folder, or text. |
| `main.rs:42` | A line suffix, ignored for matching. |

Cmd+/ shows this table in the app.

### Ranking and limits

fff-search ranks results, and fffinder re-ranks them so name matches come first, shallow paths come before deep ones, and nothing inside `.app` or `.framework` bundles shows. When a word matches files exactly, typo-only matches are dropped.

A search shows at most `result_limit` items (500 by default; **Results shown** in [Settings › Search engine](#search-engine)). With a filter on, it fetches up to 5,000 so the filter has enough to work with. Opening a file from search results records it, so files you open often rank higher for the same search later.

Search results always show where each item is. Searches can't filter by size or date themselves; use a [Nushell filter](#filters-and-smart-folders).

### What the index skips

fff-search decides what goes into the index, and no setting changes it:

- Hidden files and folders, unless the search location is itself a git repository. This keeps caches and configuration such as `~/.cache` out of a home folder index. Show Hidden Files also applies to search results, but it can't bring back files the index left out.
- Files a repository's `.gitignore` excludes.
- Outside git repositories, build and cache folders such as `node_modules`, `target/debug`, `.cargo/registry`, and, on macOS, `~/Library/Caches` and `~/Library/Application Support`.

A folder outside every search location gets its own temporary index when you search it, so a git repository searched that way includes its hidden files.

## Recents

Recents (Cmd+Shift+F) shows up to 500 files from the search locations, ranked by how often and how recently you opened them in fffinder, then by modification time. It waits for the index to be ready.

## Previews and Quick Look

Previews appear in the gallery, in the column view's last column, and in Quick Look.

| File | Preview |
| --- | --- |
| Images | The image. Formats GPUI can't draw, such as HEIC, RAW, and PSD, show the system's thumbnail of them (see [Thumbnails](#thumbnails)). |
| PDFs, videos, office and iWork documents, EPUB, 3D models | The system's thumbnail: the first page of a PDF or document, a frame of a video. Types the system can't draw show their icon. |
| Video and audio | The running time, in the caption and in the column view's card, for MP4, MOV, M4A, M4V, WAV, and FLAC files. Audio shows its icon with the time under it. |
| Folders | The folder icon and how many items it holds. |
| Zip, `.tar`, `.tar.gz`, `.tgz` | A table of what's inside: each item's path and size, up to 200 items, with the count below. |
| TrueType and OpenType fonts (`.ttf`, `.otf`, `.ttc`) | The family name, the alphabet, digits, and a sample line, drawn in the font itself. |
| Markdown (`.md`, `.markdown`) | Rendered: headings, lists, links, tables, and code blocks. Text can be selected. |
| CSV, TSV, JSON, JSONL, NUON, TOML, YAML, XML, INI, SQLite, XLSX, ODS, EML, ICS, VCF, MessagePack, plist | Opened with Nushell's `open` and shown as a table of up to 200 rows, with "Opened with Nushell" and the row count below. Files over 50 MB are skipped. |
| `.nu` scripts | Colored the way `nu-highlight` colors them with your `color_config`. |
| Code and other text | The first 64 KB, marked "preview truncated" when longer. Code in the languages syntect knows (Rust, Python, JavaScript, C and C++, Go, Java, shell, HTML, CSS, SQL, and dozens more, found by extension, by names like `Makefile`, or by a `#!` line) is colored with a light or dark theme to match the window. Files with lines over 2,000 characters, such as minified JavaScript, stay plain. |
| Anything else | The file's icon. |

Nushell previews give up after 10 seconds and show the icon. They need `nu`; without it, structured files preview as text.

### Thumbnails

The icon view, the gallery strip, and the large previews show the thumbnails the system draws for PDFs, videos, documents, fonts, and images:

| System | Drawn by |
| --- | --- |
| macOS | QuickLook, as in Finder. |
| Windows | The shell's thumbnail handlers, as in Explorer: whatever Explorer shows for the type. |
| Linux | The freedesktop.org thumbnailers your desktop installed (`.thumbnailer` files in the XDG data folders' `thumbnailers` folders, such as `evince-thumbnailer` for PDFs and `ffmpegthumbnailer` for videos), chosen by the file's type from `xdg-mime`. A type with no thumbnailer keeps its icon. |

Each item shows its icon until its thumbnail is ready. Thumbnails are cached as PNGs in `thumbnails/` in the data folder, keyed by the file's path, modification date, and size, so an edited file gets a new one. The folder can be deleted at any time.

### Nushell highlighting

`.nu` previews run `nu-highlight` in a login shell, so your `color_config` applies. Every color comes from it, with one addition: `nu-highlight` leaves comments uncolored, so fffinder draws comments in the theme's secondary (gray) text color.

### Quick Look

Space or Cmd+Y opens Quick Look for the selected item, and pressing either again closes it. Quick Look shows the item's icon and name, a large preview, and its kind, size, and modification date.

The arrow keys move to other items while Quick Look stays open, as in Finder:

- With one item selected, they move the selection the way they do in the view, and Quick Look shows the new item. In the icon grid, Up and Down move by row. In the list and in columns, Left and Right go to the previous and next item, so they step through a folder of images the same way in every view. In columns they stay in the current column.
- With several items selected, they step through just those items, wrapping around at the ends, and the selection stays as it is.

- **Show in Finder** (Show in Explorer, Show in File Manager) reveals the item and keeps Quick Look open.
- **Open** opens the item and closes Quick Look.

### Preview text size

Text, code, and table previews in Quick Look have text size controls at the bottom left:

- The small "A" makes the text one point smaller.
- The large "A" makes it one point bigger.
- The size label, such as "11.5 pt", resets it.

With Quick Look open, Cmd+= (or Cmd++) makes the text bigger, Cmd+- makes it smaller, and Cmd+0 resets it. The size ranges from 8 to 32 points, defaults to 11.5, and is saved as `preview_font_size`. The gallery uses the same size. The column view's preview card stays at 11.5 points. The window's [text size](#appearance-and-text-size) setting multiplies all of these.

## Opening files

**Open** (Return, Cmd+Down, Cmd+O, double-click, or Quick Look's Open button) opens a folder in the window, and opens a file or app with its default application. Opening several files opens each one. Opening a zip, `.tar`, `.tar.gz`, or `.tgz` archive extracts it beside itself instead, as Finder's Archive Utility does; use Open With to hand it to another app.

When no application is registered for a file (checked with LaunchServices on macOS and `xdg-mime` on Linux):

- A file that looks like text opens in your editor instead: Nushell's `buffer_editor` when "Use Nushell's file colors and editor" is on and `buffer_editor` is set, otherwise the system text editor. That is `open -t` on macOS (the default app for plain text), Notepad on Windows, and `$VISUAL` or `$EDITOR` on Linux (a notification says so when neither is set).
- A binary file shows a notification instead.

Windows has its own "Open with" chooser, so there the file goes to Windows as usual.

**Open With** in the item menu lists the apps that can open the selected files, with the default app first and marked "(default)". Choose one to open the files with it. **Other…** picks an app with the file chooser (an app bundle on macOS, a `.desktop` file or a program on Linux). **Always Open With**, for one file, lists the same apps; choosing one makes it the default for every file of that type, then opens the file with it. macOS may ask you to confirm the change. Folders don't get these menus.

The apps come from Launch Services on macOS, and on Linux from the `.desktop` entries in the XDG data folders that list the file's MIME type, with the default from `xdg-mime`. On Linux, an app's `Exec` line decides how the files are passed: `%f` and `%u` open one file per app, `%F` and `%U` all of them at once. On Windows, Open With lists the apps Explorer's Open with menu lists, with the default from the file type's association, and opens the files through the shell the way Explorer does, so Microsoft Store apps work too. **Other…** shows Windows' own "Open with" dialog, which is also where Windows lets you choose a new default, so there is no Always Open With there.

**Open in Editor** (File menu or item menu) opens files in Nushell's editor when that option is on and an editor is set. Otherwise it uses the file's default app, or the system text editor when there is none. Terminal editors open in your chosen terminal. See [Using your Nushell environment](#using-your-nushell-environment).

**Show in Finder** (Show in Explorer on Windows, Show in File Manager on Linux) reveals the selected items in the system file manager.

**Show in Enclosing Folder** appears in the item menu for a single search result. It opens the item's folder in this window with the item selected.

## Working with files

| Command | What it does |
| --- | --- |
| New Folder (Cmd+Shift+N) | Creates "untitled folder" (or "untitled folder 2", …) in the current folder and opens the Rename dialog for it. |
| New File… (Cmd+Alt+N) | Asks for a name ("untitled.txt", with the part before the extension selected) and makes an empty file with it in the current folder. A taken or invalid name is refused. |
| Duplicate (Cmd+D) | Copies each selected item beside itself as "name copy.ext", then "name copy 2.ext", and so on. |
| Make Alias (macOS), Create Shortcut (Windows) | Makes a Finder alias to each selected item beside it, named "name alias", which Finder and other apps follow to the original even after the original moves. On Windows it makes an Explorer shortcut, "name - Shortcut.lnk". Linux has no aliases; use Make Symbolic Link. |
| Make Symbolic Link | Makes a symbolic link to each selected item beside it, named "name link". The link points at the name, so it keeps working when the folder moves. On Windows, making links needs Developer Mode or an administrator. |
| Compress | Puts the selected items in a zip beside them: "name.zip" for one item, "Archive.zip" for several, numbered when the name is taken. Links are stored as links, and permissions and dates are kept. |
| Extract | Unpacks the selected zip, `.tar`, `.tar.gz`, or `.tgz` archives beside themselves. An archive with one item at the top unpacks as that item; one with several unpacks into a folder named after the archive. Names are numbered when taken. Entries that would land outside that folder are skipped. |
| Rename… (F2) | Renames one item in a dialog. Names can't be empty or contain `/` (or `\` and the other characters Windows forbids). With several items selected, opens [batch rename](#renaming-several-items). |
| Move to Trash (Cmd+Backspace, Delete) | Asks "Move "name" to the Trash?" or "Move N items to the Trash?", then moves them. Undo or Put Back restores them. In the Trash, Cmd+Backspace is Put Back. |
| Delete Immediately… (Cmd+Alt+Backspace) | Asks, then deletes the selected items for good, without the Trash. |
| Get Info (Cmd+I) | Shows everything about the selected items, or the folder shown when nothing is selected. See [Get Info](#get-info). |
| Undo (Cmd+Z in the files) | Reverses the latest rename, move, copy, new folder, or move to the Trash. See [Undo](#undo). |
| Copy (Cmd+C in the files) | Copies the selected items, to paste with Cmd+V. See [Copying and moving](#copying-and-moving). |
| Paste (Cmd+V in the files) | Copies the items on the clipboard into the folder shown. |
| Move Items Here (Cmd+Alt+V) | Moves the items on the clipboard into the folder shown. |
| Copy Path (Cmd+Alt+C) | Copies the selected paths, one per line. |
| Copy as Nushell List (Cmd+Shift+C) | Copies the selection as NUON: a quoted string for one path, a list for several. |
| Go to Folder… (Cmd+Shift+G) | Type a path. `~` is your home folder, and relative paths start from the current folder. A folder opens; a file opens its folder with the file selected. |
| Add to Sidebar | Adds a selected folder to Favorites. |
| Add Folder to Sidebar | Adds the current folder to Favorites. |
| Refresh (Cmd+R) | Reloads the folder, columns, and previews, and re-runs the search. |
| Rescan Index | Rebuilds the search index from scratch. |

## Copying and moving

Cmd+C (Edit › Copy, or Copy "name" in the item menu) puts the selected items on the clipboard. Cmd+V (Edit › Paste, or Paste Item in the empty-space menu) copies them into the folder the window shows, and Cmd+Alt+V (Edit › Move Items Here) moves them there instead. Pasting needs a folder, so it does nothing while searching or in Recents.

The clipboard holds the items' paths as text too, one per line, so pasting into a terminal or a text field gives the paths. On macOS and Windows it also holds them as files, so Finder or Explorer and other apps can paste them, and files copied there paste into fffinder. On Linux, files copied in GNOME Files, Dolphin, and other file managers paste into fffinder; GPUI can only put text on the Linux clipboard, so items copied in fffinder paste as files only into fffinder. Anywhere, text that is nothing but paths of existing items (or `file://` addresses), one per line, pastes those items.

Pasting an item into the folder it's already in makes a copy beside it: "name copy.ext", then "name copy 2.ext", and so on.

When items with the same names are already in the folder, a dialog asks what to do with all of them:

| Button | What it does |
| --- | --- |
| Stop | Copies or moves nothing. |
| Skip | Leaves the items that are there, and copies or moves the rest. |
| Keep Both | Gives the new items free names: "name 2.ext", "name 3.ext", and so on. |
| Replace | Removes the items that are there and puts the new ones in their place. The old item is removed only once its replacement is complete. |

A folder can't go inside itself, and an item can't replace the folder it's in. A notification says so.

Copies keep the original's modification date. On macOS, a copy on the same APFS volume is a clone, which is instant and takes no extra space until one of them changes. A move within a volume renames the item; a move to another volume copies it and then removes the original.

A copy, move, Compress, or Extract that takes longer than a moment shows a row above the path bar with the item being copied, a progress bar, and a Stop button. Stop finishes nothing more and removes the item that was half copied. Items already copied stay. When an item can't be copied or moved, a notification names it and says why, and the other items still go.

## Get Info

Get Info (Cmd+I, File › Get Info, or the item and empty-space menus) shows a panel about the selected item, or about the folder the window shows when nothing is selected:

| Field | What it shows |
| --- | --- |
| Kind | The kind, as the list view's Kind column shows it. |
| Size | The size in bytes, and the space it takes on disk. For a folder, everything inside it, added up in the background ("Calculating…" until then), and how many items that is. |
| Where | The folder it's in. |
| Created, Modified, Last opened | Dates. Last opened is the date macOS records when a file is opened, and elsewhere the last access time, which many systems update rarely. |
| Original | Where a symbolic link points. The other fields describe what it points at. |
| Where from | The web addresses a browser recorded for a download: the file's address and the page it came from. macOS keeps them in `kMDItemWhereFroms`, Linux in the `user.xdg.origin.url` and `user.xdg.referrer.url` attributes, and Windows in the file's `Zone.Identifier` stream. |
| Opens with | The app that opens the file by default. |
| Owner, Group | The item's owner, and on macOS and Linux its group. Windows shows the owning account as `DOMAIN\user`. |
| Permissions | On macOS and Linux, Read, Write, and Execute boxes for the owner, the group, and everyone, and the mode they add up to, such as `rw-r--r-- (644)`. Clicking a box changes the item's permissions at once. On Windows, a Read-only box. |

With several items selected, the panel shows how many there are, their kinds, their total size (folders included), and the folder they're in when they share one.

## Undo

Undo (Cmd+Z with the files focused, or Edit › Undo) reverses the latest file operation, then the one before it, and so on, up to the last 100. There is one list for all windows, so Undo in any window reverses the latest operation anywhere. A notification says what was undone, such as "Undid Rename".

| Operation | Undo |
| --- | --- |
| Rename, batch rename | Gives the items their old names. |
| Move, paste with Move Items Here, drag to a folder | Moves the items back. |
| Copy, paste, Duplicate, New Folder, New File, Make Alias, Make Symbolic Link, Compress, Extract | Moves the new items to the Trash, as Finder does. |
| Move to Trash | Puts the items back. |

Undo stops with a notification when it can't finish, for example when an item was moved away again or another item now has its old name. Delete Immediately and Empty Trash can't be undone. In a text field, Cmd+Z undoes typing as usual.

## The Trash

The Trash row at the end of the sidebar's Locations shows what is in the Trash, with the folder each item came from in the Folder column. Drop items on the Trash row to move them to the Trash at once, without the question.

- **Put Back** (Cmd+Backspace, the item menu, or File › Put Back) moves the selected items back where they came from. It stops with a notification when the folder is gone or another item has the name.
- **Delete Immediately…** (Cmd+Alt+Backspace, or the item menu) deletes the selected items for good.
- **Empty…** in the bar above the items, Empty Trash… in the empty-space menu or the fffinder menu, or Cmd+Shift+Backspace, asks, then deletes everything in the Trash for good.

On macOS, fffinder asks Finder to move items to the Trash, so Finder's own Put Back still works for them, and remembers where each one came from in `trash.json` in the data folder. fffinder can put back only items it trashed itself; for others, Put Back is unavailable, and Finder's Put Back still works. The Trash view lists your home folder's Trash, `~/.Trash`, which macOS shows only to apps with Full Disk Access. Without it, the view says so. On Linux and Windows, fffinder uses the system trash (the freedesktop.org Trash, or the Recycle Bin), which records where every item came from, so Put Back works for all of them.

## Dragging and dropping

- Drag items onto a folder in any view, a column, or a place in the sidebar (Favorites and Locations) to move them there. Hold Option (macOS) or Ctrl (Linux and Windows) when you let go to copy them instead. Dragging to another volume copies, as in Finder.
- Drag items onto empty space in a folder to move them into the folder shown.
- Drop files from other apps on a folder, a column, a sidebar place, or the file area to copy them in.
- Name clashes ask first, and long transfers show progress. See [Copying and moving](#copying-and-moving).
- Drag items from any view to another app to hand it the files. This works on macOS and Wayland; GPUI doesn't support dragging out on Windows or X11.
- Drop folders between the rows of the sidebar to add them to Favorites.
- While searching or in Recents, drop a folder on the file area to open it, or a file to open its folder with the file selected.

## The Nushell panel

Ctrl+` (or "Run a Pipeline…" in the Nushell menu, or "Run Nushell Pipeline…" in a context menu) opens the Nushell panel below the files. It runs a pipeline with the selected items as `$in`, or every item shown when nothing is selected.

Each item is a record with `name`, `path`, `type` (`file` or `dir`), `kind`, `size` (a filesize), and `modified` (a date), so pipelines such as `where size > 1mb | sort-by modified` work as they do in Nushell.

- Type a pipeline and press Return or click **Run**.
- **Clear** empties the pipeline and its result, and the examples show again. Clicking it while a pipeline runs drops that pipeline's result.
- Before the first run, and after Clear, the panel lists examples. Click one to fill it in and run it.
- A table result shows with a row number column. Other results show as text. Errors show in red above the result.
- The line at the bottom says what `$in` held and how long the run took.
- Close the panel with its close button or Ctrl+`.

Your [Nushell actions](#nushell-actions) show their results here too.

The text size of the panel is set in Settings. See [Nushell text size](#nushell-text-size).

## Filters and Smart Folders

A filter narrows what the window shows with Nushell. Open the filter bar with Cmd+Alt+F, the funnel button in the scope bar, "Filter with Nushell…" in the Actions menu, or "Filter with Nushell" in the Edit menu. It works on a folder and on search results.

Type a `where` condition:

```nu
size > 10mb and modified > 2024-01-01
```

Or start with `|` to use a whole pipeline, which must keep the `path` column:

```nu
| sort-by size --reverse | first 20
```

The filter runs 400 ms after you stop typing, or right away when you press Return. The status bar counts how many items pass. Errors show in red below the bar. The close button removes the filter.

While searching, the filter bar's **Save** button opens "Save Smart Folder". Name it, and the search, its scope folder, the kind, Name or Contents, and the filter are saved in the sidebar under Smart Folders. Clicking a Smart Folder runs the search again with the filter, so it stays up to date.

## Renaming several items

Select several items and press F2, or choose "Rename N Items…", to rename them with a Nushell closure. The closure gets each item with `name`, `stem`, `ext`, `index` (starting at 1), `path`, `type`, `kind`, `size`, and `modified`, and returns the new name. It may take the item as a parameter (`{|f| $f.name }`) or as `$in`. Text that isn't a closure is used as the body, with the item as `$f`.

Starting points are one click away: **Lowercase**, **Spaces to dashes**, **Number**, **Date first**, and **New extension**.

```nu
{|f| $f.name | str downcase }
{|f| $"($f.index | fill --alignment right --character '0' --width 3) ($f.name)" }
```

The dialog previews every old and new name 300 ms after you stop typing, or when you press Return. It flags names that are empty, contain characters the system forbids, are too long, collide with each other, or already exist. **Rename** is enabled when at least one name changes and nothing is flagged. Renaming happens in two steps through temporary names, so swaps such as `a -> b, b -> a` and case-only changes work. If a step fails, the names that already changed are put back.

Batch rename needs `nu`.

## Nushell actions

Commands exported from `actions.nu` in the settings folder appear under **Nushell Actions** in the item menu and in the toolbar's Nushell menu. Each command's name becomes a menu label (`count lines` becomes "Count Lines"), and the comment above it describes it.

An action receives the selected items as a table on `$in`, with the same columns as the [Nushell panel](#the-nushell-panel), and its result shows in the panel. Running an action fills in the panel's pipeline, so you can see and edit what ran.

The first time, fffinder writes starter actions:

| Action | What it returns |
| --- | --- |
| Count Lines | Lines in each selected text file, most first. Folders and binary files are left out. |
| Total Size | How many files and folders are selected, the files' total size, and the folders' total size. |
| Markdown Links | A `- [name](file:///path)` line per item, with the path percent-encoded so names with spaces, `#`, `(`, or `[` still link. |
| Find Duplicates | Files with the same contents (SHA-256) among the selected files and everything inside the selected folders. Empty files and symbolic links are skipped. |
| Archive | Packs the selection into `Archive <date time>.tar.gz` next to the first item. Items may come from different folders. |

When a new version of fffinder changes the starters, it updates an `actions.nu` you never edited. A file you edited is left alone.

- **Edit Actions…** opens `actions.nu` in your editor, writing the starters first if it is missing.
- **Reload Actions** reads the file again. fffinder also reloads it on its own within a few seconds of a save.
- If the file has an error, the submenu shows it as its first, disabled item.

```nu
# Lines of code per selected file.
export def "count lines" []: table -> table {
    $in | where type == file | each {|f| {name: $f.name, lines: (open --raw $f.path | lines | length)} }
}
```

## Using your Nushell environment

With **Use Nushell's file colors and editor** on in Settings, fffinder reads `$env.LS_COLORS`, `$env.config.buffer_editor`, `$VISUAL`, `$EDITOR`, and `PATH` from a `nu --login` session, so `config.nu` and `env.nu` apply.

- File names are drawn the way `ls` draws them, using Nushell's default colors when `LS_COLORS` isn't set: the color, a background behind the letters (README, LICENSE-style names are black on light yellow in Nushell's defaults), bold, italic, underline, strikethrough, dim, and reverse. A selected item uses the selection colors instead.
- "Open in Editor", "Edit Actions…", and the Open fallback use `buffer_editor`, then `$VISUAL`, then `$EDITOR`.
- Terminal editors (vi, vim, nvim, lvim, hx, helix, nano, pico, micro, kak, joe, ne, mg, emacs, amp, ed, vis, jed, mcedit) open in the terminal you chose for "Open Nushell Here". Others start directly.

Settings shows what was read, or why it couldn't be. With the option off, names use the theme's colors.

## Open Nushell Here and terminals

**Open Nushell Here** opens `nu` in a folder, in a terminal:

- From the item menu, it uses the selected folder, or a file's folder.
- From the empty-space menu or a sidebar row, it uses that folder.
- Cmd+Alt+T ("Open in Nushell" in the Nushell menu) uses the selected item's folder, the current folder, or your home folder.

Choose the terminal in Settings. **Automatic** uses the first of these that is installed:

- macOS: Ghostty, iTerm2, WezTerm, kitty, Alacritty, Rio, then Terminal.
- Windows: Windows Terminal, WezTerm, Alacritty, then a plain console window.
- Linux: `$TERMINAL` if it is set, then Ghostty, WezTerm, kitty, Alacritty, Rio, foot, GNOME Terminal, Konsole, Xfce Terminal, any other terminal found, the system default (`x-terminal-emulator`), and XTerm last.

Terminals are found the way each system records them:

- macOS: the `Info.plist` of every app in the Applications folders and of every app the index finds. An app that adds a "New … Window Here" service and opens shell scripts counts even if fffinder doesn't know it.
- Linux: `.desktop` entries in the `TerminalEmulator` category, programs on `PATH`, and AppImages in the index. Flatpak terminals are skipped, because the sandbox can't see the host's `nu`.
- Windows: the install folders, `PATH`, and the index, which finds portable and Scoop installs.

How each one starts:

- Most terminals start with their own working-directory flag and run `nu`. On macOS they open with `open -na <app> --args …`, so each starts as its own app.
- Terminal and iTerm2 on macOS open a small `.command` script that changes to the folder and runs `nu`.
- Ghostty on macOS opens a new window in the running app through AppleScript. macOS asks once whether fffinder may control Ghostty. If Ghostty isn't running, or you decline, a new Ghostty instance starts with `--working-directory`. fffinder avoids Ghostty's `-e` flag because Ghostty asks for confirmation every time it is used ([ghostty discussion #10203](https://github.com/ghostty-org/ghostty/discussions/10203)).
- **Custom Command** runs your own command. `{dir}` becomes the folder and `{nu}` the path to `nu`, and quotes group words that contain spaces. For example: `open -na WezTerm --args start --cwd {dir} -- {nu}`, `wt.exe -d {dir} {nu}`, or `foot --working-directory={dir} {nu}`.

If the terminal saved in Settings isn't installed, Automatic's choice is used and a notification says so. The choice is saved per operating system. See [settings.json](#settingsjson).

## Choosing files for scripts

`fffinder --pick [PATH] [--multiple] [--query TEXT]` opens a chooser window. The picker bar at the bottom explains what to do and has **Cancel** and **Choose** buttons. "Choose" becomes "Choose N Items" with several selected.

- Return, double-click on a file, **Choose**, or "Choose" or "Choose Items" in the item menu prints a JSON array of absolute paths to stdout and exits 0.
- Double-clicking a folder opens it.
- Escape or **Cancel** exits 1.

`fff pick` in Nushell wraps it.

## In a terminal

`fffinder --term [PATH]` shows the window inside the terminal you run it in, at PATH or else the folder you ran it from. It looks the same as the window: fffinder draws each frame offscreen on the GPU and sends it to the terminal as an image, so text, icons, and previews are the same pixels.

It needs:

- a build with the `term` feature: `nu scripts/install.nu --term`, or `cargo build --release --features term`.
- macOS or Linux.
- a terminal that implements the kitty graphics protocol, such as kitty, Ghostty, WezTerm, or Konsole. It doesn't work inside tmux.

Frames go to the terminal through shared memory when it runs on the same machine, and as compressed escape codes otherwise, such as over SSH. fffinder draws a frame only when something on screen changes, at most about 60 a second, and when the terminal falls behind it skips ahead to the newest frame. `fffinder --term --fps` shows the rate in the status bar: the frames that reached the terminal in the last second, so an idle window reads 0. Where the terminal reports mouse positions in pixels, clicks land where they would in the window; otherwise they land in the middle of a character cell.

What's different from the window:

- The terminal keeps most Cmd shortcuts for itself, so each Cmd shortcut also works with Ctrl: Ctrl+Q quits, Ctrl+F searches, and Ctrl+1 to Ctrl+4 switch views. Menus show the Ctrl form. Ctrl+C copies the selected paths, like Cmd+C; it doesn't stop fffinder.
- There's no menu bar. The item menu and the toolbar menus work as usual.
- Pasting (Cmd+V in most terminals) types into the focused text field.
- There's one window. Ctrl+Q, or closing it, quits and puts the terminal back.
- Files can't be dragged out to other apps, and Settings can't open a folder dialog. Add search locations to [settings.json](#settingsjson) instead.
- It keeps its own index, and doesn't hand off to a running fffinder or answer the [control socket](#the-control-socket).
- `fffinder --term --pick` prints the chosen paths after the terminal is put back, so `fffinder --term --pick | from json` works.

The window fills the terminal. When character cells are at least 24 pixels tall, as they usually are on a Retina screen, it's drawn at 2x, or less, down to 1x, to keep it at least 850 by 470 points; otherwise at 1x. Set `FFFINDER_TERM_SCALE`, such as `1.5`, to choose the scale.

While fffinder has the terminal, anything it would print to stderr goes to `term.log` in the data folder. The log starts with the cell size, the scale, and how frames reach the terminal.

## Menus

### Menu bar

| Menu | Items |
| --- | --- |
| fffinder | Settings… (Cmd+,), Rescan Index, Empty Trash…, Quit fffinder (Cmd+Q) |
| File | New Window (Cmd+N), New Folder, New File… (Cmd+Alt+N), Open, Open in Editor, Quick Look, Get Info (Cmd+I), Rename…, Duplicate (Cmd+D), Make Alias (macOS) or Create Shortcut (Windows), Make Symbolic Link, Compress, Extract, Move to Trash, Put Back, Delete Immediately…, Close Window (Cmd+W) |
| Edit | Undo (text fields, or the latest file operation when the files have focus), Redo, Cut (for text fields), Copy, Paste (text fields, or items when the files have focus), Move Items Here, Copy Path, Copy as Nushell List, Select All Items, Find, Filter with Nushell, Search Syntax… |
| View | as Icons, as List, as Columns, as Gallery, Toggle Sidebar, Show/Hide Hidden Files, Toggle Light/Dark, Refresh |
| Go | Back, Forward, Enclosing Folder, Recents, Home, Trash, Go to Folder… |
| Nushell | Pipeline Panel, Open in Nushell, Copy as Nushell List |

New Window opens a window at the folder you last had open.

### Item menu

Right-click one or more items:

- Choose, or Choose Items (only in `--pick` mode)
- Open
- Open With and Always Open With (files only; see [Opening files](#opening-files))
- Quick Look
- Get Info
- Open in Editor (when a file is selected)
- Show in Enclosing Folder (one search result)
- Show in Finder (Show in Explorer, Show in File Manager)
- Copy "name", or Copy N Items
- Copy Path, or Copy Paths
- Copy as Nushell List
- Open Nushell Here
- Run Nushell Pipeline…
- Nushell Actions (submenu)
- Rename…, or Rename N Items…
- Duplicate
- Make Alias (macOS), or Create Shortcut (Windows)
- Make Symbolic Link
- Compress "name", or Compress N Items
- Extract (when an archive is selected)
- Add to Sidebar (one folder that isn't a favorite)
- Move to Trash

In the Trash, the item menu has Put Back, Quick Look, Show in Finder, Copy Path, and Delete Immediately….

### Empty-space menu

Right-click empty space in a view:

- New Folder and New File… (in a folder)
- Paste Item, or Paste N Items, and Move Item Here, or Move N Items Here (in a folder, when the clipboard holds files)
- Open Nushell Here (in a folder)
- Run Nushell Pipeline…
- Get Info (in a folder)
- Add Folder to Sidebar (when the folder isn't a favorite)
- Show Hidden Files
- Refresh

In the Trash, the empty-space menu has Empty Trash….

### Nushell Actions submenu

Your actions, then **Edit Actions…** and **Reload Actions**. See [Nushell actions](#nushell-actions).

### Toolbar menus

- **Sort and View Options**: Sort By (Relevance, Name, Date Modified, Size, Kind), Ascending, Descending, Show Hidden Files.
- **Nushell**: Open Nushell Here, Run a Pipeline… (or Hide Pipeline Panel when it is open), Nushell Actions, Copy as Nushell List.
- **Actions** (…): New Folder, Quick Look, Rename…, Copy Path, Show in Finder, Add Folder to Sidebar, Move to Trash, Filter with Nushell…, Go to Folder…, Search Syntax…, Settings….

## Keyboard shortcuts

Cmd on macOS, Ctrl elsewhere.

### Anywhere

| Keys | Action |
| --- | --- |
| Cmd+Q | Quit |
| Cmd+, | Settings |
| Cmd+N | New window |

### In the window

| Keys | Action |
| --- | --- |
| Cmd+W | Close the window |
| Cmd+F | Search |
| Cmd+Alt+F | Filter with Nushell |
| Cmd+/ | Search syntax |
| Cmd+[, Alt+Left | Back |
| Cmd+], Alt+Right | Forward |
| Cmd+Up | Enclosing folder |
| Cmd+Shift+H | Home |
| Cmd+Shift+F | Recents |
| Cmd+Shift+G | Go to folder |
| Cmd+1, Cmd+2, Cmd+3, Cmd+4 | Icons, list, columns, gallery |
| Cmd+Shift+. | Show hidden files |
| Cmd+R | Refresh |
| Cmd+Alt+S | Toggle the sidebar |
| Cmd+Shift+N | New folder |
| Cmd+I | Get Info |
| Cmd+Alt+N | New file |
| Cmd+Shift+Backspace | Empty the Trash |
| Cmd+Alt+C | Copy path |
| Cmd+Shift+C | Copy as Nushell list |
| Cmd+Alt+T | Open Nushell here |
| Ctrl+` | Nushell panel (Ctrl on every system) |
| Cmd+Shift+D | Switch between light and dark |

### In the file views

| Keys | Action |
| --- | --- |
| Up, Down, Left, Right | Move the selection |
| Shift+Up, Shift+Down, Shift+Left, Shift+Right | Extend the selection |
| Home, End | First or last item |
| Return, Cmd+Down, Cmd+O | Open |
| Space, Cmd+Y | Quick Look |
| Escape | Clear the selection, end the search, or cancel `--pick` |
| Cmd+A | Select all |
| Cmd+C | Copy the selected items |
| Cmd+V | Paste items into the folder |
| Cmd+Alt+V | Move the copied items into the folder |
| F2 | Rename (several items: batch rename) |
| Cmd+D | Duplicate |
| Cmd+Backspace, Delete | Move to Trash (in the Trash: Put Back) |
| Cmd+Alt+Backspace | Delete immediately |
| Cmd+Z | Undo the latest file operation |
| Letters | Type to select |

### In Quick Look

| Keys | Action |
| --- | --- |
| Space, Cmd+Y | Close Quick Look |
| Up, Down, Left, Right | Previous or next item (see [Quick Look](#quick-look)) |
| Cmd+=, Cmd++ | Bigger preview text |
| Cmd+- | Smaller preview text |
| Cmd+0 | Reset the preview text size |

### In the search field

| Keys | Action |
| --- | --- |
| Return | Search now and move into the results |
| Down | Move into the results |

## Settings

Cmd+, (or the gear in the sidebar footer, or Settings… in the Actions menu) opens Settings. Changes apply and save right away. **Done** closes it.

### Search locations

"This Computer" searches these folders, and their indexes stay live while fffinder runs. Each row shows the folder and its file count, or "indexing…" while it scans. The X button stops indexing that folder.

- **Add Folder…** chooses another folder to index.
- **Index Entire Disk** replaces the list with the whole disk (`/`, or `C:\` on Windows). The first scan takes longer and uses more memory.
- **Reset** goes back to the defaults: your home folder, plus `/Applications` on macOS.
- **Rescan** rebuilds the index.

### Search engine

These are fff-search's options. Each change applies and saves right away.

| Control | What it does | Saved as |
| --- | --- | --- |
| Follow symbolic links | Index what symbolic links point to. | `search.follow_symlinks` |
| Watch folders for changes | Keep the indexes current while fffinder runs. Off, they update on Rescan. | `search.watch` |
| Rank recent git changes higher | In a git repository, files from the last 10 commits rank higher. | `search.git_recency` |
| Index file contents | After scanning, index file contents so content searches skip non-matching files sooner. Uses more memory. | `search.content_index` |
| Keep often-used files in memory | Load the most-opened files into memory for faster content search. | `search.preload_files` |
| Letter case in content search | **Smart** ignores case unless the text has a capital letter; **Match Case**; **Ignore Case**. | `search.case` |
| Results shown | The most items a search lists: 100, 200, 500, 1000, 2000, or 5000. | `result_limit` |
| Content search skips files over | 1 MB to 1000 MB (10 MB by default). | `search.max_file_size_mb` |
| Content search time limit | After this long a content search stops and shows what it found so far, even when that is nothing: None, or 1 to 60 seconds (5 by default). The status bar then says "stopped at the time limit". | `search.time_limit_secs` |
| Search threads | Threads for name searches; Auto uses one per processor core. Content search always uses fff-search's own threads, one per performance core on macOS and one per core elsewhere. | `search.threads` |

Changing one of the five switches rebuilds every index, so the file counts under Search locations start over. The other controls apply to the next search, and a search on screen runs again.

### Appearance and text size

- **System**, **Light**, **Dark**: the appearance. System follows the operating system as it changes.
- **Show hidden files**.
- **Text size**: a small "A" button, a large "A" button, and a percentage label such as "100%" that resets it. It scales everything in the window (text, controls, rows, the sidebar, the toolbar, and dialogs), like zooming, from 80% to 200% in steps of 10%. It also multiplies the [preview text size](#preview-text-size) and the [Nushell text size](#nushell-text-size). It is saved as `ui_scale`, where 1.0 is 100%.

### Nushell

- The path of the `nu` in use, or a note that it wasn't found. fffinder looks at the `nu_path` setting, `PATH`, and the usual install folders (`~/.cargo/bin`, `~/.local/bin`, Homebrew, `/usr/local/bin`, `/usr/bin`, and on Windows `Programs\nu\bin`).
- **Open Nushell Here in**: the terminal menu. It lists Automatic (with the terminal it picks), each terminal found, **Custom Command**, and **Detect Again**, which looks for terminals again. The line below explains the choice and says it is saved for this operating system.
- **Try It** opens Nushell in the current folder with the chosen terminal.
- The custom command field, when Custom Command is chosen.
- **Use Nushell's file colors and editor**, with a line saying what was read.
- **Nushell text size**. See below.
- The path of the settings file.

### Nushell text size

The Nushell text size setting has a small "A" button and a large "A" button that change it by one point, and a size label such as "12 pt" that resets it. It sizes the [Nushell panel](#the-nushell-panel) (the pipeline field, the examples, text results, errors, and tables), so the results of [Nushell actions](#nushell-actions) use it too. It also sizes the action names in the Nushell Actions submenu. It ranges from 8 to 32 points, defaults to 12, and is saved as `nu_font_size`.

## settings.json

Settings live in `settings.json` in the settings folder (see [Files, folders, and environment variables](#files-folders-and-environment-variables)). fffinder writes it shortly after each change, through a temporary file so a crash never leaves it half written. Missing fields use their defaults, and a file that can't be read falls back to defaults.

| Field | Default | Meaning |
| --- | --- | --- |
| `search_roots` | Home, plus `/Applications` on macOS | Folders indexed for "This Computer" searches. |
| `favorites` | The standard folders that exist | Sidebar Favorites, in order. |
| `show_hidden` | `false` | Show hidden files in folders. |
| `view_mode` | `"icons"` | `icons`, `list`, `columns`, or `gallery`. |
| `sort` | `{"key": "name", "ascending": true}` | The folder sort. Keys: `name`, `modified`, `size`, `kind`. |
| `icon_size` | `64` | Icon view size, 32 to 128. |
| `show_sidebar` | `true` | Show the sidebar. |
| `appearance` | `"system"` | `system`, `light`, or `dark`. |
| `kind_filter` | `"any"` | The search Kind menu: `any`, `folder`, `document`, `image`, `movie`, `audio`, `code`, `archive`. |
| `result_limit` | `500` | Maximum results shown for a search. |
| `terminals` | `{}` | The "Open Nushell Here" terminal per system, keyed `macos`, `linux`, `windows`. Each has `app` (a terminal id, or `custom`) and `command` (for `custom`, with `{dir}` and `{nu}`). A missing entry means Automatic. |
| `smart_folders` | `[]` | Saved searches: `name`, `query`, `kind`, `scope` (a folder, or null for every location), `contents`, and `filter`. |
| `use_nu_environment` | `false` | Use Nushell's file colors and editor. |
| `preview_font_size` | `11.5` | Quick Look and gallery preview text size, 8 to 32. |
| `nu_font_size` | `12` | Nushell panel and Nushell Actions text size, 8 to 32. |
| `ui_scale` | `1.0` | Text size for the whole window, 0.8 to 2.0 in steps of 0.1. It also multiplies `preview_font_size` and `nu_font_size`. |
| `search` | see below | fff-search's options from [Settings › Search engine](#search-engine). |
| `terminal` | none | The old single terminal command. It moves into `terminals` when the file is read. |
| `nu_path` | none | Path to `nu`. When unset, fffinder searches `PATH` and the usual install folders. |
| `last_folder` | none | The folder the next window opens at. |

The `search` record:

| Field | Default | Meaning |
| --- | --- | --- |
| `follow_symlinks` | `false` | Follow symbolic links while indexing. |
| `watch` | `true` | Keep indexes current with filesystem watchers. |
| `git_recency` | `true` | Rank files from recent git commits higher. |
| `content_index` | `false` | Index file contents after scanning. |
| `preload_files` | `false` | Keep often-used files in memory for content search. |
| `case` | `"smart"` | Content search case: `smart`, `sensitive`, or `insensitive`. |
| `max_file_size_mb` | `10` | Content search skips bigger files. |
| `time_limit_secs` | `5` | Content search time limit; `0` means none. |
| `threads` | `0` | Name search threads; `0` means one per core. |

Terminal ids: `ghostty`, `iterm2`, `wezterm`, `kitty`, `alacritty`, `rio`, `foot`, `gnome-terminal`, `konsole`, `xfce4-terminal`, `terminal` (macOS Terminal), `xterm`, `windows-terminal`, `x-terminal-emulator`, and `console` (Windows console window). Terminals known only from their metadata use `app:<bundle id>` on macOS and `desktop:<entry>` on Linux.

```json
"terminals": {
  "macos": { "app": "ghostty" },
  "windows": { "app": "windows-terminal" },
  "linux": { "app": "custom", "command": "foot --working-directory={dir} {nu}" }
}
```

## The fff Nushell module

`nu/fff.nu` is a Nushell module. The app bundle includes a copy. Load it from your config:

```nu
use /path/to/fffinder/nu/fff.nu *
fff keybindings
```

Most commands ask the running fffinder over its [control socket](#the-control-socket), so nothing is scanned in the shell and results come from the window's live index. When no window is running they fail with "no fffinder window is running"; `fff gui` starts one. `fff show` starts it for you.

The module finds the app through `$env.FFFINDER_BIN`, then `PATH`, then `/Applications/fffinder.app` and `~/Applications/fffinder.app`, then a release or debug build in the repository.

| Command | What it does |
| --- | --- |
| `fff find ...query` | Name search, ranked as the window ranks it. `--files (-f)`, `--dirs (-d)`, `--kind (-k)` (document, image, movie, audio, code, archive, folder), `--scope (-s) <path>`, `--here`, `--limit (-l)` (default 100). Returns `name`, `type`, `size`, `modified`, `git`, `path`, `score`. |
| `fff files [query]` | Every indexed file, or those whose names fuzzy-match. `--limit (-l)`. Returns `name`, `size`, `modified`, `path`, `root`. |
| `fff dirs [query]` | Every indexed folder, or those that match. `--limit (-l)`. |
| `fff search ...pattern` | Content search; a line matching any pattern counts. `--regex (-r)`, `--fuzzy (-f)`, `--context (-C)`, `--before (-B)`, `--after (-A)`, `--scope (-s)`, `--here`, `--limit (-l)` (default 200). Returns `path`, `line_number`, `column`, `line`, and `context` when asked. |
| `fff status` | The index: locations, file and folder counts, and whether it is scanning. |
| `fff watch [pattern]` | Streams file changes from the index. `--ignore (-i)` globs or path prefixes, and `--timeout (-t)` or `--max-events (-n)`; one of the two is required. |
| `fff selection` | What is selected in the front window. `--all (-a)` returns everything it shows. |
| `fff show` | Shows piped paths, `ls` or `glob` output, or any table with a `path` or `name` column in the window, like a search. `--title (-t)` names the results. |
| `fff pick ...query` | Chooses files in a `--pick` window and returns the path, or nothing when cancelled. `--root (-r)` opens another folder, and `--multiple (-m)` allows several and always returns a list. |
| `fff gui [path]` | Opens a window at a folder (default: the current one). `--query (-q)` starts a search. |
| `fff open ...query` | Finds the best document match and opens it as structured data. |
| `fff insert ...query` | Picks files and inserts them at the cursor as NUON. |
| `fff cd ...query` | Picks a folder, or a file's folder, and changes to it. |
| `fff edit ...query` | Picks a file and opens it in `$env.config.buffer_editor`, `$VISUAL`, `$EDITOR`, or `vi` (`notepad` on Windows). |
| `fff keybindings` | Adds key bindings in emacs and vi modes: Ctrl+T runs `fff insert`, and Alt+C runs `fff cd`. `--insert-key` and `--cd-key` change the letters. |

## The control socket

The control socket is experimental. Its endpoints may change between versions.

A running fffinder answers requests about its index, and requests to show things, over a local connection:

- macOS and Linux: a Unix socket at `run/fffinder.sock` in the data folder, readable only by you.
- Windows: `127.0.0.1` on a random port, with a random token required on every request.

`run/server.json` in the data folder says where to connect. The endpoints are plain HTTP with JSON, so Nushell can call them directly:

```nu
let sock = ($nu.data-dir | path dirname | path join fffinder run fffinder.sock)
http get --unix-socket $sock 'http://localhost/v1/find?query=readme&limit=5'
```

[protocol.md](protocol.md) documents every endpoint: status, find, files, dirs, search, watch, selection, show, and open. A second launch of fffinder uses `POST /v1/open` to hand off to the running copy.

## Files, folders, and environment variables

| Folder | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Settings | `~/Library/Application Support/fffinder` | `~/.config/fffinder` | `%APPDATA%\fffinder` |
| Data | `~/Library/Application Support/fffinder` | `$XDG_DATA_HOME/fffinder`, else `~/.local/share/fffinder` | `%LOCALAPPDATA%\fffinder` |

The settings folder holds `settings.json` and `actions.nu`. The data folder holds:

- `run/`: the socket and `server.json`, removed on a normal quit.
- `gui/`: the frecency and search-history databases that rank Recents and repeat searches.
- `icons/v2/`: Finder icons cached as PNGs on macOS, one per extension and one per app or special folder.
- `trash.json`: on macOS, where each item fffinder moved to the Trash came from, for Put Back.
- `thumbnails/`: thumbnails of PDFs, videos, and documents, as PNGs.
- `hang-<time>.txt`: hang reports, when the watchdog is on.
- `term.log`: stderr from the last `fffinder --term`.

| Variable | Effect |
| --- | --- |
| `FFFINDER_CONFIG_DIR` | Use another settings folder. |
| `FFFINDER_DATA_DIR` | Use another data folder, for the app and for the `fff` module. |
| `FFFINDER_WATCHDOG=1` | When the window stops responding for three seconds, save every thread's stack to `hang-<time>.txt` in the data folder (with `sample` on macOS) and print where. Other systems print the process ID to attach a debugger to. |
| `FFFINDER_BIN` | Where the `fff` module finds the app. |
| `FFFINDER_SOFTWARE_RENDERING` | On Linux, `1` draws with Mesa's software renderers and skips the GPU drivers, like `--software-rendering`, and `0` never does, even with no GPU device. See [Without a GPU](#without-a-gpu-wsl2-and-virtual-machines). |
| `FFFINDER_TERM_SCALE` | With `--term`, device pixels per point, such as `2` or `1.5`. See [In a terminal](#in-a-terminal). |
| `TERMINAL` | On Linux, the terminal Automatic prefers. |

The app used to be called fff-Finder. The variables still work with the old prefix, `FFF_FINDER_`. The first time fffinder starts, it moves its folders from `fff-Finder` to `fffinder`, unless you set the folder variables above.

## Appearance

fffinder has light and dark themes, chosen in Settings or switched with Cmd+Shift+D. With System, it follows the operating system as it changes.

On macOS it looks like Finder:

- File and folder icons are the ones Finder shows, drawn by macOS. Special folders and apps get their own icons. New icons fill in over a few frames instead of making the window wait.
- The sidebar uses Finder's sidebar icons, tinted with the accent color.
- The colors were measured from macOS's own light and dark appearances.
- The accent and selection colors follow System Settings. fffinder reads them when it starts, so restart it after changing them.

Linux and Windows use the same colors with fffinder's own icons.

## Names

The app, its bundle, and its packages are named fffinder, all lowercase: the packages are `fffinder` and `fffinder-core`, which Rust code calls `fffinder` and `fffinder_core`. Environment variables are uppercase, as `FFFINDER_*`.

## Building

You need Rust 1.92 or newer, Nushell 0.116 for the scripts, and each platform's build tools and libraries below.

### macOS

Install the Xcode Command Line Tools with `xcode-select --install`. Full Xcode isn't needed. `scripts/bundle.nu` also uses `qlmanage`, `sips`, `iconutil`, and `codesign`, which come with macOS.

### Linux

On Debian and Ubuntu, these are the packages CI builds with on Ubuntu 24.04:

```sh
sudo apt install gcc g++ clang pkg-config libfontconfig-dev libwayland-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan1
```

On Fedora and Arch, these should be the same packages, but nobody has tested them yet:

```sh
sudo dnf install gcc gcc-c++ clang pkgconf-pkg-config fontconfig-devel wayland-devel \
  libxkbcommon-x11-devel libX11-devel libxcb-devel openssl-devel libzstd-devel vulkan-loader
sudo pacman -S --needed base-devel clang fontconfig wayland libxkbcommon-x11 libx11 \
  libxcb openssl zstd vulkan-icd-loader
```

To run fffinder you also need a Vulkan or OpenGL driver. Mesa's drivers (`mesa-vulkan-drivers` on Debian, Ubuntu, and Fedora; `vulkan-radeon` or `vulkan-intel` on Arch) cover AMD and Intel GPUs, and NVIDIA's driver brings its own. Desktop installs usually have one already.

### Windows

Install the Visual Studio 2022 Build Tools with the "Desktop development with C++" workload, which includes the MSVC compiler and the Windows SDK. The build uses the SDK's resource compiler, `rc.exe`, to put the app icon into `fffinder.exe`, so Explorer, the taskbar, and the title bar show it.

### Scripts

| Script | What it does |
| --- | --- |
| `scripts/build.nu` | Builds the workspace. `--debug` skips optimizations, `--test` runs the tests, `--lint` runs clippy, and `--term` adds terminal mode. |
| `scripts/install.nu` | Builds a release and copies `fffinder` to `~/.cargo/bin`, or to `--dest`. `--term` adds terminal mode. |
| `scripts/bundle.nu` | Packages the app into `dist/` for this platform. `--skip-build` packages the last release build, `--install` installs the Linux desktop entry, and `--term` adds terminal mode. |
| `scripts/screenshots.nu` | Renders `docs/images` (macOS). |
| `scripts/windows-icon.nu` | Renders `assets/app-icon.svg` into `assets/windows/fffinder.ico`, the icon the Windows build embeds (macOS). Run it after changing the SVG. |

The scripts ask Cargo where it builds, so `CARGO_TARGET_DIR` and `build.target-dir` in a Cargo config work. When the app isn't installed, the `fff` module looks for a build in `$env.CARGO_TARGET_DIR`, then in `target/`.

### Without a GPU (WSL2 and virtual machines)

On Linux, fffinder draws with the first graphics driver that works, trying GPUs before Mesa's software renderers (llvmpipe for OpenGL, lavapipe for Vulkan), which run on the CPU. With no usable GPU, as in WSL2 without GPU support and in many virtual machines, it ends up drawing in software.

On the way, Mesa can print a warning for each hardware driver it tried, such as `libEGL warning: failed to get driver for name fd -1`, `MESA-LOADER: failed to retrieve device information`, and `MESA: error: ZINK: failed to choose pdev`. If the window opens and draws, they're harmless.

fffinder goes straight to the software renderers, skipping the hardware drivers and their warnings, when:

- you pass `--software-rendering`, or set `FFFINDER_SOFTWARE_RENDERING=1`.
- the system shows no GPU device at all: nothing in `/dev/dri`, no `/dev/dxg` (WSL's GPU), and no `/dev/nvidia*`. No hardware driver could work then. `FFFINDER_SOFTWARE_RENDERING=0` turns this off.

It doesn't switch on its own just because it runs in WSL2 or a virtual machine, since many of those have a working GPU: WSLg passes the Windows GPU through `/dev/dxg`, and virtual machines often have a 3D-capable virtual GPU. When a GPU device exists but no driver for it works, fffinder falls back to software by itself and prints one line on stderr that says so and suggests the flag:

```text
fffinder: no GPU driver worked; drawing in software with llvmpipe (…). Pass --software-rendering …
```

Software rendering sets Mesa's `LIBGL_ALWAYS_SOFTWARE=1` and the Vulkan loader's `VK_LOADER_DRIVERS_SELECT=*lvp*` (Vulkan loader 1.3.234 or newer), unless you've set them yourself. In WSLg that also stops it from using the Windows GPU. It needs Mesa's software drivers: `libgl1-mesa-dri` and `mesa-vulkan-drivers` on Debian and Ubuntu. To make it stick for the desktop entry, change its `Exec` line to `fffinder --software-rendering %f`.

## Development

```nu
nu scripts/build.nu --test --lint    # build, test, clippy
nu scripts/screenshots.nu            # regenerate docs/images (macOS)
cargo run -- ~/src --query readme    # run from source
```

### Tests

The UI tests open the real window headlessly and use it with mouse clicks and key presses:

- `tests/controls.rs` clicks every control and checks what each click changed: toolbar buttons and menus, the search field and scope bar, the filter bar, sidebar rows and Smart Folders, the path bar, the icon-size slider, list headers, items in all four views, context menus and Nushell actions, every button and field in every dialog (batch rename included), the Nushell panel, and the picker bar.
- `tests/finder.rs` presses every keyboard shortcut except Quit and New Window, which belong to the app rather than the window, and types to select.
- `tests/server.rs` talks to the socket server, and `tests/nu_module.rs` runs `fff.nu` in a real `nu` against it.

A step that runs longer than 20 seconds fails the run and names the step, so a hang shows up as a failure instead of a `cargo test` that never ends. Opening a file or showing it in Finder is recorded instead of performed, and nothing is moved to the Trash.

### Example tools

`examples/drive.rs` replays the same kind of steps in the visible window against your real index and prints the window's state after each one. It never writes your settings.

```nu
cargo run --example drive -- key:cmd-, click:show-hidden key:escape key:cmd-shift-g 'type:~/src' key:enter
```

`examples/screenshot.rs` renders the window to a PNG with Metal, so a UI change can be checked without opening it. `examples/macos_colors.rs` prints the macOS colors the themes are based on, in light and dark.

`examples/terminals.rs` lists the terminals found and what Automatic picks. With `--verify` it opens each one with a short script in place of `nu` that records the folder it started in, then closes it. Terminal and iTerm2 keep their windows open, so it checks them only when you name them.

```nu
cargo run --example terminals -- --verify
```

### Hangs

If the window ever stops responding, run it with `FFFINDER_WATCHDOG=1`. After three seconds without a response it saves every thread's stack to `hang-<time>.txt` in the data folder and prints the path. Other platforms print the process ID to attach a debugger to.
