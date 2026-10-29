//! The control server over a real socket, against a fixture index: every
//! endpoint's shape, errors, the token on TCP, and requests that need the
//! window.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fffinder::server::{self, Endpoint, Options, UiRequest, client};
use fffinder_core::{Engine, EngineOptions};
use serde_json::{Value, json};

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("fff-server-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tree/Projects/atlas")).unwrap();
        std::fs::write(
            dir.join("tree/Roadmap.md"),
            "# Roadmap\nship the atlas\nthen rest\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tree/notes.txt"),
            "plain notes\nTODO: atlas maps\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tree/Projects/atlas/Project Atlas.txt"),
            "atlas one\natlas two\n",
        )
        .unwrap();
        Fixture { dir }
    }

    fn tree(&self) -> PathBuf {
        self.dir.join("tree")
    }

    fn engine(&self, watch: bool) -> Arc<Engine> {
        let engine = Engine::new(EngineOptions {
            roots: vec![self.tree()],
            app_dirs: Vec::new(),
            db_dir: None,
            watch,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(20)));
        engine
    }

    fn options(&self, tcp: bool) -> Options {
        Options {
            run_dir: self.dir.join(if tcp { "run-tcp" } else { "run" }),
            tcp,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn get(endpoint: &Endpoint, target: &str) -> Value {
    client::json(endpoint, "GET", target, None).unwrap_or_else(|e| panic!("{target}: {e}"))
}

#[cfg(unix)]
fn status_of(endpoint: &Endpoint, method: &str, target: &str, body: Option<&Value>) -> u16 {
    client::request(endpoint, method, target, body, Duration::from_secs(10))
        .unwrap()
        .0
}

#[cfg(unix)]
fn paths(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect()
}

#[cfg(unix)]
#[test]
fn answers_index_queries_over_a_unix_socket() {
    let fx = Fixture::new("unix");
    let server = server::start_with(fx.engine(false), None, fx.options(false)).unwrap();
    let endpoint = client::discover_in(&fx.options(false).run_dir).unwrap();
    assert_eq!(endpoint, server.endpoint);
    assert!(matches!(endpoint, Endpoint::Unix(_)));

    let status = get(&endpoint, "/v1/status");
    assert_eq!(status["protocol"], 1);
    assert_eq!(status["files"], 3);
    assert_eq!(status["roots"].as_array().unwrap().len(), 1);
    assert!(status["dirs"].as_u64().unwrap() >= 2, "{status}");

    let found = get(&endpoint, "/v1/find?query=atlas");
    let rows = found.as_array().unwrap();
    assert_eq!(rows[0]["rank"], 1);
    assert!(
        paths(&found)
            .iter()
            .any(|p| p.ends_with("Project Atlas.txt")),
        "{found}"
    );
    assert!(
        rows.iter().any(|r| r["type"] == "dir"),
        "folders match too: {found}"
    );
    let file = rows.iter().find(|r| r["type"] == "file").unwrap();
    assert_eq!(file["size"], 20);
    assert!(file["modified"].as_u64().unwrap() > 1_600_000_000);

    let files_only = get(&endpoint, "/v1/find?query=atlas&type=file");
    assert!(
        files_only
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["type"] == "file")
    );
    let dirs_only = get(&endpoint, "/v1/find?query=atlas&type=dir");
    assert!(
        dirs_only
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["type"] == "dir")
    );
    let docs = get(&endpoint, "/v1/find?query=road+map&kind=document&limit=1");
    assert_eq!(paths(&docs).len(), 1);
    assert!(paths(&docs)[0].ends_with("Roadmap.md"));

    let mut all = paths(&get(&endpoint, "/v1/files"));
    all.sort();
    assert_eq!(all.len(), 3);
    assert!(
        all[0].ends_with("Projects/atlas/Project Atlas.txt"),
        "{all:?}"
    );
    let listed = get(&endpoint, "/v1/files?limit=1");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0]["root"].as_str().unwrap().ends_with("tree"));
    assert!(paths(&get(&endpoint, "/v1/files?query=notes"))[0].ends_with("notes.txt"));
    let dirs = paths(&get(&endpoint, "/v1/dirs"));
    assert!(dirs.iter().any(|d| d.ends_with("Projects")), "{dirs:?}");
    assert!(dirs.iter().any(|d| d.ends_with("atlas")), "{dirs:?}");

    let hits = get(&endpoint, "/v1/search?pattern=atlas+maps&after=1");
    let hit = &hits[0];
    assert!(hit["path"].as_str().unwrap().ends_with("notes.txt"));
    assert_eq!(hit["line_number"], 2);
    assert_eq!(hit["line"], "TODO: atlas maps");
    assert_eq!(hit["byte_offset"], 12);
    assert_eq!(hit["matches"], json!([{"start": 18, "end": 28}]));
    let regex = get(&endpoint, "/v1/search?pattern=atl.s+t&mode=regex");
    assert_eq!(regex.as_array().unwrap().len(), 1, "{regex}");
    let several = get(&endpoint, "/v1/search?pattern=Roadmap&pattern=plain+notes");
    assert_eq!(several.as_array().unwrap().len(), 2, "{several}");

    // Mistakes get a status and a message.
    assert_eq!(status_of(&endpoint, "GET", "/v1/find", None), 400);
    assert_eq!(
        status_of(&endpoint, "GET", "/v1/find?query=x&type=car", None),
        400
    );
    assert_eq!(
        status_of(&endpoint, "GET", "/v1/search?pattern=x&limit=lots", None),
        400
    );
    assert_eq!(status_of(&endpoint, "GET", "/v1/nope", None), 404);
    assert_eq!(
        status_of(&endpoint, "POST", "/v1/status", Some(&json!({}))),
        405
    );
    assert_eq!(
        status_of(&endpoint, "GET", "/v1/selection", None),
        503,
        "no window to ask"
    );
    let err = client::json(&endpoint, "GET", "/v1/find", None).unwrap_err();
    assert!(err.to_string().contains("query is required"), "{err}");

    // One server per run folder.
    let second = server::start_with(fx.engine(false), None, fx.options(false));
    assert!(second.is_err());
    drop(server);
    assert!(
        client::discover_in(&fx.options(false).run_dir).is_none(),
        "server.json is removed"
    );
}

#[test]
fn tcp_requires_the_token() {
    let fx = Fixture::new("tcp");
    let server = server::start_with(fx.engine(false), None, fx.options(true)).unwrap();
    let Endpoint::Tcp { url, token } = &server.endpoint else {
        panic!("expected TCP");
    };
    assert_eq!(token.len(), 32);
    assert_eq!(get(&server.endpoint, "/v1/status")["files"], 3);

    let mut stream = std::net::TcpStream::connect(url.trim_start_matches("http://")).unwrap();
    stream
        .write_all(b"GET /v1/status HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 401"), "{raw}");
}

#[test]
fn window_requests_reach_the_ui_thread() {
    let fx = Fixture::new("ui");
    let (tx, rx) = async_channel::unbounded::<UiRequest>();
    let server = server::start_with(fx.engine(false), Some(tx), fx.options(true)).unwrap();
    let ui = std::thread::spawn(move || {
        let mut seen = Vec::new();
        while let Ok(request) = rx.recv_blocking() {
            match request {
                UiRequest::Selection { all, reply } => {
                    seen.push(format!("selection all={all}"));
                    reply
                        .send(Ok(json!({"location": "folder", "items": []})))
                        .unwrap();
                }
                UiRequest::Show {
                    paths,
                    title,
                    reply,
                } => {
                    seen.push(format!("show {} {title:?}", paths.len()));
                    reply
                        .send(Ok(json!({"shown": paths.len(), "missing": []})))
                        .unwrap();
                }
                UiRequest::Open { path, query, reply } => {
                    seen.push(format!("open {path:?} {query:?}"));
                    reply.send(Err("no such folder".into())).unwrap();
                }
            }
        }
        seen
    });
    let endpoint = server.endpoint.clone();
    assert_eq!(
        get(&endpoint, "/v1/selection?all=true")["location"],
        "folder"
    );
    let shown = client::json(
        &endpoint,
        "POST",
        "/v1/show",
        // Absolute on every platform, unlike "/a".
        Some(&json!({
            "paths": [fx.tree().join("Roadmap.md"), fx.tree().join("notes.txt")],
            "title": "Big"
        })),
    )
    .unwrap();
    assert_eq!(shown["shown"], 2);
    let bad = client::json(
        &endpoint,
        "POST",
        "/v1/show",
        Some(&json!({"paths": ["relative"]})),
    );
    assert!(
        bad.unwrap_err()
            .to_string()
            .contains("not an absolute path")
    );
    let refused = client::json(&endpoint, "POST", "/v1/open", Some(&json!({"query": "x"})));
    assert!(refused.unwrap_err().to_string().contains("no such folder"));
    drop(server);
    drop(endpoint);
    // The channel closes once the server's threads let go of it.
    std::thread::sleep(Duration::from_millis(100));
    let _ = ui;
}

#[test]
fn watch_streams_changes_as_json_lines() {
    let fx = Fixture::new("watch");
    let server = server::start_with(fx.engine(true), None, fx.options(true)).unwrap();
    let endpoint = server.endpoint.clone();
    let new_file = fx.tree().join("fresh.txt");
    // The watcher may need a moment after the scan; retry until it subscribes.
    let mut body = String::new();
    for _ in 0..40 {
        let target = "/v1/watch?max_events=1&timeout_ms=8000";
        let watcher = {
            let endpoint = endpoint.clone();
            std::thread::spawn(move || {
                client::request(&endpoint, "GET", target, None, Duration::from_secs(15)).unwrap()
            })
        };
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(&new_file, "new").unwrap();
        let (status, bytes) = watcher.join().unwrap();
        if status == 200 {
            body = String::from_utf8(bytes).unwrap();
            break;
        }
        let _ = std::fs::remove_file(&new_file);
        std::thread::sleep(Duration::from_millis(250));
    }
    let line: Value = serde_json::from_str(body.lines().next().expect("one event")).unwrap();
    assert!(
        line["path"].as_str().unwrap().ends_with("fresh.txt"),
        "{line}"
    );
    assert!(
        ["created", "modified"].contains(&line["kind"].as_str().unwrap()),
        "{line}"
    );
    let bad = client::request(
        &endpoint,
        "GET",
        "/v1/watch?max_events=x",
        None,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(bad.0, 400);
}
