//! List the terminals "Open Nushell Here" can use, and check that each one
//! really starts in the folder it was given.
//!
//! ```text
//! cargo run --example terminals                    # what was found, and what Automatic picks
//! cargo run --example terminals -- --index         # also search the index (waits for the scan)
//! cargo run --example terminals -- --verify        # open each terminal, then close it
//! cargo run --example terminals -- --verify kitty  # just these
//! ```
//!
//! `--verify` runs a small script in place of `nu` that writes its working
//! directory to a file and exits, then stops what it started. Terminal and
//! iTerm2 keep their window open afterwards, so they are checked only when
//! named.

use fffinder_core::terminals::{self, Launch, Terminal};
use fffinder_core::{Config, Engine, EngineOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let use_index = args.iter().any(|a| a == "--index");
    let verify = args.iter().any(|a| a == "--verify");
    let only: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .collect();

    let engine = use_index.then(|| {
        let config = Config::load();
        let engine = Engine::new(EngineOptions {
            app_dirs: config.app_dirs(),
            roots: config.search_roots,
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        engine.wait_ready(Duration::from_secs(300));
        engine
    });
    let started = Instant::now();
    let found = terminals::detect(engine.as_deref());
    println!("found {} in {:?}:", found.len(), started.elapsed());
    for t in &found {
        let place = t
            .location
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        println!("  {:<28} {:<24} {place}", t.id, t.name);
    }
    let env_terminal = std::env::var("TERMINAL").ok();
    if let Some(auto) = terminals::automatic(&found, env_terminal.as_deref()) {
        println!("Automatic picks {}", auto.name);
    }
    if !verify {
        return;
    }

    let work = std::env::temp_dir().join(format!("fff-terminal-check-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("temp folder");
    let marker = write_marker(&work);
    let mut failed = 0;
    for t in &found {
        let script_based = matches!(t.launch, Launch::CommandFile { .. });
        let wanted = if only.is_empty() {
            !script_based && t.id != "console"
        } else {
            only.contains(&t.id.as_str())
        };
        if !wanted {
            continue;
        }
        match check(t, &work, &marker) {
            Ok(elapsed) => println!(
                "ok    {:<20} started in the folder after {elapsed:?}",
                t.name
            ),
            Err(why) => {
                failed += 1;
                println!("FAIL  {:<20} {why}", t.name);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

/// A stand-in for `nu`: record the working directory, linger a moment, exit.
fn write_marker(work: &Path) -> PathBuf {
    if cfg!(windows) {
        let path = work.join("marker.cmd");
        std::fs::write(
            &path,
            "@echo off\r\ncd > started.txt\r\ntimeout /t 2 >nul\r\n",
        )
        .unwrap();
        path
    } else {
        let path = work.join("marker.sh");
        std::fs::write(&path, "#!/bin/sh\npwd -P > started.txt\nsleep 2\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
}

fn check(t: &Terminal, work: &Path, marker: &Path) -> Result<Duration, String> {
    let dir = work.join(format!("{} folder", t.id.replace(':', "-")));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let plan = t.plan(marker, &dir);
    let started = Instant::now();
    plan.run().map_err(|e| format!("could not start: {e}"))?;
    let out = dir.join("started.txt");
    let result = loop {
        if let Ok(text) = std::fs::read_to_string(&out) {
            let got = PathBuf::from(text.trim());
            let want = dir.canonicalize().unwrap_or(dir.clone());
            break if got == want {
                Ok(started.elapsed())
            } else {
                Err(format!("started in {} instead", got.display()))
            };
        }
        if started.elapsed() > Duration::from_secs(15) {
            break Err("nothing ran within 15 s".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // Stop only what this check started: processes running the marker.
    std::thread::sleep(Duration::from_millis(500));
    if cfg!(unix) {
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg(marker)
            .status();
    }
    result
}
