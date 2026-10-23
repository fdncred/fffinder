#!/usr/bin/env nu
# Build fffinder, and optionally test and lint it.

use common.nu *

def main [
    --debug   # Build without optimizations
    --test    # Also run `cargo test --workspace`
    --lint    # Also run clippy with warnings as errors
    --term    # Include terminal mode, `fffinder --term`
] {
    let target = build --debug=$debug --term=$term
    cd (repo-root)
    let features = if $term { [--features term] } else { [] }
    if $lint { ^cargo clippy --workspace --all-targets ...$features -- -D warnings }
    if $test { ^cargo test --workspace ...$features }
    let binary = $target | path join $'fffinder(exe-suffix)'
    {binary: $binary, size: (ls $binary | get 0.size)}
}
