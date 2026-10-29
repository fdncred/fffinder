//! The control server: answers queries about the live index, and requests
//! to show things, over a local socket (see `docs/protocol.md`). Searches
//! run on the engine directly; anything about windows goes to the UI thread
//! as a [`UiRequest`].

pub mod client;
pub mod http;

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use fffinder_core::{
    Engine, GrepModeKind, GrepRequest, KindFilter, SearchRequest, config, entry::display_name,
};
use serde_json::{Value, json};

use http::Request;

pub const PROTOCOL: u32 = 1;

/// The UI thread's answer to a [`UiRequest`].
pub type Reply = std::sync::mpsc::SyncSender<Result<Value, String>>;

/// Requests that need a window, handled on the UI thread.
pub enum UiRequest {
    Selection {
        all: bool,
        reply: Reply,
    },
    Show {
        paths: Vec<PathBuf>,
        title: Option<String>,
        reply: Reply,
    },
    Open {
        path: Option<PathBuf>,
        query: Option<String>,
        reply: Reply,
    },
}

/// Where a server listens, as written to `server.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Unix(PathBuf),
    Tcp { url: String, token: String },
}

pub fn run_dir() -> PathBuf {
    config::data_dir().join("run")
}

pub fn info_path() -> PathBuf {
    run_dir().join("server.json")
}

/// Where and how to listen.
#[derive(Debug, Clone)]
pub struct Options {
    /// Holds `server.json` and the socket.
    pub run_dir: PathBuf,
    /// Loopback TCP with a token instead of a Unix socket.
    pub tcp: bool,
}

impl Options {
    /// The app's: a Unix socket on macOS and Linux, TCP on Windows or when
    /// `FFFINDER_SERVER_TCP` is set.
    pub fn for_app() -> Options {
        Options {
            run_dir: run_dir(),
            tcp: cfg!(windows) || config::env_var("FFFINDER_SERVER_TCP").is_some(),
        }
    }
}

/// A running server. Dropping it removes `server.json` and the socket.
pub struct Server {
    pub endpoint: Endpoint,
    info: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.info);
        if let Endpoint::Unix(path) = &self.endpoint {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct Context {
    engine: Arc<Engine>,
    ui: Option<async_channel::Sender<UiRequest>>,
    token: Option<String>,
}

/// Start serving where the app does, unless another live server already
/// answers there.
pub fn start(
    engine: Arc<Engine>,
    ui: Option<async_channel::Sender<UiRequest>>,
) -> io::Result<Server> {
    start_with(engine, ui, Options::for_app())
}

pub fn start_with(
    engine: Arc<Engine>,
    ui: Option<async_channel::Sender<UiRequest>>,
    options: Options,
) -> io::Result<Server> {
    let info = options.run_dir.join("server.json");
    if client::discover_in(&options.run_dir).is_some_and(|e| client::status(&e).is_ok()) {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another fffinder window is already serving",
        ));
    }
    create_private_dir(&options.run_dir)?;
    let (endpoint, token) = if options.tcp {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        let token = new_token();
        let ctx = Arc::new(Context {
            engine,
            ui,
            token: Some(token.clone()),
        });
        std::thread::Builder::new()
            .name("fff-server".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let ctx = ctx.clone();
                    std::thread::spawn(move || serve(stream, &ctx));
                }
            })?;
        (
            Endpoint::Tcp {
                url,
                token: token.clone(),
            },
            Some(token),
        )
    } else {
        (listen_unix(&options.run_dir, engine, ui)?, None)
    };
    let mut info_json = json!({ "protocol": PROTOCOL, "pid": std::process::id() });
    match &endpoint {
        Endpoint::Unix(path) => info_json["socket"] = json!(path),
        Endpoint::Tcp { url, .. } => {
            info_json["url"] = json!(url);
            info_json["token"] = json!(token);
        }
    }
    write_private(&info, &serde_json::to_vec_pretty(&info_json)?)?;
    Ok(Server { endpoint, info })
}

#[cfg(unix)]
fn listen_unix(
    run_dir: &Path,
    engine: Arc<Engine>,
    ui: Option<async_channel::Sender<UiRequest>>,
) -> io::Result<Endpoint> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    let path = run_dir.join("fffinder.sock");
    // Nobody answered above, so a socket file left here is stale.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let ctx = Arc::new(Context {
        engine,
        ui,
        token: None,
    });
    std::thread::Builder::new()
        .name("fff-server".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let ctx = ctx.clone();
                std::thread::spawn(move || serve(stream, &ctx));
            }
        })?;
    Ok(Endpoint::Unix(path))
}

#[cfg(not(unix))]
fn listen_unix(
    _run_dir: &Path,
    _engine: Arc<Engine>,
    _ui: Option<async_channel::Sender<UiRequest>>,
) -> io::Result<Endpoint> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix sockets need macOS or Linux",
    ))
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)
}

/// 128 unpredictable bits as hex, from the standard library's randomly
/// keyed hasher.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for salt in 0..2u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        hasher.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

/// A connection the server can also poll for a closed peer.
trait Connection: Read + Write {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Connection for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}

#[cfg(unix)]
impl Connection for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }
}

fn serve<C: Connection>(mut stream: C, ctx: &Context) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let request = match http::read_request(&mut stream) {
        Ok(request) => request,
        Err(err) => {
            let _ = http::write_error(&mut stream, 400, &err.to_string());
            return;
        }
    };
    if let Some(token) = &ctx.token
        && request.authorization.as_deref() != Some(&format!("Bearer {token}"))
    {
        let _ = http::write_error(&mut stream, 401, "missing or wrong token");
        return;
    }
    let result = route(&request, &mut stream, ctx);
    if let Err(err) = result {
        log::debug!("control request {} failed: {err}", request.path);
    }
}

/// A 4xx or 5xx answer.
struct Failure(u16, String);

impl Failure {
    fn bad(message: impl Into<String>) -> Failure {
        Failure(400, message.into())
    }
}

fn route<C: Connection>(request: &Request, out: &mut C, ctx: &Context) -> io::Result<()> {
    let result = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/status") => Ok(status(&ctx.engine)),
        ("GET", "/v1/find") => find(request, &ctx.engine).map(return_array),
        ("GET", "/v1/files") => Ok(return_array(list(request, &ctx.engine, false))),
        ("GET", "/v1/dirs") => Ok(return_array(list(request, &ctx.engine, true))),
        ("GET", "/v1/search") => search(request, &ctx.engine).map(return_array),
        ("GET", "/v1/watch") => return watch(request, &ctx.engine, out),
        ("GET", "/v1/selection") => ui(ctx, |reply| UiRequest::Selection {
            all: request
                .param("all")
                .is_some_and(|v| v == "true" || v == "1"),
            reply,
        }),
        ("POST", "/v1/show") => show(request, ctx),
        ("POST", "/v1/open") => open(request, ctx),
        (
            _,
            "/v1/status" | "/v1/find" | "/v1/files" | "/v1/dirs" | "/v1/search" | "/v1/watch"
            | "/v1/selection" | "/v1/show" | "/v1/open",
        ) => Err(Failure(
            405,
            format!("{} is not allowed here", request.method),
        )),
        _ => Err(Failure(404, format!("no endpoint {}", request.path))),
    };
    match result {
        Ok(Answer::Json(value)) => http::write_json(out, 200, &value),
        Ok(Answer::Array(rows)) => http::write_array(out, rows),
        Err(Failure(status, message)) => http::write_error(out, status, &message),
    }
}

enum Answer {
    Json(Value),
    Array(Vec<Value>),
}

fn return_array(rows: Vec<Value>) -> Answer {
    Answer::Array(rows)
}

fn unix_seconds(time: Option<SystemTime>) -> Option<u64> {
    time?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn status(engine: &Engine) -> Answer {
    let roots = engine.status();
    Answer::Json(json!({
        "app": config::APP_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "pid": std::process::id(),
        "watch": true,
        "scanning": roots.iter().any(|r| r.scanning),
        "finishing": roots.iter().any(|r| r.finishing),
        "files": roots.iter().map(|r| r.indexed_files).sum::<usize>(),
        "dirs": roots.iter().map(|r| r.indexed_dirs).sum::<usize>(),
        "roots": roots.iter().map(|r| json!({
            "path": r.path,
            "scanning": r.scanning,
            "finishing": r.finishing,
            "files": r.indexed_files,
            "dirs": r.indexed_dirs,
            "git_repo": r.git_repo,
        })).collect::<Vec<_>>(),
    }))
}

fn number(request: &Request, name: &str, default: usize) -> Result<usize, Failure> {
    match request.param(name) {
        None | Some("") => Ok(default),
        Some(text) => text
            .parse()
            .map_err(|_| Failure::bad(format!("{name} must be a whole number, not {text:?}"))),
    }
}

fn scope(request: &Request) -> Result<Option<PathBuf>, Failure> {
    match request.param("scope").filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) if Path::new(s).is_absolute() => Ok(Some(PathBuf::from(s))),
        Some(s) => Err(Failure::bad(format!(
            "scope must be an absolute path, not {s:?}"
        ))),
    }
}

fn find(request: &Request, engine: &Engine) -> Result<Vec<Value>, Failure> {
    let query = request
        .param("query")
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| Failure::bad("query is required"))?;
    let kind_param = request.param("type").unwrap_or("all");
    let mut kind = match request.param("kind").filter(|k| !k.is_empty()) {
        Some(name) => KindFilter::from_name(name)
            .ok_or_else(|| Failure::bad(format!("unknown kind {name:?}")))?,
        None => KindFilter::Any,
    };
    let files_only = match kind_param {
        "all" | "" => false,
        "file" => true,
        "dir" => {
            kind = KindFilter::Folder;
            false
        }
        other => {
            return Err(Failure::bad(format!(
                "type must be all, file, or dir, not {other:?}"
            )));
        }
    };
    let response = engine.search(&SearchRequest {
        query: query.to_string(),
        scope: scope(request)?,
        kind,
        limit: number(request, "limit", 100)?.max(1),
        show_hidden: true,
        files_only,
    });
    Ok(response
        .hits
        .into_iter()
        .enumerate()
        .map(|(ix, hit)| {
            let mut row = json!({
                "type": if hit.entry.is_dir { "dir" } else { "file" },
                "path": hit.entry.path,
                "rank": ix + 1,
                "score": hit.score,
            });
            if !hit.entry.is_dir {
                row["size"] = json!(hit.entry.size.unwrap_or(0));
                row["modified"] = json!(unix_seconds(hit.entry.modified));
            }
            if let Some(git) = hit.git_status {
                row["git"] = json!(git);
            }
            row
        })
        .collect())
}

fn list(request: &Request, engine: &Engine, dirs: bool) -> Vec<Value> {
    let limit = number(request, "limit", 0).unwrap_or(0);
    engine
        .list(dirs, request.param("query"), limit)
        .into_iter()
        .map(|item| {
            if dirs {
                json!({ "path": item.path, "root": item.root })
            } else {
                json!({
                    "path": item.path,
                    "root": item.root,
                    "size": item.size,
                    "modified": item.modified,
                })
            }
        })
        .collect()
}

fn search(request: &Request, engine: &Engine) -> Result<Vec<Value>, Failure> {
    let patterns: Vec<String> = request
        .params("pattern")
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if patterns.is_empty() {
        return Err(Failure::bad("pattern is required"));
    }
    let mode = match request.param("mode").unwrap_or("plain") {
        "plain" | "" => GrepModeKind::Plain,
        "regex" => GrepModeKind::Regex,
        "fuzzy" => GrepModeKind::Fuzzy,
        other => {
            return Err(Failure::bad(format!(
                "mode must be plain, regex, or fuzzy, not {other:?}"
            )));
        }
    };
    let response = engine.grep(&GrepRequest {
        patterns,
        mode,
        scope: scope(request)?,
        limit: number(request, "limit", 200)?.max(1),
        before: number(request, "before", 0)?,
        after: number(request, "after", 0)?,
        per_file: 0,
        trim: false,
    });
    Ok(response
        .hits
        .into_iter()
        .map(|hit| {
            let offset = hit.byte_offset;
            json!({
                "path": hit.path,
                "line_number": hit.line,
                "column": hit.column,
                "byte_offset": offset,
                "line": hit.text,
                "matches": hit.ranges.iter().map(|(a, b)| json!({
                    "start": offset + *a as u64,
                    "end": offset + *b as u64,
                })).collect::<Vec<_>>(),
                "context_before": hit.context_before,
                "context_after": hit.context_after,
            })
        })
        .collect())
}

/// Stream change events as JSON lines until a limit or the client leaves.
fn watch<C: Connection>(request: &Request, engine: &Engine, out: &mut C) -> io::Result<()> {
    let parsed = (|| {
        let timeout = match number(request, "timeout_ms", 0)? {
            0 => None,
            ms => Some(Instant::now() + Duration::from_millis(ms as u64)),
        };
        let max_events = number(request, "max_events", 0)?;
        let subscription = engine
            .watch(
                request.param("pattern").unwrap_or(""),
                &request.params("ignore"),
            )
            .map_err(|err| Failure::bad(err.to_string()))?;
        Ok::<_, Failure>((timeout, max_events, subscription))
    })();
    let (deadline, max_events, subscription) = match parsed {
        Ok(parts) => parts,
        Err(Failure(status, message)) => return http::write_error(out, status, &message),
    };
    http::write_head(out, 200, "application/x-ndjson", None)?;
    out.flush()?;
    let mut sent = 0;
    let mut probe = [0u8; 1];
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) || (max_events > 0 && sent >= max_events) {
            return Ok(());
        }
        match subscription.events.recv_timeout(Duration::from_millis(250)) {
            Ok(batch) => {
                for event in batch {
                    let mut line = json!({ "kind": event.kind.as_str(), "path": event.path });
                    if let Some(from) = event.from {
                        line["from"] = json!(from);
                    }
                    serde_json::to_writer(&mut *out, &line).map_err(io::Error::other)?;
                    out.write_all(b"\n")?;
                    sent += 1;
                    if max_events > 0 && sent >= max_events {
                        break;
                    }
                }
                out.flush()?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // A read of zero bytes means the client hung up.
                out.set_read_timeout(Some(Duration::from_millis(1)))?;
                match out.read(&mut probe) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) => {}
                    Err(err) => return Err(err),
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Hand a request to the UI thread and wait for its answer.
fn ui(ctx: &Context, make: impl FnOnce(Reply) -> UiRequest) -> Result<Answer, Failure> {
    let Some(sender) = &ctx.ui else {
        return Err(Failure(503, "no window to ask".into()));
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    sender
        .send_blocking(make(tx))
        .map_err(|_| Failure(503, "the window is closing".into()))?;
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(value)) => Ok(Answer::Json(value)),
        Ok(Err(message)) => Err(Failure::bad(message)),
        Err(_) => Err(Failure(503, "the window did not answer".into())),
    }
}

fn body(request: &Request) -> Result<Value, Failure> {
    if request.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&request.body)
        .map_err(|err| Failure::bad(format!("bad JSON body: {err}")))
}

fn show(request: &Request, ctx: &Context) -> Result<Answer, Failure> {
    let body = body(request)?;
    let paths: Vec<PathBuf> = body
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| Failure::bad("paths must be a list of absolute paths"))?
        .iter()
        .map(|p| {
            p.as_str()
                .filter(|p| Path::new(p).is_absolute())
                .map(PathBuf::from)
                .ok_or_else(|| Failure::bad(format!("not an absolute path: {p}")))
        })
        .collect::<Result<_, _>>()?;
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string);
    ui(ctx, |reply| UiRequest::Show {
        paths,
        title,
        reply,
    })
}

fn open(request: &Request, ctx: &Context) -> Result<Answer, Failure> {
    let body = body(request)?;
    let path = match body.get("path").and_then(Value::as_str) {
        Some(p) if Path::new(p).is_absolute() => Some(PathBuf::from(p)),
        Some(p) => return Err(Failure::bad(format!("path must be absolute, not {p:?}"))),
        None => None,
    };
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_string);
    ui(ctx, |reply| UiRequest::Open { path, query, reply })
}

/// A row describing one item, as `/v1/selection` lists them.
pub fn item_json(entry: &fffinder_core::FileEntry) -> Value {
    let mut row = json!({
        "path": entry.path,
        "name": display_name(&entry.path),
        "type": if entry.is_dir { "dir" } else { "file" },
        "modified": unix_seconds(entry.modified),
    });
    if !entry.is_dir {
        row["size"] = json!(entry.size.unwrap_or(0));
    }
    row
}
