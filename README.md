<img src="assets/app-icon.svg" alt="fffinder icon" width="80" height="80">

# fffinder

A Finder-style file browser with instant fuzzy search across your whole computer, written in Rust. Search uses [fff-search](https://crates.io/crates/fff-search), the UI is built with [GPUI Kit](https://github.com/longbridge/gpui-kit), and Nushell is built in. Your shell can query the index the app keeps live, and the app runs Nushell on the files you select.

This README is an overview. [The manual](docs/manual.md) documents every view, menu, shortcut, setting, and command.

![Icon view in dark mode](docs/images/icons-dark.png)

## What it does

- Indexes your home folder (and `/Applications` on macOS) in the background and keeps the index live. On the development Mac that's about 670,000 files in 4 to 6 seconds, and a search over all of them returns in 30 to 110 ms.
- Searches by name with typo tolerance, or by file contents. You can narrow a search to the current folder, a kind, an extension, a glob, or a git status.
- Browses folders as icons, a list, columns, or a gallery, with a sidebar, back and forward history, and a clickable path bar.
- Previews files in the gallery, the column view, and Quick Look (Space). CSV, JSON, TOML, YAML, SQLite and other structured files open through Nushell's `open` and show as tables.
- Handles everyday file work: open, open in your editor, show in Finder, copy paths, rename one item or many, new folder, move to Trash, and drag files out to other apps.
- Has light and dark themes that follow the system, and a text size setting for the whole window.
- Runs inside a terminal with `fffinder --term`, drawn with the kitty graphics protocol so it looks the same as the window. It needs a build with `--features term` and a terminal like kitty, Ghostty, or WezTerm.

![Search results](docs/images/search-dark.png)

## Nushell integration

### The `fff` module

`nu/fff.nu` lets your shell search the running app's index, so nothing is scanned in the shell. Load it from your config:

```nu
use /path/to/fffinder/nu/fff.nu *
fff keybindings      # Ctrl+T inserts picked paths, Alt+C picks a folder to cd into
```

```nu
fff find budget --kind document        # name search, ranked like the window
fff search TODO FIXME --context 1      # content search
fff selection | get path               # what's selected in the front window
ls **/*.log | where size > 10mb | fff show --title 'Big logs'
fff pick *.csv | open $in              # choose a file in the window, use it here
```

The manual lists [every `fff` command](docs/manual.md#the-fff-nushell-module).

### Inside the app

- **The Nushell panel** (Ctrl+`) runs a pipeline with the selected files as `$in`. Each row has `name`, `path`, `type`, `kind`, `size`, and `modified`, so `where size > 1mb | sort-by modified` works as you'd expect.
- **Filters** (Cmd+Alt+F) narrow a folder or search results with a `where` condition such as `size > 10mb and modified > 2024-01-01`. Save a search and its filter in the sidebar as a Smart Folder.
- **Batch rename** (F2 with several items selected) renames with a closure, such as `{|f| $f.name | str downcase }`, and previews every new name first.
- **Your own actions.** Commands you export from `actions.nu` in the settings folder appear in the context menu and get the selection on `$in`. The first run writes a few starters, such as Count Lines and Find Duplicates.
- **Your Nushell environment.** With "Use Nushell's file colors and editor" on in Settings, file names use your `LS_COLORS` and "Open in Editor" uses your `buffer_editor`.
- **Open Nushell Here** (Cmd+Alt+T) opens `nu` in the folder, in the terminal you pick in Settings.
- **`fffinder --pick`** turns the window into a file chooser for scripts and prints the chosen paths as JSON.

![Nushell pipeline panel](docs/images/nushell-dark.png)

### The local socket (experimental)

The `fff` module talks to the app over a local socket, and your own tools can too. The endpoints are plain HTTP with JSON. They're experimental and may change between versions. On macOS and Linux the socket is `run/fffinder.sock` in the data folder, and only your user can open it. Windows uses 127.0.0.1 with a random token.

```nu
let sock = ($nu.data-dir | path dirname | path join fffinder run fffinder.sock)
http get --unix-socket $sock 'http://localhost/v1/find?query=readme&limit=5'
```

[docs/protocol.md](docs/protocol.md) describes every endpoint.

## Install

You need Rust 1.92 or newer and Nushell 0.116, plus:

- macOS: Xcode Command Line Tools (`xcode-select --install`).
- Linux: `gcc g++ clang pkg-config libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev`, plus a Vulkan driver.
- Windows: Visual Studio 2022 Build Tools with the C++ workload.

[Building](docs/manual.md#building) in the manual has the Fedora and Arch packages, and covers WSL2 and virtual machines without a GPU.

```nu
nu scripts/install.nu            # release build into ~/.cargo/bin, or --dest ~/bin
nu scripts/install.nu --term     # also `fffinder --term`, the window inside a terminal
nu scripts/bundle.nu             # or package it: dist/fffinder.app on macOS, a .desktop folder on Linux
nu scripts/bundle.nu --install   # Linux: also install the desktop entry
```

`install.nu` prints the `use` line that loads the `fff` module.

On macOS, give fffinder Full Disk Access in System Settings > Privacy & Security, then reopen it, so it can index Desktop, Documents, Downloads, and other protected folders without asking. On Linux, a big home folder can use up the inotify watch limit. Raise it with `sudo sysctl fs.inotify.max_user_watches=1048576`.

## Using it

### Search syntax

| Type            | Meaning                                                 |
| --------------- | ------------------------------------------------------- |
| `budget report` | Fuzzy words, typo tolerant. Every word must match.      |
| `*.pdf *.md`    | Extensions. Several are ORed.                           |
| `src/**/*.rs`   | A glob against the path inside the search location.     |
| `/notes/ todo`  | Only paths inside a folder named `notes`.               |
| `git:modified`  | Git status: `modified`, `untracked`, `staged`, `clean`. |
| `!*.log !test`  | Exclude an extension, glob, folder, or text.            |

Cmd+/ shows the full syntax in the app.

### Keyboard

Cmd on macOS, Ctrl elsewhere. The manual lists [every shortcut](docs/manual.md#keyboard-shortcuts).

| Keys           | Action                        |
| -------------- | ----------------------------- |
| Cmd+F          | Search                        |
| Cmd+Alt+F      | Filter with Nushell           |
| Cmd+1 to Cmd+4 | Icons, list, columns, gallery |
| Space          | Quick Look                    |
| Cmd+Shift+G    | Go to folder                  |
| F2             | Rename                        |
| Ctrl+`         | Nushell panel                 |
| Cmd+Alt+T      | Open Nushell here             |
| Cmd+,          | Settings                      |

### Settings

In Settings you choose the search locations (or the entire disk), fff-search's options, the appearance, text sizes, and the terminal for Open Nushell Here. Everything saves to `settings.json` in `~/Library/Application Support/fffinder` on macOS, `~/.config/fffinder` on Linux, or `%APPDATA%\fffinder` on Windows. The manual covers [each setting](docs/manual.md#settings) and [every field](docs/manual.md#settingsjson).

## How it's built

```text
crates/fffinder-core   search engine wrapper, file model, listing, places, settings, terminals
src/                     the GPUI Kit app (src/finder/ holds the window and its views)
src/server/              the local socket server and its client
src/macos.rs             macOS integration through AppKit
nu/fff.nu                the `fff` Nushell module
assets/actions.nu        the starter Nushell actions
docs/                    the manual, the socket protocol, and screenshots
scripts/*.nu             build, install, bundle, screenshots, Windows icon
examples/                headless screenshots, a window driver, and diagnostics
tests/                   UI, socket, and `fff` module tests
```

## Development

```nu
nu scripts/build.nu --test --lint    # build, test, clippy
nu scripts/screenshots.nu            # regenerate docs/images (macOS)
cargo run -- ~/src --query readme    # run from source
```

The UI tests open the real window headlessly, click every control, and press its keyboard shortcuts. The manual's [Development](docs/manual.md#development) section covers the tests, the example tools, and the hang watchdog.

## Limits

- fff-search skips hidden files unless the search location is itself a git repository. It also skips files a repository's `.gitignore` excludes, and outside git repositories it skips build and cache folders such as `node_modules` and `~/Library/Caches`. See [What the index skips](docs/manual.md#what-the-index-skips).
- Searches can't filter by size or date. A Nushell filter can, on the results a search returns.
- The first scan has to finish before searches return results. The search runs again on its own when the scan is done.
- Only the macOS build has been run and tested so far. CI builds and tests Linux and Windows too, but expect rough edges there until they get hands-on use.
