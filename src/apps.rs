//! Open With: the apps registered for a file's type, opening files with
//! one of them, and making one the default. macOS asks Launch Services,
//! Linux reads `.desktop` entries and asks `xdg-mime`, and Windows shows
//! its own "Open with" dialog. Tests set the apps on the
//! [`Recorder`](crate::system::Recorder) and record the rest.

use crate::system::{Recorder, SystemCall};
use gpui_kit::{App, AppContext as _, Task};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// An app that can open a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApp {
    pub name: String,
    /// The app bundle (macOS), `.desktop` file (Linux), or program.
    pub path: PathBuf,
    pub is_default: bool,
}

/// Lookups by file extension, since the answer depends only on the type.
fn cache() -> &'static Mutex<HashMap<String, Vec<OpenApp>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<OpenApp>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn type_key(file: &Path) -> String {
    file.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Apps that can open `file`, the default first, then by name.
pub fn apps_for(file: &Path, cx: &App) -> Vec<OpenApp> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        return recorder.apps();
    }
    let key = type_key(file);
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let mut apps = imp::lookup(file);
    apps.sort_by_key(|a| (!a.is_default, a.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name == b.name);
    if let Ok(mut cache) = cache().lock() {
        cache.insert(key, apps.clone());
    }
    apps
}

/// Open `files` with the app at `app`.
pub fn open_with(app: &Path, files: &[PathBuf], cx: &mut App) -> Task<io::Result<()>> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        recorder.record(SystemCall::OpenWith {
            app: app.to_path_buf(),
            paths: files.to_vec(),
        });
        return Task::ready(Ok(()));
    }
    let (app, files) = (app.to_path_buf(), files.to_vec());
    cx.background_spawn(async move { imp::launch(&app, &files) })
}

/// Make `app` the default for files of `file`'s type. macOS may ask the
/// user to confirm first.
pub fn make_default(app: &Path, file: &Path, cx: &mut App) -> Task<io::Result<()>> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        recorder.record(SystemCall::SetDefaultApp {
            app: app.to_path_buf(),
            file: file.to_path_buf(),
        });
        return Task::ready(Ok(()));
    }
    if let Ok(mut cache) = cache().lock() {
        cache.remove(&type_key(file));
    }
    let (app, file) = (app.to_path_buf(), file.to_path_buf());
    cx.background_spawn(async move { imp::set_default(&app, &file) })
}

/// Whether the system has its own chooser instead of a list (Windows).
pub const SYSTEM_CHOOSER: bool = cfg!(windows);

/// Show the system's "Open with" dialog for `file` (Windows).
pub fn system_chooser(file: &Path, cx: &mut App) -> Task<io::Result<()>> {
    if let Some(recorder) = cx.try_global::<Recorder>() {
        recorder.record(SystemCall::OpenWith {
            app: PathBuf::new(),
            paths: vec![file.to_path_buf()],
        });
        return Task::ready(Ok(()));
    }
    let file = file.to_path_buf();
    cx.background_spawn(async move {
        std::process::Command::new("rundll32.exe")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(&file)
            .spawn()
            .map(drop)
    })
}

/// `open -a`'s and `xdg-mime`'s failures, as errors.
#[cfg_attr(windows, allow(dead_code))]
fn run(command: &mut std::process::Command) -> io::Result<()> {
    let out = command.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn lookup(file: &Path) -> Vec<OpenApp> {
        let default = crate::macos::default_app(file);
        crate::macos::apps_for(file)
            .into_iter()
            .map(|path| OpenApp {
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                is_default: Some(&path) == default.as_ref(),
                path,
            })
            .collect()
    }

    pub fn launch(app: &Path, files: &[PathBuf]) -> io::Result<()> {
        run(std::process::Command::new("open")
            .arg("-a")
            .arg(app)
            .arg("--")
            .args(files))
    }

    pub fn set_default(app: &Path, file: &Path) -> io::Result<()> {
        crate::macos::set_default_app(app, file).map_err(io::Error::other)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    pub fn lookup(file: &Path) -> Vec<OpenApp> {
        crate::win::apps_for(file)
            .into_iter()
            .map(|(name, path, is_default)| OpenApp {
                name,
                path,
                is_default,
            })
            .collect()
    }

    pub fn launch(app: &Path, files: &[PathBuf]) -> io::Result<()> {
        crate::win::open_with(app, files).map_err(io::Error::other)
    }

    pub fn set_default(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::Error::other(
            "Windows sets default apps in its own Open with dialog",
        ))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::*;
    use fffinder_core::desktop_apps::{self, DesktopApp};

    /// Installed apps, read once.
    fn installed() -> &'static [DesktopApp] {
        static APPS: OnceLock<Vec<DesktopApp>> = OnceLock::new();
        APPS.get_or_init(|| desktop_apps::desktop_apps(&fffinder_core::terminals::desktop_dirs()))
    }

    fn mime_type(file: &Path) -> Option<String> {
        crate::system::xdg_query(&["filetype".as_ref(), file.as_os_str()]).filter(|m| !m.is_empty())
    }

    pub fn lookup(file: &Path) -> Vec<OpenApp> {
        let Some(mime) = mime_type(file) else {
            return Vec::new();
        };
        let default = crate::system::xdg_query(&["default".as_ref(), mime.as_ref()]);
        desktop_apps::apps_for_mime(installed(), &mime)
            .into_iter()
            .map(|app| OpenApp {
                name: app.name.clone(),
                path: app.path.clone(),
                is_default: default.as_deref() == Some(app.id.as_str()),
            })
            .collect()
    }

    pub fn launch(app: &Path, files: &[PathBuf]) -> io::Result<()> {
        if app.extension().is_none_or(|e| e != "desktop") {
            return std::process::Command::new(app)
                .args(files)
                .spawn()
                .map(drop);
        }
        let text = std::fs::read_to_string(app)?;
        let entry = fffinder_core::terminals::parse_desktop_entry(&text)
            .ok_or_else(|| io::Error::other(format!("{} isn't an app", app.display())))?;
        for words in desktop_apps::commands(&entry.exec_line, files) {
            if let Some((program, args)) = words.split_first() {
                std::process::Command::new(program).args(args).spawn()?;
            }
        }
        Ok(())
    }

    pub fn set_default(app: &Path, file: &Path) -> io::Result<()> {
        let mime = mime_type(file)
            .ok_or_else(|| io::Error::other("xdg-mime can't tell the file's type"))?;
        let id = app
            .file_name()
            .ok_or_else(|| io::Error::other("not an app"))?;
        run(std::process::Command::new("xdg-mime")
            .arg("default")
            .arg(id)
            .arg(mime))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn launch_services_lists_apps_for_text() {
        let file = std::env::temp_dir().join(format!("fffinder-apps-{}.txt", std::process::id()));
        std::fs::write(&file, "text").unwrap();
        let apps = imp::lookup(&file);
        let _ = std::fs::remove_file(&file);
        assert!(apps.iter().any(|a| a.name == "TextEdit"), "{apps:?}");
        assert!(apps.iter().any(|a| a.is_default));
    }
}
