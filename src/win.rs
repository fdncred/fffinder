//! Windows integration through the Win32 API: putting files on the
//! clipboard the way Explorer does, the apps registered for a file type
//! (Explorer's Open with list) opened through the shell, and shortcuts.

use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IDataObject, IPersistFile,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::DROPFILES;
use windows::Win32::UI::Shell::{
    ASSOC_FILTER_RECOMMENDED, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR, ASSOCSTR_EXECUTABLE,
    ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW, BHID_DataObject, IAssocHandler, IShellItem,
    IShellLinkW, SHAssocEnumHandlers, SHCreateItemFromParsingName, ShellLink,
};
use windows::core::{HSTRING, Interface as _, PCWSTR, PWSTR};

/// `bytes` in movable global memory, which the clipboard takes over.
fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    // SAFETY: the block is allocated with room for `bytes`, locked while
    // they're copied in, and unlocked before it's handed on.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let at = GlobalLock(memory).cast::<u8>();
        if at.is_null() {
            let _ = GlobalFree(Some(memory));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len());
        let _ = GlobalUnlock(memory);
        Some(memory)
    }
}

/// Put `paths` on the clipboard as files (`CF_HDROP`, which Explorer
/// pastes) and `text` as text. Returns whether that worked.
pub fn write_file_clipboard(paths: &[PathBuf], text: &str) -> bool {
    // A DROPFILES header, then each path in UTF-16 ending in a NUL, then
    // one more NUL.
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        fWide: true.into(),
        ..Default::default()
    };
    // SAFETY: DROPFILES is plain data, read as its own bytes.
    let mut files: Vec<u8> = unsafe {
        std::slice::from_raw_parts(
            (&raw const header).cast::<u8>(),
            std::mem::size_of::<DROPFILES>(),
        )
    }
    .to_vec();
    for path in paths {
        let wide = path.as_os_str().encode_wide().chain([0]);
        files.extend(wide.flat_map(u16::to_le_bytes));
    }
    files.extend([0, 0]);
    let text: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: the clipboard is opened and closed here; each memory block
    // belongs to the clipboard once SetClipboardData takes it, and is freed
    // here when it doesn't.
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let ok = EmptyClipboard().is_ok()
            && [(CF_HDROP, files), (CF_UNICODETEXT, text)]
                .iter()
                .all(|(format, data)| {
                    let Some(memory) = global(data) else {
                        return false;
                    };
                    let set = SetClipboardData(u32::from(format.0), Some(HANDLE(memory.0))).is_ok();
                    if !set {
                        let _ = GlobalFree(Some(memory));
                    }
                    set
                });
        let _ = CloseClipboard();
        ok
    }
}

/// `.ext` for `path`, how Windows names a file type.
fn extension(path: &Path) -> Option<HSTRING> {
    let ext = path.extension()?;
    let mut dotted = std::ffi::OsString::from(".");
    dotted.push(ext);
    Some(HSTRING::from(dotted.as_os_str()))
}

/// One string about the app registered for a file type.
fn assoc_query(what: ASSOCSTR, ext: &HSTRING) -> Option<String> {
    let mut len = 0u32;
    // SAFETY: the first call only reports the length; the buffer for the
    // second holds that many UTF-16 units, the NUL included.
    unsafe {
        let _ = AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            what,
            ext,
            PCWSTR::null(),
            None,
            &mut len,
        );
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            what,
            ext,
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
        let text = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
        (!text.is_empty()).then_some(text)
    }
}

/// The app Windows opens `path` with: its name, and its program.
pub fn default_app(path: &Path) -> Option<(String, PathBuf)> {
    let ext = extension(path)?;
    let name = assoc_query(ASSOCSTR_FRIENDLYAPPNAME, &ext)?;
    let program = assoc_query(ASSOCSTR_EXECUTABLE, &ext).map(PathBuf::from)?;
    Some((name, program))
}

/// Start COM on this thread for the shell's objects; a thread that already
/// has it keeps what it has.
fn com() {
    // SAFETY: called before any COM object is made on this thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
}

/// A string the shell allocated, copied and freed.
fn take(text: windows::core::Result<PWSTR>) -> Option<String> {
    let text = text.ok()?;
    // SAFETY: the shell returned a NUL-terminated string from the COM
    // allocator, which is freed once copied.
    unsafe {
        let copy = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast_const().cast()));
        copy
    }
}

/// Explorer's Open with list for `path`'s type, in its order, with each
/// app's name and program.
fn handlers(path: &Path) -> Vec<(IAssocHandler, String, PathBuf)> {
    let Some(ext) = extension(path) else {
        return Vec::new();
    };
    com();
    let mut found = Vec::new();
    // SAFETY: the enumerator hands out one handler at a time, owned by us.
    unsafe {
        let Ok(list) = SHAssocEnumHandlers(&ext, ASSOC_FILTER_RECOMMENDED) else {
            return found;
        };
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            if list.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(handler) = slot[0].take() else {
                break;
            };
            if let (Some(name), Some(program)) =
                (take(handler.GetUIName()), take(handler.GetName()))
            {
                found.push((handler, name, PathBuf::from(program)));
            }
        }
    }
    found
}

/// The apps that can open `path`: name, program, and whether it's the default.
pub fn apps_for(path: &Path) -> Vec<(String, PathBuf, bool)> {
    let default = default_app(path).map(|(_, program)| program);
    handlers(path)
        .into_iter()
        .map(|(_, name, program)| {
            let is_default = default
                .as_ref()
                .is_some_and(|d| d.as_os_str().eq_ignore_ascii_case(program.as_os_str()));
            (name, program, is_default)
        })
        .collect()
}

/// Open `files` with the app whose program is `program`, through the shell,
/// so Store apps open too; a program not in the list is run directly.
pub fn open_with(program: &Path, files: &[PathBuf]) -> Result<(), String> {
    let Some(first) = files.first() else {
        return Ok(());
    };
    let handler = handlers(first)
        .into_iter()
        .find(|(_, _, p)| p.as_os_str().eq_ignore_ascii_case(program.as_os_str()))
        .map(|(handler, _, _)| handler);
    let Some(handler) = handler else {
        return std::process::Command::new(program)
            .args(files)
            .spawn()
            .map(drop)
            .map_err(|e| e.to_string());
    };
    for file in files {
        let name = HSTRING::from(file.as_os_str());
        // SAFETY: the item and its data object live until Invoke returns.
        unsafe {
            let item: IShellItem =
                SHCreateItemFromParsingName(&name, None).map_err(|e| e.message())?;
            let data: IDataObject = item
                .BindToHandler(None, &BHID_DataObject)
                .map_err(|e| e.message())?;
            handler.Invoke(&data).map_err(|e| e.message())?;
        }
    }
    Ok(())
}

/// An Explorer shortcut (`.lnk`) at `link` that opens `target`.
pub fn make_shortcut(target: &Path, link: &Path) -> Result<(), String> {
    com();
    // SAFETY: the shell link object lives until it's saved.
    unsafe {
        let shortcut: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        shortcut
            .SetPath(&HSTRING::from(target.as_os_str()))
            .map_err(|e| e.message())?;
        if let Some(dir) = target.parent() {
            let _ = shortcut.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()));
        }
        let file: IPersistFile = shortcut.cast().map_err(|e| e.message())?;
        file.Save(&HSTRING::from(link.as_os_str()), true)
            .map_err(|e| e.message())
    }
}
