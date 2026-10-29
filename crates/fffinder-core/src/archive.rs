//! Zip and tar archives: Compress puts items in a zip, and Extract unpacks
//! a zip, tar, or gzipped tar beside itself, the way Finder's Archive
//! Utility does. Both report progress and stop when asked. Nothing is left
//! half made: the work happens under a hidden name and is renamed at the end.

use crate::ops::{
    Progress, TransferError, free_name, io_error, make_symlink, numbered_name, remove_any,
    sized_while, staging_path,
};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

const CHUNK: usize = 256 * 1024;
/// A gzipped tar is listed by reading through it, so a preview stops after
/// this many unpacked bytes and says there's more.
const LIST_BYTES: u64 = 64 * 1024 * 1024;

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
    let temp = staging_path(zip);
    // Only what this job made is removed when it fails.
    let file = File::create_new(&temp).map_err(io_error(&temp))?;
    let result = sized_while(sources, progress, || {
        let mut writer = zip::ZipWriter::new(file);
        for source in sources {
            progress.set_current(crate::entry::display_name(source));
            let base = source.parent().unwrap_or(Path::new(""));
            add_to_zip(&mut writer, source, base, progress)?;
        }
        writer.finish().map_err(zip_error(&temp))?;
        fs::rename(&temp, zip).map_err(io_error(zip))
    });
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

/// One item inside an archive, for a preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The path inside the archive, with `/` separators.
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// The first `limit` items in an archive, and whether there are more. A
/// plain tar skips over what its items hold; a gzipped one has to be read
/// through, so it's read only as far as needed and at most [`LIST_BYTES`].
pub fn list(path: &Path, limit: usize) -> io::Result<(Vec<Listed>, bool)> {
    list_within(path, limit, LIST_BYTES)
}

fn list_within(path: &Path, limit: usize, max_bytes: u64) -> io::Result<(Vec<Listed>, bool)> {
    let file = File::open(path)?;
    match format(path) {
        Some(Format::Zip) => {
            let mut zip = zip::ZipArchive::new(file).map_err(io::Error::other)?;
            let listed = (0..zip.len().min(limit))
                .filter_map(|i| {
                    let entry = zip.by_index_raw(i).ok()?;
                    Some(Listed {
                        name: entry.name().ok()?.trim_end_matches('/').to_string(),
                        size: entry.size(),
                        is_dir: entry.is_dir(),
                    })
                })
                .collect();
            Ok((listed, zip.len() > limit))
        }
        Some(Format::Tar) => {
            let mut tar = tar::Archive::new(io::BufReader::new(file));
            let mut listed = Vec::new();
            let more = list_tar(tar.entries_with_seek()?, limit, &mut listed)?;
            Ok((listed, more))
        }
        Some(Format::TarGz) => {
            let unpacked = flate2::read::GzDecoder::new(io::BufReader::new(file)).take(max_bytes);
            let mut tar = tar::Archive::new(unpacked);
            let mut listed = Vec::new();
            let more = list_tar(tar.entries()?, limit, &mut listed);
            match more {
                Ok(more) => Ok((listed, more)),
                // Running out of bytes to read means the cap was reached.
                Err(_) if tar.into_inner().limit() == 0 => Ok((listed, true)),
                Err(err) => Err(err),
            }
        }
        None => Err(io::Error::other("not a zip or tar archive")),
    }
}

/// Add tar items to `listed` until it holds `limit`; returns whether there
/// are more.
fn list_tar<R: Read>(
    entries: tar::Entries<'_, R>,
    limit: usize,
    listed: &mut Vec<Listed>,
) -> io::Result<bool> {
    for entry in entries {
        if listed.len() == limit {
            return Ok(true);
        }
        let entry = entry?;
        let header = entry.header();
        listed.push(Listed {
            name: entry
                .path()?
                .to_string_lossy()
                .trim_end_matches('/')
                .to_string(),
            size: header.size().unwrap_or(0),
            is_dir: header.entry_type().is_dir(),
        });
    }
    Ok(false)
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
    let temp = staging_path(&dir.join(&name));
    progress.set_current(crate::entry::display_name(archive));
    // Only what this job made is removed when it fails.
    fs::create_dir(&temp).map_err(io_error(&temp))?;
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
    progress.counted();
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
        // Everything else is unpacked by now, so a link to a folder can tell.
        let is_dir = link.parent().is_some_and(|dir| dir.join(&target).is_dir());
        make_symlink(&target, &link, is_dir).map_err(io_error(&link))?;
    }
    Ok(())
}

fn extract_tar(
    archive: &Path,
    format: Format,
    into: &Path,
    progress: &Progress,
) -> Result<(), TransferError> {
    let file = File::open(archive).map_err(io_error(archive))?;
    progress.expect(file.metadata().map(|m| m.len()).unwrap_or(0));
    progress.counted();
    let counted = Counted {
        inner: io::BufReader::new(file),
        progress,
    };
    let reader: Box<dyn Read> = match format {
        Format::TarGz => Box::new(flate2::read::GzDecoder::new(counted)),
        Format::Tar | Format::Zip => Box::new(counted),
    };
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
    fn lists_contents() {
        let dir = temp("list");
        let zip = dir.join("folder.zip");
        compress(&[dir.join("folder")], &zip, &Progress::default()).unwrap();
        let (listed, more) = list(&zip, 10).unwrap();
        let names: Vec<&str> = listed.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "folder",
                "folder/a.txt",
                "folder/inner",
                "folder/inner/b.txt"
            ]
        );
        assert!(!more && listed[0].is_dir && listed[1].size == 5);
        assert!(list(&zip, 2).unwrap().1, "more than the limit");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_a_gzipped_tar_only_so_far() {
        let dir = temp("list-cap");
        fs::write(dir.join("folder/big.bin"), vec![7u8; 256 * 1024]).unwrap();
        let tgz = dir.join("pack.tgz");
        let gz = flate2::write::GzEncoder::new(File::create(&tgz).unwrap(), Default::default());
        let mut builder = tar::Builder::new(gz);
        builder
            .append_path_with_name(dir.join("c.txt"), "c.txt")
            .unwrap();
        builder
            .append_path_with_name(dir.join("folder/big.bin"), "big.bin")
            .unwrap();
        builder
            .append_path_with_name(dir.join("c.txt"), "last.txt")
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let (all, more) = list(&tgz, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert!(!more);
        // The cap falls inside the big item: what came before it is listed.
        let (some, more) = list_within(&tgz, 10, 64 * 1024).unwrap();
        let names: Vec<&str> = some.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["c.txt", "big.bin"]);
        assert!(more);

        let plain = dir.join("pack.tar");
        let mut builder = tar::Builder::new(File::create(&plain).unwrap());
        builder
            .append_path_with_name(dir.join("folder/big.bin"), "big.bin")
            .unwrap();
        builder
            .append_path_with_name(dir.join("c.txt"), "c.txt")
            .unwrap();
        builder.finish().unwrap();
        let (listed, more) = list(&plain, 1).unwrap();
        assert_eq!(listed[0].name, "big.bin");
        assert!(more);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn archives_with_one_name_extract_at_once() {
        let dir = temp("same-name");
        let zip = dir.join("a.zip");
        compress(&[dir.join("folder")], &zip, &Progress::default()).unwrap();
        let tgz = dir.join("a.tar.gz");
        let gz = flate2::write::GzEncoder::new(File::create(&tgz).unwrap(), Default::default());
        let mut builder = tar::Builder::new(gz);
        builder.append_dir_all("pack", dir.join("folder")).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        // Both work under a hidden name made from "a"; neither may take or
        // remove the other's.
        let (one, two) = std::thread::scope(|scope| {
            let one = scope.spawn(|| extract(&zip, &Progress::default()));
            let two = scope.spawn(|| extract(&tgz, &Progress::default()));
            (one.join().unwrap(), two.join().unwrap())
        });
        assert_eq!(one.unwrap(), dir.join("folder 2"));
        assert_eq!(two.unwrap(), dir.join("pack"));
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
