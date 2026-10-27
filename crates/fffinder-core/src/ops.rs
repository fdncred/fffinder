//! Copying and moving files and folders: finding name clashes before
//! anything changes, then doing the work with byte progress and a way to
//! stop part way. A replaced item is only removed once its replacement is
//! complete, so a failed or stopped copy leaves the destination as it was.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Files at least this big are copied in chunks, so progress moves and Stop
/// works part way through them.
const CHUNKED_COPY: u64 = 32 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Copy,
    Move,
}

/// What to do with an item whose name is already taken in the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clash {
    /// Remove the item that is there and put the new one in its place.
    Replace,
    /// Leave both where they are.
    Skip,
    /// Give the new item a free name: "name 2.ext".
    KeepBoth,
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("“{0}” can’t be put inside itself")]
    IntoItself(String),
    #[error("“{0}” can’t replace the folder it’s in")]
    ReplacesParent(String),
    #[error("{}: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("stopped")]
    Cancelled,
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> TransferError + '_ {
    move |source| TransferError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Items to copy or move into one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub sources: Vec<PathBuf>,
    pub dest: PathBuf,
    pub mode: Mode,
}

impl Transfer {
    /// Where `source` lands when nothing is in the way.
    pub fn target(&self, source: &Path) -> Option<PathBuf> {
        source.file_name().map(|name| self.dest.join(name))
    }

    /// Whether `source` is already in the destination folder: a copy makes
    /// "name copy", a move leaves it alone.
    pub fn in_place(&self, source: &Path) -> bool {
        source.parent() == Some(self.dest.as_path())
    }

    /// Refuse a folder copied or moved into itself.
    pub fn check(&self) -> Result<(), TransferError> {
        match self.sources.iter().find(|s| self.dest.starts_with(s)) {
            Some(source) => Err(TransferError::IntoItself(display_name(source))),
            None => Ok(()),
        }
    }

    /// Sources whose name is taken in the destination by another item.
    pub fn clashes(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .filter(|s| !self.in_place(s))
            .filter(|s| self.target(s).is_some_and(|t| t.symlink_metadata().is_ok()))
            .cloned()
            .collect()
    }
}

/// Shared between the thread doing a transfer and the window showing it.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
    current: Mutex<String>,
}

impl Progress {
    /// Ask the transfer to stop; it does so between chunks.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// How far along the bytes are, from 0 to 1. Moves within a volume
    /// have no bytes to copy and count as done.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 1.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32
    }

    /// The name of the item being copied or moved now.
    pub fn current(&self) -> String {
        self.current
            .lock()
            .map(|name| name.clone())
            .unwrap_or_default()
    }

    pub(crate) fn set_current(&self, name: String) {
        if let Ok(mut current) = self.current.lock() {
            *current = name;
        }
    }

    pub(crate) fn advance(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn expect(&self, bytes: u64) {
        self.total.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn check(&self) -> Result<(), TransferError> {
        if self.is_cancelled() {
            Err(TransferError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// What a transfer did.
#[derive(Debug, Default)]
pub struct Report {
    /// Each item copied or moved, and where it is now.
    pub done: Vec<(PathBuf, PathBuf)>,
    pub skipped: Vec<PathBuf>,
    /// Items that couldn't be copied or moved, and why.
    pub failed: Vec<(PathBuf, String)>,
    /// Stop was pressed; the item in progress was removed.
    pub cancelled: bool,
}

/// Copy or move every source into the destination, settling name clashes
/// with `clash`. Errors on one item don't stop the others.
pub fn run(transfer: &Transfer, clash: Clash, progress: &Progress) -> Report {
    let mut report = Report::default();
    if let Err(err) = transfer.check() {
        report.failed = transfer
            .sources
            .iter()
            .map(|s| (s.clone(), err.to_string()))
            .collect();
        return report;
    }
    if transfer.mode == Mode::Copy {
        progress.expect(transfer.sources.iter().map(|s| tree_size(s)).sum());
    }
    for source in &transfer.sources {
        if progress.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        progress.set_current(name.clone());
        let is_dir = source.is_dir();
        let in_place = transfer.in_place(source);
        let taken = transfer
            .target(source)
            .filter(|t| t.symlink_metadata().is_ok());
        let (target, replace) = match (in_place, transfer.mode, taken) {
            (true, Mode::Move, _) => continue,
            (true, Mode::Copy, _) => (copy_name(&transfer.dest, &name, is_dir), false),
            (false, _, None) => (transfer.dest.join(&name), false),
            (false, _, Some(target)) => match clash {
                Clash::Skip => {
                    progress.advance(tree_size(source));
                    report.skipped.push(source.clone());
                    continue;
                }
                Clash::KeepBoth => (numbered_name(&transfer.dest, &name, is_dir), false),
                Clash::Replace if source.starts_with(&target) => {
                    report.failed.push((
                        source.clone(),
                        TransferError::ReplacesParent(name).to_string(),
                    ));
                    continue;
                }
                Clash::Replace => (target, true),
            },
        };
        match transfer_one(source, &target, transfer.mode, replace, progress) {
            Ok(()) => report.done.push((source.clone(), target)),
            Err(TransferError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(err) => report.failed.push((source.clone(), err.to_string())),
        }
    }
    report
}

/// Move one item to exactly `target`, which must be free: a rename, or a
/// copy and delete across volumes.
pub fn move_item(source: &Path, target: &Path) -> Result<(), TransferError> {
    transfer_one(source, target, Mode::Move, false, &Progress::default())
}

fn transfer_one(
    source: &Path,
    target: &Path,
    mode: Mode,
    replace: bool,
    progress: &Progress,
) -> Result<(), TransferError> {
    // A replaced item waits beside its replacement until that is complete.
    let staging = if replace {
        staging_path(target)
    } else {
        target.to_path_buf()
    };
    if mode == Mode::Move {
        match fs::rename(source, &staging) {
            Ok(()) => return finish_replace(&staging, target, replace, Some(source)),
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
                progress.expect(tree_size(source));
            }
            Err(err) => return Err(io_error(source)(err)),
        }
    }
    if let Err(err) = copy_tree(source, &staging, progress) {
        let _ = remove_any(&staging);
        return Err(err);
    }
    finish_replace(&staging, target, replace, None)?;
    if mode == Mode::Move {
        remove_any(source).map_err(io_error(source))?;
    }
    Ok(())
}

/// Swap a complete replacement in for the item it replaces. When that
/// fails, a moved item goes back to `restore`.
fn finish_replace(
    staging: &Path,
    target: &Path,
    replace: bool,
    restore: Option<&Path>,
) -> Result<(), TransferError> {
    if !replace {
        return Ok(());
    }
    let swapped = remove_any(target).and_then(|()| fs::rename(staging, target));
    swapped.map_err(|err| {
        match restore {
            Some(original) => {
                let _ = fs::rename(staging, original);
            }
            None => {
                let _ = remove_any(staging);
            }
        }
        io_error(target)(err)
    })
}

/// A hidden name beside `target` for a copy that isn't finished yet.
fn staging_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.fffinder-{}", std::process::id()))
}

/// Copy a file, folder, or symbolic link (as a link) to `target`, which
/// must not exist yet.
pub fn copy_tree(source: &Path, target: &Path, progress: &Progress) -> Result<(), TransferError> {
    progress.check()?;
    let meta = fs::symlink_metadata(source).map_err(io_error(source))?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        let link = fs::read_link(source).map_err(io_error(source))?;
        return make_symlink(&link, target, source.is_dir()).map_err(io_error(target));
    }
    if kind.is_dir() {
        fs::create_dir(target).map_err(io_error(target))?;
        for entry in fs::read_dir(source).map_err(io_error(source))? {
            let entry = entry.map_err(io_error(source))?;
            copy_tree(&entry.path(), &target.join(entry.file_name()), progress)?;
        }
        return fs::set_permissions(target, meta.permissions()).map_err(io_error(target));
    }
    copy_file(source, target, &meta, progress)
}

fn copy_file(
    source: &Path,
    target: &Path,
    meta: &fs::Metadata,
    progress: &Progress,
) -> Result<(), TransferError> {
    let len = meta.len();
    if len < CHUNKED_COPY || clones_instantly(meta, target) {
        fs::copy(source, target).map_err(io_error(source))?;
        progress.advance(len);
    } else {
        let mut from = File::open(source).map_err(io_error(source))?;
        let mut to = File::create_new(target).map_err(io_error(target))?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            progress.check()?;
            let read = from.read(&mut buf).map_err(io_error(source))?;
            if read == 0 {
                break;
            }
            to.write_all(&buf[..read]).map_err(io_error(target))?;
            progress.advance(read as u64);
        }
        fs::set_permissions(target, meta.permissions()).map_err(io_error(target))?;
    }
    // Copies keep the original's modification date, as in Finder.
    if let Ok(modified) = meta.modified() {
        let _ = File::options()
            .write(true)
            .open(target)
            .and_then(|file| file.set_modified(modified));
    }
    Ok(())
}

/// APFS clones a file on the same volume without copying its bytes, so
/// `fs::copy` (which clones) is instant there.
#[cfg(target_os = "macos")]
fn clones_instantly(meta: &fs::Metadata, target: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    target
        .parent()
        .and_then(|dir| fs::metadata(dir).ok())
        .is_some_and(|dir| dir.dev() == meta.dev())
}

#[cfg(not(target_os = "macos"))]
fn clones_instantly(_: &fs::Metadata, _: &Path) -> bool {
    false
}

#[cfg(unix)]
fn make_symlink(link: &Path, target: &Path, _is_dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(link, target)
}

#[cfg(windows)]
fn make_symlink(link: &Path, target: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(link, target)
    } else {
        std::os::windows::fs::symlink_file(link, target)
    }
}

/// Bytes in a file, or in every file under a folder. Links count as nothing.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return if meta.file_type().is_symlink() {
            0
        } else {
            meta.len()
        };
    }
    fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| tree_size(&e.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// Whether `a` and `b` are on the same volume, so dragging between them
/// moves instead of copying. Paths that can't be read count as the same.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (fs::symlink_metadata(a), fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev(),
            _ => true,
        }
    }
    #[cfg(windows)]
    {
        let drive = |p: &Path| {
            p.components()
                .next()
                .map(|c| c.as_os_str().to_ascii_lowercase())
        };
        drive(a) == drive(b)
    }
}

/// Delete a file, link, or whole folder.
pub fn remove_any(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// `report.txt` → (`report`, `.txt`); folders and dotfiles have no extension.
pub fn split_extension(name: &str, is_dir: bool) -> (&str, &str) {
    match name.rfind('.').filter(|&i| i > 0 && !is_dir) {
        Some(i) => name.split_at(i),
        None => (name, ""),
    }
}

/// The first name in `dir` that isn't taken, trying `make(n)` for
/// n = 1, 2, 3, …
pub fn free_name(dir: &Path, make: impl Fn(usize) -> String) -> PathBuf {
    (1..)
        .map(|n| dir.join(make(n)))
        .find(|p| p.symlink_metadata().is_err())
        .expect("some numbered name is free")
}

/// Finder's name for a copy in the same folder: "name copy.ext", then
/// "name copy 2.ext", …
pub fn copy_name(dir: &Path, name: &str, is_dir: bool) -> PathBuf {
    let (stem, ext) = split_extension(name, is_dir);
    free_name(dir, |n| match n {
        1 => format!("{stem} copy{ext}"),
        n => format!("{stem} copy {n}{ext}"),
    })
}

/// "Keep Both": "name 2.ext", then "name 3.ext", …
pub fn numbered_name(dir: &Path, name: &str, is_dir: bool) -> PathBuf {
    let (stem, ext) = split_extension(name, is_dir);
    free_name(dir, |n| format!("{stem} {}{ext}", n + 1))
}

fn display_name(path: &Path) -> String {
    crate::entry::display_name(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir =
                std::env::temp_dir().join(format!("fffinder-ops-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("src/sub")).unwrap();
            fs::create_dir_all(dir.join("dest")).unwrap();
            fs::write(dir.join("src/a.txt"), "alpha").unwrap();
            fs::write(dir.join("src/sub/b.txt"), "beta").unwrap();
            Dir(dir)
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.path(rel)).unwrap()
        }

        fn transfer(&self, sources: &[&str], dest: &str, mode: Mode) -> Transfer {
            Transfer {
                sources: sources.iter().map(|s| self.path(s)).collect(),
                dest: self.path(dest),
                mode,
            }
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copies_files_and_folders() {
        let dir = Dir::new("copy");
        let transfer = dir.transfer(&["src/a.txt", "src/sub"], "dest", Mode::Copy);
        let progress = Progress::default();
        let report = run(&transfer, Clash::Skip, &progress);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(dir.read("dest/a.txt"), "alpha");
        assert_eq!(dir.read("dest/sub/b.txt"), "beta");
        assert_eq!(dir.read("src/a.txt"), "alpha", "a copy leaves the original");
        assert_eq!(report.done.len(), 2);
        assert_eq!(progress.fraction(), 1.0);
    }

    #[test]
    fn copies_keep_the_modification_date() {
        let dir = Dir::new("mtime");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        File::options()
            .write(true)
            .open(dir.path("src/a.txt"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        run(
            &dir.transfer(&["src/a.txt"], "dest", Mode::Copy),
            Clash::Skip,
            &Progress::default(),
        );
        let copied = fs::metadata(dir.path("dest/a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(copied, old);
    }

    #[test]
    fn moves_files_and_folders() {
        let dir = Dir::new("move");
        let report = run(
            &dir.transfer(&["src/a.txt", "src/sub"], "dest", Mode::Move),
            Clash::Skip,
            &Progress::default(),
        );
        assert_eq!(report.done.len(), 2);
        assert!(!dir.path("src/a.txt").exists() && !dir.path("src/sub").exists());
        assert_eq!(dir.read("dest/sub/b.txt"), "beta");
    }

    #[test]
    fn clashes_skip_replace_or_keep_both() {
        let dir = Dir::new("clash");
        fs::write(dir.path("dest/a.txt"), "old").unwrap();
        let copy = dir.transfer(&["src/a.txt", "src/sub"], "dest", Mode::Copy);
        assert_eq!(copy.clashes(), [dir.path("src/a.txt")]);

        let report = run(&copy, Clash::Skip, &Progress::default());
        assert_eq!(report.skipped, [dir.path("src/a.txt")]);
        assert_eq!(dir.read("dest/a.txt"), "old");

        let report = run(&copy, Clash::KeepBoth, &Progress::default());
        assert!(
            report
                .done
                .contains(&(dir.path("src/a.txt"), dir.path("dest/a 2.txt")))
        );
        assert!(
            report
                .done
                .contains(&(dir.path("src/sub"), dir.path("dest/sub 2")))
        );
        assert_eq!(dir.read("dest/a 2.txt"), "alpha");

        run(&copy, Clash::Replace, &Progress::default());
        assert_eq!(dir.read("dest/a.txt"), "alpha");
        assert_eq!(dir.read("dest/sub/b.txt"), "beta");
    }

    #[test]
    fn copying_in_place_makes_a_copy() {
        let dir = Dir::new("in-place");
        let transfer = dir.transfer(&["src/a.txt", "src/a.txt"], "src", Mode::Copy);
        assert!(transfer.clashes().is_empty());
        let report = run(&transfer, Clash::Skip, &Progress::default());
        let made: Vec<PathBuf> = report.done.into_iter().map(|(_, to)| to).collect();
        assert_eq!(
            made,
            [dir.path("src/a copy.txt"), dir.path("src/a copy 2.txt")]
        );

        let report = run(
            &dir.transfer(&["src/a.txt"], "src", Mode::Move),
            Clash::Replace,
            &Progress::default(),
        );
        assert!(report.done.is_empty() && report.failed.is_empty());
        assert_eq!(
            dir.read("src/a.txt"),
            "alpha",
            "moving in place does nothing"
        );
    }

    #[test]
    fn refuses_a_folder_into_itself() {
        let dir = Dir::new("itself");
        let transfer = dir.transfer(&["src"], "src/sub", Mode::Move);
        assert!(matches!(
            transfer.check(),
            Err(TransferError::IntoItself(_))
        ));
        let report = run(&transfer, Clash::Skip, &Progress::default());
        assert_eq!(report.failed.len(), 1);
        assert!(dir.path("src/sub/b.txt").exists());
    }

    #[test]
    fn refuses_to_replace_the_folder_an_item_is_in() {
        let dir = Dir::new("parent");
        fs::create_dir(dir.path("src/sub/sub")).unwrap();
        let report = run(
            &dir.transfer(&["src/sub/sub"], "src", Mode::Move),
            Clash::Replace,
            &Progress::default(),
        );
        assert_eq!(report.failed.len(), 1);
        assert!(dir.path("src/sub/b.txt").exists());
    }

    #[test]
    fn stopping_removes_the_unfinished_item() {
        let dir = Dir::new("cancel");
        let progress = Progress::default();
        progress.cancel();
        let report = run(
            &dir.transfer(&["src/sub"], "dest", Mode::Copy),
            Clash::Skip,
            &progress,
        );
        assert!(report.cancelled);
        assert!(!dir.path("dest/sub").exists());

        let progress = Progress::default();
        assert!(matches!(
            {
                progress.cancel();
                copy_tree(&dir.path("src/sub"), &dir.path("dest/sub"), &progress)
            },
            Err(TransferError::Cancelled)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn copies_links_as_links() {
        let dir = Dir::new("links");
        std::os::unix::fs::symlink("a.txt", dir.path("src/link")).unwrap();
        run(
            &dir.transfer(&["src/link"], "dest", Mode::Copy),
            Clash::Skip,
            &Progress::default(),
        );
        assert_eq!(
            fs::read_link(dir.path("dest/link")).unwrap(),
            Path::new("a.txt")
        );
    }

    #[test]
    fn names_for_copies() {
        let dir = Dir::new("names");
        assert_eq!(split_extension("report.txt", false), ("report", ".txt"));
        assert_eq!(split_extension(".bashrc", false), (".bashrc", ""));
        assert_eq!(split_extension("v1.2", true), ("v1.2", ""));
        assert_eq!(
            copy_name(&dir.path("src"), "a.txt", false),
            dir.path("src/a copy.txt")
        );
        fs::write(dir.path("src/a copy.txt"), "").unwrap();
        assert_eq!(
            copy_name(&dir.path("src"), "a.txt", false),
            dir.path("src/a copy 2.txt")
        );
        assert_eq!(
            numbered_name(&dir.path("src"), "sub", true),
            dir.path("src/sub 2")
        );
        assert_eq!(tree_size(&dir.path("src")), 9);
        assert!(same_volume(&dir.path("src/a.txt"), &dir.path("dest")));
    }
}
