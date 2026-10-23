#!/usr/bin/env nu
# Build and install fffinder.
#
#   nu scripts/install.nu                 # release build into ~/.cargo/bin
#   nu scripts/install.nu --dest ~/bin    # somewhere else on PATH
#   nu scripts/install.nu --term          # with `fffinder --term` (macOS and Linux)
#
# Then add `use <repo>/nu/fff.nu *` to config.nu for `fff pick`, `fff show`,
# `fff find`, and the rest, and `fff keybindings` for Ctrl+T and Alt+C.

use common.nu *

def main [
    --dest: path        # Install directory (default: ~/.cargo/bin)
    --debug             # Install a debug build
    --term              # Include terminal mode, `fffinder --term`
] {
    let target = build --debug=$debug --term=$term
    let dest = $dest | default ($nu.home-dir | path join .cargo bin) | path expand
    mkdir $dest
    let file = $'fffinder(exe-suffix)'
    cp --force ($target | path join $file) $dest
    let module = repo-root | path join nu fff.nu
    print $"Nushell commands: add `use ($module) *` and `fff keybindings` to config.nu."
    $dest | path join $file
}
