//! Raw Win32 clipboard: save every format we safely can, set text, restore.
//!
//! What a snapshot keeps: every format whose data is an `HGLOBAL` memory
//! block. That covers text (all encodings), RTF, HTML, `CF_DIB`/`CF_DIBV5`
//! images, file lists (`CF_HDROP`) and the registered formats Office and
//! browsers use. It does **not** keep:
//! - GDI-handle formats (`CF_BITMAP`, `CF_ENHMETAFILE`, `CF_METAFILEPICT`,
//!   `CF_PALETTE`). Windows synthesizes `CF_DIB` from a bitmap and that is
//!   kept, so images survive; vector metafiles don't.
//! - Owner-display and private (`CF_PRIVATEFIRST..`, `CF_GDIOBJFIRST..`)
//!   formats, whose handles mean nothing outside the owning process.
//! - Delayed-rendered data stays correct (reading it forces rendering), but
//!   a huge selection (an Excel sheet) is copied; formats past
//!   [`SNAPSHOT_BUDGET`] bytes are dropped with a warning.
//!
//! Clipboard history (Win+V) and cloud clipboard are told to ignore our
//! transient text via the documented registered formats.

use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HANDLE, HGLOBAL};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Ole::{
    CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_ENHMETAFILE,
    CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_UNICODETEXT,
};

use crate::error::PlatformError;

/// Most bytes a snapshot holds before further formats are dropped.
pub(crate) const SNAPSHOT_BUDGET: usize = 128 * 1024 * 1024;

const CF_PRIVATEFIRST: u32 = 0x0200;
const CF_GDIOBJLAST: u32 = 0x03FF;

/// Saved clipboard content: `(format, bytes)` in enumeration order.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    formats: Vec<(u32, Vec<u8>)>,
}

/// The clipboard, opened; closed on drop.
struct Open;

impl Open {
    /// Another process may hold the clipboard briefly; retry for ~0.5 s.
    fn new() -> Result<Self, PlatformError> {
        for attempt in 0..50 {
            // SAFETY: a null owner is allowed; see module docs of arboard /
            // clipboard-win which do the same. We never use delayed
            // rendering, so no owner window is needed to answer messages.
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(Open);
            }
            if attempt < 49 {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Err(last_error("OpenClipboard"))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: we opened it.
        unsafe { CloseClipboard() };
    }
}

fn last_error(api: &'static str) -> PlatformError {
    // SAFETY: plain call.
    PlatformError::Win32 {
        api,
        code: unsafe { GetLastError() },
    }
}

fn skippable(format: u32) -> bool {
    const GDI_OR_OWNER: [u16; 8] = [
        CF_BITMAP,
        CF_METAFILEPICT,
        CF_PALETTE,
        CF_ENHMETAFILE,
        CF_OWNERDISPLAY,
        CF_DSPBITMAP,
        CF_DSPMETAFILEPICT,
        CF_DSPENHMETAFILE,
    ];
    GDI_OR_OWNER.iter().any(|&f| u32::from(f) == format)
        || (CF_PRIVATEFIRST..=CF_GDIOBJLAST).contains(&format)
}

/// Copies an HGLOBAL's bytes.
fn read_global(handle: HANDLE) -> Option<Vec<u8>> {
    // SAFETY: `handle` is an HGLOBAL owned by the clipboard, valid while it
    // is open; we only read `size` bytes from the locked pointer.
    unsafe {
        let size = GlobalSize(handle as HGLOBAL);
        if size == 0 {
            return None;
        }
        let ptr = GlobalLock(handle as HGLOBAL) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        GlobalUnlock(handle as HGLOBAL);
        Some(bytes)
    }
}

/// Puts `bytes` on the (open, emptied) clipboard as `format`.
fn write_global(format: u32, bytes: &[u8]) -> Result<(), PlatformError> {
    // SAFETY: we allocate, fill and hand over a movable block; ownership
    // passes to the system on SetClipboardData success, otherwise we free it.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if mem.is_null() {
            return Err(last_error("GlobalAlloc"));
        }
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            GlobalFree(mem);
            return Err(last_error("GlobalLock"));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        GlobalUnlock(mem);
        if SetClipboardData(format, mem as HANDLE).is_null() {
            let err = last_error("SetClipboardData");
            GlobalFree(mem);
            return Err(err);
        }
    }
    Ok(())
}

/// Registered formats that keep transient content out of Win+V history,
/// cloud clipboard and well-behaved clipboard managers.
fn exclusion_formats() -> &'static [u32] {
    static IDS: OnceLock<Vec<u32>> = OnceLock::new();
    IDS.get_or_init(|| {
        [
            "ExcludeClipboardContentFromMonitorProcessing",
            "CanIncludeInClipboardHistory",
            "CanUploadToCloudClipboard",
        ]
        .iter()
        .filter_map(|name| {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: NUL-terminated UTF-16 string.
            let id = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
            (id != 0).then_some(id)
        })
        .collect()
    })
}

fn mark_transient() -> Result<(), PlatformError> {
    // A DWORD 0 = "no" for the two Can* formats; any data works for Exclude.
    for &id in exclusion_formats() {
        write_global(id, &0u32.to_ne_bytes())?;
    }
    Ok(())
}

/// Saves the current clipboard content.
pub(crate) fn snapshot() -> Result<Snapshot, PlatformError> {
    let _open = Open::new()?;
    let mut snap = Snapshot::default();
    let mut used = 0usize;
    let mut format = 0u32;
    loop {
        // SAFETY: clipboard is open; 0 starts the enumeration.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if skippable(format) || exclusion_formats().contains(&format) {
            continue;
        }
        // SAFETY: clipboard is open.
        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            continue;
        }
        let Some(bytes) = read_global(handle) else {
            continue;
        };
        if used + bytes.len() > SNAPSHOT_BUDGET {
            tracing::warn!(
                format,
                size = bytes.len(),
                "clipboard format too large to save; dropped"
            );
            continue;
        }
        used += bytes.len();
        snap.formats.push((format, bytes));
    }
    Ok(snap)
}

/// Replaces the clipboard with `text`; returns the new sequence number.
pub(crate) fn set_text(text: &str, transient: bool) -> Result<u32, PlatformError> {
    let open = Open::new()?;
    // SAFETY: clipboard is open.
    if unsafe { EmptyClipboard() } == 0 {
        return Err(last_error("EmptyClipboard"));
    }
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_ne_bytes)
        .collect();
    write_global(u32::from(CF_UNICODETEXT), &bytes)?;
    if transient {
        mark_transient()?;
    }
    drop(open);
    Ok(sequence())
}

/// Puts a snapshot back. Restored content is marked transient too: it is
/// already in the user's history, and re-adding it would duplicate it.
pub(crate) fn restore(snapshot: &Snapshot) -> Result<(), PlatformError> {
    let _open = Open::new()?;
    // SAFETY: clipboard is open.
    if unsafe { EmptyClipboard() } == 0 {
        return Err(last_error("EmptyClipboard"));
    }
    for (format, bytes) in &snapshot.formats {
        if let Err(e) = write_global(*format, bytes) {
            tracing::warn!(format, error = %e, "could not restore clipboard format");
        }
    }
    if !snapshot.formats.is_empty() {
        mark_transient()?;
    }
    Ok(())
}

/// Increments every time anyone changes the clipboard.
pub(crate) fn sequence() -> u32 {
    // SAFETY: plain call.
    unsafe { GetClipboardSequenceNumber() }
}
