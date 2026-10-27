//! Apps from freedesktop.org `.desktop` entries, for Open With on Linux:
//! which apps open a MIME type, and the commands that open files with one.

use crate::terminals::{exec_words, is_field_code, parse_desktop_entry};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// An installed app that says which MIME types it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    /// The entry's file name, such as `org.gnome.TextEditor.desktop`.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub exec_line: String,
    pub mime_types: Vec<String>,
}

/// Every visible app in `dirs`. An id found in an earlier folder hides the
/// same id in later ones, as the XDG data folders are ordered.
pub fn desktop_apps(dirs: &[PathBuf]) -> Vec<DesktopApp> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            let Some(id) = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".desktop"))
            else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(entry) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_desktop_entry(&text))
                .filter(|e| !e.hidden && !e.mime_types.is_empty())
            else {
                continue;
            };
            apps.push(DesktopApp {
                id,
                name: entry.name,
                path,
                exec_line: entry.exec_line,
                mime_types: entry.mime_types,
            });
        }
    }
    apps
}

/// Apps that open `mime`, sorted by name.
pub fn apps_for_mime<'a>(apps: &'a [DesktopApp], mime: &str) -> Vec<&'a DesktopApp> {
    let mut found: Vec<&DesktopApp> = apps
        .iter()
        .filter(|a| a.mime_types.iter().any(|m| m == mime))
        .collect();
    found.sort_by_key(|a| a.name.to_lowercase());
    found
}

/// The commands that open `files` with an `Exec` line, each a program and
/// its arguments. `%f` and `%u` take one file, so there is one command per
/// file; `%F` and `%U` take them all; with no field code the files go at
/// the end. Other field codes are dropped.
pub fn commands(exec_line: &str, files: &[PathBuf]) -> Vec<Vec<String>> {
    let words = exec_words(exec_line);
    let shown = |p: &Path| p.to_string_lossy().into_owned();
    let one_each = words.iter().any(|w| w == "%f" || w == "%u");
    let all = words.iter().any(|w| w == "%F" || w == "%U");
    let build = |chosen: &[PathBuf]| -> Vec<String> {
        let mut out = Vec::with_capacity(words.len() + chosen.len());
        for word in &words {
            match word.as_str() {
                "%f" | "%u" | "%F" | "%U" => out.extend(chosen.iter().map(|p| shown(p))),
                code if is_field_code(code) => {}
                plain => out.push(plain.replace("%%", "%")),
            }
        }
        if !one_each && !all {
            out.extend(chosen.iter().map(|p| shown(p)));
        }
        out
    };
    if one_each && !all {
        files
            .iter()
            .map(|f| build(std::slice::from_ref(f)))
            .collect()
    } else {
        vec![build(files)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_apps_for_a_type() {
        let dir = std::env::temp_dir().join(format!("fffinder-desktop-{}", std::process::id()));
        let (first, second) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let entry = |name: &str, mime: &str| {
            format!(
                "[Desktop Entry]\nType=Application\nName={name}\nExec={name} %F\nMimeType={mime};\n"
            )
        };
        std::fs::write(first.join("ed.desktop"), entry("Ed", "text/plain")).unwrap();
        std::fs::write(second.join("ed.desktop"), entry("Shadowed", "text/plain")).unwrap();
        std::fs::write(
            second.join("view.desktop"),
            entry("Viewer", "image/png;text/plain"),
        )
        .unwrap();
        std::fs::write(second.join("img.desktop"), entry("Images", "image/png")).unwrap();
        let apps = desktop_apps(&[first, second]);
        let names: Vec<&str> = apps_for_mime(&apps, "text/plain")
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["Ed", "Viewer"], "an earlier folder's entry wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_commands_from_field_codes() {
        let files = [PathBuf::from("/a b.txt"), PathBuf::from("/c.txt")];
        assert_eq!(
            commands("ed %F", &files),
            [vec!["ed", "/a b.txt", "/c.txt"]]
        );
        assert_eq!(
            commands("ed --new %f", &files),
            [
                vec!["ed", "--new", "/a b.txt"],
                vec!["ed", "--new", "/c.txt"]
            ]
        );
        assert_eq!(
            commands("\"/opt/my ed\" %i", &files[..1]),
            [vec!["/opt/my ed", "/a b.txt"]]
        );
    }
}
