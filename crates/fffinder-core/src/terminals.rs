//! Terminals for "Open Nushell Here": finding the ones installed and
//! starting one in a folder, running `nu`.
//!
//! Each platform's own record of what is installed comes first: app bundles'
//! `Info.plist` on macOS, `.desktop` entries in the `TerminalEmulator`
//! category on Linux, and the usual install folders on Windows. `PATH` adds
//! programs installed without those, and the search index adds bundles,
//! AppImages, and programs anywhere in the indexed folders.
//!
//! Detection takes its inputs as arguments and compiles on every platform,
//! so the logic for all three is tested wherever the tests run.

use crate::app_list::{self, AppList};
use crate::engine::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// The key settings are stored under, as in `std::env::consts::OS`.
    pub fn key(self) -> &'static str {
        match self {
            Platform::MacOs => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::MacOs => "macOS",
            Platform::Linux => "Linux",
            Platform::Windows => "Windows",
        }
    }
}

/// The terminal setting for one operating system.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSetting {
    /// A detected terminal's id (`ghostty`, `windows-terminal`, …), or
    /// [`CUSTOM`]. `None` picks one automatically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The command for [`CUSTOM`]; `{dir}` and `{nu}` are replaced. It is
    /// kept when another terminal is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// The [`TerminalSetting::app`] value for a custom command.
pub const CUSTOM: &str = "custom";

/// How a terminal is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// macOS: `open -na <bundle> --args <args>`, so the terminal starts as
    /// its own app instead of as a child of fffinder.
    OpenApp { bundle: PathBuf, args: Vec<String> },
    /// macOS: open a `.command` script with the app. The script changes to
    /// the folder and runs `nu`. For Terminal, iTerm2, and terminals known
    /// only from their bundle.
    CommandFile { bundle: PathBuf },
    /// macOS Ghostty: a window in the running app through AppleScript, or a
    /// new instance when it isn't running or scripting isn't allowed. Its
    /// `-e` flag would ask "Allow Ghostty to Execute" every time.
    Ghostty { bundle: PathBuf },
    /// Run `program` with `args` from the folder.
    Exec { program: PathBuf, args: Vec<String> },
    /// Windows: `nu` in a new console window.
    Console,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// Saved in settings: a known terminal's id, or `app:<bundle id>` and
    /// `desktop:<entry>` for terminals found only through their metadata.
    pub id: String,
    pub name: String,
    /// The app bundle or program; `None` for the Windows console.
    pub location: Option<PathBuf>,
    pub launch: Launch,
}

/// A process to start, with the script it needs written first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub script: Option<Script>,
    /// Windows: give the process its own console window.
    pub new_console: bool,
    /// Started when this process exits with an error. When set, `run`
    /// waits for this process, so call it off the UI thread.
    pub fallback: Option<Box<Plan>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub path: PathBuf,
    pub text: String,
}

/// How a known terminal starts on macOS.
#[derive(PartialEq)]
enum Mac {
    /// `open -na <bundle> --args <args>`.
    Open,
    /// A `.command` script opened with the app.
    Script,
    Ghostty,
}

struct Known {
    id: &'static str,
    name: &'static str,
    /// macOS bundle identifiers.
    bundle_ids: &'static [&'static str],
    /// Program names (Linux and Windows, without `.exe`); on macOS also
    /// matched against the bundle's file name.
    programs: &'static [&'static str],
    /// `{dir}` and `{nu}` are replaced.
    args: &'static [&'static str],
    mac: Mac,
    /// Picked automatically only when nothing earlier is installed.
    novelty: bool,
}

/// Known terminals, in the order "Automatic" prefers them: ones you
/// installed yourself before the one that came with the system.
const KNOWN: &[Known] = &[
    Known {
        id: "ghostty",
        name: "Ghostty",
        bundle_ids: &["com.mitchellh.ghostty"],
        programs: &["ghostty"],
        args: &["--working-directory={dir}", "-e", "{nu}"],
        mac: Mac::Ghostty,
        novelty: false,
    },
    Known {
        id: "windows-terminal",
        name: "Windows Terminal",
        bundle_ids: &[],
        programs: &["wt"],
        args: &["-d", "{dir}", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "iterm2",
        name: "iTerm2",
        bundle_ids: &["com.googlecode.iterm2"],
        programs: &["iterm"],
        args: &[],
        mac: Mac::Script,
        novelty: false,
    },
    Known {
        id: "wezterm",
        name: "WezTerm",
        bundle_ids: &["com.github.wez.wezterm"],
        programs: &["wezterm"],
        args: &["start", "--cwd", "{dir}", "--", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "kitty",
        name: "kitty",
        bundle_ids: &["net.kovidgoyal.kitty"],
        programs: &["kitty"],
        args: &["--directory", "{dir}", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "alacritty",
        name: "Alacritty",
        bundle_ids: &["org.alacritty", "io.alacritty"],
        programs: &["alacritty"],
        args: &["--working-directory", "{dir}", "-e", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "rio",
        name: "Rio",
        bundle_ids: &["com.raphaelamorim.rio"],
        programs: &["rio"],
        args: &["--working-dir", "{dir}", "-e", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "foot",
        name: "foot",
        bundle_ids: &[],
        programs: &["foot"],
        args: &["--working-directory={dir}", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "gnome-terminal",
        name: "GNOME Terminal",
        bundle_ids: &[],
        programs: &["gnome-terminal"],
        args: &["--working-directory={dir}", "--", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "konsole",
        name: "Konsole",
        bundle_ids: &[],
        programs: &["konsole"],
        args: &["--workdir", "{dir}", "-e", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "xfce4-terminal",
        name: "Xfce Terminal",
        bundle_ids: &[],
        programs: &["xfce4-terminal"],
        args: &["--working-directory={dir}", "-x", "{nu}"],
        mac: Mac::Open,
        novelty: false,
    },
    Known {
        id: "terminal",
        name: "Terminal",
        bundle_ids: &["com.apple.Terminal"],
        programs: &[],
        args: &[],
        mac: Mac::Script,
        novelty: false,
    },
    Known {
        id: "xterm",
        name: "XTerm",
        bundle_ids: &[],
        programs: &["xterm"],
        args: &["-e", "{nu}"],
        mac: Mac::Open,
        novelty: true,
    },
];

fn known(id: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.id == id)
}

fn known_program(program: &str) -> Option<&'static Known> {
    let program = program.to_ascii_lowercase();
    KNOWN
        .iter()
        .find(|k| k.programs.iter().any(|p| *p == program))
}

fn rank(terminal: &Terminal) -> usize {
    KNOWN
        .iter()
        .position(|k| k.id == terminal.id)
        .unwrap_or(KNOWN.len())
}

/// Known terminals first, in preference order, then the others by name.
fn sort(found: &mut [Terminal]) {
    found.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn program_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

// ----- detection --------------------------------------------------------------------

/// Installed terminals for this platform, in preference order. With an
/// engine, terminals anywhere in the indexed folders are included too.
pub fn detect(engine: Option<&Engine>) -> Vec<Terminal> {
    let path = search_path();
    match Platform::current() {
        Platform::MacOs => {
            let indexed = engine
                .map(|e| e.glob("**/*.app/Contents/Info.plist", 5000))
                .unwrap_or_default();
            let bundles: Vec<PathBuf> = indexed
                .iter()
                .filter_map(|plist| plist.parent()?.parent().map(Path::to_path_buf))
                .collect();
            detect_mac(&app_list::mac_app_dirs(), &bundles, &path)
        }
        Platform::Linux => {
            let appimages = engine
                .map(|e| e.glob("**/*.AppImage", 500))
                .unwrap_or_default();
            detect_linux(&desktop_dirs(), &path, &appimages)
        }
        Platform::Windows => {
            let mut indexed = Vec::new();
            if let Some(engine) = engine {
                for program in ["wezterm", "alacritty"] {
                    indexed.extend(engine.glob(&format!("**/{program}.exe"), 50));
                }
            }
            detect_windows(&windows_dirs(), &path, &indexed)
        }
    }
}

/// `PATH`, plus where package managers put programs that a GUI app's
/// minimal `PATH` may lack.
pub fn search_path() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/snap/bin",
    ] {
        dirs.push(PathBuf::from(extra));
    }
    if let Some(home) = dirs::home_dir() {
        for extra in [
            ".cargo/bin",
            ".local/bin",
            ".nix-profile/bin",
            "scoop/shims",
        ] {
            dirs.push(home.join(extra));
        }
    }
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

fn find_program(path: &[PathBuf], name: &str, exe: bool) -> Option<PathBuf> {
    let file = if exe {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    path.iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

// macOS ------------------------------------------------------------------------

/// What an app bundle's `Info.plist` says about it.
#[derive(Debug, Default)]
struct BundleInfo {
    id: String,
    name: String,
    services: Vec<String>,
    /// Document types: UTIs and file extensions, lowercased.
    documents: Vec<String>,
}

fn read_bundle(bundle: &Path) -> Option<BundleInfo> {
    let value = plist::Value::from_file(bundle.join("Contents/Info.plist")).ok()?;
    let dict = value.as_dictionary()?;
    let text = |key: &str| {
        dict.get(key)
            .and_then(|v| v.as_string())
            .map(str::to_string)
    };
    let stem = bundle
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut info = BundleInfo {
        id: text("CFBundleIdentifier").unwrap_or_default(),
        name: text("CFBundleDisplayName")
            .or_else(|| text("CFBundleName"))
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(stem),
        ..Default::default()
    };
    let array = |v: &plist::Value| v.as_array().cloned().unwrap_or_default();
    if let Some(services) = dict.get("NSServices") {
        for service in array(services) {
            let item = service
                .as_dictionary()
                .and_then(|s| s.get("NSMenuItem"))
                .and_then(|m| m.as_dictionary())
                .and_then(|m| m.get("default"))
                .and_then(|d| d.as_string());
            info.services.extend(item.map(str::to_string));
        }
    }
    if let Some(types) = dict.get("CFBundleDocumentTypes") {
        for doc in array(types) {
            let Some(doc) = doc.as_dictionary() else {
                continue;
            };
            for key in ["LSItemContentTypes", "CFBundleTypeExtensions"] {
                if let Some(values) = doc.get(key) {
                    info.documents.extend(
                        array(values)
                            .iter()
                            .filter_map(|v| v.as_string())
                            .map(str::to_ascii_lowercase),
                    );
                }
            }
        }
    }
    Some(info)
}

/// An app the system offers as a terminal: it adds a "New … Window Here"
/// (or "New Terminal at Folder") service and opens shell scripts.
fn looks_like_terminal(info: &BundleInfo) -> bool {
    let folder_service = info.services.iter().any(|s| {
        let s = s.to_lowercase();
        s.contains("window here") || s.contains("tab here") || s.contains("terminal at folder")
    });
    let runs_scripts = info.documents.iter().any(|d| {
        matches!(
            d.as_str(),
            "public.unix-executable"
                | "public.shell-script"
                | "com.apple.terminal.shell-script"
                | "command"
                | "sh"
                | "tool"
        )
    });
    folder_service && runs_scripts
}

fn inside_bundle(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .any(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
}

fn mac_terminal(bundle: &Path, info: &BundleInfo) -> Option<Terminal> {
    let stem = program_name(bundle);
    let known = KNOWN.iter().find(|k| {
        k.bundle_ids.contains(&info.id.as_str())
            || (!k.programs.is_empty() && k.programs.iter().any(|p| *p == stem))
    });
    let (id, name, launch) = match known {
        Some(k) if k.mac == Mac::Ghostty => (
            k.id.to_string(),
            k.name.to_string(),
            Launch::Ghostty {
                bundle: bundle.into(),
            },
        ),
        Some(k) if k.mac == Mac::Script => (
            k.id.to_string(),
            k.name.to_string(),
            Launch::CommandFile {
                bundle: bundle.into(),
            },
        ),
        Some(k) => (
            k.id.to_string(),
            k.name.to_string(),
            Launch::OpenApp {
                bundle: bundle.into(),
                args: strings(k.args),
            },
        ),
        None if looks_like_terminal(info) => (
            format!("app:{}", if info.id.is_empty() { &stem } else { &info.id }),
            info.name.clone(),
            Launch::CommandFile {
                bundle: bundle.into(),
            },
        ),
        None => return None,
    };
    Some(Terminal {
        id,
        name,
        location: Some(bundle.into()),
        launch,
    })
}

/// Terminals among the apps in `app_dirs` (and their plain subfolders, as
/// [`AppList`] reads them) and the `extra` bundles (found by the index),
/// plus known terminals on `path` that have no bundle.
pub fn detect_mac(app_dirs: &[PathBuf], extra: &[PathBuf], path: &[PathBuf]) -> Vec<Terminal> {
    let list = AppList::scan(app_dirs);
    let mut bundles: Vec<PathBuf> = list.apps().cloned().collect();
    bundles.extend(extra.iter().filter(|b| !inside_bundle(b)).cloned());

    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for bundle in bundles {
        let Some(info) = read_bundle(&bundle) else {
            continue;
        };
        if let Some(terminal) = mac_terminal(&bundle, &info)
            && seen.insert(terminal.id.clone())
        {
            found.push(terminal);
        }
    }
    // Programs installed without an app, e.g. with `cargo install`.
    for k in KNOWN {
        if k.mac == Mac::Script || seen.contains(k.id) {
            continue;
        }
        if let Some(program) = k.programs.iter().find_map(|p| find_program(path, p, false)) {
            seen.insert(k.id.to_string());
            found.push(Terminal {
                id: k.id.into(),
                name: k.name.into(),
                location: Some(program.clone()),
                launch: Launch::Exec {
                    program,
                    args: strings(k.args),
                },
            });
        }
    }
    sort(&mut found);
    found
}

// Linux ------------------------------------------------------------------------

/// Where `.desktop` entries live: `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`
/// `applications` folders, and snap's.
pub fn desktop_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut dirs = vec![data_home.join("applications")];
    dirs.extend(
        data_dirs
            .split(':')
            .map(|d| Path::new(d).join("applications")),
    );
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

/// The parts of a `.desktop` file this needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub name: String,
    /// `Exec` split into words, without field codes like `%U`.
    pub exec: Vec<String>,
    /// `Exec` as written, field codes and all.
    pub exec_line: String,
    pub try_exec: Option<String>,
    /// The MIME types it opens.
    pub mime_types: Vec<String>,
    pub terminal_emulator: bool,
    /// `NoDisplay` or `Hidden`.
    pub hidden: bool,
}

/// Parse the `[Desktop Entry]` group of a `.desktop` file.
pub fn parse_desktop_entry(text: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut exec_line = String::new();
    let mut try_exec = None;
    let mut mime_types = Vec::new();
    let mut is_app = false;
    let mut terminal_emulator = false;
    let mut hidden = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => name = Some(value.to_string()),
            "Exec" => {
                exec = Some(split_exec(value));
                exec_line = value.to_string();
            }
            "MimeType" => {
                mime_types = value
                    .split(';')
                    .filter(|m| !m.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            "TryExec" => try_exec = Some(value.to_string()),
            "Type" => is_app = value == "Application",
            "Categories" => terminal_emulator = value.split(';').any(|c| c == "TerminalEmulator"),
            "NoDisplay" | "Hidden" => hidden |= value == "true",
            _ => {}
        }
    }
    let exec = exec.filter(|e| !e.is_empty())?;
    is_app.then_some(DesktopEntry {
        name: name?,
        exec,
        exec_line,
        try_exec,
        mime_types,
        terminal_emulator,
        hidden,
    })
}

/// Split an `Exec` value into words: double quotes group, a backslash
/// escapes, and field codes (`%f`, `%U`, …) are dropped.
fn split_exec(exec: &str) -> Vec<String> {
    exec_words(exec)
        .into_iter()
        .filter(|w| !is_field_code(w))
        .map(|w| w.replace("%%", "%"))
        .collect()
}

/// `%f`, `%U`, and the other two-letter field codes of an `Exec` line.
pub fn is_field_code(word: &str) -> bool {
    word.len() == 2 && word.starts_with('%') && word != "%%"
}

/// Split an `Exec` value into words, keeping field codes and `%%`.
pub fn exec_words(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut in_word = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' if quoted => {
                word.extend(chars.next());
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

fn resolve_program(word: &str, path: &[PathBuf]) -> Option<PathBuf> {
    let candidate = Path::new(word);
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    find_program(path, word, false)
}

/// Terminals from `.desktop` entries in `desktop_dirs`, known terminals on
/// `path`, and terminal AppImages from the index.
pub fn detect_linux(
    desktop_dirs: &[PathBuf],
    path: &[PathBuf],
    appimages: &[PathBuf],
) -> Vec<Terminal> {
    let mut found: Vec<Terminal> = Vec::new();
    let mut seen = HashSet::new();
    let mut entries_seen = HashSet::new();
    for dir in desktop_dirs {
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "desktop"))
            .collect();
        files.sort();
        for file in files {
            // The first directory wins, as the spec says.
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !entries_seen.insert(stem.clone()) {
                continue;
            }
            let Some(entry) = std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| parse_desktop_entry(&t))
            else {
                continue;
            };
            if !entry.terminal_emulator || entry.hidden {
                continue;
            }
            if let Some(try_exec) = &entry.try_exec
                && resolve_program(try_exec, path).is_none()
            {
                continue;
            }
            let mut words = entry.exec.iter().map(String::as_str);
            let mut first = words.next().unwrap_or_default();
            // `env VAR=value program …`
            if first == "env" {
                first = words.find(|w| !w.contains('=')).unwrap_or_default();
            }
            // Flatpak terminals run sandboxed, where the host's `nu` is out of reach.
            if program_name(Path::new(first)) == "flatpak" {
                continue;
            }
            let Some(program) = resolve_program(first, path) else {
                continue;
            };
            let terminal = match known_program(&program_name(&program)) {
                Some(k) => Terminal {
                    id: k.id.into(),
                    name: k.name.into(),
                    location: Some(program.clone()),
                    launch: Launch::Exec {
                        program,
                        args: strings(k.args),
                    },
                },
                None => {
                    // Most terminals take `-e command` and start in their own
                    // working directory, which is the folder.
                    let mut args: Vec<String> = words.map(str::to_string).collect();
                    args.extend(["-e".to_string(), "{nu}".to_string()]);
                    Terminal {
                        id: format!("desktop:{stem}"),
                        name: entry.name.clone(),
                        location: Some(program.clone()),
                        launch: Launch::Exec { program, args },
                    }
                }
            };
            if seen.insert(terminal.id.clone()) {
                found.push(terminal);
            }
        }
    }
    for k in KNOWN {
        if k.mac == Mac::Script || k.id == "windows-terminal" || seen.contains(k.id) {
            continue;
        }
        let program = k
            .programs
            .iter()
            .find_map(|p| find_program(path, p, false))
            .or_else(|| {
                appimages
                    .iter()
                    .find(|a| {
                        let name = a.to_string_lossy().to_lowercase();
                        let file = program_name(a);
                        name.ends_with(".appimage")
                            && k.programs.iter().any(|p| file.starts_with(p))
                    })
                    .cloned()
            });
        if let Some(program) = program {
            seen.insert(k.id.to_string());
            found.push(Terminal {
                id: k.id.into(),
                name: k.name.into(),
                location: Some(program.clone()),
                launch: Launch::Exec {
                    program,
                    args: strings(k.args),
                },
            });
        }
    }
    // Debian's configured default.
    if let Some(program) = find_program(path, "x-terminal-emulator", false) {
        found.push(Terminal {
            id: "x-terminal-emulator".into(),
            name: "System Default (x-terminal-emulator)".into(),
            location: Some(program.clone()),
            launch: Launch::Exec {
                program,
                args: strings(&["-e", "{nu}"]),
            },
        });
    }
    sort(&mut found);
    found
}

// Windows ----------------------------------------------------------------------

/// Folders Windows terminals install into.
#[derive(Debug, Clone, Default)]
pub struct WindowsDirs {
    pub program_files: Vec<PathBuf>,
    pub local_app_data: Option<PathBuf>,
}

fn windows_dirs() -> WindowsDirs {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    WindowsDirs {
        program_files: ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(var)
            .collect(),
        local_app_data: var("LOCALAPPDATA"),
    }
}

/// Windows Terminal, WezTerm, and Alacritty from their install folders,
/// `path`, or the index (`indexed` executables); the console is always there.
pub fn detect_windows(dirs: &WindowsDirs, path: &[PathBuf], indexed: &[PathBuf]) -> Vec<Terminal> {
    let mut found = Vec::new();
    let mut add = |k: &Known, program: PathBuf| {
        found.push(Terminal {
            id: k.id.into(),
            name: k.name.into(),
            location: Some(program.clone()),
            launch: Launch::Exec {
                program,
                args: strings(k.args),
            },
        })
    };
    let installed = |rel: &str| {
        dirs.program_files
            .iter()
            .map(|d| d.join(rel))
            .find(|p| p.is_file())
    };

    let wt = dirs
        .local_app_data
        .as_ref()
        .map(|d| d.join("Microsoft/WindowsApps/wt.exe"))
        .filter(|p| p.is_file())
        .or_else(|| find_program(path, "wt", true));
    if let (Some(k), Some(program)) = (known("windows-terminal"), wt) {
        add(k, program);
    }
    for (id, rel) in [
        ("wezterm", "WezTerm/wezterm.exe"),
        ("alacritty", "Alacritty/alacritty.exe"),
    ] {
        let Some(k) = known(id) else { continue };
        let program = installed(rel)
            .or_else(|| find_program(path, id, true))
            .or_else(|| {
                indexed
                    .iter()
                    .find(|p| program_name(p) == id && p.is_file())
                    .cloned()
            });
        if let Some(program) = program {
            add(k, program);
        }
    }
    found.push(Terminal {
        id: "console".into(),
        name: "Console Window".into(),
        location: None,
        launch: Launch::Console,
    });
    found
}

// ----- choosing and launching ---------------------------------------------------------

/// What "Automatic" picks: on Linux `$TERMINAL` first; otherwise the first
/// installed terminal, skipping the novelty ones while anything else is there.
pub fn automatic<'a>(found: &'a [Terminal], env_terminal: Option<&str>) -> Option<&'a Terminal> {
    if let Some(wanted) = env_terminal.map(|t| program_name(Path::new(t.trim())))
        && !wanted.is_empty()
        && let Some(t) = found.iter().find(|t| {
            t.location
                .as_deref()
                .is_some_and(|l| program_name(l) == wanted)
        })
    {
        return Some(t);
    }
    found
        .iter()
        .find(|t| known(&t.id).is_none_or(|k| !k.novelty) && !t.id.starts_with("app:"))
        .or_else(|| found.first())
}

/// The terminal a setting refers to, and a warning when it had to fall
/// back because the chosen one isn't installed.
pub fn resolve<'a>(
    found: &'a [Terminal],
    setting: &TerminalSetting,
    env_terminal: Option<&str>,
) -> Option<(&'a Terminal, Option<String>)> {
    let fallback = automatic(found, env_terminal)?;
    match setting.app.as_deref() {
        None | Some(CUSTOM) => Some((fallback, None)),
        Some(id) => match found.iter().find(|t| t.id == id) {
            Some(t) => Some((t, None)),
            None => Some((
                fallback,
                Some(format!(
                    "The terminal chosen in Settings ({id}) isn't installed; used {}.",
                    fallback.name
                )),
            )),
        },
    }
}

fn fill(template: &str, nu: &Path, dir: &Path) -> String {
    template
        .replace("{dir}", &dir.to_string_lossy())
        .replace("{nu}", &nu.to_string_lossy())
}

static SCRIPTS: AtomicU64 = AtomicU64::new(0);

/// Quote for `/bin/sh`.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Opens a Ghostty window in `item 1 of argv` running `item 2`, failing when
/// Ghostty isn't running (or is too old to script) so the caller falls back.
const GHOSTTY_SCRIPT: &[&str] = &[
    "on run argv",
    "if application id \"com.mitchellh.ghostty\" is not running then error \"not running\"",
    "tell application id \"com.mitchellh.ghostty\"",
    "set cfg to new surface configuration",
    "set initial working directory of cfg to item 1 of argv",
    "set command of cfg to item 2 of argv",
    "new window with configuration cfg",
    "activate",
    "end tell",
    "end run",
];

/// What a terminal runs: `nu` for "Open Nushell Here", or an editor and its
/// file for "Open in Editor".
#[derive(Debug, Clone, Copy)]
struct Run<'a> {
    program: &'a Path,
    args: &'a [String],
    /// Start `nu` as a login shell where the launch decides the flags.
    login: bool,
}

impl Run<'_> {
    /// Template arguments with `{dir}` filled in and a lone `{nu}` replaced by
    /// the program followed by its arguments.
    fn fill(&self, template: &[String], dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for arg in template {
            if arg == "{nu}" {
                out.push(self.program.to_string_lossy().into_owned());
                out.extend(self.args.iter().cloned());
            } else {
                out.push(fill(arg, self.program, dir));
            }
        }
        out
    }

    /// The program and its arguments as one `/bin/sh` command.
    fn shell_words(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|w| sh_quote(&w))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Terminal {
    /// The process that opens this terminal in `dir`, running `nu`.
    pub fn plan(&self, nu: &Path, dir: &Path) -> Plan {
        self.plan_run(
            Run {
                program: nu,
                args: &[],
                login: true,
            },
            dir,
        )
    }

    /// The process that opens this terminal in `dir`, running `program` with
    /// `args` (an editor and a file, say).
    pub fn plan_command(&self, program: &Path, args: &[String], dir: &Path) -> Plan {
        self.plan_run(
            Run {
                program,
                args,
                login: false,
            },
            dir,
        )
    }

    fn plan_run(&self, run: Run<'_>, dir: &Path) -> Plan {
        let cwd = dir.to_path_buf();
        match &self.launch {
            Launch::OpenApp { bundle, args } => {
                let mut all = vec![
                    "-na".to_string(),
                    bundle.to_string_lossy().into_owned(),
                    "--args".to_string(),
                ];
                all.extend(run.fill(args, dir));
                Plan {
                    program: "/usr/bin/open".into(),
                    args: all,
                    cwd,
                    script: None,
                    new_console: false,
                    fallback: None,
                }
            }
            Launch::Ghostty { bundle } => {
                let command = run.shell_words();
                let dir_text = dir.to_string_lossy().into_owned();
                let mut args: Vec<String> = GHOSTTY_SCRIPT
                    .iter()
                    .flat_map(|line| ["-e".to_string(), line.to_string()])
                    .collect();
                args.extend([dir_text.clone(), command.clone()]);
                let new_instance = Plan {
                    program: "/usr/bin/open".into(),
                    args: vec![
                        "-na".into(),
                        bundle.to_string_lossy().into_owned(),
                        "--args".into(),
                        format!("--working-directory={dir_text}"),
                        format!("--initial-command={command}"),
                    ],
                    cwd: cwd.clone(),
                    script: None,
                    new_console: false,
                    fallback: None,
                };
                Plan {
                    program: "/usr/bin/osascript".into(),
                    args,
                    cwd,
                    script: None,
                    new_console: false,
                    fallback: Some(Box::new(new_instance)),
                }
            }
            Launch::CommandFile { bundle } => {
                let n = SCRIPTS.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("fffinder-nu-{}-{n}.command", std::process::id()));
                let mut command = run.shell_words();
                if run.login && run.args.is_empty() {
                    command.push_str(" --login");
                }
                let text = format!(
                    "#!/bin/sh\ncd {} || exit 1\nrm -f {}\nexec {command}\n",
                    sh_quote(&dir.to_string_lossy()),
                    sh_quote(&path.to_string_lossy()),
                );
                Plan {
                    program: "/usr/bin/open".into(),
                    args: vec![
                        "-a".into(),
                        bundle.to_string_lossy().into_owned(),
                        path.to_string_lossy().into_owned(),
                    ],
                    cwd,
                    script: Some(Script { path, text }),
                    new_console: false,
                    fallback: None,
                }
            }
            Launch::Exec { program, args } => Plan {
                program: program.clone(),
                args: run.fill(args, dir),
                cwd,
                script: None,
                new_console: false,
                fallback: None,
            },
            Launch::Console => Plan {
                program: run.program.to_path_buf(),
                args: run.args.to_vec(),
                cwd,
                script: None,
                new_console: true,
                fallback: None,
            },
        }
    }
}

/// A custom command, split like a shell would (quotes group words), with
/// `{dir}` and `{nu}` replaced in each word.
pub fn custom_plan(template: &str, nu: &Path, dir: &Path) -> Option<Plan> {
    custom_plan_command(template, nu, &[], dir)
}

/// A custom command running `program` with `args` where it says `{nu}`.
pub fn custom_plan_command(
    template: &str,
    program: &Path,
    args: &[String],
    dir: &Path,
) -> Option<Plan> {
    let run = Run {
        program,
        args,
        login: false,
    };
    let words = run.fill(&split_command(template), dir);
    let (first, rest) = words.split_first()?;
    Some(Plan {
        program: first.into(),
        args: rest.to_vec(),
        cwd: dir.to_path_buf(),
        script: None,
        new_console: false,
        fallback: None,
    })
}

fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for c in command.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (_, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

impl Plan {
    /// Write the script, if any, and start the process.
    pub fn run(&self) -> std::io::Result<()> {
        if let Some(script) = &self.script {
            std::fs::write(&script.path, &script.text)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script.path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut command = Command::new(&self.program);
        command.args(&self.args).current_dir(&self.cwd);
        #[cfg(windows)]
        if self.new_console {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            command.creation_flags(CREATE_NEW_CONSOLE);
        }
        let Some(fallback) = &self.fallback else {
            return command.spawn().map(drop);
        };
        let ok = command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok { Ok(()) } else { fallback.run() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Tmp {
            let dir = std::env::temp_dir().join(format!("fff-terms-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }

        fn file(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        fn bundle(&self, rel: &str, id: &str, services: &[&str], documents: &[&str]) -> PathBuf {
            let mut dict = plist::Dictionary::new();
            dict.insert("CFBundleIdentifier".into(), id.into());
            let services: Vec<plist::Value> = services
                .iter()
                .map(|s| {
                    let mut item = plist::Dictionary::new();
                    item.insert("default".into(), (*s).into());
                    let mut service = plist::Dictionary::new();
                    service.insert("NSMenuItem".into(), item.into());
                    service.into()
                })
                .collect();
            dict.insert("NSServices".into(), services.into());
            let mut doc = plist::Dictionary::new();
            let types: Vec<plist::Value> = documents.iter().map(|d| (*d).into()).collect();
            doc.insert("LSItemContentTypes".into(), types.into());
            dict.insert(
                "CFBundleDocumentTypes".into(),
                vec![plist::Value::from(doc)].into(),
            );
            let bundle = self.0.join(rel);
            std::fs::create_dir_all(bundle.join("Contents")).unwrap();
            plist::Value::from(dict)
                .to_file_xml(bundle.join("Contents/Info.plist"))
                .unwrap();
            bundle
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ids(found: &[Terminal]) -> Vec<&str> {
        found.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn mac_finds_known_and_service_terminals() {
        let tmp = Tmp::new("mac");
        let apps = tmp.0.join("Applications");
        // One level down, as in `/System/Applications/Utilities`.
        tmp.bundle(
            "Applications/Utilities/Terminal.app",
            "com.apple.Terminal",
            &[],
            &[],
        );
        tmp.bundle(
            "Applications/Ghostty.app",
            "com.mitchellh.ghostty",
            &[],
            &[],
        );
        tmp.bundle(
            "Applications/Shellfish.app",
            "dev.example.shellfish",
            &["New Shellfish Window Here"],
            &["public.unix-executable"],
        );
        // An editor that opens scripts but offers no terminal service.
        tmp.bundle(
            "Applications/Editor.app",
            "dev.example.editor",
            &[],
            &["sh"],
        );
        // Found by the index, outside the app folders.
        let elsewhere = tmp.bundle("Downloads/WezTerm.app", "com.github.wez.wezterm", &[], &[]);
        let nested = tmp.bundle(
            "Applications/Ghostty.app/Contents/Helper.app",
            "dev.example.helper",
            &["New Helper Window Here"],
            &["sh"],
        );
        let found = detect_mac(
            std::slice::from_ref(&apps),
            &[elsewhere.clone(), nested],
            &[],
        );
        assert_eq!(
            ids(&found),
            [
                "ghostty",
                "wezterm",
                "terminal",
                "app:dev.example.shellfish"
            ]
        );
        assert_eq!(found[3].name, "Shellfish");
        assert_eq!(
            found[0].launch,
            Launch::Ghostty {
                bundle: apps.join("Ghostty.app")
            }
        );
        assert_eq!(
            found[1].launch,
            Launch::OpenApp {
                bundle: elsewhere.clone(),
                args: strings(&["start", "--cwd", "{dir}", "--", "{nu}"])
            }
        );
        assert_eq!(found[1].location.as_deref(), Some(elsewhere.as_path()));
        assert!(matches!(found[2].launch, Launch::CommandFile { .. }));
        assert_eq!(automatic(&found, None).unwrap().id, "ghostty");
    }

    #[test]
    fn mac_includes_programs_without_a_bundle() {
        let tmp = Tmp::new("mac-path");
        let bin = tmp.0.join("bin");
        tmp.file("bin/alacritty", "");
        let found = detect_mac(&[], &[], std::slice::from_ref(&bin));
        assert_eq!(ids(&found), ["alacritty"]);
        assert!(
            matches!(&found[0].launch, Launch::Exec { program, .. } if *program == bin.join("alacritty"))
        );
    }

    #[test]
    fn parses_desktop_entries() {
        let entry = parse_desktop_entry(
            "[Desktop Entry]\nType=Application\nName=Ptyxis\nName[de]=Ptyxis DE\n\
             Exec=ptyxis --new-window %U\nCategories=GNOME;GTK;System;TerminalEmulator;\n\
             [Desktop Action new-tab]\nExec=ptyxis --tab\n",
        )
        .unwrap();
        assert_eq!(entry.name, "Ptyxis");
        assert_eq!(entry.exec, ["ptyxis", "--new-window"]);
        assert!(entry.terminal_emulator && !entry.hidden);
        assert_eq!(
            split_exec(r#""/opt/My Term/term" --title "a \"b\"" 100%%"#),
            ["/opt/My Term/term", "--title", "a \"b\"", "100%"]
        );
        assert!(parse_desktop_entry("[Desktop Entry]\nType=Link\nName=x\nExec=x\n").is_none());
    }

    #[test]
    fn linux_finds_desktop_entries_path_programs_and_appimages() {
        let tmp = Tmp::new("linux");
        let bin = tmp.0.join("bin");
        for program in ["konsole", "ptyxis", "x-terminal-emulator", "foot", "gedit"] {
            tmp.file(&format!("bin/{program}"), "");
        }
        let apps = tmp.0.join("share/applications");
        let entry = |name: &str, exec: &str, extra: &str| {
            format!(
                "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n\
                 Categories=System;TerminalEmulator;\n{extra}"
            )
        };
        tmp.file(
            "share/applications/org.kde.konsole.desktop",
            &entry("Konsole", "konsole", ""),
        );
        tmp.file(
            "share/applications/org.gnome.Ptyxis.desktop",
            &entry("Ptyxis", "ptyxis --new-window", ""),
        );
        tmp.file(
            "share/applications/hidden.desktop",
            &entry("Hidden", "konsole", "NoDisplay=true\n"),
        );
        tmp.file(
            "share/applications/com.mitchellh.ghostty.desktop",
            &entry("Ghostty", "/usr/bin/flatpak run com.mitchellh.ghostty", ""),
        );
        tmp.file(
            "share/applications/gone.desktop",
            &entry("Gone", "gone-term", "TryExec=gone-term\n"),
        );
        tmp.file(
            "share/applications/gedit.desktop",
            "[Desktop Entry]\nType=Application\nName=Text Editor\nExec=gedit %U\n",
        );
        let appimage = tmp.file("Apps/WezTerm-20240203-Ubuntu20.04.AppImage", "");
        let found = detect_linux(
            &[apps],
            std::slice::from_ref(&bin),
            std::slice::from_ref(&appimage),
        );
        assert_eq!(
            ids(&found),
            [
                "wezterm",
                "foot",
                "konsole",
                "desktop:org.gnome.Ptyxis",
                "x-terminal-emulator"
            ]
        );
        let ptyxis = &found[3];
        assert_eq!(ptyxis.name, "Ptyxis");
        assert_eq!(
            ptyxis.launch,
            Launch::Exec {
                program: bin.join("ptyxis"),
                args: strings(&["--new-window", "-e", "{nu}"])
            }
        );
        assert_eq!(found[0].location.as_deref(), Some(appimage.as_path()));
        // `$TERMINAL` wins, then the first installed known terminal.
        assert_eq!(
            automatic(&found, Some("/usr/bin/konsole")).unwrap().id,
            "konsole"
        );
        assert_eq!(automatic(&found, None).unwrap().id, "wezterm");
    }

    #[test]
    fn windows_finds_install_folders_path_and_index() {
        let tmp = Tmp::new("windows");
        let dirs = WindowsDirs {
            program_files: vec![tmp.0.join("Program Files")],
            local_app_data: Some(tmp.0.join("AppData/Local")),
        };
        tmp.file("AppData/Local/Microsoft/WindowsApps/wt.exe", "");
        tmp.file("Program Files/WezTerm/wezterm.exe", "");
        let scoop = tmp.file("scoop/apps/alacritty/current/alacritty.exe", "");
        let found = detect_windows(&dirs, &[], std::slice::from_ref(&scoop));
        assert_eq!(
            ids(&found),
            ["windows-terminal", "wezterm", "alacritty", "console"]
        );
        assert_eq!(automatic(&found, None).unwrap().id, "windows-terminal");
        assert_eq!(found[2].location.as_deref(), Some(scoop.as_path()));
        let none = detect_windows(&WindowsDirs::default(), &[], &[]);
        assert_eq!(ids(&none), ["console"]);
    }

    #[test]
    fn plans_fill_in_the_folder_and_nu() {
        let nu = Path::new("/bin/nu");
        let dir = Path::new("/tmp/my dir");
        let ghostty = Terminal {
            id: "ghostty".into(),
            name: "Ghostty".into(),
            location: None,
            launch: Launch::OpenApp {
                bundle: "/Applications/Ghostty.app".into(),
                args: strings(&["--working-directory={dir}", "-e", "{nu}"]),
            },
        };
        let plan = ghostty.plan(nu, dir);
        assert_eq!(plan.program, Path::new("/usr/bin/open"));
        assert_eq!(
            plan.args,
            [
                "-na",
                "/Applications/Ghostty.app",
                "--args",
                "--working-directory=/tmp/my dir",
                "-e",
                "/bin/nu"
            ]
        );
        let terminal = Terminal {
            launch: Launch::CommandFile {
                bundle: "/System/Applications/Utilities/Terminal.app".into(),
            },
            ..ghostty.clone()
        };
        let plan = terminal.plan(nu, Path::new("/tmp/it's"));
        let script = plan.script.unwrap();
        assert!(script.text.contains(r"cd '/tmp/it'\''s'"));
        assert!(script.text.contains("exec '/bin/nu' --login"));
        assert_eq!(plan.args.last().unwrap(), &script.path.to_string_lossy());

        let ghostty = Terminal {
            launch: Launch::Ghostty {
                bundle: "/Applications/Ghostty.app".into(),
            },
            ..ghostty.clone()
        };
        let plan = ghostty.plan(Path::new("/opt/my tools/nu"), dir);
        assert_eq!(plan.program, Path::new("/usr/bin/osascript"));
        assert_eq!(
            &plan.args[plan.args.len() - 2..],
            ["/tmp/my dir", "'/opt/my tools/nu'"]
        );
        let fallback = plan.fallback.unwrap();
        assert_eq!(
            fallback.args,
            [
                "-na",
                "/Applications/Ghostty.app",
                "--args",
                "--working-directory=/tmp/my dir",
                "--initial-command='/opt/my tools/nu'"
            ]
        );

        // An editor and its file go where `{nu}` would.
        let file = vec!["/tmp/my dir/notes.md".to_string()];
        let kitty = Terminal {
            launch: Launch::Exec {
                program: "/usr/bin/kitty".into(),
                args: strings(&["--directory", "{dir}", "{nu}"]),
            },
            ..ghostty.clone()
        };
        let plan = kitty.plan_command(Path::new("/usr/bin/nvim"), &file, dir);
        assert_eq!(
            plan.args,
            [
                "--directory",
                "/tmp/my dir",
                "/usr/bin/nvim",
                "/tmp/my dir/notes.md"
            ]
        );
        let plan = ghostty.plan_command(Path::new("/usr/bin/nvim"), &file, dir);
        assert_eq!(
            plan.args.last().unwrap(),
            "'/usr/bin/nvim' '/tmp/my dir/notes.md'"
        );
        let script = terminal
            .plan_command(Path::new("/usr/bin/hx"), &file, dir)
            .script
            .unwrap();
        assert!(
            script
                .text
                .contains("exec '/usr/bin/hx' '/tmp/my dir/notes.md'\n")
        );
        let custom = custom_plan_command(
            "wezterm start --cwd {dir} -- {nu}",
            Path::new("nvim"),
            &file,
            dir,
        )
        .unwrap();
        assert_eq!(
            custom.args,
            [
                "start",
                "--cwd",
                "/tmp/my dir",
                "--",
                "nvim",
                "/tmp/my dir/notes.md"
            ]
        );

        let custom = custom_plan(r#""/opt/My Term/term" -d {dir} -e '{nu}'"#, nu, dir).unwrap();
        assert_eq!(custom.program, Path::new("/opt/My Term/term"));
        assert_eq!(custom.args, ["-d", "/tmp/my dir", "-e", "/bin/nu"]);
        assert!(custom_plan("   ", nu, dir).is_none());
    }

    #[test]
    fn resolve_falls_back_when_the_choice_is_gone() {
        let t = |id: &str| Terminal {
            id: id.into(),
            name: id.into(),
            location: None,
            launch: Launch::Console,
        };
        let found = vec![t("ghostty"), t("terminal")];
        let pick = |app: Option<&str>| {
            let setting = TerminalSetting {
                app: app.map(str::to_string),
                command: None,
            };
            resolve(&found, &setting, None).map(|(t, warn)| (t.id.clone(), warn.is_some()))
        };
        assert_eq!(pick(None), Some(("ghostty".into(), false)));
        assert_eq!(pick(Some("terminal")), Some(("terminal".into(), false)));
        assert_eq!(pick(Some("kitty")), Some(("ghostty".into(), true)));
        assert!(resolve(&[], &TerminalSetting::default(), None).is_none());
        // XTerm is picked only when nothing else is installed, and terminals
        // known only from their bundle never are.
        assert_eq!(automatic(&[t("xterm")], None).unwrap().id, "xterm");
        let several = vec![t("xterm"), t("app:dev.example.x"), t("desktop:ptyxis")];
        assert_eq!(automatic(&several, None).unwrap().id, "desktop:ptyxis");
    }
}
