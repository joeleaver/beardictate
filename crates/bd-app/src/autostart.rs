//! Starting at login on Windows: a value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which needs no
//! admin rights and shows in Task Manager's Startup apps, where the user can
//! also turn it off. (Linux uses an XDG autostart entry; see
//! [`crate::settings::Paths::set_start_at_login`].)

use std::io;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "beardictate";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn check(status: u32) -> io::Result<()> {
    if status == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(status as i32)) }
}

pub fn enabled() -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    // SAFETY: NUL-terminated strings; null out-pointers only ask whether the
    // value exists.
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
    };
    status == ERROR_SUCCESS
}

/// Adds or removes the Run value, pointing at this executable.
pub fn set(on: bool) -> io::Result<()> {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    if !on {
        // SAFETY: NUL-terminated strings.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        return if status == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(status) };
    }
    let exe = std::env::current_exe()?;
    let command = wide(&format!("\"{}\"", exe.display()));
    // SAFETY: `command` is a NUL-terminated REG_SZ of the stated byte length.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * 2) as u32,
        )
    };
    check(status)
}
