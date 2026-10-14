//! Windows backend: `SendInput` with `KEYEVENTF_UNICODE`, the Win32
//! clipboard, and UIPI detection.

mod clipboard;
mod process;
mod sendinput;

use std::time::Duration;

use crate::config::{ForegroundApp, InjectorConfig, PasteChord, Settings};
use crate::driver::{Backend, Chord, Target, Typed};
use crate::error::PlatformError;
use crate::text::Token;
use crate::win_keys::paste_events;

pub use sendinput::INJECTED_MARKER;

pub(crate) struct WindowsBackend {
    own_integrity: Option<u32>,
    /// Foreground window at the start of the current injection.
    hwnd: isize,
    /// Clipboard sequence number right after our last `clipboard_set`.
    our_sequence: Option<u32>,
}

impl WindowsBackend {
    pub(crate) fn new(_config: &InjectorConfig) -> Result<Self, PlatformError> {
        let own_integrity = match process::own_integrity() {
            Ok(rid) => Some(rid),
            Err(e) => {
                tracing::warn!(error = %e, "could not read our own integrity level; UIPI checks disabled");
                None
            }
        };
        Ok(Self {
            own_integrity,
            hwnd: 0,
            our_sequence: None,
        })
    }

    pub(crate) fn name(&self) -> &'static str {
        "windows"
    }

    /// Nothing to set up on Windows: `SendInput` needs no permission.
    pub(crate) fn prepare(&mut self, _config: &InjectorConfig) -> Result<(), PlatformError> {
        Ok(())
    }

    pub(crate) fn foreground_app(&self) -> Option<ForegroundApp> {
        let fg = process::foreground()?;
        Some(ForegroundApp {
            exe_name: fg.exe_name?,
            pid: fg.pid,
        })
    }
}

impl Backend for WindowsBackend {
    type Snapshot = clipboard::Snapshot;

    fn target(&mut self) -> Result<Target, PlatformError> {
        let Some(fg) = process::foreground() else {
            return Ok(Target::Blocked {
                reason: "no window has keyboard focus (a UAC prompt or the lock screen may be up)"
                    .into(),
            });
        };
        self.hwnd = fg.hwnd;
        let name = fg
            .exe_name
            .clone()
            .unwrap_or_else(|| format!("process {}", fg.pid));
        match (fg.integrity, self.own_integrity) {
            (Some(theirs), Some(ours)) if theirs > ours => {
                return Ok(Target::Blocked {
                    reason: format!(
                        "{name} runs with {} rights and Windows blocks typing into it from a \
                         normal app; paste it there yourself, or run beardictate as \
                         administrator",
                        process::integrity_name(theirs)
                    ),
                });
            }
            (None, Some(_)) => {
                // Usually a protected process. Typing may be dropped silently,
                // but guessing "blocked" would refuse many working targets.
                tracing::warn!(app = %name, "could not read the focused app's integrity level");
            }
            _ => {}
        }
        Ok(Target::Ready(fg.exe_name.map(|exe_name| ForegroundApp {
            exe_name,
            pid: fg.pid,
        })))
    }

    fn wait_modifiers_released(&mut self, timeout: Duration) -> Result<bool, PlatformError> {
        Ok(sendinput::wait_modifiers_released(timeout))
    }

    fn type_tokens(
        &mut self,
        tokens: &[Token],
        settings: &Settings,
    ) -> Result<Typed, PlatformError> {
        Ok(sendinput::type_tokens(tokens, settings, self.hwnd))
    }

    fn send_paste_chord(
        &mut self,
        chord: PasteChord,
        _settings: &Settings,
    ) -> Result<Chord, PlatformError> {
        let events = paste_events(chord);
        let sent = sendinput::send(&events);
        if sent == events.len() {
            return Ok(Chord::Sent);
        }
        // Release whatever went down so no modifier sticks.
        let (_, owed) = crate::win_keys::account_partial(std::slice::from_ref(&events), sent);
        sendinput::send(&owed);
        Ok(Chord::Impossible {
            reason: "Windows refused the synthetic paste keystroke".into(),
        })
    }

    fn clipboard_snapshot(&mut self) -> Result<Self::Snapshot, PlatformError> {
        clipboard::snapshot()
    }

    fn clipboard_set(&mut self, text: &str, transient: bool) -> Result<(), PlatformError> {
        self.our_sequence = Some(clipboard::set_text(text, transient)?);
        Ok(())
    }

    fn clipboard_still_ours(&mut self, _text: &str) -> Result<bool, PlatformError> {
        Ok(self.our_sequence == Some(clipboard::sequence()))
    }

    fn clipboard_restore(&mut self, snapshot: Self::Snapshot) -> Result<(), PlatformError> {
        clipboard::restore(&snapshot)?;
        self.our_sequence = None;
        Ok(())
    }
}
