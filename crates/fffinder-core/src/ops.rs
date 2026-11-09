//! Copying and moving files and folders: finding name clashes before
//! anything changes, then doing the work with byte progress and a way to
//! stop part way.
//!
//! Nothing here deletes what the user had. A copy is built under a hidden
//! name and renamed into place only when it is whole, with a rename that
//! refuses a taken name, so an item that appears at the name meanwhile is
//! never overwritten. Replace sets the old item aside as "name (replaced)"
//! rather than deleting it; the app then moves it to the Trash. A move
//! across volumes removes its original only after the copy is flushed to
//! disk and holds as many bytes.

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
    /// Replace would set aside the item a link points to, or a folder that
    /// holds other items of the same job.
    #[error("“{0}” can’t replace an item it links to or that holds what’s being copied or moved")]
    ReplacesSource(String),
    #[error("{}: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
    /// A move across volumes copied the item, but the original at `path`
    /// couldn't be removed afterwards.
    #[error("the original couldn’t be removed: {source}")]
    OriginalLeft { path: PathBuf, source: io::Error },
    #[error("stopped")]
    Cancelled,
}

/// Turn an I/O error on `path` into a [`TransferError`], for `map_err`.
pub fn io_error(path: &Path) -> impl FnOnce(io::Error) -> TransferError + '_ {
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

    /// Whether `source`'s bytes are copied: always for a copy, and for a
    /// move only across volumes, since within one a move is a rename.
    fn copies_bytes(&self, source: &Path) -> bool {
        self.mode == Mode::Copy || !same_volume(source, &self.dest)
    }

    /// Refuse a folder copied or moved into itself, however either path is
    /// spelled: through a link, a firmlink (`/tmp` and `/private/tmp`), or in
    /// another case on a volume that ignores case.
    pub fn check(&self) -> Result<(), TransferError> {
        let dest = resolved(&self.dest);
        match self
            .sources
            .iter()
            .find(|s| dest.starts_with(resolved_item(s)))
        {
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
    /// Whether `total` holds every byte the job will copy.
    counted: AtomicBool,
    cancelled: AtomicBool,
    current: Mutex<String>,
}

impl Progress {
    /// Ask the transfer to stop; it does so between chunks.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether [`Self::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// How far along the bytes are, from 0 to 1. It stays at 0 until every
    /// byte to copy is counted, so it never runs ahead of the work. A job
    /// with nothing to copy, like a move within a volume, is then done.
    pub fn fraction(&self) -> f32 {
        // Acquire pairs with the Release in `counted`: once that is seen,
        // so is every byte added to `total` before it.
        if !self.counted.load(Ordering::Acquire) {
            return 0.0;
        }
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

    /// Every byte the job will copy has been passed to [`Self::expect`].
    pub(crate) fn counted(&self) {
        self.counted.store(true, Ordering::Release);
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
    /// Items moved across volumes whose copy is complete (and in `done`)
    /// but whose original couldn't be removed, and why.
    pub left: Vec<(PathBuf, String)>,
    /// Items that Replace took the place of: where each one was, and the
    /// name it was set aside under beside it, "name (replaced).ext". None of
    /// them is deleted; the app moves them to the Trash.
    pub replaced: Vec<(PathBuf, PathBuf)>,
    /// Stop was pressed; the item in progress was removed.
    pub cancelled: bool,
}

/// Copy or move every source into the destination, settling name clashes
/// with `clash`. Errors on one item don't stop the others. The work starts
/// at once while another thread adds up how many bytes it has to copy.
pub fn run(transfer: &Transfer, clash: Clash, progress: &Progress) -> Report {
    if let Err(err) = transfer.check() {
        return Report {
            failed: transfer
                .sources
                .iter()
                .map(|s| (s.clone(), err.to_string()))
                .collect(),
            ..Report::default()
        };
    }
    let copied = transfer.sources.iter().filter(|s| transfer.copies_bytes(s));
    sized_while(copied, progress, || run_items(transfer, clash, progress))
}

fn run_items(transfer: &Transfer, clash: Clash, progress: &Progress) -> Report {
    let mut report = Report::default();
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
        // Two sources with one name, from different folders, or names that
        // differ only in case on a volume that ignores it: the second finds
        // the first one there. That one is never replaced or skipped.
        let ours = taken
            .as_deref()
            .is_some_and(|t| report.done.iter().any(|(_, made)| same_item(made, t)));
        let (target, replace) = match (in_place, transfer.mode, taken) {
            (true, Mode::Move, _) => continue,
            (true, Mode::Copy, _) => (copy_name(&transfer.dest, &name, is_dir), false),
            (false, _, None) => (transfer.dest.join(&name), false),
            (false, _, Some(_)) if ours => (numbered_name(&transfer.dest, &name, is_dir), false),
            (false, _, Some(target)) => match clash {
                Clash::Skip => {
                    // This item's bytes were counted; count them as done.
                    if transfer.copies_bytes(source) {
                        progress.advance(tree_size(source, progress));
                    }
                    report.skipped.push(source.clone());
                    continue;
                }
                Clash::KeepBoth => (numbered_name(&transfer.dest, &name, is_dir), false),
                Clash::Replace => match replace_refusal(transfer, source, &target) {
                    Some(err) => {
                        report.failed.push((source.clone(), err.to_string()));
                        continue;
                    }
                    None => (target, true),
                },
            },
        };
        match transfer_one(source, &target, transfer.mode, replace, progress) {
            Ok(placed) => {
                if let Some(aside) = placed.replaced {
                    report.replaced.push((target.clone(), aside));
                }
                if let Some(err) = placed.left {
                    let left = TransferError::OriginalLeft {
                        path: source.clone(),
                        source: err,
                    };
                    report.left.push((source.clone(), left.to_string()));
                }
                report.done.push((source.clone(), target));
            }
            Err(TransferError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(err) => report.failed.push((source.clone(), err.to_string())),
        }
    }
    report
}

/// Why Replace can't put `source` at `target`, when it can't. Setting the
/// item at `target` aside would take along `source` itself (its own folder),
/// another item of the same job, or what `source` links to, leaving a link
/// that points at nothing.
fn replace_refusal(transfer: &Transfer, source: &Path, target: &Path) -> Option<TransferError> {
    let there = resolved_item(target);
    if resolved_item(source).starts_with(&there) {
        return Some(TransferError::ReplacesParent(display_name(source)));
    }
    let holds_another = transfer
        .sources
        .iter()
        .any(|other| resolved_item(other).starts_with(&there));
    // `resolved` follows a link at the end too, to what it points to.
    let links_into = resolved(source).starts_with(&there);
    (holds_another || links_into).then(|| TransferError::ReplacesSource(display_name(source)))
}

/// Whether two paths name the same item on disk, however each is spelled.
fn same_item(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Windows file systems ignore case, and std can't read file ids yet.
        let key = |p: &Path| resolved_item(p).to_string_lossy().to_lowercase();
        a.symlink_metadata().is_ok() && b.symlink_metadata().is_ok() && key(a) == key(b)
    }
}

/// Run `work` while another thread adds up the bytes under `sources` for
/// `progress`, so the work doesn't wait for a walk of every folder first.
/// The count stops when the work ends or Stop is pressed.
pub(crate) fn sized_while<'a, T>(
    sources: impl IntoIterator<Item = &'a PathBuf> + Send,
    progress: &Progress,
    work: impl FnOnce() -> T,
) -> T {
    let finished = AtomicBool::new(false);
    // A scoped thread may borrow from this function's stack, since the scope
    // waits for it before returning.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for source in sources {
                count_bytes(source, progress, &finished);
            }
            progress.counted();
        });
        let result = work();
        finished.store(true, Ordering::Relaxed);
        result
    })
}

/// Add the bytes in a file, or in every file under a folder, to the bytes
/// `progress` expects, as they're found.
fn count_bytes(path: &Path, progress: &Progress, finished: &AtomicBool) {
    if finished.load(Ordering::Relaxed) || progress.is_cancelled() {
        return;
    }
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            count_bytes(&entry.path(), progress, finished);
        }
    } else if !meta.file_type().is_symlink() {
        progress.expect(meta.len());
    }
}

/// Move one item to exactly `target`, which must be free: a rename, or a
/// copy and delete across volumes.
///
/// # Errors
///
/// Fails, leaving the item where it was, when it can't be moved or `target`
/// is taken; `OriginalLeft` when a move across volumes made a complete copy
/// but couldn't remove the original.
pub fn move_item(source: &Path, target: &Path) -> Result<(), TransferError> {
    let placed = transfer_one(source, target, Mode::Move, false, &Progress::default())?;
    match placed.left {
        Some(err) => Err(TransferError::OriginalLeft {
            path: source.to_path_buf(),
            source: err,
        }),
        None => Ok(()),
    }
}

/// What became of one item put where it was sent.
#[derive(Debug, Default)]
struct Placed {
    /// The item it took the place of, set aside beside it.
    replaced: Option<PathBuf>,
    /// Why the original of a move across volumes is still there.
    left: Option<io::Error>,
}

fn transfer_one(
    source: &Path,
    target: &Path,
    mode: Mode,
    replace: bool,
    progress: &Progress,
) -> Result<Placed, TransferError> {
    if mode == Mode::Move {
        match place(source, target, replace) {
            Ok(replaced) => {
                return Ok(Placed {
                    replaced,
                    left: None,
                });
            }
            // `run` counted its bytes; copy and delete instead.
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
                return move_by_copy(source, target, replace, progress);
            }
            Err(err) => return Err(io_error(source)(err)),
        }
    }
    let staging = copy_to_staging(source, target, progress)?;
    let replaced = place(&staging, target, replace).map_err(|err| {
        let _ = remove_any(&staging);
        io_error(target)(err)
    })?;
    Ok(Placed {
        replaced,
        left: None,
    })
}

/// Move `source` to `target` on another volume: copy it, flush the copy to
/// disk and check it holds every byte, put it in place, and only then
/// remove the original. Until that last step the original is untouched.
fn move_by_copy(
    source: &Path,
    target: &Path,
    replace: bool,
    progress: &Progress,
) -> Result<Placed, TransferError> {
    let staging = copy_to_staging(source, target, progress)?;
    let replaced = settle_copy(source, &staging)
        .and_then(|()| place(&staging, target, replace).map_err(io_error(target)))
        .inspect_err(|_| {
            let _ = remove_any(&staging);
        })?;
    Ok(Placed {
        replaced,
        left: remove_any(source).err(),
    })
}

/// Copy `source` to a fresh hidden name beside `target` and return it. When
/// the copy fails or is stopped, what was made of it is removed: only this
/// job's own hidden copy, never anything at `target`.
fn copy_to_staging(
    source: &Path,
    target: &Path,
    progress: &Progress,
) -> Result<PathBuf, TransferError> {
    let staging = fresh_staging(target);
    copy_tree(source, &staging, progress).inspect_err(|_| {
        let _ = remove_any(&staging);
    })?;
    Ok(staging)
}

/// Flush a finished copy to disk and check it holds as many bytes as the
/// original, before the original of a move is removed. A file that changed
/// while it was copied fails the check, so the original stays.
fn settle_copy(original: &Path, copy: &Path) -> Result<(), TransferError> {
    sync_tree(copy).map_err(io_error(copy))?;
    let whole = Progress::default();
    let (want, got) = (tree_size(original, &whole), tree_size(copy, &whole));
    if want == got {
        return Ok(());
    }
    Err(TransferError::Io {
        path: copy.to_path_buf(),
        source: io::Error::other(format!(
            "the copy holds {got} bytes but the original {want}, so the original was kept"
        )),
    })
}

/// Flush every file and folder under `path` to disk. A file system that
/// can't flush (some network and FUSE volumes) is taken at its word.
#[cfg(unix)]
fn sync_tree(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
    } else if !meta.is_file() {
        return Ok(());
    }
    match File::open(path).and_then(|file| file.sync_all()) {
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(())
        }
        result => result,
    }
}

/// Windows only flushes files opened for writing, which a read-only copy
/// can't be, so it's left to the system there.
#[cfg(not(unix))]
fn sync_tree(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Rename `from` to `target`. With `replace`, the item at `target` is first
/// renamed aside to "name (replaced).ext" beside it, and that path comes
/// back; when `from` then can't go in, the old item goes back. Nothing is
/// deleted either way.
fn place(from: &Path, target: &Path, replace: bool) -> io::Result<Option<PathBuf>> {
    if !replace {
        return rename_no_replace(from, target).map(|()| None);
    }
    let aside = match set_aside(target) {
        Ok(aside) => aside,
        // Gone since the clash was found: there's nothing to replace.
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return rename_no_replace(from, target).map(|()| None);
        }
        Err(err) => return Err(err),
    };
    if let Err(err) = rename_no_replace(from, target) {
        let _ = rename_no_replace(&aside, target);
        return Err(err);
    }
    Ok(Some(aside))
}

/// Rename the item at `target` to "name (replaced).ext" beside it, numbered
/// when that's taken, and return where it is now.
fn set_aside(target: &Path) -> io::Result<PathBuf> {
    let is_dir = target.symlink_metadata()?.is_dir();
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = display_name(target);
    let (stem, ext) = split_extension(&name, is_dir);
    rename_to_free_name(target, dir, |n| match n {
        1 => format!("{stem} (replaced){ext}"),
        n => format!("{stem} (replaced {n}){ext}"),
    })
}

/// Rename `from` into `dir` under the first name `make(n)` gives, for
/// n = 1, 2, 3, …, that nothing has, and return it. The rename itself claims
/// each name, so an item that takes one meanwhile is never replaced.
///
/// # Errors
///
/// What the rename failed with, other than a taken name.
pub fn rename_to_free_name(
    from: &Path,
    dir: &Path,
    make: impl Fn(usize) -> String,
) -> io::Result<PathBuf> {
    (1..)
        .find_map(|n| {
            let to = dir.join(make(n));
            match rename_no_replace(from, &to) {
                Ok(()) => Some(Ok(to)),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => None,
                Err(err) => Some(Err(err)),
            }
        })
        .expect("some numbered name is free")
}

/// Rename `from` to `to` only while nothing is at `to`. `fs::rename`
/// replaces a file there on every system, and an empty folder on Unix, so
/// a name found free a moment ago could otherwise lose what took it since.
///
/// # Errors
///
/// `AlreadyExists` when `to` is taken, `CrossesDevices` when the two are on
/// different volumes, and otherwise what the rename failed with.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match rename_exclusive(from, to) {
        // SMB, FAT, exFAT, and older Linux kernels can't refuse a taken name
        // in the rename itself, so there the name is checked just before.
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
            ) =>
        {
            if to.symlink_metadata().is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            fs::rename(from, to)
        }
        result => result,
    }
}

#[cfg(target_os = "macos")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both are NUL-terminated strings that live past the call.
    let failed = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } != 0;
    if failed {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // The system call itself: glibc only wraps it from 2.28 on. A kernel
    // without it answers ENOSYS, which `rename_no_replace` handles.
    // SAFETY: both are NUL-terminated strings that live past the call.
    let failed = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0;
    if failed {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn rename_exclusive(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(windows)]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
    use windows::core::HSTRING;
    let (from, to) = (
        HSTRING::from(from.as_os_str()),
        HSTRING::from(to.as_os_str()),
    );
    // Without MOVEFILE_REPLACE_EXISTING a taken name fails the move, and
    // without MOVEFILE_COPY_ALLOWED so does another volume.
    // SAFETY: MoveFileExW only reads the two paths.
    unsafe { MoveFileExW(&from, &to, MOVE_FILE_FLAGS(0)) }
        .map_err(|err| io::Error::from_raw_os_error(err.code().0 & 0xFFFF))
}

/// A path as the C string the system calls take.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn c_path(path: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt as _;
    Ok(std::ffi::CString::new(path.as_os_str().as_bytes())?)
}

/// A hidden name beside `target` that nothing has yet, for work that isn't
/// finished.
fn fresh_staging(target: &Path) -> PathBuf {
    std::iter::repeat_with(|| staging_path(target))
        .find(|path| path.symlink_metadata().is_err())
        .expect("staging names never run out")
}

/// A hidden name beside `target` for work that isn't finished yet. Each
/// call gets its own, so jobs running at once never share one.
pub(crate) fn staging_path(target: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Names stop at 255 bytes; leave room for what's added around this one.
    let mut keep = name.len().min(200);
    while !name.is_char_boundary(keep) {
        keep -= 1;
    }
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(
        ".{}.fffinder-{}-{n}",
        &name[..keep],
        std::process::id()
    ))
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
        copy_xattrs(source, target);
        return fs::set_permissions(target, meta.permissions()).map_err(io_error(target));
    }
    if is_special(&kind) {
        // Opening a pipe waits for a writer, forever, and Stop can't end it.
        return Err(TransferError::Io {
            path: source.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::Unsupported,
                "pipes, sockets, and devices can’t be copied",
            ),
        });
    }
    copy_file(source, target, &meta, progress)
}

/// A pipe, socket, or device: not a file whose bytes can be copied.
#[cfg(unix)]
fn is_special(kind: &fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    kind.is_fifo() || kind.is_socket() || kind.is_block_device() || kind.is_char_device()
}

#[cfg(not(unix))]
fn is_special(_: &fs::FileType) -> bool {
    false
}

/// Copy extended attributes, such as Finder tags, comments, and where a
/// download came from. Best effort: a volume without them still gets the
/// data.
#[cfg(unix)]
fn copy_xattrs(source: &Path, target: &Path) {
    let Ok(names) = xattr::list(source) else {
        return;
    };
    for name in names {
        if let Ok(Some(value)) = xattr::get(source, &name) {
            let _ = xattr::set(target, &name, &value);
        }
    }
}

#[cfg(not(unix))]
fn copy_xattrs(_: &Path, _: &Path) {}

fn copy_file(
    source: &Path,
    target: &Path,
    meta: &fs::Metadata,
    progress: &Progress,
) -> Result<(), TransferError> {
    let len = meta.len();
    if len < CHUNKED_COPY || clones_instantly(meta, target) {
        fs::copy(source, target).map_err(io_error(source))?;
        // macOS's copy keeps extended attributes; Linux's doesn't.
        if !cfg!(target_os = "macos") {
            copy_xattrs(source, target);
        }
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
        copy_xattrs(source, target);
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

/// Make a symbolic link at `link` that points to `points_to`. Windows has
/// separate links for folders and files, so `is_dir` says which.
///
/// # Errors
///
/// Fails when the link can't be made, as when `link` is taken.
#[cfg(unix)]
pub fn make_symlink(points_to: &Path, link: &Path, _is_dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(points_to, link)
}

/// Make a symbolic link at `link` that points to `points_to`. Windows has
/// separate links for folders and files, so `is_dir` says which.
///
/// # Errors
///
/// Fails when the link can't be made, as when `link` is taken or Windows
/// doesn't allow this user to make links.
#[cfg(windows)]
pub fn make_symlink(points_to: &Path, link: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(points_to, link)
    } else {
        std::os::windows::fs::symlink_file(points_to, link)
    }
}

/// Bytes in a file, or in every file under a folder. Links count as nothing.
/// The walk stops early, with what it has so far, once Stop is pressed.
pub fn tree_size(path: &Path, progress: &Progress) -> u64 {
    if progress.is_cancelled() {
        return 0;
    }
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
                .map(|e| tree_size(&e.path(), progress))
                .sum()
        })
        .unwrap_or(0)
}

/// `path` with links, firmlinks, and case spelled as the disk has them, or
/// as given when it can't be read.
fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Like [`resolved`], but a link at the end stays a link: copying one
/// copies the link, not what it points to.
fn resolved_item(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => resolved(path),
    }
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

/// `paths` without those inside another of them, or listed twice: trashing,
/// moving, or deleting a folder takes what's in it along. The order is kept.
pub fn outermost(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut order: Vec<usize> = (0..paths.len()).collect();
    // Sorted, a folder comes right before everything inside it.
    order.sort_by(|&a, &b| paths[a].cmp(&paths[b]).then(a.cmp(&b)));
    let mut keep = vec![true; paths.len()];
    let mut outer: Option<&Path> = None;
    for ix in order {
        match outer {
            Some(folder) if paths[ix].starts_with(folder) => keep[ix] = false,
            _ => outer = Some(&paths[ix]),
        }
    }
    paths
        .iter()
        .zip(keep)
        .filter(|&(_, keep)| keep)
        .map(|(path, _)| path.clone())
        .collect()
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

    /// Names in `dir`, hidden ones included, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn rename_no_replace_refuses_a_taken_name() {
        let dir = Dir::new("no-replace");
        fs::write(dir.path("dest/a.txt"), "old").unwrap();
        let err = rename_no_replace(&dir.path("src/a.txt"), &dir.path("dest/a.txt")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(dir.read("dest/a.txt"), "old");
        assert_eq!(dir.read("src/a.txt"), "alpha");

        // `fs::rename` would replace an empty folder on Unix.
        fs::create_dir(dir.path("dest/sub")).unwrap();
        let err = rename_no_replace(&dir.path("src/sub"), &dir.path("dest/sub")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(dir.path("src/sub/b.txt").exists());

        rename_no_replace(&dir.path("src/a.txt"), &dir.path("dest/b.txt")).unwrap();
        assert_eq!(dir.read("dest/b.txt"), "alpha");
    }

    #[test]
    fn free_names_are_claimed_by_the_rename() {
        let dir = Dir::new("free-rename");
        fs::write(dir.path("dest/a.txt"), "1").unwrap();
        fs::write(dir.path("dest/a 2.txt"), "2").unwrap();
        let landed = rename_to_free_name(&dir.path("src/a.txt"), &dir.path("dest"), |n| match n {
            1 => "a.txt".into(),
            n => format!("a {n}.txt"),
        })
        .unwrap();
        assert_eq!(landed, dir.path("dest/a 3.txt"));
        assert_eq!(dir.read("dest/a.txt"), "1");
        assert_eq!(dir.read("dest/a 2.txt"), "2");
    }

    #[test]
    fn replace_sets_the_old_item_aside() {
        let dir = Dir::new("replace-aside");
        fs::write(dir.path("dest/a.txt"), "old").unwrap();
        fs::create_dir(dir.path("dest/sub")).unwrap();
        fs::write(dir.path("dest/sub/only-here.txt"), "keep me").unwrap();
        let report = run(
            &dir.transfer(&["src/a.txt", "src/sub"], "dest", Mode::Copy),
            Clash::Replace,
            &Progress::default(),
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(dir.read("dest/a.txt"), "alpha");
        assert_eq!(dir.read("dest/a (replaced).txt"), "old");
        assert_eq!(dir.read("dest/sub/b.txt"), "beta");
        assert_eq!(
            dir.read("dest/sub (replaced)/only-here.txt"),
            "keep me",
            "a replaced folder keeps what the new one doesn't have"
        );
        assert_eq!(
            report.replaced,
            [
                (dir.path("dest/a.txt"), dir.path("dest/a (replaced).txt")),
                (dir.path("dest/sub"), dir.path("dest/sub (replaced)")),
            ]
        );

        // Again: the first set-aside name is taken now.
        fs::write(dir.path("src/a.txt"), "newer").unwrap();
        let report = run(
            &dir.transfer(&["src/a.txt"], "dest", Mode::Move),
            Clash::Replace,
            &Progress::default(),
        );
        assert_eq!(dir.read("dest/a.txt"), "newer");
        assert_eq!(dir.read("dest/a (replaced 2).txt"), "alpha");
        assert_eq!(dir.read("dest/a (replaced).txt"), "old");
        assert!(!dir.path("src/a.txt").exists(), "it was moved");
        assert_eq!(report.replaced.len(), 1);
        assert!(
            names_in(&dir.path("dest"))
                .iter()
                .all(|n| !n.starts_with('.')),
            "nothing hidden is left: {:?}",
            names_in(&dir.path("dest"))
        );
    }

    #[test]
    fn same_named_sources_are_both_kept() {
        for (mode, clash) in [
            (Mode::Copy, Clash::Replace),
            (Mode::Copy, Clash::Skip),
            (Mode::Move, Clash::Replace),
            (Mode::Move, Clash::Skip),
        ] {
            let dir = Dir::new(&format!("same-names-{mode:?}-{clash:?}"));
            fs::create_dir(dir.path("other")).unwrap();
            fs::write(dir.path("other/a.txt"), "other").unwrap();
            let report = run(
                &dir.transfer(&["src/a.txt", "other/a.txt"], "dest", mode),
                clash,
                &Progress::default(),
            );
            assert!(
                report.failed.is_empty(),
                "{mode:?} {clash:?}: {:?}",
                report.failed
            );
            assert!(report.replaced.is_empty() && report.skipped.is_empty());
            assert_eq!(dir.read("dest/a.txt"), "alpha", "{mode:?} {clash:?}");
            assert_eq!(dir.read("dest/a 2.txt"), "other", "{mode:?} {clash:?}");
        }
    }

    #[test]
    fn replace_refuses_to_set_aside_another_source() {
        let dir = Dir::new("replace-holds");
        fs::create_dir(dir.path("dest/sub")).unwrap();
        fs::write(dir.path("dest/sub/x.txt"), "x").unwrap();
        let report = run(
            &dir.transfer(&["src/sub", "dest/sub/x.txt"], "dest", Mode::Move),
            Clash::Replace,
            &Progress::default(),
        );
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert_eq!(report.failed[0].0, dir.path("src/sub"));
        assert!(dir.path("src/sub/b.txt").exists(), "the refused item stays");
        assert_eq!(dir.read("dest/x.txt"), "x", "the other one moved");
    }

    #[cfg(unix)]
    #[test]
    fn replace_refuses_a_link_to_what_it_would_replace() {
        let dir = Dir::new("replace-link");
        fs::write(dir.path("dest/a.txt"), "the real one").unwrap();
        fs::create_dir(dir.path("links")).unwrap();
        std::os::unix::fs::symlink(dir.path("dest/a.txt"), dir.path("links/a.txt")).unwrap();
        for mode in [Mode::Copy, Mode::Move] {
            let report = run(
                &dir.transfer(&["links/a.txt"], "dest", mode),
                Clash::Replace,
                &Progress::default(),
            );
            assert_eq!(report.failed.len(), 1, "{mode:?}");
            assert_eq!(dir.read("dest/a.txt"), "the real one");
            assert!(
                !fs::symlink_metadata(dir.path("dest/a.txt"))
                    .unwrap()
                    .is_symlink()
            );
        }
    }

    #[test]
    fn a_taken_target_is_never_written_over() {
        // As when something takes the name after the clash check.
        let dir = Dir::new("taken-target");
        fs::write(dir.path("dest/a.txt"), "old").unwrap();
        fs::create_dir(dir.path("dest/sub")).unwrap();
        for mode in [Mode::Copy, Mode::Move] {
            for (source, target) in [("src/a.txt", "dest/a.txt"), ("src/sub", "dest/sub")] {
                let result = transfer_one(
                    &dir.path(source),
                    &dir.path(target),
                    mode,
                    false,
                    &Progress::default(),
                );
                assert!(result.is_err(), "{mode:?} {source}");
            }
        }
        assert_eq!(dir.read("dest/a.txt"), "old");
        assert!(fs::read_dir(dir.path("dest/sub")).unwrap().next().is_none());
        assert_eq!(dir.read("src/a.txt"), "alpha");
        assert_eq!(dir.read("src/sub/b.txt"), "beta");
        assert_eq!(
            names_in(&dir.path("dest")),
            ["a.txt", "sub"],
            "no copy is left behind"
        );
    }

    #[test]
    fn a_move_across_volumes_keeps_the_original_until_the_copy_is_whole() {
        let dir = Dir::new("move-by-copy");
        move_by_copy(
            &dir.path("src/sub"),
            &dir.path("dest/sub"),
            false,
            &Progress::default(),
        )
        .unwrap();
        assert_eq!(dir.read("dest/sub/b.txt"), "beta");
        assert!(
            !dir.path("src/sub").exists(),
            "the original goes once the copy is in place"
        );

        // A copy that stops part way leaves the original as it was.
        let progress = Progress::default();
        progress.cancel();
        let result = move_by_copy(
            &dir.path("src/a.txt"),
            &dir.path("dest/a.txt"),
            false,
            &progress,
        );
        assert!(matches!(result, Err(TransferError::Cancelled)));
        assert_eq!(dir.read("src/a.txt"), "alpha");
        assert_eq!(names_in(&dir.path("dest")), ["sub"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_move_across_volumes_with_an_unreadable_file_moves_nothing() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Dir::new("move-unreadable");
        let locked = dir.path("src/sub/locked.txt");
        fs::write(&locked, "secret").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&locked).is_ok() {
            return; // Running as root, which reads anything.
        }
        let result = move_by_copy(
            &dir.path("src/sub"),
            &dir.path("dest/sub"),
            false,
            &Progress::default(),
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.is_err());
        assert_eq!(dir.read("src/sub/b.txt"), "beta");
        assert_eq!(dir.read("src/sub/locked.txt"), "secret");
        assert!(names_in(&dir.path("dest")).is_empty(), "no partial copy");
    }

    #[test]
    fn a_copy_that_differs_from_its_original_is_caught() {
        let dir = Dir::new("settle");
        copy_tree(
            &dir.path("src"),
            &dir.path("dest/src"),
            &Progress::default(),
        )
        .unwrap();
        assert!(settle_copy(&dir.path("src"), &dir.path("dest/src")).is_ok());
        fs::write(dir.path("src/a.txt"), "alpha, changed while it was copied").unwrap();
        assert!(settle_copy(&dir.path("src"), &dir.path("dest/src")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn pipes_are_refused_rather_than_waited_on() {
        let dir = Dir::new("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(dir.path("src/sub/pipe"))
            .status();
        if !made.is_ok_and(|status| status.success()) {
            return;
        }
        let (src, dest) = (dir.path("src"), dir.path("dest/src"));
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(copy_tree(&src, &dest, &Progress::default()).is_err());
        });
        let refused = receive
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("copying a folder with a pipe in it finishes");
        assert!(refused);
    }

    #[cfg(unix)]
    #[test]
    fn folder_tags_and_comments_are_copied() {
        let dir = Dir::new("xattrs");
        if xattr::set(dir.path("src/sub"), "user.fffinder-test", b"tagged").is_err() {
            return; // This file system has no extended attributes.
        }
        copy_tree(
            &dir.path("src/sub"),
            &dir.path("dest/sub"),
            &Progress::default(),
        )
        .unwrap();
        assert_eq!(
            xattr::get(dir.path("dest/sub"), "user.fffinder-test").unwrap(),
            Some(b"tagged".to_vec())
        );
    }

    #[test]
    fn long_names_still_have_room_to_stage() {
        let name = format!("{}.txt", "é".repeat(120));
        assert_eq!(name.len(), 244);
        let staged = staging_path(&Path::new("/a").join(&name));
        let staged = staged.file_name().unwrap().to_string_lossy().into_owned();
        assert!(staged.len() <= 255, "{} bytes", staged.len());
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
        assert_eq!(tree_size(&dir.path("src"), &Progress::default()), 9);
        assert!(same_volume(&dir.path("src/a.txt"), &dir.path("dest")));
        let stopped = Progress::default();
        stopped.cancel();
        assert_eq!(
            tree_size(&dir.path("src"), &stopped),
            0,
            "Stop ends the walk"
        );
    }

    #[test]
    fn keeps_only_the_outermost_items() {
        let paths: Vec<PathBuf> = ["/a/b/c", "/a", "/ab", "/a/b", "/x", "/a", "/a b/c"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let expected: Vec<PathBuf> = ["/a", "/ab", "/x", "/a b/c"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(outermost(&paths), expected);
    }

    #[test]
    fn staging_names_differ_per_call() {
        let target = Path::new("/a/b.txt");
        assert_ne!(staging_path(target), staging_path(target));
        // Compared by parts, since Windows joins the name on with `\`.
        let staged = staging_path(target);
        assert_eq!(staged.parent(), target.parent());
        let name = staged
            .file_name()
            .expect("a staging name")
            .to_string_lossy();
        assert!(name.starts_with(".b.txt.fffinder-"), "{name}");
    }

    #[test]
    fn progress_starts_at_nothing_done() {
        let dir = Dir::new("fraction");
        let progress = Progress::default();
        assert_eq!(progress.fraction(), 0.0, "nothing is counted yet");
        let transfer = dir.transfer(&["src/a.txt"], "dest", Mode::Move);
        let report = run(&transfer, Clash::Skip, &progress);
        assert_eq!(report.done.len(), 1);
        assert_eq!(progress.fraction(), 1.0, "a rename copies no bytes");
    }

    #[test]
    fn counts_bytes_while_copying() {
        let dir = Dir::new("sized");
        let progress = Progress::default();
        let sources = [dir.path("src")];
        let copied = sized_while(&sources, &progress, || {
            copy_tree(&dir.path("src"), &dir.path("dest/src"), &progress)
        });
        assert!(copied.is_ok());
        assert_eq!(dir.read("dest/src/sub/b.txt"), "beta");
        assert_eq!(progress.fraction(), 1.0);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_folder_into_itself_through_a_link() {
        let dir = Dir::new("itself-link");
        std::os::unix::fs::symlink(dir.path("src"), dir.path("alias")).unwrap();
        let transfer = dir.transfer(&["src"], "alias/sub", Mode::Copy);
        assert!(matches!(
            transfer.check(),
            Err(TransferError::IntoItself(_))
        ));
        // Copying the link itself into the folder it points to is fine: the
        // copy is a link, not the folder.
        let link = dir.transfer(&["alias"], "src/sub", Mode::Copy);
        assert!(link.check().is_ok());
    }

    #[test]
    fn refuses_a_folder_into_itself_in_another_case() {
        let dir = Dir::new("itself-case");
        // Only where the volume ignores case, as macOS and Windows do.
        if !dir.path("SRC").exists() {
            return;
        }
        let transfer = dir.transfer(&["src"], "SRC/sub", Mode::Copy);
        assert!(matches!(
            transfer.check(),
            Err(TransferError::IntoItself(_))
        ));
    }
}
