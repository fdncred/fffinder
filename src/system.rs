//! Handing things to the operating system: open a file with its default
//! app or a text editor, show it in Finder/Explorer, or start a terminal. Tests install a
//! [`Recorder`] instead, so nothing opens on the machine running them.
//! (GPUI's test platform doesn't implement the first two at all.) Files
//! copied to the clipboard live here too.

use fffinder_core::terminals::Plan;
use gpui_kit::{App, AppContext as _, ClipboardEntry, ClipboardItem, Global, Task};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemCall {
    Open(PathBuf),
    /// Opened in the system's text editor (`open -t`, Notepad).
    EditText(PathBuf),
    Reveal(PathBuf),
    Terminal {
        program: PathBuf,
        args: Vec<String>,
    },
}

/// When set as a global, calls are recorded here and not performed.
#[derive(Clone, Default)]
pub struct Recorder {
    calls: Rc<RefCell<Vec<SystemCall>>>,
    /// Files that no app claims.
    no_app: Rc<RefCell<HashSet<PathBuf>>>,
}

impl Global for Recorder {}

impl Recorder {
    pub fn calls(&self) -> Vec<SystemCall> {
        self.calls.borrow().clone()
    }

    /// Act as if no app is registered to open `path`.
    pub fn set_no_app(&self, path: impl Into<PathBuf>) {
        self.no_app.borrow_mut().insert(path.into());
    }

    fn record(&self, call: SystemCall) {
        self.calls.borrow_mut().push(call);
    }
}

pub fn open(path: &Path, cx: &mut App) {
    match cx.try_global::<Recorder>() {
        Some(recorder) => recorder.record(SystemCall::Open(path.into())),
        None => cx.open_with_system(path),
    }
}

/// Whether an app is registered to open the file at `path`; `None` when the
/// system can't tell (Windows asks with its own "Open with" chooser).
pub fn has_default_app(path: &Path, cx: &App) -> Option<bool> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        return Some(!recorder.no_app.borrow().contains(path));
    }
    #[cfg(target_os = "macos")]
    {
        Some(crate::macos::has_default_app(path))
    }
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        xdg_has_default_app(path)
    }
}

/// Asks `xdg-mime` for the file's type and the app set for that type.
#[cfg(not(any(target_os = "macos", windows)))]
fn xdg_has_default_app(path: &Path) -> Option<bool> {
    let query = |args: &[&std::ffi::OsStr]| {
        let out = std::process::Command::new("xdg-mime")
            .arg("query")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let mime = query(&["filetype".as_ref(), path.as_os_str()]).filter(|m| !m.is_empty())?;
    Some(!query(&["default".as_ref(), mime.as_ref()])?.is_empty())
}

/// Open `path` in the system's text editor: whatever opens plain text on
/// macOS, Notepad on Windows. `None` where there isn't one (Linux).
pub fn edit_text(path: &Path, cx: &mut App) -> Option<Task<std::io::Result<()>>> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        recorder.record(SystemCall::EditText(path.into()));
        return Some(Task::ready(Ok(())));
    }
    let path = path.to_path_buf();
    if cfg!(target_os = "macos") {
        Some(cx.background_spawn(async move {
            let out = std::process::Command::new("open")
                .args(["-t", "--"])
                .arg(&path)
                .output()?;
            if out.status.success() {
                Ok(())
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(std::io::Error::other(err))
            }
        }))
    } else if cfg!(windows) {
        Some(cx.background_spawn(async move {
            std::process::Command::new("notepad")
                .arg(&path)
                .spawn()
                .map(drop)
        }))
    } else {
        None
    }
}

/// Start a terminal. The plan may wait on AppleScript (and a permission
/// prompt), so it runs on a background thread; errors come back in the task.
pub fn launch(plan: Plan, cx: &mut App) -> Task<std::io::Result<()>> {
    match cx.try_global::<Recorder>() {
        Some(recorder) => {
            recorder.record(SystemCall::Terminal {
                program: plan.program.clone(),
                args: plan.args.clone(),
            });
            Task::ready(Ok(()))
        }
        None => cx.background_spawn(async move { plan.run() }),
    }
}

pub fn reveal(path: &Path, cx: &mut App) {
    match cx.try_global::<Recorder>() {
        Some(recorder) => recorder.record(SystemCall::Reveal(path.into())),
        None => cx.reveal_path(path),
    }
}

/// Files copied in fffinder, so a paste in any window finds them. The
/// system clipboard holds their paths as text, which says whether this copy
/// is still the latest.
struct FileClipboard {
    paths: Vec<PathBuf>,
    text: String,
}

impl Global for FileClipboard {}

/// Put files on the clipboard: as files for apps that take them (Finder,
/// on macOS) and as their paths, one per line, for text fields and
/// terminals.
pub fn copy_files(paths: &[PathBuf], cx: &mut App) {
    let text = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    cx.set_global(FileClipboard {
        paths: paths.to_vec(),
        text: text.clone(),
    });
    #[cfg(target_os = "macos")]
    if cx.try_global::<Recorder>().is_none() && crate::macos::write_file_clipboard(paths, &text) {
        return;
    }
    #[cfg(windows)]
    if cx.try_global::<Recorder>().is_none() && crate::win::write_file_clipboard(paths, &text) {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

/// The files named by clipboard text: GNOME Files' and Dolphin's copies
/// (an `x-special/…-clipboard` line, `copy` or `cut`, then `file://`
/// URIs), `file://` URIs, or absolute paths, one per line. Every line has
/// to name an item that exists; otherwise it's just text.
pub fn paths_in_text(text: &str) -> Vec<PathBuf> {
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.first().is_some_and(|l| l.starts_with("x-special/")) {
        lines.remove(0);
        if matches!(lines.first(), Some(&"copy" | &"cut")) {
            lines.remove(0);
        }
    }
    let paths: Option<Vec<PathBuf>> = lines
        .iter()
        .map(|line| match line.strip_prefix("file://") {
            Some(uri) => file_uri_path(uri),
            None => Some(PathBuf::from(line)).filter(|p| p.is_absolute()),
        })
        .collect();
    paths
        .filter(|paths| !paths.is_empty() && paths.iter().all(|p| p.symlink_metadata().is_ok()))
        .unwrap_or_default()
}

/// The path in a `file://` URI after the scheme: an optional host, then
/// percent-encoded path bytes (`/C:/Users/…` on Windows).
fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let path = &uri[uri.find('/')?..];
    let mut bytes = Vec::with_capacity(path.len());
    let mut rest = path.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        match (byte, tail) {
            (b'%', [hi, lo, more @ ..]) => {
                let hex = std::str::from_utf8(&[*hi, *lo]).ok()?.to_string();
                bytes.push(u8::from_str_radix(&hex, 16).ok()?);
                rest = more;
            }
            _ => {
                bytes.push(byte);
                rest = tail;
            }
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // `/C:/x` is a Windows drive path.
    let path = match path.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() && cfg!(windows) => &path[1..],
        _ => &path,
    };
    Some(PathBuf::from(path))
}

/// Files on the clipboard: copied in another app, or in fffinder.
pub fn clipboard_files(cx: &App) -> Vec<PathBuf> {
    let Some(item) = cx.read_from_clipboard() else {
        return Vec::new();
    };
    let external: Vec<PathBuf> = item
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
            ClipboardEntry::String(_) | ClipboardEntry::Image(_) => None,
        })
        .flatten()
        .cloned()
        .collect();
    if !external.is_empty() {
        return external;
    }
    match (cx.try_global::<FileClipboard>(), item.text()) {
        (Some(copied), Some(text)) if copied.text == text => copied.paths.clone(),
        // Other apps' copies, on Linux, and paths copied as text anywhere.
        (_, Some(text)) => paths_in_text(&text),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::paths_in_text;

    #[test]
    fn reads_file_lists_other_apps_copy() {
        let dir = std::env::temp_dir().join(format!("fffinder-clip {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.txt");
        std::fs::write(&file, "a").unwrap();
        let uri = format!(
            "file://{}",
            file.to_string_lossy()
                .replace('\\', "/")
                .replace(' ', "%20")
        );
        let uri = if cfg!(windows) {
            uri.replacen("file://", "file:///", 1)
        } else {
            uri
        };
        // GNOME Files' format, a bare URI list, and a plain path.
        let gnome = format!("x-special/nautilus-clipboard\ncopy\n{uri}\n");
        assert_eq!(paths_in_text(&gnome), std::slice::from_ref(&file));
        assert_eq!(paths_in_text(&uri), std::slice::from_ref(&file));
        assert_eq!(
            paths_in_text(&file.to_string_lossy()),
            std::slice::from_ref(&file)
        );
        assert!(paths_in_text("hello").is_empty());
        assert!(paths_in_text(&format!("{}\nnot a path", file.display())).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
