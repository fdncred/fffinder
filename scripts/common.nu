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

# Version from the workspace manifest.
export def version []: nothing -> string {
    open (repo-root | path join Cargo.toml) | get workspace.package.version
}
