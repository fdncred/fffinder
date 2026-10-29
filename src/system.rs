//! Handing things to the operating system: open a file with its default
//! app or a text editor, show it in Finder/Explorer, or start a terminal. Tests install a
//! [`Recorder`] instead, so nothing opens on the machine running them.
//! (GPUI's test platform doesn't implement the first two at all.) Files
//! copied to the clipboard live here too.

use fffinder_core::servers::ServerError;
use fffinder_core::terminals::Plan;
use gpui_kit::{App, AppContext as _, ClipboardEntry, ClipboardItem, Global, Task};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Why the system (AppKit, QuickLook, Launch Services, the Windows shell)
/// didn't do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum SystemError {
    /// The system's own description of what went wrong.
    #[error("{0}")]
    Refused(String),
    /// The part of the system named didn't answer in time.
    #[error("{0} didn’t answer")]
    NoAnswer(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(windows)]
    #[error("{}", .0.message())]
    Windows(#[from] windows::core::Error),
    #[cfg(windows)]
    #[error(transparent)]
    Png(#[from] png::EncodingError),
}

/// Why Connect to Server couldn't open a server.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Address(#[from] ServerError),
    /// This system can't mount this kind of server; what to do instead.
    #[error("{0}")]
    Unsupported(&'static str),
    /// The system's mounting tool failed, with its own message.
    #[error(transparent)]
    Mount(#[from] std::io::Error),
    /// The server was mounted, or seemed to be, but where isn't known.
    #[error("{0}")]
    NoMountPoint(String),
}

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
    /// Opened with a chosen app (Open With). An empty `app` is Windows'
    /// own chooser.
    OpenWith {
        app: PathBuf,
        paths: Vec<PathBuf>,
    },
    /// Made the default app for a file's type (Always Open With).
    SetDefaultApp {
        app: PathBuf,
        file: PathBuf,
    },
    /// Ejected or unmounted a volume.
    Eject(PathBuf),
    /// Connected to a server (Connect to Server), by the address typed.
    Connect(String),
}

/// When set as a global, calls are recorded here and not performed.
#[derive(Clone, Default)]
pub struct Recorder {
    calls: Rc<RefCell<Vec<SystemCall>>>,
    /// Files that no app claims.
    no_app: Rc<RefCell<HashSet<PathBuf>>>,
    /// What Open With lists, for every file.
    apps: Rc<RefCell<Vec<crate::apps::OpenApp>>>,
    /// Where Connect to Server says a server was mounted.
    mount_point: Rc<RefCell<Option<PathBuf>>>,
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

    /// Have Open With list `apps` for every file.
    pub fn set_apps(&self, apps: Vec<crate::apps::OpenApp>) {
        *self.apps.borrow_mut() = apps;
    }

    /// Have Connect to Server succeed, mounting at `path`.
    pub fn set_mount_point(&self, path: impl Into<PathBuf>) {
        *self.mount_point.borrow_mut() = Some(path.into());
    }

    pub(crate) fn apps(&self) -> Vec<crate::apps::OpenApp> {
        self.apps.borrow().clone()
    }

    pub(crate) fn record(&self, call: SystemCall) {
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
    Some(xdg_default_entry(path)?.is_some())
}

/// `xdg-mime query …`, trimmed; `None` when it fails or isn't installed.
#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn xdg_query(args: &[&std::ffi::OsStr]) -> Option<String> {
    let out = std::process::Command::new("xdg-mime")
        .arg("query")
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A file's MIME type: from its extension in shared-mime-info's database
/// when it has a known one, which needs no process, or else from
/// `xdg-mime`, which looks inside it. It may start a process, so call it
/// off the UI thread.
#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn mime_type(path: &Path) -> Option<String> {
    use fffinder_core::thumbnailers;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static GLOBS: OnceLock<HashMap<String, String>> = OnceLock::new();
    let globs = GLOBS.get_or_init(|| thumbnailers::mime_globs(&thumbnailers::data_dirs()));
    let name = path.file_name()?.to_string_lossy();
    match thumbnailers::mime_for_name(globs, &name) {
        Some(mime) => Some(mime.to_string()),
        None => xdg_query(&["filetype".as_ref(), path.as_os_str()]).filter(|m| !m.is_empty()),
    }
}

/// The `.desktop` entry set to open the file's type: `Some(None)` when the
/// type has none, `None` when `xdg-mime` can't tell.
#[cfg(not(any(target_os = "macos", windows)))]
fn xdg_default_entry(path: &Path) -> Option<Option<String>> {
    let mime = mime_type(path)?;
    let entry = xdg_query(&["default".as_ref(), mime.as_ref()])?;
    Some((!entry.is_empty()).then_some(entry))
}

/// The name of the app that opens `path` by default, when the system says.
/// It may start a process, so call it off the UI thread.
pub fn default_app_name(path: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::default_app(path)
            .and_then(|app| app.file_stem().map(|s| s.to_string_lossy().into_owned()))
    }
    #[cfg(windows)]
    {
        crate::win::default_app(path).map(|(name, _)| name)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let id = xdg_default_entry(path)??;
        let name = fffinder_core::terminals::desktop_dirs()
            .iter()
            .find_map(|dir| std::fs::read_to_string(dir.join(&id)).ok())
            .and_then(|text| fffinder_core::terminals::parse_desktop_entry(&text))
            .map(|entry| entry.name);
        Some(name.unwrap_or_else(|| id.trim_end_matches(".desktop").to_string()))
    }
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

/// Eject a removable disk, or unmount a server, at `path`.
pub fn eject(path: &Path, cx: &mut App) -> Task<std::io::Result<()>> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        recorder.record(SystemCall::Eject(path.into()));
        return Task::ready(Ok(()));
    }
    let path = path.to_path_buf();
    cx.background_spawn(async move { volumes::eject(&path) })
}

/// Mount the server at `address` the way the system does, and say where.
pub fn connect(address: &str, cx: &mut App) -> Task<Result<PathBuf, ConnectError>> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        recorder.record(SystemCall::Connect(address.into()));
        let mounted = recorder.mount_point.borrow().clone();
        return Task::ready(
            mounted.ok_or_else(|| ConnectError::NoMountPoint("The server didn’t answer".into())),
        );
    }
    let server = fffinder_core::servers::Server::parse(address);
    cx.background_spawn(async move { volumes::connect(&server?) })
}

/// Run a command; its error output becomes the error.
fn run(command: &mut std::process::Command) -> std::io::Result<String> {
    let out = command.stdin(std::process::Stdio::null()).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(std::io::Error::other(if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err
        }))
    }
}

#[cfg(target_os = "macos")]
mod volumes {
    use super::{ConnectError, run};
    use fffinder_core::servers::Server;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// `eject` takes a whole removable disk away; servers and disk images
    /// only unmount.
    pub fn eject(path: &Path) -> std::io::Result<()> {
        run(Command::new("diskutil").arg("eject").arg(path))
            .or_else(|_| run(Command::new("diskutil").arg("unmount").arg(path)))
            .map(drop)
    }

    /// AppleScript's `mount volume` asks for a password in the system's own
    /// dialog when the server needs one, and keeps it in the keychain.
    pub fn connect(server: &Server) -> Result<PathBuf, ConnectError> {
        let url = server.mac_url().ok_or(ConnectError::Unsupported(
            "macOS can’t mount SFTP servers itself. Mount them with a tool such as macFUSE’s sshfs, then open the folder.",
        ))?;
        let out = run(Command::new("osascript")
            .args(["-e", "on run argv"])
            .args(["-e", "return POSIX path of (mount volume (item 1 of argv))"])
            .args(["-e", "end run"])
            .arg(url))?;
        Ok(PathBuf::from(out.trim()))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod volumes {
    use super::{ConnectError, run};
    use fffinder_core::servers::{Server, gio_local_path};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// GIO ejects disks and unmounts servers; plain `umount` is the fallback.
    pub fn eject(path: &Path) -> std::io::Result<()> {
        run(Command::new("gio").args(["mount", "-e"]).arg(path))
            .or_else(|_| run(Command::new("umount").arg(path)))
            .map(drop)
    }

    /// `gio mount` mounts under `$XDG_RUNTIME_DIR/gvfs`, and `gio info`
    /// says where. Passwords come from the keyring, since there's no
    /// terminal to ask in.
    pub fn connect(server: &Server) -> Result<PathBuf, ConnectError> {
        let uri = server.gio_uri().ok_or(ConnectError::Unsupported(
            "This kind of server isn’t supported",
        ))?;
        // Already mounted is fine.
        let mounted = run(Command::new("gio").args(["mount", &uri]));
        let info = run(Command::new("gio").args(["info", &uri]));
        match (gio_local_path(info.as_deref().unwrap_or_default()), mounted) {
            (Some(path), _) => Ok(path),
            (None, Err(err)) => Err(err.into()),
            (None, Ok(_)) => Err(ConnectError::NoMountPoint(
                "GIO mounted the server but didn’t say where".into(),
            )),
        }
    }
}

#[cfg(windows)]
mod volumes {
    use super::{ConnectError, run};
    use fffinder_core::servers::{Server, net_use_drive};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A mapped network drive is disconnected with `net use`; a removable
    /// or optical drive is ejected with Explorer's own Eject command.
    pub fn eject(path: &Path) -> std::io::Result<()> {
        let drive: String = path
            .to_string_lossy()
            .chars()
            .take_while(|c| *c != '\\')
            .collect();
        if fffinder_core::places::is_network_drive(path) {
            return run(Command::new("net").args(["use", &drive, "/delete", "/y"])).map(drop);
        }
        let script = format!(
            "(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{drive}\\').InvokeVerb('Eject')"
        );
        run(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
            .map(drop)
    }

    /// File shares open as `\\server\share` paths; WebDAV gets a drive
    /// letter from `net use`.
    pub fn connect(server: &Server) -> Result<PathBuf, ConnectError> {
        if let Some(unc) = server.unc_path() {
            let path = PathBuf::from(unc);
            return match path.is_dir() {
                true => Ok(path),
                false => Err(ConnectError::NoMountPoint(format!(
                    "{} can’t be reached",
                    path.display()
                ))),
            };
        }
        let url = server.webdav_url().ok_or(ConnectError::Unsupported(
            "Windows can connect to SMB file shares and WebDAV servers",
        ))?;
        let out = run(Command::new("net").args(["use", "*", &url]))?;
        net_use_drive(&out).map(PathBuf::from).ok_or_else(|| {
            ConnectError::NoMountPoint(format!("net use didn’t say which drive it used: {out}"))
        })
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
