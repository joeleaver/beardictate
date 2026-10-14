//! `SendInput` plumbing: typing token groups in bursts and sending chords.

use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VK_CONTROL,
    VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use super::process;
use crate::config::Settings;
use crate::driver::Typed;
use crate::text::Token;
use crate::win_keys::{KeyEvent, account_partial, bursts, token_events};

/// Marks our events in `dwExtraInfo` so a low-level keyboard hook (rinch's
/// hold-to-talk hook) can recognize and ignore them. ASCII "BDIC".
pub const INJECTED_MARKER: usize = 0x4244_4943;

fn to_input(ev: &KeyEvent) -> INPUT {
    let ki = match *ev {
        KeyEvent::Unicode { unit, up } => KEYBDINPUT {
            wVk: 0,
            wScan: unit,
            dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: INJECTED_MARKER,
        },
        KeyEvent::Vk { vk, extended, up } => KEYBDINPUT {
            wVk: vk,
            // SAFETY: plain call. Real scan codes make games/RDP/VMs that read
            // scan codes rather than virtual keys see the key too.
            wScan: unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16,
            dwFlags: (if up { KEYEVENTF_KEYUP } else { 0 })
                | (if extended { KEYEVENTF_EXTENDEDKEY } else { 0 }),
            time: 0,
            dwExtraInfo: INJECTED_MARKER,
        },
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki },
    }
}

/// Sends `events` in one `SendInput` call; returns how many were inserted.
pub(crate) fn send(events: &[KeyEvent]) -> usize {
    if events.is_empty() {
        return 0;
    }
    let inputs: Vec<INPUT> = events.iter().map(to_input).collect();
    // SAFETY: `inputs` is a valid array of `len` INPUT structs.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    } as usize;
    if sent < inputs.len() {
        // SAFETY: plain call.
        let code = unsafe { GetLastError() };
        tracing::warn!(
            sent,
            total = inputs.len(),
            code,
            "SendInput inserted fewer events"
        );
    }
    sent
}

/// Types `tokens` in bursts. Stops (reporting what was delivered) if input
/// is refused or focus moves to another window mid-way, so the rest doesn't
/// land somewhere the user didn't intend.
pub(crate) fn type_tokens(tokens: &[Token], settings: &Settings, hwnd: isize) -> Typed {
    let groups: Vec<Vec<KeyEvent>> = tokens
        .iter()
        .map(|&t| token_events(t, settings.newline))
        .collect();
    let chunk = settings.batch_chars;
    for (i, (first, events)) in bursts(&groups, chunk).into_iter().enumerate() {
        if i > 0 {
            thread::sleep(settings.batch_delay);
            if process::foreground_hwnd() != hwnd {
                return Typed::Partial {
                    delivered: first,
                    reason: "keyboard focus moved to another window".into(),
                };
            }
        }
        let sent = send(&events);
        if sent < events.len() {
            let burst_groups = &groups[first..(first + chunk).min(groups.len())];
            let (done, owed) = account_partial(burst_groups, sent);
            if !owed.is_empty() {
                send(&owed);
            }
            let delivered = first + done;
            let reason = "Windows refused the synthetic input (another app may be blocking \
                          input, or the secure desktop is active)";
            return if delivered == 0 {
                Typed::Impossible {
                    reason: reason.into(),
                }
            } else {
                Typed::Partial {
                    delivered,
                    reason: reason.into(),
                }
            };
        }
    }
    Typed::Done
}

/// Whether any of Shift/Ctrl/Alt/Win is physically down right now.
fn modifiers_down() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        // SAFETY: plain call. The high bit is "currently down".
        .any(|&vk| unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000 != 0)
}

/// Polls until no modifier is held or `timeout` passes.
pub(crate) fn wait_modifiers_released(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !modifiers_down() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}
