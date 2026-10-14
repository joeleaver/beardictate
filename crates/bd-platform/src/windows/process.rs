//! Who has focus, and whether UIPI would drop our input to it.
//!
//! User Interface Privilege Isolation silently discards `SendInput` events
//! aimed at a window whose process runs at a higher integrity level than
//! ours (an elevated "Run as administrator" app, Task Manager, regedit).
//! `SendInput` still reports success, so the only way to know is to compare
//! integrity levels up front.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::error::PlatformError;

/// The foreground window and what we could learn about its process.
#[derive(Debug, Clone)]
pub(crate) struct Foreground {
    /// The `HWND` as an integer (raw pointers aren't `Send`); only compared.
    pub hwnd: isize,
    pub pid: u32,
    pub exe_name: Option<String>,
    /// Mandatory-label RID (0x2000 medium, 0x3000 high, 0x4000 system), or
    /// `None` if the process could not be queried.
    pub integrity: Option<u32>,
}

/// The current foreground window handle as an integer (0 = none).
pub(crate) fn foreground_hwnd() -> isize {
    // SAFETY: no arguments; returns a handle or null.
    unsafe { GetForegroundWindow() as isize }
}

/// Describes the foreground window, or `None` when no window has focus
/// (e.g. the secure desktop of a UAC prompt or the lock screen is up).
pub(crate) fn foreground() -> Option<Foreground> {
    // SAFETY: no arguments.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `hwnd` came from GetForegroundWindow; `pid` is a valid out-pointer.
    let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if tid == 0 || pid == 0 {
        return None;
    }
    let process = open_process(pid);
    let exe_name = process.as_ref().and_then(image_name);
    let integrity = match &process {
        Some(p) => integrity_of(p.as_raw_handle() as HANDLE).ok(),
        None => None,
    };
    Some(Foreground {
        hwnd: hwnd as isize,
        pid,
        exe_name,
        integrity,
    })
}

/// Our own integrity level RID.
pub(crate) fn own_integrity() -> Result<u32, PlatformError> {
    // SAFETY: the pseudo-handle from GetCurrentProcess needs no closing.
    integrity_of(unsafe { GetCurrentProcess() })
}

fn open_process(pid: u32) -> Option<OwnedHandle> {
    // SAFETY: plain call; a null return means failure.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        tracing::debug!(pid, code = unsafe { GetLastError() }, "OpenProcess failed");
        return None;
    }
    // SAFETY: `h` is a fresh, owned process handle.
    Some(unsafe { OwnedHandle::from_raw_handle(h) })
}

fn image_name(process: &OwnedHandle) -> Option<String> {
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: buffer and length describe valid writable memory.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle() as HANDLE,
            PROCESS_NAME_WIN32,
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    let path = OsString::from_wide(&buf[..len as usize]);
    Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

fn integrity_of(process: HANDLE) -> Result<u32, PlatformError> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` is a valid process handle; `token` is an out-pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error("OpenProcessToken"));
    }
    // SAFETY: `token` is a fresh, owned token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let raw = token.as_raw_handle() as HANDLE;

    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; fails with ERROR_INSUFFICIENT_BUFFER.
    unsafe {
        GetTokenInformation(
            raw,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(last_error("GetTokenInformation"));
    }
    // u64 storage keeps the TOKEN_MANDATORY_LABEL suitably aligned.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `needed` bytes.
    let ok = unsafe {
        GetTokenInformation(
            raw,
            TokenIntegrityLevel,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(last_error("GetTokenInformation"));
    }
    // SAFETY: on success the buffer starts with a TOKEN_MANDATORY_LABEL whose
    // SID points into the same buffer, which outlives these reads.
    unsafe {
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err(PlatformError::Win32 {
                api: "GetSidSubAuthorityCount",
                code: 0,
            });
        }
        Ok(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

fn last_error(api: &'static str) -> PlatformError {
    // SAFETY: plain call.
    PlatformError::Win32 {
        api,
        code: unsafe { GetLastError() },
    }
}

/// Human name for an integrity RID, for the reason shown to the user.
pub(crate) fn integrity_name(rid: u32) -> &'static str {
    match rid {
        r if r >= 0x4000 => "system",
        r if r >= 0x3000 => "high (administrator)",
        r if r >= 0x2000 => "medium",
        r if r >= 0x1000 => "low",
        _ => "untrusted",
    }
}
