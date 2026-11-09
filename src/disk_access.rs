//! Full Disk Access on macOS: whether fffinder has it, and the System
//! Settings pane that turns it on. Without it, macOS keeps fffinder out of
//! the Trash, Mail, Safari, and other apps' data, so those folders show
//! nothing and searches miss what's inside them. macOS grants it to the app
//! that started the process: fffinder when it runs from its app bundle, or
//! the terminal it was started from.

use gpui_kit::Global;

/// System Settings › Privacy & Security › Full Disk Access.
pub const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// Whether this process may read everything on the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// A protected place could be read.
    Granted,
    /// macOS refused a read of a protected place.
    Denied,
    /// Nothing protected was there to try, or this isn't macOS.
    Unknown,
}

/// What [`check`] found when fffinder started. macOS only applies a change
/// to Full Disk Access when the app starts again, so this stays true for the
/// life of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked(pub Access);

impl Global for Checked {}

/// Whether indexing is waiting for Full Disk Access. Without it, indexing the
/// home folder makes macOS ask about Desktop, Documents, Downloads, and each
/// other protected folder in turn, every dialog taking the focus. So when
/// startup finds access off and asking is on, indexing waits until the user
/// turns access on (and reopens fffinder) or chooses to go ahead without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexHeld(pub bool);

impl Global for IndexHeld {}

/// Whether indexing should wait for Full Disk Access.
///
/// # Arguments
///
/// * `access` - What [`check`] found
/// * `ask` - The `ask_for_full_disk_access` setting
///
/// # Returns
///
/// `true` only when access is surely off and the user wants to be asked
pub fn holds_index(access: Access, ask: bool) -> bool {
    access == Access::Denied && ask
}

/// Whether indexing is waiting for Full Disk Access now.
///
/// # Arguments
///
/// * `cx` - The app, which holds the flag
pub fn index_held(cx: &gpui_kit::App) -> bool {
    cx.try_global::<IndexHeld>().is_some_and(|held| held.0)
}

/// Try reading places macOS guards with Full Disk Access. None of them makes
/// macOS ask the user, the way the Desktop or Documents folders would; a
/// missing permission just fails the read.
///
/// # Returns
///
/// What the reads say, or [`Access::Unknown`] on other systems
pub fn check() -> Access {
    #[cfg(target_os = "macos")]
    {
        let database = std::iter::once(std::path::PathBuf::from(
            "/Library/Application Support/com.apple.TCC/TCC.db",
        ))
        .map(|path| std::fs::File::open(path).map(drop));
        // Without a home folder, only the database tells.
        let folders = dirs::home_dir()
            .into_iter()
            .flat_map(|home| [home.join(".Trash"), home.join("Library/Safari")])
            .map(|path| std::fs::read_dir(path).map(drop));
        judge(database.chain(folders))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Access::Unknown
    }
}

/// Decide from the reads in `tries`, stopping at the first refusal. A place
/// that isn't there says nothing either way.
///
/// # Arguments
///
/// * `tries` - Each read's result, made only as it's needed
///
/// # Returns
///
/// [`Access::Denied`] if a read was refused, [`Access::Granted`] if one
/// worked, or [`Access::Unknown`]
#[cfg(any(target_os = "macos", test))]
fn judge(tries: impl IntoIterator<Item = std::io::Result<()>>) -> Access {
    let mut granted = false;
    for result in tries {
        match result {
            Ok(()) => granted = true,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                return Access::Denied;
            }
            Err(_) => {}
        }
    }
    if granted {
        Access::Granted
    } else {
        Access::Unknown
    }
}

/// Whether fffinder is running from its app bundle, which is then what
/// macOS grants permissions to, rather than the app that started it.
///
/// # Returns
///
/// `true` when the executable is inside a `.app` folder
pub fn in_app_bundle() -> bool {
    std::env::current_exe().is_ok_and(|exe| {
        exe.ancestors()
            .any(|dir| dir.extension().is_some_and(|ext| ext == "app"))
    })
}

#[cfg(test)]
mod tests {
    use super::{Access, judge};
    use std::io::{Error, ErrorKind};

    fn refused() -> std::io::Result<()> {
        Err(Error::from(ErrorKind::PermissionDenied))
    }

    fn missing() -> std::io::Result<()> {
        Err(Error::from(ErrorKind::NotFound))
    }

    #[test]
    fn a_refused_read_means_no_access() {
        assert_eq!(judge([Ok(()), refused()]), Access::Denied);
        assert_eq!(judge([missing(), refused(), Ok(())]), Access::Denied);
    }

    #[test]
    fn a_read_that_works_means_access() {
        assert_eq!(judge([missing(), Ok(())]), Access::Granted);
    }

    #[test]
    fn nothing_to_read_tells_nothing() {
        assert_eq!(judge([missing(), missing()]), Access::Unknown);
        assert_eq!(judge(std::iter::empty()), Access::Unknown);
    }

    #[test]
    fn indexing_waits_only_when_access_is_off_and_asking_is_on() {
        use super::holds_index;
        assert!(holds_index(Access::Denied, true));
        assert!(
            !holds_index(Access::Denied, false),
            "Don’t Ask Again indexes"
        );
        assert!(!holds_index(Access::Granted, true));
        assert!(
            !holds_index(Access::Unknown, true),
            "a picker, --term, or not macOS"
        );
    }

    #[test]
    fn reading_stops_at_the_first_refusal() {
        let mut made = 0;
        let tries = [refused(), Ok(())].into_iter().inspect(|_| made += 1);
        assert_eq!(judge(tries), Access::Denied);
        assert_eq!(made, 1);
    }
}
