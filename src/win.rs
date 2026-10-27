//! Windows integration through the Win32 API: putting files on the
//! clipboard the way Explorer does, and the apps registered for a file type.

use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::DROPFILES;
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR, ASSOCSTR_EXECUTABLE, ASSOCSTR_FRIENDLYAPPNAME,
    AssocQueryStringW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

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
