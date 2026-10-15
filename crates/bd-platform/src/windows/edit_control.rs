//! Direct insertion into standard Win32 text controls with `EM_REPLACESEL`.
//!
//! Windows 11 Notepad mangles synthetic keystrokes: its RichEdit control
//! (`RichEditD2DPT`) handles `SendInput` input late, so characters are
//! dropped or repeated and Shift goes missing, and its autocorrect rewrites
//! words while the next ones arrive ("Bryndle" -> "brindle"). That happens
//! with Unicode packets and with layout keys and scan codes alike, at any
//! speed tried, while `EM_REPLACESEL` put the same 295 characters in exactly
//! (NICO-TOP, 2026-10-15). It inserts the whole text at the caret in one
//! step, replacing any selection as typing would, undoable, without touching
//! the clipboard. `EM_REPLACESEL` is below `WM_USER`, so Windows copies the
//! string into the target process.

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ES_READONLY, GUITHREADINFO, GWL_STYLE, GetClassNameW, GetGUIThreadInfo,
    GetWindowLongPtrW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
};

use crate::text::Token;

/// From `Win32_UI_Controls`, a windows-sys feature not worth enabling for one
/// constant.
const EM_REPLACESEL: u32 = 0x00C2;

/// How long a hung target may take before we fall back to typing.
const TIMEOUT_MS: u32 = 2000;

/// A focused text control that takes `EM_REPLACESEL`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EditControl {
    hwnd: HWND,
    /// RichEdit ends a paragraph with `\r`; a plain Edit wants `\r\n`.
    rich: bool,
}

/// Which control has keyboard focus in the foreground window `top`, if it is
/// an editable Edit or RichEdit control.
pub(crate) fn focused(top: isize) -> Option<EditControl> {
    let top = top as HWND;
    // SAFETY: a null pid pointer is allowed.
    let thread = unsafe { GetWindowThreadProcessId(top, std::ptr::null_mut()) };
    if thread == 0 {
        return None;
    }
    // SAFETY: GUITHREADINFO is plain data; cbSize is set as the API requires.
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    // SAFETY: `info` is a valid, sized out-struct.
    if unsafe { GetGUIThreadInfo(thread, &mut info) } == 0 || info.hwndFocus.is_null() {
        return None;
    }
    let hwnd = info.hwndFocus;
    let mut buf = [0u16; 64];
    // SAFETY: `buf` holds `buf.len()` UTF-16 units.
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    let class = String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0)]);
    let rich = class.to_ascii_lowercase().starts_with("richedit");
    if !rich && !class.eq_ignore_ascii_case("Edit") {
        return None;
    }
    // SAFETY: plain call on a window handle.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    if style & ES_READONLY as u32 != 0 {
        return None;
    }
    tracing::debug!(%class, "inserting into the focused text control");
    Some(EditControl { hwnd, rich })
}

/// The text for `tokens` as the control stores it, or `None` when they hold
/// something only a key press can do (Backspace).
pub(crate) fn text_for(control: &EditControl, tokens: &[Token]) -> Option<Vec<u16>> {
    let newline: &[u16] = if control.rich { &[0x0D] } else { &[0x0D, 0x0A] };
    let mut out = Vec::with_capacity(tokens.len() + 1);
    for &token in tokens {
        match token {
            Token::Char(c) => {
                let mut buf = [0u16; 2];
                out.extend_from_slice(c.encode_utf16(&mut buf));
            }
            Token::Newline => out.extend_from_slice(newline),
            Token::Tab => out.push(0x09),
            Token::Backspace => return None,
        }
    }
    out.push(0);
    Some(out)
}

/// Inserts `text` (NUL-terminated UTF-16) at the caret. False if the
/// control didn't answer in time.
pub(crate) fn insert(control: &EditControl, text: &[u16]) -> bool {
    let mut result = 0usize;
    // SAFETY: `text` is NUL-terminated and outlives the call; wParam 1 makes
    // the insertion undoable.
    let answered = unsafe {
        SendMessageTimeoutW(
            control.hwnd,
            EM_REPLACESEL,
            1 as WPARAM,
            text.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            &mut result,
        )
    };
    answered != 0
}
