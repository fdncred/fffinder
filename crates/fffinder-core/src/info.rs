//! What Get Info shows about an item: sizes, dates, owner and permissions,
//! a link's target, and on macOS where a download came from. Folder sizes
//! come from a parallel walk, since they can take a while.

use rayon::prelude::*;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// One item's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub is_dir: bool,
    /// Bytes in a file. Folders are measured with [`folder_size`].
    pub size: Option<u64>,
    /// Space the file takes on disk, in whole blocks (Unix).
    pub on_disk: Option<u64>,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    /// When the item was last opened: what macOS records, else the access
    /// time.
    pub last_opened: Option<SystemTime>,
    /// Where a symbolic link points.
    pub link_target: Option<PathBuf>,
    /// Unix permission bits, like `0o644`.
    pub mode: Option<u32>,
    pub readonly: bool,
    pub owner: Option<String>,
    pub group: Option<String>,
    /// The web pages a download came from (macOS).
    pub where_from: Vec<String>,
}

/// Read everything Get Info shows about `path`, without walking folders.
///
/// # Errors
///
/// Fails when the item can't be read at all; details that can't be read
/// are left out.
pub fn read(path: &Path) -> io::Result<Info> {
    let link = fs::symlink_metadata(path)?;
    let link_target = link
        .file_type()
        .is_symlink()
        .then(|| fs::read_link(path).ok())
        .flatten();
    // Describe what a link points at, or the link when it points nowhere.
    let meta = fs::metadata(path).unwrap_or(link);
    let is_dir = meta.is_dir();
    let (mode, on_disk, owner, group) = ownership(path, &meta);
    Ok(Info {
        path: path.to_path_buf(),
        name: crate::entry::display_name(path),
        kind: crate::kind::kind_label(path, is_dir),
        is_dir,
        size: (!is_dir).then_some(meta.len()),
        on_disk: on_disk.filter(|_| !is_dir),
        created: meta.created().ok(),
        modified: meta.modified().ok(),
        last_opened: last_used(path).or_else(|| meta.accessed().ok()),
        link_target,
        mode,
        readonly: meta.permissions().readonly(),
        owner,
        group,
        where_from: where_from(path),
    })
}

/// Permission bits, space on disk, owner, and group, as far as the system
/// has them.
#[cfg(unix)]
fn ownership(
    _: &Path,
    meta: &fs::Metadata,
) -> (Option<u32>, Option<u64>, Option<String>, Option<String>) {
    use std::os::unix::fs::MetadataExt as _;
    let name = |n: &std::ffi::OsStr| n.to_string_lossy().into_owned();
    (
        Some(meta.mode() & 0o7777),
        Some(meta.blocks() * 512),
        uzers::get_user_by_uid(meta.uid()).map(|u| name(u.name())),
        uzers::get_group_by_gid(meta.gid()).map(|g| name(g.name())),
    )
}

/// Windows has no permission bits or groups here; the owner comes from
/// the file's security descriptor, and the space on disk from its
/// compressed size.
#[cfg(windows)]
fn ownership(
    path: &Path,
    meta: &fs::Metadata,
) -> (Option<u32>, Option<u64>, Option<String>, Option<String>) {
    let on_disk = (!meta.is_dir()).then(|| win::size_on_disk(path)).flatten();
    (None, on_disk, win::owner(path), None)
}

#[cfg(windows)]
mod win {
    use std::path::Path;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        LookupAccountSidW, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE,
    };
    use windows::Win32::Storage::FileSystem::GetCompressedFileSizeW;
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    /// `DOMAIN\user` for the account that owns `path`.
    pub fn owner(path: &Path) -> Option<String> {
        let name = HSTRING::from(path.as_os_str());
        let mut sid = PSID::default();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the owner SID points into the descriptor, which is freed
        // only after the SID's name has been read.
        unsafe {
            if GetNamedSecurityInfoW(
                &name,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(&mut sid),
                None,
                None,
                None,
                &mut descriptor,
            )
            .is_err()
            {
                return None;
            }
            let (mut user_len, mut domain_len) = (256u32, 256u32);
            let (mut user, mut domain) = (vec![0u16; 256], vec![0u16; 256]);
            let mut kind = SID_NAME_USE::default();
            let found = LookupAccountSidW(
                PCWSTR::null(),
                sid,
                Some(PWSTR(user.as_mut_ptr())),
                &mut user_len,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut domain_len,
                &mut kind,
            );
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            found.ok()?;
            let user = String::from_utf16_lossy(&user[..user_len as usize]);
            let domain = String::from_utf16_lossy(&domain[..domain_len as usize]);
            Some(if domain.is_empty() {
                user
            } else {
                format!("{domain}\\{user}")
            })
        }
    }

    /// What the file takes on disk, which compression and sparseness can
    /// make smaller than its size.
    pub fn size_on_disk(path: &Path) -> Option<u64> {
        let name = HSTRING::from(path.as_os_str());
        let mut high = 0u32;
        // SAFETY: `high` receives the upper 32 bits of the size.
        let low = unsafe { GetCompressedFileSizeW(&name, Some(&mut high)) };
        (low != u32::MAX).then(|| (u64::from(high) << 32) | u64::from(low))
    }
}

#[cfg(not(any(unix, windows)))]
fn ownership(
    _: &Path,
    _: &fs::Metadata,
) -> (Option<u32>, Option<u64>, Option<String>, Option<String>) {
    (None, None, None, None)
}

/// macOS records the last time an item was opened in this attribute: a
/// `timespec`, seconds then nanoseconds, little-endian.
#[cfg(target_os = "macos")]
fn last_used(path: &Path) -> Option<SystemTime> {
    let bytes = xattr::get(path, "com.apple.lastuseddate#PS").ok()??;
    let secs = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
    let nanos = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, nanos as u32))
}

#[cfg(not(target_os = "macos"))]
fn last_used(_: &Path) -> Option<SystemTime> {
    None
}

/// Browsers record a download's page and file URLs in this attribute, as
/// a binary property list of strings.
#[cfg(target_os = "macos")]
fn where_from(path: &Path) -> Vec<String> {
    xattr::get(path, "com.apple.metadata:kMDItemWhereFroms")
        .ok()
        .flatten()
        .and_then(|bytes| plist::from_bytes::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|url| !url.is_empty())
        .collect()
}

/// Chromium and Firefox record a download's address and the page it came
/// from in freedesktop.org attributes.
#[cfg(all(unix, not(target_os = "macos")))]
fn where_from(path: &Path) -> Vec<String> {
    ["user.xdg.origin.url", "user.xdg.referrer.url"]
        .into_iter()
        .filter_map(|attr| xattr::get(path, attr).ok().flatten())
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
        .filter(|url| !url.is_empty())
        .collect()
}

/// Browsers mark downloads with a `Zone.Identifier` stream holding the
/// file's address and the page it came from.
#[cfg(windows)]
fn where_from(path: &Path) -> Vec<String> {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    let text = fs::read_to_string(stream).unwrap_or_default();
    zone_urls(&text)
}

/// `HostUrl=` and `ReferrerUrl=` from a `Zone.Identifier` stream.
#[cfg_attr(not(windows), allow(dead_code))]
fn zone_urls(text: &str) -> Vec<String> {
    ["HostUrl=", "ReferrerUrl="]
        .into_iter()
        .filter_map(|key| text.lines().find_map(|line| line.trim().strip_prefix(key)))
        .map(str::trim)
        .filter(|url| !url.is_empty() && *url != "about:internet")
        .map(str::to_string)
        .collect()
}

/// What a folder holds, all the way down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderSize {
    pub bytes: u64,
    pub on_disk: u64,
    /// Files and folders inside, not counting the folder itself.
    pub items: u64,
}

impl std::ops::Add for FolderSize {
    type Output = FolderSize;

    fn add(self, other: FolderSize) -> FolderSize {
        FolderSize {
            bytes: self.bytes + other.bytes,
            on_disk: self.on_disk + other.on_disk,
            items: self.items + other.items,
        }
    }
}

/// Add up everything inside `path`, reading subfolders in parallel. Links
/// aren't followed. Returns what was counted so far once `stop` is set.
pub fn folder_size(path: &Path, stop: &AtomicBool) -> FolderSize {
    let Ok(entries) = fs::read_dir(path) else {
        return FolderSize::default();
    };
    entries
        .filter_map(Result::ok)
        .par_bridge()
        .map(|entry| {
            if stop.load(Ordering::Relaxed) {
                return FolderSize::default();
            }
            // A directory entry's metadata doesn't follow links.
            let Ok(meta) = entry.metadata() else {
                return FolderSize::default();
            };
            let this = FolderSize {
                bytes: if meta.is_dir() { 0 } else { meta.len() },
                on_disk: blocks_on_disk(&meta),
                items: 1,
            };
            if meta.is_dir() {
                this + folder_size(&entry.path(), stop)
            } else {
                this
            }
        })
        .reduce(FolderSize::default, |a, b| a + b)
}

#[cfg(unix)]
fn blocks_on_disk(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn blocks_on_disk(meta: &fs::Metadata) -> u64 {
    if meta.is_dir() { 0 } else { meta.len() }
}

/// `0o754` → `rwxr-xr--`.
pub fn mode_string(mode: u32) -> String {
    (0..9)
        .map(|bit| {
            let on = mode & (0o400 >> bit) != 0;
            match (on, bit % 3) {
                (false, _) => '-',
                (true, 0) => 'r',
                (true, 1) => 'w',
                (true, _) => 'x',
            }
        })
        .collect()
}

/// Change an item's permission bits (Unix) or its read-only flag.
///
/// # Errors
///
/// Fails when the item isn't yours to change.
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(mode & 0o200 == 0);
        fs::set_permissions(path, permissions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fffinder-info-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        fs::write(dir.join("a.txt"), "12345").unwrap();
        fs::write(dir.join("sub/b.txt"), "123").unwrap();
        fs::write(dir.join("sub/deeper/c.txt"), "1").unwrap();
        dir
    }

    #[test]
    fn reads_a_file() {
        let dir = temp("file");
        let info = read(&dir.join("a.txt")).unwrap();
        assert_eq!(info.name, "a.txt");
        assert_eq!(info.size, Some(5));
        assert!(!info.is_dir && info.modified.is_some());
        #[cfg(unix)]
        {
            assert!(info.owner.is_some());
            assert!(info.on_disk.is_some());
            set_mode(&dir.join("a.txt"), 0o640).unwrap();
            assert_eq!(read(&dir.join("a.txt")).unwrap().mode, Some(0o640));
            std::os::unix::fs::symlink("a.txt", dir.join("link")).unwrap();
            let link = read(&dir.join("link")).unwrap();
            assert_eq!(link.link_target.as_deref(), Some(Path::new("a.txt")));
            assert_eq!(link.size, Some(5), "a link describes what it points at");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn adds_up_a_folder() {
        let dir = temp("folder");
        let size = folder_size(&dir, &AtomicBool::new(false));
        assert_eq!(size.bytes, 9);
        assert_eq!(size.items, 5, "three files and two folders");
        assert_eq!(folder_size(&dir, &AtomicBool::new(true)).items, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_zone_identifiers() {
        let stream = "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://example.com/\r\nHostUrl=https://example.com/a.zip\r\n";
        assert_eq!(
            zone_urls(stream),
            ["https://example.com/a.zip", "https://example.com/"]
        );
        assert!(zone_urls("[ZoneTransfer]\nZoneId=3\nHostUrl=about:internet\n").is_empty());
    }

    #[test]
    fn mode_strings() {
        assert_eq!(mode_string(0o754), "rwxr-xr--");
        assert_eq!(mode_string(0o600), "rw-------");
    }
}
