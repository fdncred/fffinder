# Shared helpers for the fffinder build scripts.

const SCRIPTS_DIR = path self | path dirname

# Repository root (the parent of scripts/).
export def repo-root []: nothing -> path {
    $SCRIPTS_DIR | path dirname
}

# Executable suffix for this platform.
export def exe-suffix []: nothing -> string {
    if $nu.os-info.name == 'windows' { '.exe' } else { '' }
}

# Cargo's target directory. Cargo works it out, so CARGO_TARGET_DIR and
# `build.target-dir` in a Cargo config are respected.
export def target-dir []: nothing -> path {
    cd (repo-root)
    ^cargo metadata --format-version 1 --no-deps | from json | get target_directory
}

# Build the workspace; returns the target directory holding the binaries.
# `--term` adds `fffinder --term`, the window drawn inside a terminal.
export def build [--debug, --term]: nothing -> path {
    let args = if $debug { [build --workspace] } else { [build --release --workspace] }
    let args = if $term { $args | append [--features term] } else { $args }
    cd (repo-root)
    ^cargo ...$args
    target-dir | path join (if $debug { 'debug' } else { 'release' })
}

# Render an SVG to a 1024-pixel PNG in `dir` with Quick Look (macOS).
export def render-svg [svg: path, dir: path]: nothing -> path {
    ^qlmanage -t -s 1024 -o $dir $svg | ignore
    $dir | path join $'($svg | path basename).png'
}

# The Linux desktop entry: a menu item, and on Wayland the window icon,
# which the compositor finds through the window's app id, `fffinder`.
export def desktop-entry [exec: string]: nothing -> string {
    # The spec splits Exec on spaces outside double quotes.
    let exec = if ($exec | str contains ' ') { $'"($exec)"' } else { $exec }
    [
        '[Desktop Entry]'
        'Type=Application'
        'Name=fffinder'
        'Comment=Fast fuzzy file search'
        $'Exec=($exec) %f'
        'Icon=fffinder'
        'Terminal=false'
        'Categories=Utility;FileTools;FileManager;'
        'Keywords=find;search;files;fuzzy;'
        'StartupWMClass=fffinder'
    ] | str join (char nl)
}

# Install the desktop entry, starting `exec`, and the icon for this user (Linux).
export def install-desktop-entry [exec: string]: nothing -> nothing {
    let share = $nu.home-dir | path join .local share
    let apps = $share | path join applications
    let icons = $share | path join icons hicolor scalable apps
    mkdir $apps $icons
    cp (repo-root | path join assets app-icon.svg) ($icons | path join fffinder.svg)
    desktop-entry $exec | save --force ($apps | path join fffinder.desktop)
}

# Version from the workspace manifest.
export def version []: nothing -> string {
    open (repo-root | path join Cargo.toml) | get workspace.package.version
}
