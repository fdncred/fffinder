//! Sidebar locations: standard favorites and mounted volumes, per platform.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Icon hint for a sidebar row; the GUI maps these to glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceIcon {
    Applications,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Movies,
    Home,
    Cloud,
    Drive,
    External,
    Folder,
    /// A server mounted over the network.
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub icon: PlaceIcon,
    /// Removable volumes can be ejected.
    pub ejectable: bool,
}

impl Place {
    fn new(name: impl Into<String>, path: PathBuf, icon: PlaceIcon) -> Place {
        Place {
            name: name.into(),
            path,
            icon,
            ejectable: false,
        }
    }
}

/// Finder's default favorites that exist on this machine.
pub fn default_favorites() -> Vec<Place> {
    let mut places = Vec::new();
    #[cfg(target_os = "macos")]
    places.push(Place::new(
        "Applications",
        PathBuf::from("/Applications"),
        PlaceIcon::Applications,
    ));
    let candidates = [
        (dirs::document_dir(), "Documents", PlaceIcon::Documents),
        (dirs::download_dir(), "Downloads", PlaceIcon::Downloads),
        (dirs::desktop_dir(), "Desktop", PlaceIcon::Desktop),
        (dirs::picture_dir(), "Pictures", PlaceIcon::Pictures),
        (dirs::audio_dir(), "Music", PlaceIcon::Music),
        (
            dirs::video_dir(),
            if cfg!(target_os = "macos") {
                "Movies"
            } else {
                "Videos"
            },
            PlaceIcon::Movies,
        ),
    ];
    for (dir, name, icon) in candidates {
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            places.push(Place::new(name, dir, icon));
        }
    }
    places
}

/// Map an arbitrary favorite folder to a label and icon.
pub fn favorite_for(path: &Path) -> Place {
    default_favorites()
        .into_iter()
        .find(|p| p.path == path)
        .unwrap_or_else(|| {
            Place::new(
                crate::entry::display_name(path),
                path.to_path_buf(),
                PlaceIcon::Folder,
            )
        })
}

/// Home, cloud drive, and mounted volumes.
pub fn locations() -> Vec<Place> {
    let mut places = Vec::new();
    if let Some(home) = dirs::home_dir() {
        #[cfg(target_os = "macos")]
        {
            let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
            if icloud.is_dir() {
                places.push(Place::new("iCloud Drive", icloud, PlaceIcon::Cloud));
            }
        }
        places.push(Place::new(
            crate::entry::display_name(&home),
            home,
            PlaceIcon::Home,
        ));
    }
    places.extend(volumes());
    places
}

#[cfg(target_os = "macos")]
fn volumes() -> Vec<Place> {
    let mut out = Vec::new();
    let root_dev = device_id(Path::new("/"));
    let Ok(read) = std::fs::read_dir("/Volumes") else {
        return vec![Place::new("Macintosh HD", "/".into(), PlaceIcon::Drive)];
    };
    let mut boot_name = None;
    for entry in read.flatten() {
        let path = entry.path();
        let name = crate::entry::display_name(&path);
        // System volumes macOS mounts but Finder hides.
        const SYSTEM: [&str; 7] = [
            "Recovery",
            "Preboot",
            "VM",
            "Update",
            "xarts",
            "iSCPreboot",
            "Hardware",
        ];
        if name.starts_with('.') || SYSTEM.contains(&name.as_str()) {
            continue;
        }
        // The boot volume appears in /Volumes as a symlink to "/".
        if std::fs::read_link(&path).is_ok_and(|t| t == Path::new("/"))
            || (root_dev.is_some() && device_id(&path) == root_dev)
        {
            boot_name = Some(name);
            continue;
        }
        let mut place = Place::new(name, path, PlaceIcon::External);
        place.ejectable = true;
        out.push(place);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.insert(
        0,
        Place::new(
            boot_name.unwrap_or_else(|| "Macintosh HD".into()),
            "/".into(),
            PlaceIcon::Drive,
        ),
    );
    out
}

#[cfg(all(unix, not(target_os = "macos")))]
fn volumes() -> Vec<Place> {
    let mut out = vec![Place::new("Computer", "/".into(), PlaceIcon::Drive)];
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return out;
    };
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount)) = (fields.next(), fields.next()) else {
            continue;
        };
        let mount = mount.replace("\\040", " ");
        let user_mount = mount.starts_with("/media/")
            || mount.starts_with("/run/media/")
            || mount.starts_with("/mnt/");
        if !user_mount || !device.starts_with('/') {
            continue;
        }
        let path = PathBuf::from(&mount);
        let mut place = Place::new(crate::entry::display_name(&path), path, PlaceIcon::External);
        place.ejectable = true;
        out.push(place);
    }
    // Servers GIO mounted (Connect to Server, or a desktop's file manager).
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
        && let Ok(read) = std::fs::read_dir(PathBuf::from(runtime).join("gvfs"))
    {
        for entry in read.flatten() {
            let path = entry.path();
            let mut place = Place::new(crate::entry::display_name(&path), path, PlaceIcon::Network);
            place.ejectable = true;
            out.push(place);
        }
    }
    out
}

/// Space left for new files on the volume holding `path`, in bytes.
pub fn free_space(path: &Path) -> Option<u64> {
    fs4::available_space(path).ok()
}

/// What kind of drive a Windows drive root is.
#[cfg(windows)]
fn drive_type(root: &Path) -> u32 {
    let name = windows::core::HSTRING::from(root.as_os_str());
    // SAFETY: GetDriveTypeW only reads the path.
    unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(&name) }
}

/// Whether `path` is on a mapped network drive (Windows).
#[cfg(windows)]
pub fn is_network_drive(path: &Path) -> bool {
    let root: PathBuf = path.components().take(2).collect();
    drive_type(&root) == windows::Win32::System::WindowsProgramming::DRIVE_REMOTE
}

/// Drive letters, named by their volume labels as Explorer names them, with
/// removable, optical, and network drives ejectable.
#[cfg(windows)]
fn volumes() -> Vec<Place> {
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    use windows::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_REMOTE, DRIVE_REMOVABLE};
    let mut out = Vec::new();
    // A: and B: are skipped: probing an empty floppy drive can raise a system dialog.
    for letter in b'C'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", letter as char));
        if !root.exists() {
            continue;
        }
        let name = windows::core::HSTRING::from(root.as_os_str());
        let mut label = [0u16; 261];
        // SAFETY: the label buffer holds the longest volume name and its NUL.
        let read =
            unsafe { GetVolumeInformationW(&name, Some(&mut label), None, None, None, None) };
        let len = label.iter().position(|&c| c == 0).unwrap_or(0);
        let label = Some(String::from_utf16_lossy(&label[..len])).filter(|l| !l.trim().is_empty());
        let (icon, ejectable, kind) = match drive_type(&root) {
            DRIVE_REMOVABLE | DRIVE_CDROM => (PlaceIcon::External, true, "Removable Disk"),
            DRIVE_REMOTE => (PlaceIcon::Network, true, "Network Drive"),
            _ => (PlaceIcon::Drive, false, "Local Disk"),
        };
        let shown = read.ok().and(label).unwrap_or_else(|| kind.to_string());
        let mut place = Place::new(format!("{shown} ({}:)", letter as char), root, icon);
        place.ejectable = ejectable;
        out.push(place);
    }
    out
}

#[cfg(target_os = "macos")]
fn device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// Breadcrumb components from the volume root down to `path`.
pub fn breadcrumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut crumbs: Vec<(String, PathBuf)> = path
        .ancestors()
        .map(|p| (crate::entry::display_name(p), p.to_path_buf()))
        .collect();
    crumbs.reverse();
    #[cfg(target_os = "macos")]
    if let Some(first) = crumbs.first_mut()
        && first.1 == Path::new("/")
    {
        first.0 = volumes()
            .into_iter()
            .next()
            .map(|v| v.name)
            .unwrap_or_else(|| "Macintosh HD".into());
    }
    crumbs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumbs_root_first() {
        let home = dirs::home_dir().unwrap();
        let crumbs = breadcrumbs(&home);
        assert!(crumbs.len() >= 2);
        assert_eq!(crumbs.last().unwrap().1, home);
    }

    #[test]
    fn has_locations() {
        assert!(!locations().is_empty());
    }
}
