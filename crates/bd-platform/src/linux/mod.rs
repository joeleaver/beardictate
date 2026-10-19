//! Linux: Wayland (libei via the RemoteDesktop portal) or X11 (XTest),
//! picked from the session environment, with a shared arboard clipboard.

mod clipboard;
mod keysym;
pub(crate) mod wayland;
mod x11;
mod x11_slots;

use std::time::Duration;

use crate::config::{ForegroundApp, InjectorConfig, PasteChord, Settings};
use crate::driver::{Backend, Chord, Target, Typed};
use crate::error::PlatformError;
use crate::text::Token;

/// Which display protocol the session speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Session {
    Wayland,
    X11,
}

/// Wayland if `WAYLAND_DISPLAY` is set or `XDG_SESSION_TYPE=wayland`
/// (the portal works over D-Bus either way); otherwise X11 if `DISPLAY` is
/// set. XTest is never used on Wayland: through Xwayland it only reaches X11
/// apps.
pub(crate) fn detect_session(
    wayland_display: Option<&str>,
    session_type: Option<&str>,
    display: Option<&str>,
) -> Option<Session> {
    let set = |v: Option<&str>| v.is_some_and(|s| !s.is_empty());
    if set(wayland_display) || session_type == Some("wayland") {
        Some(Session::Wayland)
    } else if set(display) {
        Some(Session::X11)
    } else {
        None
    }
}

enum Display {
    Wayland(wayland::WaylandBackend),
    // Boxed: the X11 connection's buffers dwarf the Wayland handle.
    X11(Box<x11::X11Backend>),
}

pub(crate) struct LinuxBackend {
    display: Display,
    clipboard: clipboard::Clipboard,
}

impl LinuxBackend {
    pub(crate) fn new(config: &InjectorConfig) -> Result<Self, PlatformError> {
        let var = |k: &str| std::env::var(k).ok();
        let session = detect_session(
            var("WAYLAND_DISPLAY").as_deref(),
            var("XDG_SESSION_TYPE").as_deref(),
            var("DISPLAY").as_deref(),
        )
        .ok_or(PlatformError::NoSession)?;
        let display = match session {
            Session::Wayland => Display::Wayland(wayland::WaylandBackend::new(config)),
            Session::X11 => Display::X11(Box::new(x11::X11Backend::connect()?)),
        };
        Ok(Self {
            display,
            clipboard: clipboard::Clipboard::default(),
        })
    }

    /// Wayland backend with a custom libei source (the live test's nested
    /// KWin instead of the portal).
    #[cfg(test)]
    pub(crate) fn wayland_with_connector(
        config: &InjectorConfig,
        connector: wayland::Connector,
    ) -> Self {
        Self {
            display: Display::Wayland(wayland::WaylandBackend::with_connector(config, connector)),
            clipboard: clipboard::Clipboard::default(),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self.display {
            Display::Wayland(_) => "wayland",
            Display::X11(_) => "x11",
        }
    }

    pub(crate) fn prepare(&mut self, config: &InjectorConfig) -> Result<(), PlatformError> {
        match &mut self.display {
            Display::Wayland(w) => w.prepare(config),
            Display::X11(_) => Ok(()),
        }
    }

    pub(crate) fn foreground_app(&self) -> Option<ForegroundApp> {
        match &self.display {
            Display::Wayland(_) => None,
            Display::X11(x) => x.foreground_app(),
        }
    }
}

/// [`crate::FocusProbe`] on Linux: its own X11 connection, or nothing on
/// Wayland, where no client may know which app has the focus.
pub(crate) struct LinuxFocus(Option<Box<x11::X11Backend>>);

impl LinuxFocus {
    pub(crate) fn new() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        let session = detect_session(
            var("WAYLAND_DISPLAY").as_deref(),
            var("XDG_SESSION_TYPE").as_deref(),
            var("DISPLAY").as_deref(),
        );
        let x11 = (session == Some(Session::X11))
            .then(|| x11::X11Backend::connect().map_err(|e| tracing::warn!("focus probe: {e}")).ok())
            .flatten();
        Self(x11.map(Box::new))
    }

    pub(crate) fn current(&self) -> Option<ForegroundApp> {
        self.0.as_ref()?.foreground_app()
    }
}

impl Backend for LinuxBackend {
    type Snapshot = clipboard::Snapshot;

    fn target(&mut self) -> Result<Target, PlatformError> {
        match &self.display {
            Display::Wayland(_) => Ok(Target::Ready(None)),
            Display::X11(x) => x.target(),
        }
    }

    fn wait_modifiers_released(&mut self, timeout: Duration) -> Result<bool, PlatformError> {
        match &mut self.display {
            Display::Wayland(w) => w.wait_modifiers_released(timeout),
            Display::X11(x) => x.wait_modifiers_released(timeout),
        }
    }

    fn type_tokens(
        &mut self,
        tokens: &[Token],
        settings: &Settings,
    ) -> Result<Typed, PlatformError> {
        match &mut self.display {
            Display::Wayland(w) => w.type_tokens(tokens, settings),
            Display::X11(x) => x.type_tokens(tokens, settings),
        }
    }

    fn send_paste_chord(
        &mut self,
        chord: PasteChord,
        settings: &Settings,
    ) -> Result<Chord, PlatformError> {
        match &mut self.display {
            Display::Wayland(w) => w.send_paste_chord(chord),
            Display::X11(x) => x.send_paste_chord(chord, settings),
        }
    }

    fn clipboard_snapshot(&mut self) -> Result<Self::Snapshot, PlatformError> {
        self.clipboard.snapshot()
    }

    fn clipboard_set(&mut self, text: &str, transient: bool) -> Result<(), PlatformError> {
        self.clipboard.set_text(text, transient)
    }

    fn clipboard_still_ours(&mut self, text: &str) -> Result<bool, PlatformError> {
        self.clipboard.holds_text(text)
    }

    fn clipboard_restore(&mut self, snapshot: Self::Snapshot) -> Result<(), PlatformError> {
        self.clipboard.restore(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_detection() {
        assert_eq!(
            detect_session(Some("wayland-0"), None, Some(":0")),
            Some(Session::Wayland)
        );
        assert_eq!(
            detect_session(None, Some("wayland"), None),
            Some(Session::Wayland)
        );
        assert_eq!(
            detect_session(Some(""), Some("x11"), Some(":0")),
            Some(Session::X11)
        );
        assert_eq!(detect_session(None, Some("tty"), None), None);
    }
}
