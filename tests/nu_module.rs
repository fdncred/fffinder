//! `nu/fff.nu` against a real server on a fixture index, run in a real
//! `nu`. Skipped when `nu` isn't installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use fffinder::server::{self, Options, UiRequest};
use fffinder_core::{Engine, EngineOptions};
use serde_json::{Value, json};

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!("fff-nu-module-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tree/Projects/atlas")).unwrap();
        std::fs::write(dir.join("tree/Roadmap.md"), "# Roadmap\n").unwrap();
        std::fs::write(
            dir.join("tree/notes.txt"),
            "plain notes\nTODO: atlas maps\n",
        )
        .unwrap();
        std::fs::write(dir.join("tree/Projects/atlas/Project Atlas.txt"), "atlas\n").unwrap();
        Fixture { dir }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn module() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("nu/fff.nu")
}

/// Run `script` with fff.nu loaded; returns stdout parsed as JSON.
fn nu(nu: &Path, data_dir: &Path, cwd: &Path, script: &str) -> Result<Value, String> {
    let full = format!(
        "use {} *; {script} | to json --raw",
        serde_json::to_string(&module()).unwrap()
    );
    let out = Command::new(nu)
        .args(["--no-config-file", "-c", &full])
        .env("FFFINDER_DATA_DIR", data_dir)
        .env("NO_COLOR", "1")
        .current_dir(cwd)
        .output()
        .unwrap();
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn fff_nu_commands_talk_to_the_window() {
    let Some(nu_bin) = fffinder::nushell::find_nu(None) else {
        eprintln!("nu not installed; skipping");
        return;
    };
    let fx = Fixture::new();
    let tree = fx.dir.join("tree");
    let data = fx.dir.join("data");
    let engine: Arc<Engine> = Engine::new(EngineOptions {
        roots: vec![tree.clone()],
        app_dirs: Vec::new(),
        db_dir: None,
        watch: false,
        search: Default::default(),
    });
    assert!(engine.wait_ready(Duration::from_secs(20)));
    let (tx, rx) = async_channel::unbounded::<UiRequest>();
    let _server = server::start_with(
        engine,
        Some(tx),
        Options {
            run_dir: data.join("run"),
            tcp: cfg!(windows),
        },
    )
    .unwrap();
    // A stand-in for the window.
    std::thread::spawn(move || {
        while let Ok(request) = rx.recv_blocking() {
            match request {
                UiRequest::Selection { all, reply } => {
                    let _ = reply.send(Ok(json!({
                        "location": "folder",
                        "items": [{"path": "/x/a.txt", "name": "a.txt", "type": "file", "size": 3, "modified": 1_700_000_000, "all": all}]
                    })));
                }
                UiRequest::Show {
                    paths,
                    title,
                    reply,
                } => {
                    let _ = reply.send(Ok(json!({"shown": paths.len(), "missing": [], "title": title, "paths": paths})));
                }
                UiRequest::Open { reply, .. } => {
                    let _ = reply.send(Ok(json!({"ok": true})));
                }
            }
        }
    });
    let run = |script: &str| {
        nu(&nu_bin, &data, &tree, script).unwrap_or_else(|e| panic!("{script}:\n{e}"))
    };

    assert_eq!(run("fff status | get files"), 3);
    let found = run("fff find atlas");
    let rows = found.as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["name"] == "Project Atlas.txt" && r["type"] == "file"),
        "{found}"
    );
    assert!(
        rows.iter()
            .any(|r| r["name"] == "atlas" && r["type"] == "dir"),
        "{found}"
    );
    assert_eq!(
        run("fff find atlas --files | get type | uniq"),
        json!(["file"])
    );
    assert_eq!(
        run("fff find atlas --dirs | get type | uniq"),
        json!(["dir"])
    );
    assert_eq!(
        run("fff find notes --here --limit 1 | get 0.name"),
        "notes.txt"
    );
    // Sizes and dates arrive as Nushell types.
    assert_eq!(
        run("fff find notes --files | get 0.size | describe"),
        "filesize"
    );
    assert_eq!(
        run("fff find notes --files | get 0.modified | describe"),
        "datetime"
    );

    assert_eq!(run("fff files | length"), 3);
    assert_eq!(run("fff files notes | get 0.name"), "notes.txt");
    let dirs = run("fff dirs | get name");
    assert!(dirs.as_array().unwrap().contains(&json!("atlas")), "{dirs}");

    let hits = run("fff search 'atlas maps'");
    assert_eq!(hits[0]["line_number"], 2);
    assert!(hits[0]["path"].as_str().unwrap().ends_with("notes.txt"));
    let context = run("fff search 'atlas maps' --context 1 | get 0.context");
    assert_eq!(context, json!(["plain notes", "TODO: atlas maps"]));
    assert_eq!(run("fff search --regex 'TO.O' Roadmap | length"), 2);

    let selection = run("fff selection");
    assert_eq!(selection[0]["name"], "a.txt");
    assert_eq!(
        run("fff selection --all | get 0.size | describe"),
        "filesize"
    );

    let shown = run("ls | fff show --title Mine");
    assert_eq!(shown["shown"], 3);
    assert_eq!(shown["title"], "Mine");
    assert!(
        shown["paths"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| Path::new(p.as_str().unwrap()).is_absolute()),
        "relative names from ls are expanded: {shown}"
    );
    assert_eq!(run("['notes.txt'] | fff show | get shown"), 1);

    // Without a window, every command says so.
    let none = fx.dir.join("nowhere");
    std::fs::create_dir_all(&none).unwrap();
    let err = nu(&nu_bin, &none, &tree, "fff find x").unwrap_err();
    assert!(err.contains("no fffinder window is running"), "{err}");
    let err = nu(&nu_bin, &data, &tree, "fff watch").unwrap_err();
    assert!(err.contains("--timeout or --max-events"), "{err}");
}
