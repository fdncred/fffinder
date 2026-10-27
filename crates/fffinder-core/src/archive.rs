//! Zip and tar archives: Compress puts items in a zip, and Extract unpacks
//! a zip, tar, or gzipped tar beside itself, the way Finder's Archive
//! Utility does. Both report progress and stop when asked. Nothing is left
//! half made: the work happens under a hidden name and is renamed at the end.

use crate::ops::{Progress, TransferError, free_name, numbered_name, remove_any, tree_size};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

const CHUNK: usize = 256 * 1024;

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> TransferError + '_ {
    move |source| TransferError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn zip_error(path: &Path) -> impl FnOnce(zip::result::ZipError) -> TransferError + '_ {
    move |err| TransferError::Io {
        path: path.to_path_buf(),
        source: io::Error::other(err),
    }
}

/// The kinds of archive Extract understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
}

/// What kind of archive `path` is, by its name.
pub fn format(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".zip") {
        Some(Format::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Format::TarGz)
    } else if name.ends_with(".tar") {
        Some(Format::Tar)
    } else {
        None
    }
}

/// `photos.tar.gz` → `photos`.
fn stem(path: &Path) -> String {
    let name = crate::entry::display_name(path);
    let lower = name.to_lowercase();
    let cut = [".tar.gz", ".tgz", ".tar", ".zip"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map_or(0, |ext| ext.len());
    name[..name.len() - cut].to_string()
}

/// A hidden name in `dir` for work in progress.
fn staging(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.fffinder-{}", std::process::id()))
}

/// Where Compress puts its zip: "name.zip" for one item, "Archive.zip"
/// for several, numbered when the name is taken.
pub fn zip_path(sources: &[PathBuf]) -> Option<PathBuf> {
    let dir = sources.first()?.parent()?;
    let name = match sources {
        [one] => crate::entry::display_name(one),
        _ => "Archive".into(),
    };
    let zip = format!("{name}.zip");
    Some(match dir.join(&zip).symlink_metadata() {
        Ok(_) => numbered_name(dir, &zip, false),
        Err(_) => dir.join(zip),
    })
}

/// Put `sources` (files and folders, which share a parent) into a new zip
/// at `zip`. Links are stored as links.
pub fn compress(sources: &[PathBuf], zip: &Path, progress: &Progress) -> Result<(), TransferError> {
    let dir = zip.parent().unwrap_or(Path::new("."));
    let temp = staging(dir, &crate::entry::display_name(zip));
    progress.expect(sources.iter().map(|s| tree_size(s)).sum());
    let result = (|| {
        let file = File::create_new(&temp).map_err(io_error(&temp))?;
        let mut writer = zip::ZipWriter::new(file);
        for source in sources {
            progress.set_current(crate::entry::display_name(source));
            let base = source.parent().unwrap_or(Path::new(""));
            add_to_zip(&mut writer, source, base, progress)?;
        }
        writer.finish().map_err(zip_error(&temp))?;
        fs::rename(&temp, zip).map_err(io_error(zip))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn add_to_zip(
    writer: &mut zip::ZipWriter<File>,
    path: &Path,
    base: &Path,
    progress: &Progress,
) -> Result<(), TransferError> {
    progress.check()?;
    let meta = fs::symlink_metadata(path).map_err(io_error(path))?;
    // Zip names use `/` on every system.
    let name = path
        .strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let mut options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(meta.len() >= u32::MAX as u64);
    if let Some(time) = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).naive_local())
        .and_then(|t| zip::DateTime::try_from(t).ok())
    {
        options = options.last_modified_time(time);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        options = options.unix_permissions(meta.permissions().mode() & 0o7777);
    }
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(io_error(path))?;
        return writer
            .add_symlink(name, target.to_string_lossy(), options)
            .map_err(zip_error(path));
    }
    if meta.is_dir() {
        writer
            .add_directory(format!("{name}/"), options)
            .map_err(zip_error(path))?;
        let mut children: Vec<PathBuf> = fs::read_dir(path)
            .map_err(io_error(path))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        children.sort();
        for child in children {
            add_to_zip(writer, &child, base, progress)?;
        }
        return Ok(());
    }
    writer.start_file(name, options).map_err(zip_error(path))?;
    let mut file = File::open(path).map_err(io_error(path))?;
    copy_with_progress(&mut file, writer, progress).map_err(io_error(path))?
}

/// Copy everything from `from` to `to`, counting bytes and stopping when
/// asked. The outer `Result` is the I/O, the inner one Stop.
fn copy_with_progress(
    from: &mut impl Read,
    to: &mut impl io::Write,
    progress: &Progress,
) -> io::Result<Result<(), TransferError>> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        if let Err(stopped) = progress.check() {
            return Ok(Err(stopped));
        }
        let read = from.read(&mut buf)?;
        if read == 0 {
            return Ok(Ok(()));
        }
        to.write_all(&buf[..read])?;
        progress.advance(read as u64);
    }
}

/// Counts the compressed bytes a tar reader takes, for progress.
struct Counted<'a, R> {
    inner: R,
    progress: &'a Progress,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.progress.advance(read as u64);
        Ok(read)
    }
}

/// Unpack an archive beside itself. One item at the top of the archive
/// lands as itself; several land in a folder named after the archive.
/// Returns where the result is.
pub fn extract(archive: &Path, progress: &Progress) -> Result<PathBuf, TransferError> {
    let format = format(archive).ok_or_else(|| TransferError::Io {
        path: archive.to_path_buf(),
        source: io::Error::other("not a zip or tar archive"),
    })?;
    let dir = archive.parent().unwrap_or(Path::new("."));
    let name = stem(archive);
    let temp = staging(dir, &name);
    progress.set_current(crate::entry::display_name(archive));
    let unpacked = match format {
        Format::Zip => extract_zip(archive, &temp, progress),
        Format::Tar | Format::TarGz => extract_tar(archive, format, &temp, progress),
    };
    let landed = unpacked.and_then(|()| settle(&temp, dir, &name));
    if landed.is_err() {
        let _ = remove_any(&temp);
    }
    landed
}

/// Move what was unpacked into `temp` to its place in `dir`.
fn settle(temp: &Path, dir: &Path, name: &str) -> Result<PathBuf, TransferError> {
    let top: Vec<PathBuf> = fs::read_dir(temp)
        .map_err(io_error(temp))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    if let [only] = top.as_slice() {
        let only_name = crate::entry::display_name(only);
        let target = match dir.join(&only_name).symlink_metadata() {
            Ok(_) => numbered_name(dir, &only_name, only.is_dir()),
            Err(_) => dir.join(&only_name),
        };
        fs::rename(only, &target).map_err(io_error(&target))?;
        let _ = fs::remove_dir(temp);
        return Ok(target);
    }
    let target = free_name(dir, |n| match n {
        1 => name.to_string(),
        n => format!("{name} {n}"),
    });
    fs::rename(temp, &target).map_err(io_error(&target))?;
    Ok(target)
}

fn extract_zip(archive: &Path, into: &Path, progress: &Progress) -> Result<(), TransferError> {
    let file = File::open(archive).map_err(io_error(archive))?;
    let mut zip = zip::ZipArchive::new(file).map_err(zip_error(archive))?;
    progress.expect(
        (0..zip.len())
            .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.size()))
            .sum(),
    );
    fs::create_dir(into).map_err(io_error(into))?;
    // Links come last, so no file is written through one.
    let mut links = Vec::new();
    for i in 0..zip.len() {
        progress.check()?;
        let mut entry = zip.by_index(i).map_err(zip_error(archive))?;
        // Names that would land outside the folder (`../x`, `/x`) are skipped.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = into.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(io_error(&out))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        if entry.is_symlink() {
            let mut target = String::new();
            entry.read_to_string(&mut target).map_err(io_error(&out))?;
            links.push((out, PathBuf::from(target)));
            continue;
        }
        let mut file = File::create_new(&out).map_err(io_error(&out))?;
        copy_with_progress(&mut entry, &mut file, progress).map_err(io_error(&out))??;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o7777));
        }
    }
    for (link, target) in links {
        make_link(&target, &link).map_err(io_error(&link))?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn extract_tar(
    archive: &Path,
    format: Format,
    into: &Path,
    progress: &Progress,
) -> Result<(), TransferError> {
    let file = File::open(archive).map_err(io_error(archive))?;
    progress.expect(file.metadata().map(|m| m.len()).unwrap_or(0));
    let counted = Counted {
        inner: io::BufReader::new(file),
        progress,
    };
    let reader: Box<dyn Read> = match format {
        Format::TarGz => Box::new(flate2::read::GzDecoder::new(counted)),
        Format::Tar | Format::Zip => Box::new(counted),
    };
    fs::create_dir(into).map_err(io_error(into))?;
    let mut tar = tar::Archive::new(reader);
    tar.set_preserve_mtime(true);
    for entry in tar.entries().map_err(io_error(archive))? {
        progress.check()?;
        // `unpack_in` refuses entries that would land outside the folder.
        entry
            .and_then(|mut e| e.unpack_in(into))
            .map_err(io_error(archive))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fffinder-archive-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("folder/inner")).unwrap();
        fs::write(dir.join("folder/a.txt"), "alpha").unwrap();
        fs::write(dir.join("folder/inner/b.txt"), "beta").unwrap();
        fs::write(dir.join("c.txt"), "gamma").unwrap();
        dir
    }

    #[test]
    fn compresses_and_extracts_a_zip() {
        let dir = temp("zip");
        let one = zip_path(&[dir.join("folder")]).unwrap();
        assert_eq!(one, dir.join("folder.zip"));
        compress(&[dir.join("folder")], &one, &Progress::default()).unwrap();

        // One item at the top lands as itself, numbered beside the original.
        let out = extract(&one, &Progress::default()).unwrap();
        assert_eq!(out, dir.join("folder 2"));
        assert_eq!(fs::read_to_string(out.join("inner/b.txt")).unwrap(), "beta");

        // Several land in a folder named after the archive.
        let both = [dir.join("folder"), dir.join("c.txt")];
        let many = zip_path(&both).unwrap();
        assert_eq!(many, dir.join("Archive.zip"));
        compress(&both, &many, &Progress::default()).unwrap();
        let out = extract(&many, &Progress::default()).unwrap();
        assert_eq!(out, dir.join("Archive"));
        assert_eq!(fs::read_to_string(out.join("c.txt")).unwrap(), "gamma");
        assert_eq!(
            fs::read_to_string(out.join("folder/a.txt")).unwrap(),
            "alpha"
        );
        let leftovers = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains(".fffinder-"));
        assert!(!leftovers, "no work-in-progress names remain");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_a_gzipped_tar() {
        let dir = temp("tar");
        let tgz = dir.join("pack.tar.gz");
        let gz = flate2::write::GzEncoder::new(File::create(&tgz).unwrap(), Default::default());
        let mut builder = tar::Builder::new(gz);
        builder.append_dir_all("pack", dir.join("folder")).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(format(&tgz), Some(Format::TarGz));
        let progress = Progress::default();
        let out = extract(&tgz, &progress).unwrap();
        assert_eq!(out, dir.join("pack"));
        assert_eq!(fs::read_to_string(out.join("a.txt")).unwrap(), "alpha");
        assert_eq!(progress.fraction(), 1.0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stopping_leaves_nothing() {
        let dir = temp("stop");
        let progress = Progress::default();
        progress.cancel();
        let zip = dir.join("x.zip");
        assert!(compress(&[dir.join("folder")], &zip, &progress).is_err());
        assert!(!zip.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "only the fixture");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn names() {
        assert_eq!(stem(Path::new("/a/photos.tar.gz")), "photos");
        assert_eq!(stem(Path::new("/a/Notes.ZIP")), "Notes");
        assert_eq!(format(Path::new("x.tgz")), Some(Format::TarGz));
        assert_eq!(format(Path::new("x.txt")), None);
    }
}
