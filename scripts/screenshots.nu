#!/usr/bin/env nu
# Render the README screenshots from a generated demo folder (macOS, Metal).
#
#   nu scripts/screenshots.nu            # writes docs/images/*.png

use common.nu *

def main [
    --out: path  # Output directory (default: docs/images)
] {
    let root = repo-root
    let out = $out | default ($root | path join docs images) | path expand
    mkdir $out
    let demo = make-demo
    cd $root
    ^cargo build --example screenshot
    let tool = target-dir | path join debug examples screenshot
    let home = $nu.home-dir
    let shots = [
        {file: icons-dark.png, args: [--view icons --dark]}
        {file: list-light.png, args: [--view list --light --select 1]}
        {file: search-dark.png, args: [--view list --dark --scope-folder --query proj]}
        {file: gallery-dark.png, args: [--view gallery --dark --select 1]}
        {file: quicklook-dark.png, args: [--view icons --dark --select 2 --quicklook]}
        {file: nushell-dark.png, args: [--view list --dark --nu 'where type == file | select name size kind | sort-by size --reverse']}
        # The demo is a Favorite here, so the tree opens it there rather
        # than under the temporary folder it really lives in.
        {
            file: tree-dark.png
            dir: ($demo | path join Projects)
            args: [--view list --dark]
            settings: {
                sidebar_mode: tree
                favorites: [/Applications $demo ($home | path join Documents) ($home | path join Downloads)]
            }
        }
    ]
    let base = $demo | path dirname
    let written = $shots | each {|shot|
        let file = $out | path join $shot.file
        # Each shot starts from the default settings plus its own, so the
        # pictures don't depend on how this computer is set up.
        let config = $base | path join config ($shot.file | path parse | get stem)
        mkdir $config
        $shot.settings? | default {} | save ($config | path join settings.json)
        with-env {FFFINDER_CONFIG_DIR: $config, FFFINDER_DATA_DIR: ($base | path join data)} {
            ^$tool $file --dir ($shot.dir? | default $demo) ...$shot.args | ignore
        }
        $file
    }
    rm --recursive --force $base
    $written
}

# A small, made-up project folder, so screenshots show no personal files.
def make-demo []: nothing -> path {
    let base = mktemp --directory
    let demo = $base | path join 'Studio'
    mkdir $demo
    for dir in [Assets 'Brand guidelines' Exports Product Research Projects] {
        mkdir ($demo | path join $dir)
    }
    [[client city revenue signed]; [Acme Lisbon 120000 2026-01-04] [Globex Austin 98000 2026-03-19] [Initech Berlin 143500 2025-11-02] [Umbrella Tokyo 77250 2026-07-30]]
    | save ($demo | path join Budget.csv)
    {primary: '#0A84FF', accent: '#FFD60A', background: '#1D2633'} | save ($demo | path join Colors.json)
    "# Launch checklist\n\n- [x] Index the disk\n- [ ] Ship the socket\n" | save ($demo | path join 'Launch checklist.md')
    "Meeting notes\n\n* Search is instant\n* Nushell pipelines on selections\n" | save ($demo | path join 'Meeting notes.txt')
    "Project brief: a Finder-style search app in Rust.\n" | save ($demo | path join 'Project brief.txt')
    "# Studio\n\nDemo folder for fffinder screenshots.\n" | save ($demo | path join README.md)
    "# Roadmap\n\n1. Fuzzy search\n2. Nushell everywhere\n" | save ($demo | path join Roadmap.md)
    "Project Atlas\n" | save ($demo | path join Projects 'Project Atlas.txt')
    "Project Borealis\n" | save ($demo | path join Projects 'Project Borealis.md')
    # Folders inside folders, for the sidebar's tree.
    for dir in [Atlas Borealis] {
        mkdir ($demo | path join Projects $dir Drafts)
        "Kickoff\n" | save ($demo | path join Projects $dir 'Kickoff.md')
    }
    cp (repo-root | path join assets app-icon.svg) ($demo | path join Assets Logo.svg)
    $demo
}
