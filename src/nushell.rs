//! Nushell integration that doesn't need GPUI: locating `nu`, running a
//! pipeline over the selected files, previewing structured files with
//! `open`, and NUON formatting. Terminals live in
//! `fffinder_core::terminals`.

use fffinder_core::{FileEntry, format};
use serde_json::Value as Json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Locate `nu`: the configured path, `PATH`, then common install locations
/// (apps launched from Finder or a desktop menu get a minimal `PATH`).
pub fn find_nu(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured.filter(|p| p.is_file()) {
        return Some(path.to_path_buf());
    }
    let exe = if cfg!(windows) { "nu.exe" } else { "nu" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut fallbacks: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        fallbacks.push(home.join(".cargo/bin").join(exe));
        fallbacks.push(home.join(".local/bin").join(exe));
    }
    #[cfg(unix)]
    fallbacks.extend(
        [
            "/opt/homebrew/bin/nu",
            "/usr/local/bin/nu",
            "/usr/bin/nu",
            "/home/linuxbrew/.linuxbrew/bin/nu",
        ]
        .map(PathBuf::from),
    );
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        fallbacks.push(local.join("Programs/nu/bin/nu.exe"));
    }
    fallbacks.into_iter().find(|p| p.is_file())
}

/// Result of running Nushell, shaped for display.
#[derive(Debug, Clone, PartialEq)]
pub enum NuOutput {
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Text(String),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NuRun {
    pub output: NuOutput,
    pub error: Option<String>,
    pub elapsed: std::time::Duration,
}

/// The selection as JSON records, the same shape `fff` returns in Nushell.
pub fn entries_json(entries: &[FileEntry]) -> String {
    let rows: Vec<Json> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "path": e.path.to_string_lossy(),
                "type": if e.is_dir { "dir" } else { "file" },
                "kind": e.kind_label(),
                "size": e.size,
                "modified": e.modified.map(format::iso_date),
            })
        })
        .collect();
    Json::Array(rows).to_string()
}

/// Definitions prepended to every script: `fff-render` turns values JSON
/// can't keep (file sizes, durations, dates) into Nushell's display text.
const PRELUDE: &str = r#"
def fff-cell [c: any] {
    match ($c | describe) {
        'filesize' | 'duration' => ($c | into string)
        'datetime' | 'date' => ($c | format date '%Y-%m-%d %H:%M:%S')
        _ => $c
    }
}
def fff-render [] {
    let v = $in
    let t = $v | describe
    if ($t | str starts-with 'table') {
        $v | update cells {|c| fff-cell $c }
    } else if ($t | str starts-with 'list') {
        $v | each {|c| fff-cell $c }
    } else if ($t | str starts-with 'record') {
        $v | items {|k, x| {key: $k, value: (fff-cell $x)} }
    } else {
        fff-cell $v
    }
}
"#;

/// Reads the entries from stdin as a table with sizes and dates turned back
/// into Nushell types.
const TYPED_INPUT: &str = "$in | from json \
    | update size {|r| if $r.size == null { null } else { $r.size | into filesize } } \
    | update modified {|r| if $r.modified == null { null } else { $r.modified | into datetime } }";

/// Longest a filter may run before it is stopped.
pub const FILTER_TIMEOUT: Duration = Duration::from_secs(10);

/// The paths of the entries a filter keeps. `filter` is a `where` condition
/// (`size > 1mb and kind == Folder`) or, starting with `|`, a pipeline whose
/// rows keep their `path` column.
pub fn filter_paths(
    nu: &Path,
    filter: &str,
    entries: &[FileEntry],
    cwd: &Path,
) -> Result<Vec<PathBuf>, String> {
    let filter = filter.trim();
    let stage = if filter.starts_with('|') {
        filter.to_string()
    } else {
        format!("| where {filter}")
    };
    let script = format!("{TYPED_INPUT}\n{stage}\n| get path | to json --raw");
    let out = exec(
        nu,
        Job {
            stdin: Some(entries_json(entries)),
            timeout: Some(FILTER_TIMEOUT),
            ..Job::new(&script, cwd)
        },
    )?;
    serde_json::from_str::<Vec<String>>(out.trim())
        .map(|paths| paths.into_iter().map(PathBuf::from).collect())
        .map_err(|_| "The filter must keep rows with a path column.".to_string())
}

/// The new names a rename closure gives the entries, in order. Each item has
/// the usual columns plus `stem`, `ext`, and a 1-based `index`; the closure
/// may take it as a parameter (`{|f| $f.name }`) or as `$in`. Text that
/// isn't a closure is treated as its body, with the item as `$f`.
pub fn rename_names(
    nu: &Path,
    closure: &str,
    entries: &[FileEntry],
    cwd: &Path,
) -> Result<Vec<String>, String> {
    let closure = closure.trim();
    let closure = if closure.starts_with('{') {
        closure.to_string()
    } else {
        format!("{{|f| {closure} }}")
    };
    let script = format!(
        "let rename = {closure}\n\
         {TYPED_INPUT} | enumerate | each {{|it| \
             let parts = $it.item.name | path parse; \
             $it.item | insert index ($it.index + 1) | insert stem $parts.stem \
                 | insert ext ($parts.extension? | default '') \
         }} | each {{|f| $f | do $rename $f | into string }} | to json --raw"
    );
    let out = exec(
        nu,
        Job {
            stdin: Some(entries_json(entries)),
            timeout: Some(FILTER_TIMEOUT),
            ..Job::new(&script, cwd)
        },
    )?;
    let names: Vec<String> = serde_json::from_str(out.trim())
        .map_err(|_| "The closure must return a name for each item.".to_string())?;
    if names.len() != entries.len() {
        return Err("The closure must return one name per item.".into());
    }
    Ok(names)
}

/// `--login` when Nushell has a config folder, so the user's config applies.
/// Without one there's no config to load, and `nu --login` would create the
/// folder and print "Config file created at: …" on stdout.
pub fn login_args() -> &'static [&'static str] {
    if nu_config_home().is_some_and(|dir| dir.is_dir()) {
        &["--login"]
    } else {
        &[]
    }
}

/// Nushell's config folder, found the way `nu` finds it: an absolute
/// `$XDG_CONFIG_HOME`, or else the platform's config folder, plus `nushell`.
fn nu_config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(dirs::config_dir)
        .map(|dir| dir.join("nushell"))
}

/// `source` highlighted by `nu-highlight` in a login shell, so the user's
/// `color_config` applies. Returns the ANSI-colored text.
pub fn highlight_nu(nu: &Path, source: &str) -> Result<String, String> {
    let child = Command::new(nu)
        .args(login_args())
        .args([
            "--stdin",
            "-c",
            "$env.config.use_ansi_coloring = true; $in | nu-highlight",
        ])
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run {}: {err}", nu.display()))?;
    let output = wait_for(child, Some(source.to_string()), Some(PREVIEW_TIMEOUT))
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "nu-highlight took too long".to_string())?;
    if !output.status.success() {
        return Err(strip_ansi(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The comments in `nu-highlight` output. Nushell has no comment style, so
/// it leaves them plain: a comment is a `#` that starts a word and runs to the
/// end of its line without touching anything `styled`. Strings, raw strings
/// and interpolations that contain `#` are all styled, so they never match.
pub fn comment_ranges(
    text: &str,
    styled: &[std::ops::Range<usize>],
) -> Vec<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.trim_end_matches(['\n', '\r']).len();
        let starts_word = |i: usize| {
            i == line_start
                || bytes[i - 1].is_ascii_whitespace()
                || b";|({[".contains(&bytes[i - 1])
        };
        let comment = (line_start..line_end)
            .filter(|&i| bytes[i] == b'#' && starts_word(i))
            .find(|&i| !styled.iter().any(|r| r.start < line_end && r.end > i));
        if let Some(start) = comment {
            comments.push(start..line_end);
        }
        line_start += line.len();
    }
    comments
}

/// Run a script with no input; returns its stdout or its error message.
pub fn run_raw(nu: &Path, script: &str, cwd: &Path) -> Result<String, String> {
    exec(
        nu,
        Job {
            timeout: Some(FILTER_TIMEOUT),
            ..Job::new(script, cwd)
        },
    )
}

/// One `nu -c` run: the script and what it starts with.
struct Job<'a> {
    script: &'a str,
    /// JSON the script reads as its input, through `--stdin`.
    stdin: Option<String>,
    cwd: &'a Path,
    env: &'a [(&'a str, &'a std::ffi::OsStr)],
    /// How long it may run before it is stopped; `None` waits for it.
    timeout: Option<Duration>,
    /// Whether the plugins registered with `plugin add` load, so `open` can
    /// use their `from` commands (`from ini` from `nu_plugin_formats`, say).
    plugins: bool,
}

impl<'a> Job<'a> {
    /// `script` run in `cwd` with no input, no time limit, and no plugins.
    fn new(script: &'a str, cwd: &'a Path) -> Self {
        Self {
            script,
            stdin: None,
            cwd,
            env: &[],
            timeout: None,
            plugins: false,
        }
    }

    /// Run the script to its end. `Ok(None)` means the timeout stopped it.
    fn run(self, nu: &Path) -> std::io::Result<Option<std::process::Output>> {
        self.command(nu)
            .spawn()
            .and_then(|child| wait_for(child, self.stdin, self.timeout))
    }

    /// The `nu` command line for this run, its output piped back.
    fn command(&self, nu: &Path) -> Command {
        let mut cmd = Command::new(nu);
        // `nu -c` reads neither config.nu nor env.nu on its own; it does read
        // the plugin registry, unless `--no-config-file` turns that off too.
        if !self.plugins {
            cmd.arg("--no-config-file");
        }
        if self.stdin.is_some() {
            cmd.arg("--stdin");
        }
        cmd.arg("-c")
            .arg(self.script)
            .current_dir(self.cwd)
            .env("NO_COLOR", "1")
            .envs(self.env.iter().copied())
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

/// The message for a run its timeout stopped.
fn timed_out(timeout: Option<Duration>) -> String {
    format!(
        "nu took longer than {} s and was stopped",
        timeout.unwrap_or_default().as_secs()
    )
}

/// Run a script and return its stdout, or its error message.
fn exec(nu: &Path, job: Job) -> Result<String, String> {
    let timeout = job.timeout;
    let output = job
        .run(nu)
        .map_err(|err| format!("could not run {}: {err}", nu.display()))?
        .ok_or_else(|| timed_out(timeout))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
        Err(if stderr.trim().is_empty() {
            format!("nu exited with {}", output.status)
        } else {
            stderr.trim().to_string()
        })
    }
}

/// Run a user pipeline with the entries as its input table.
///
/// The pipeline is the user's own code; the entries travel on stdin as JSON,
/// never spliced into the script.
pub fn run_pipeline(nu: &Path, pipeline: &str, entries: &[FileEntry], cwd: &Path) -> NuRun {
    let script = format!("{TYPED_INPUT} | do {{\n{pipeline}\n}} | fff-render | to json --raw");
    let script = format!("{PRELUDE}\n{script}");
    run_script(
        nu,
        Job {
            stdin: Some(entries_json(entries)),
            ..Job::new(&script, cwd)
        },
    )
}

/// Longest a preview may take before it is abandoned and the file shows its icon.
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(10);

/// Preview a structured file (CSV, JSON, TOML, YAML, SQLite, …) with `open`.
pub fn preview_file(nu: &Path, path: &Path) -> NuRun {
    const SCRIPT: &str = r#"
let data = open $env.FFF_PREVIEW_PATH
let type = $data | describe
let data = if ($type | str starts-with 'table') or ($type | str starts-with 'list') {
    $data | first 500
} else if $type == 'SQLiteDatabase' {
    $data | schema | get tables | transpose table info | each {|t| {table: $t.table, columns: ($t.info.columns | length)} }
} else {
    $data
}
$data | fff-render | to json --raw
"#;
    let cwd = path.parent().unwrap_or(Path::new("."));
    run_script(
        nu,
        Job {
            env: &[("FFF_PREVIEW_PATH", path.as_os_str())],
            timeout: Some(PREVIEW_TIMEOUT),
            plugins: true,
            ..Job::new(&format!("{PRELUDE}\n{SCRIPT}"), cwd)
        },
    )
}

/// Files bigger than this, in bytes, aren't opened with Nushell to preview
/// or explore them.
pub const OPEN_LIMIT: u64 = 50_000_000;

/// Most rows explore mode reads from a table or list at the top of a file,
/// and from each table of a SQLite database.
pub const EXPLORE_ROWS: usize = 10_000;

/// A structured file as `open` reads it, nested values and all.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub data: Json,
    /// How many rows the table or list at the top holds, when it is one,
    /// before it was cut to [`EXPLORE_ROWS`].
    pub rows: Option<usize>,
}

/// Open a structured file for explore mode, keeping what `preview_file`
/// abbreviates. A SQLite database becomes a record of its tables, with each
/// blob shown as its size. The plugins registered with `plugin add` load, so
/// formats they read, such as INI with `nu_plugin_formats`, open as data.
///
/// # Errors
///
/// A short message when the file can't be read as data: Nushell's headline
/// and reason when `open` fails (a parse error, say), or when the file reads
/// as raw bytes, or as text because no `from` command reads its extension.
/// Also when `nu` can't run, takes longer than
/// [`PREVIEW_TIMEOUT`], or prints something that isn't JSON.
pub fn explore_file(nu: &Path, path: &Path) -> Result<Opened, String> {
    // `describe` names a database exactly `SQLiteDatabase`; a looser match
    // also catches files that only mention SQLite, like a Cargo.toml with a
    // `sqlite` feature. Errors come back as data from `catch`, so Nushell's
    // printed report, which quotes the environment the path came from, never
    // reaches the window.
    const SCRIPT: &str = r#"
let path = $env.FFF_PREVIEW_PATH
let cap = $env.FFF_EXPLORE_ROWS | into int
try {
    let data = match $env.FFF_PREVIEW_EXT {
        'jsonl' => (open --raw $path | from json --objects)
        _ => (open $path)
    }
    let type = $data | describe
    let opened = if $type == 'SQLiteDatabase' {
        let tables = $data | columns | each {|table|
            let name = $table | str replace --all '"' '""'
            let cells = $data
                | query db 'SELECT name FROM pragma_table_info(?)' --params [$table]
                | get name
                | each {|column|
                    let column = $column | str replace --all '"' '""'
                    $"CASE WHEN typeof\(\"($column)\") = 'blob' THEN '[binary ' || length\(\"($column)\") || ' bytes]' ELSE \"($column)\" END AS \"($column)\""
                }
                | str join ', '
            {$table: ($data | query db $"SELECT ($cells) FROM \"($name)\" LIMIT ($cap)")}
        }
        {rows: null, data: ($tables | into record)}
    } else if $type == 'binary' {
        {error: {msg: 'Nushell reads this file as raw bytes, not as data.'}}
    } else if $type == 'string' and (scope commands | where name == $"from ($env.FFF_PREVIEW_EXT)" | is-empty) {
        {error: {msg: $"Nushell has no `from ($env.FFF_PREVIEW_EXT)` command, so it reads this file as plain text. A plugin can add one: `from ini` comes with nu_plugin_formats, for example."}}
    } else if ($type | str starts-with 'table') or ($type | str starts-with 'list') {
        {rows: ($data | length), data: ($data | first $cap)}
    } else {
        {rows: null, data: $data}
    }
    $opened | to json --raw
} catch {|err|
    {error: ($err.details? | default {msg: $err.msg})} | to json --raw
}
"#;
    let cwd = path.parent().unwrap_or(Path::new("."));
    let cap = EXPLORE_ROWS.to_string();
    let ext = lowercase_extension(path);
    let env = [
        ("FFF_PREVIEW_PATH", path.as_os_str()),
        ("FFF_PREVIEW_EXT", std::ffi::OsStr::new(&ext)),
        ("FFF_EXPLORE_ROWS", std::ffi::OsStr::new(&cap)),
    ];
    let stdout = exec(
        nu,
        Job {
            env: &env,
            timeout: Some(PREVIEW_TIMEOUT),
            plugins: true,
            ..Job::new(SCRIPT, cwd)
        },
    )?;
    let mut opened: Json = serde_json::from_str(&stdout).map_err(|err| err.to_string())?;
    if let Some(error) = opened.get("error") {
        return Err(error_text(error));
    }
    Ok(Opened {
        rows: opened
            .get("rows")
            .and_then(Json::as_u64)
            .and_then(|rows| usize::try_from(rows).ok()),
        data: opened.get_mut("data").map(Json::take).unwrap_or_default(),
    })
}

/// Nushell's error record, as `catch` hands it over, cut down to its headline
/// and the most specific reason under it, such as the parser's line and
/// column. The source snippets Nushell prints around errors are left out.
fn error_text(error: &Json) -> String {
    let headline = error
        .get("msg")
        .and_then(Json::as_str)
        .unwrap_or("Nushell couldn't open the file.");
    // `inner` holds the errors this one wraps, so the deepest is the parser's
    // own. Top-level help only says to try `open --raw`, so it is skipped.
    let reason = std::iter::successors(Some(error), |level| level.get("inner")?.get(0))
        .enumerate()
        .filter_map(|(depth, level)| {
            let help = level
                .get("help")
                .and_then(Json::as_str)
                .filter(|_| depth > 0);
            help.or_else(|| level.get("labels")?.get(0)?.get("text")?.as_str())
        })
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .last();
    match reason {
        Some(reason) if !reason.eq_ignore_ascii_case(headline) => format!("{headline}\n{reason}"),
        _ => headline.to_string(),
    }
}

/// The extension of `path` in lowercase, or `""` when it has none.
fn lowercase_extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// File types `open` turns into structured data.
pub fn is_structured(path: &Path) -> bool {
    matches!(
        lowercase_extension(path).as_str(),
        "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "nuon"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "ini"
            | "sqlite"
            | "sqlite3"
            | "db"
            | "xlsx"
            | "ods"
            | "eml"
            | "ics"
            | "vcf"
            | "msgpack"
            | "msgpackz"
            | "plist"
    )
}

fn run_script(nu: &Path, job: Job) -> NuRun {
    let started = Instant::now();
    let timeout = job.timeout;
    let result = job.run(nu);
    let elapsed = started.elapsed();
    match result {
        Err(err) => NuRun {
            output: NuOutput::Empty,
            error: Some(format!("could not run {}: {err}", nu.display())),
            elapsed,
        },
        Ok(None) => NuRun {
            output: NuOutput::Empty,
            error: Some(timed_out(timeout)),
            elapsed,
        },
        Ok(Some(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
            let output = parse_output(&stdout);
            let error = (!out.status.success()
                || (output == NuOutput::Empty && !stderr.trim().is_empty()))
            .then(|| {
                let msg = stderr.trim().to_string();
                if msg.is_empty() {
                    format!("nu exited with {}", out.status)
                } else {
                    msg
                }
            });
            NuRun {
                output,
                error,
                elapsed,
            }
        }
    }
}

/// Feed stdin, collect output, and kill the child if it outlives `timeout`
/// (`Ok(None)`). Pipes are drained on threads so a large output can't
/// deadlock against a full pipe.
fn wait_for(
    mut child: std::process::Child,
    stdin: Option<String>,
    timeout: Option<Duration>,
) -> std::io::Result<Option<std::process::Output>> {
    if let Some(input) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        std::thread::spawn(move || {
            let _ = pipe.write_all(input.as_bytes());
        });
    }
    let Some(timeout) = timeout else {
        return child.wait_with_output().map(Some);
    };
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Some(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }))
}

/// Turn `to json` output into a table or text.
pub fn parse_output(stdout: &str) -> NuOutput {
    let text = stdout.trim();
    if text.is_empty() {
        return NuOutput::Empty;
    }
    let Ok(json) = serde_json::from_str::<Json>(text) else {
        return NuOutput::Text(text.to_string());
    };
    match json {
        Json::Null => NuOutput::Empty,
        Json::String(s) => NuOutput::Text(s),
        Json::Array(items) => {
            if items.is_empty() {
                return NuOutput::Empty;
            }
            let mut columns: Vec<String> = Vec::new();
            if is_table(&items) {
                for item in &items {
                    if let Json::Object(map) = item {
                        for key in map.keys() {
                            if !columns.contains(key) {
                                columns.push(key.clone());
                            }
                        }
                    }
                }
                let rows = items
                    .iter()
                    .map(|item| {
                        columns
                            .iter()
                            .map(|c| item.get(c).map(cell).unwrap_or_default())
                            .collect()
                    })
                    .collect();
                NuOutput::Table { columns, rows }
            } else {
                NuOutput::Table {
                    columns: vec!["value".into()],
                    rows: items.iter().map(|v| vec![cell(v)]).collect(),
                }
            }
        }
        Json::Object(map) => NuOutput::Table {
            columns: vec!["key".into(), "value".into()],
            rows: map.iter().map(|(k, v)| vec![k.clone(), cell(v)]).collect(),
        },
        other => NuOutput::Text(cell(&other)),
    }
}

/// One table cell, abbreviated like Nushell does for nested data.
pub fn cell(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => n.to_string(),
        Json::String(s) => s.clone(),
        Json::Array(items) if is_table(items) => {
            format!("[table {} row{}]", items.len(), plural(items.len()))
        }
        Json::Array(items) => format!("[list {} item{}]", items.len(), plural(items.len())),
        Json::Object(map) => format!("{{record {} field{}}}", map.len(), plural(map.len())),
    }
}

/// Whether a list is what Nushell calls a table: records, at least one.
pub fn is_table(items: &[Json]) -> bool {
    !items.is_empty() && items.iter().all(Json::is_object)
}

/// The `s` that makes a noun plural for `n` of them.
pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Paths as NUON: a quoted string for one path, a list for several.
pub fn to_nuon(paths: &[PathBuf]) -> String {
    let quoted: Vec<String> = paths
        .iter()
        .map(|p| Json::String(p.to_string_lossy().into_owned()).to_string())
        .collect();
    match quoted.as_slice() {
        [one] => one.clone(),
        many => format!("[{}]", many.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_comments_nu_highlight_left_plain() {
        let text = "# top\nlet x = 1 # trailing\n\"a # b\" #c\nls#not\n  # indented\r\n";
        let at = |needle: &str| {
            let start = text.find(needle).unwrap();
            start..start + needle.len()
        };
        // What nu-highlight colors: `let`, `x`, `1`, the string, and `ls#not`.
        let styled = [
            at("let"),
            at("x ="),
            at("1 "),
            at("\"a # b\""),
            at("ls#not"),
        ];
        let found: Vec<&str> = comment_ranges(text, &styled)
            .into_iter()
            .map(|r| &text[r])
            .collect();
        assert_eq!(found, ["# top", "# trailing", "#c", "# indented"]);
        assert!(comment_ranges("", &[]).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_child_is_stopped_at_the_timeout() {
        let child = Command::new("sleep")
            .arg("5")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let out = wait_for(child, None, Some(Duration::from_millis(200))).unwrap();
        assert!(out.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn parses_tables() {
        let out = parse_output(r#"[{"a":1,"b":"x"},{"a":2,"c":[1,2]}]"#);
        assert_eq!(
            out,
            NuOutput::Table {
                columns: vec!["a".into(), "b".into(), "c".into()],
                rows: vec![
                    vec!["1".into(), "x".into(), "".into()],
                    vec!["2".into(), "".into(), "[list 2 items]".into()],
                ],
            }
        );
        assert_eq!(parse_output("\"hi\""), NuOutput::Text("hi".into()));
        assert_eq!(parse_output("[]"), NuOutput::Empty);
        assert!(matches!(parse_output(r#"{"k":1}"#), NuOutput::Table { .. }));
    }

    #[test]
    fn nuon_quoting() {
        assert_eq!(to_nuon(&["/a b".into()]), r#""/a b""#);
        assert_eq!(
            to_nuon(&["/a".into(), "/b\"c".into()]),
            r#"["/a", "/b\"c"]"#
        );
    }

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi("\u{1b}[31mError\u{1b}[0m: x"), "Error: x");
    }

    /// Runs only where `nu` is installed.
    #[test]
    fn filters_with_real_nu() {
        let Some(nu) = find_nu(None) else { return };
        let dir = std::env::temp_dir().join(format!("fff-nu-filter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.txt");
        let big = dir.join("big.log");
        std::fs::write(&small, "x").unwrap();
        std::fs::write(&big, "y".repeat(5000)).unwrap();
        let entries: Vec<FileEntry> = [&small, &big]
            .iter()
            .map(|p| FileEntry::from_path(p).unwrap())
            .collect();

        assert_eq!(
            filter_paths(&nu, "size > 1kb", &entries, &dir).unwrap(),
            std::slice::from_ref(&big)
        );
        assert_eq!(
            filter_paths(
                &nu,
                "name =~ 'small' and modified > 2020-01-01",
                &entries,
                &dir
            )
            .unwrap(),
            std::slice::from_ref(&small)
        );
        assert_eq!(
            filter_paths(&nu, "| sort-by size --reverse | first 1", &entries, &dir).unwrap(),
            std::slice::from_ref(&big)
        );
        assert!(
            filter_paths(&nu, "size > 1tb", &entries, &dir)
                .unwrap()
                .is_empty()
        );
        let typo = filter_paths(&nu, "sise > 1kb", &entries, &dir).unwrap_err();
        assert!(typo.contains("sise"), "{typo}");
        let dropped = filter_paths(&nu, "| select name", &entries, &dir).unwrap_err();
        assert!(dropped.contains("path"), "{dropped}");

        let names = |closure: &str| rename_names(&nu, closure, &entries, &dir);
        assert_eq!(
            names("{|f| $f.name | str upcase }").unwrap(),
            ["SMALL.TXT", "BIG.LOG"]
        );
        assert_eq!(names("{ $in.stem }").unwrap(), ["small", "big"]);
        assert_eq!(
            names("$\"($f.index) ($f.stem).($f.ext)\"").unwrap(),
            ["1 small.txt", "2 big.log"]
        );
        assert!(names("{|f| $f.nope }").unwrap_err().contains("nope"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Runs only where `nu` is installed.
    #[test]
    fn pipeline_and_preview_with_real_nu() {
        let Some(nu) = find_nu(None) else { return };
        let dir = std::env::temp_dir().join(format!("fff-nu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("people.csv");
        std::fs::write(&csv, "name,age\nana,31\nbo,42\n").unwrap();
        let entries = vec![FileEntry::from_path(&csv).unwrap()];

        let run = run_pipeline(&nu, "select name size", &entries, &dir);
        assert_eq!(run.error, None, "{run:?}");
        let NuOutput::Table { columns, rows } = run.output else {
            panic!("expected table: {run:?}")
        };
        assert_eq!(columns, ["name", "size"]);
        assert_eq!(rows[0][0], "people.csv");
        // File sizes come back formatted the way Nushell shows them.
        assert_eq!(rows[0][1], "22 B");

        let run = preview_file(&nu, &csv);
        assert_eq!(run.error, None, "{run:?}");
        assert_eq!(
            run.output,
            NuOutput::Table {
                columns: vec!["name".into(), "age".into()],
                rows: vec![
                    vec!["ana".into(), "31".into()],
                    vec!["bo".into(), "42".into()]
                ],
            }
        );

        let run = run_pipeline(&nu, "this-command-does-not-exist", &entries, &dir);
        assert!(run.error.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Runs only where `nu` is installed.
    #[test]
    fn explores_nested_data_with_real_nu() {
        let Some(nu) = find_nu(None) else { return };
        let dir = std::env::temp_dir().join(format!("fff-nu-explore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let toml = dir.join("Cargo.toml");
        std::fs::write(
            &toml,
            "[package]\nname = \"fff\"\nedition = \"2024\"\n\n[[bin]]\nname = \"a\"\n",
        )
        .unwrap();
        let opened = explore_file(&nu, &toml).unwrap();
        assert_eq!(opened.rows, None, "a record isn't cut short");
        assert_eq!(
            opened.data,
            serde_json::json!({"package": {"name": "fff", "edition": "2024"}, "bin": [{"name": "a"}]})
        );
        let keys: Vec<&String> = opened.data["package"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["name", "edition"], "fields keep the file's order");

        let csv = dir.join("people.csv");
        std::fs::write(&csv, "name,age\nana,31\nbo,42\n").unwrap();
        let opened = explore_file(&nu, &csv).unwrap();
        assert_eq!(opened.rows, Some(2));
        assert_eq!(opened.data[1]["age"], 42);

        let db = dir.join("shop.db");
        let made = Command::new(&nu)
            .args(["--no-config-file", "-c"])
            .arg("[[item qty]; [pen 3]] | into sqlite $env.DB --table-name stock")
            .env("DB", &db)
            .status()
            .unwrap();
        assert!(made.success());
        let opened = explore_file(&nu, &db).unwrap();
        assert_eq!(
            opened.data,
            serde_json::json!({"stock": [{"item": "pen", "qty": 3}]}),
            "a database is a record of its tables"
        );

        let blobs = dir.join("blobs.sqlite");
        let made = Command::new(&nu)
            .args(["--no-config-file", "-c"])
            .arg("[[name data]; [a 0x[00ff]]] | into sqlite $env.DB --table-name files")
            .env("DB", &blobs)
            .status()
            .unwrap();
        assert!(made.success());
        assert_eq!(
            explore_file(&nu, &blobs).unwrap().data,
            serde_json::json!({"files": [{"name": "a", "data": "[binary 2 bytes]"}]}),
            "a blob shows its size, not its bytes"
        );

        // Nushell's own Cargo.toml has a `sqlite` feature, which once made
        // this record look like a database.
        let features = dir.join("features.toml");
        std::fs::write(&features, "[features]\nsqlite = [\"nu-command/sqlite\"]\n").unwrap();
        assert_eq!(
            explore_file(&nu, &features).unwrap().data,
            serde_json::json!({"features": {"sqlite": ["nu-command/sqlite"]}})
        );

        let lines = dir.join("events.jsonl");
        std::fs::write(&lines, "{\"a\":1}\n{\"a\":2}\n").unwrap();
        let opened = explore_file(&nu, &lines).unwrap();
        assert_eq!(opened.rows, Some(2), "each line is a row");
        assert_eq!(opened.data[1]["a"], 2);

        let broken = dir.join("broken.toml");
        std::fs::write(&broken, "a = [\n").unwrap();
        let err = explore_file(&nu, &broken).unwrap_err();
        assert!(err.to_lowercase().contains("toml"), "{err}");
        assert!(err.contains("line 1"), "the parser's position: {err}");
        assert!(
            !err.contains("Environment") && !err.contains("FFF_"),
            "no environment in the message: {err}"
        );

        let junk = dir.join("junk.db");
        std::fs::write(&junk, [0u8, 0xff, 0x10, 0x80]).unwrap();
        assert_eq!(
            explore_file(&nu, &junk).unwrap_err(),
            "Nushell reads this file as raw bytes, not as data."
        );

        // `open` only gets text back when no `from` command reads the type.
        let text = dir.join("notes.db");
        std::fs::write(&text, "not a database\n").unwrap();
        let err = explore_file(&nu, &text).unwrap_err();
        assert!(err.contains("no `from db` command"), "{err}");

        let err = explore_file(&nu, &dir.join("missing.json")).unwrap_err();
        assert!(!err.contains("Environment"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plugins_load_only_when_asked() {
        let cwd = Path::new(".");
        let args = |job: Job| -> Vec<String> {
            let cmd = job.command(Path::new("nu"));
            cmd.get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            args(Job::new("ls", cwd)),
            ["--no-config-file", "-c", "ls"],
            "a plain run loads nothing of the user's"
        );
        assert_eq!(
            args(Job {
                plugins: true,
                stdin: Some("[]".into()),
                ..Job::new("ls", cwd)
            }),
            ["--stdin", "-c", "ls"],
            "`nu -c` reads the plugin registry, and still no config files"
        );
    }

    #[test]
    fn error_text_keeps_the_headline_and_deepest_reason() {
        let toml = serde_json::json!({
            "msg": "Error while parsing as toml",
            "labels": [{"text": "Could not parse 'a.toml' with `from toml`"}],
            "help": "Check out `help from toml`",
            "inner": [{
                "msg": "Can't convert to structured toml data.",
                "labels": [{"text": "can't convert string to structured toml data"}],
                "help": "TOML parse error at line 1, column 6\nunclosed array\n",
                "inner": []
            }]
        });
        assert_eq!(
            error_text(&toml),
            "Error while parsing as toml\nTOML parse error at line 1, column 6\nunclosed array"
        );

        let json = serde_json::json!({
            "msg": "Error while parsing as json",
            "inner": [{
                "msg": "Error while parsing JSON text",
                "labels": [{"text": "error parsing JSON text"}],
                "help": null,
                "inner": [{"labels": [{"text": "EOF at line 1 column 10"}], "inner": []}]
            }]
        });
        assert_eq!(
            error_text(&json),
            "Error while parsing as json\nEOF at line 1 column 10"
        );

        let denied = serde_json::json!({
            "msg": "I/O error",
            "labels": [{"text": "Permission denied"}],
            "help": "The error occurred at '/secret/path'",
            "inner": []
        });
        assert_eq!(error_text(&denied), "I/O error\nPermission denied");

        let missing =
            serde_json::json!({"msg": "File not found", "labels": [{"text": "file not found"}]});
        assert_eq!(error_text(&missing), "File not found", "no repeated reason");
        assert_eq!(
            error_text(&serde_json::json!({})),
            "Nushell couldn't open the file."
        );
    }
}
