//! freedesktop.org thumbnailers (Linux): the `.thumbnailer` files desktops
//! install so file managers can draw PDFs, videos, and documents, and the
//! command each one runs.

use crate::terminals::exec_words;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One installed thumbnailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnailer {
    /// The command, with `%i` (input path), `%u` (input URI), `%o` (output
    /// path), and `%s` (size) in it.
    pub exec: String,
    pub mime_types: Vec<String>,
    /// A program that has to be installed for the command to work.
    pub try_exec: Option<String>,
}

/// Parse the `[Thumbnailer Entry]` group of a `.thumbnailer` file.
pub fn parse(text: &str) -> Option<Thumbnailer> {
    let mut in_entry = false;
    let (mut exec, mut try_exec, mut mime_types) = (None, None, Vec::new());
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Thumbnailer Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_entry) else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Exec" => exec = Some(value.to_string()),
            "TryExec" => try_exec = Some(value.to_string()),
            "MimeType" => {
                mime_types = value
                    .split(';')
                    .filter(|m| !m.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            _ => {}
        }
    }
    Some(Thumbnailer {
        exec: exec.filter(|e| !e.is_empty())?,
        mime_types,
        try_exec,
    })
}

/// `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, in that order of precedence.
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
    let shared = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(shared.split(':').map(PathBuf::from))
        .collect()
}

/// Every usable thumbnailer in the `thumbnailers` folders of `data_dirs`:
/// a file name found earlier hides the same name later, and ones whose
/// `TryExec` program isn't installed are left out.
pub fn installed(data_dirs: &[PathBuf]) -> Vec<Thumbnailer> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for dir in data_dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("thumbnailers")) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            let is_thumbnailer = path.extension().is_some_and(|e| e == "thumbnailer");
            if !is_thumbnailer || !seen.insert(path.file_name().map(|n| n.to_os_string())) {
                continue;
            }
            if let Some(t) = std::fs::read_to_string(&path).ok().and_then(|t| parse(&t))
                && t.try_exec.as_deref().is_none_or(on_path)
            {
                found.push(t);
            }
        }
    }
    found
}

/// Whether `program` is an absolute path to a file or is found on `PATH`.
fn on_path(program: &str) -> bool {
    let program = Path::new(program);
    if program.is_absolute() {
        return program.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The first thumbnailer that draws `mime`.
pub fn for_mime<'a>(thumbnailers: &'a [Thumbnailer], mime: &str) -> Option<&'a Thumbnailer> {
    thumbnailers
        .iter()
        .find(|t| t.mime_types.iter().any(|m| m == mime))
}

/// `file://` and the path, percent-encoded where URIs need it.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

/// The program and arguments that draw `input` into a PNG at `output`,
/// `size` pixels across.
pub fn command(exec: &str, input: &Path, output: &Path, size: u32) -> Vec<String> {
    exec_words(exec)
        .into_iter()
        .map(|word| {
            word.replace("%%", "\u{0}")
                .replace("%i", &input.to_string_lossy())
                .replace("%u", &file_uri(input))
                .replace("%o", &output.to_string_lossy())
                .replace("%s", &size.to_string())
                .replace('\u{0}', "%")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_thumbnailers_and_builds_commands() {
        let text = "[Thumbnailer Entry]\nTryExec=/no/such/evince-thumbnailer\nExec=evince-thumbnailer -s %s %u %o\nMimeType=application/pdf;application/x-bzpdf;\n";
        let t = parse(text).unwrap();
        assert_eq!(t.mime_types, ["application/pdf", "application/x-bzpdf"]);
        assert_eq!(
            command(
                &t.exec,
                Path::new("/a b/c.pdf"),
                Path::new("/tmp/o.png"),
                256
            ),
            [
                "evince-thumbnailer",
                "-s",
                "256",
                "file:///a%20b/c.pdf",
                "/tmp/o.png"
            ]
        );
        assert!(for_mime(std::slice::from_ref(&t), "application/pdf").is_some());
        assert!(for_mime(std::slice::from_ref(&t), "video/mp4").is_none());

        let dir = std::env::temp_dir().join(format!("fffinder-thumbs-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("thumbnailers")).unwrap();
        std::fs::write(dir.join("thumbnailers/pdf.thumbnailer"), text).unwrap();
        let usable = "[Thumbnailer Entry]\nExec=thumb %i %o\nMimeType=video/mp4;\n";
        std::fs::write(dir.join("thumbnailers/video.thumbnailer"), usable).unwrap();
        let found = installed(std::slice::from_ref(&dir));
        assert_eq!(
            found.len(),
            1,
            "a thumbnailer whose program is missing is left out"
        );
        assert_eq!(found[0].mime_types, ["video/mp4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
