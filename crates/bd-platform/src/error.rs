//! The crate's concrete error type, boxed into [`bd_core::BoxError`] at the
//! [`bd_core::Injector`] boundary.

/// Something went wrong delivering text.
///
/// "Could not deliver, so the text is on the clipboard" is *not* an error: it
/// is [`bd_core::InjectOutcome::LeftOnClipboard`]. These are the failures
/// where even that was impossible, or where the platform is not supported.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// No injector exists for this OS yet (macOS, BSDs, ...).
    #[error("text injection is not implemented on {os} yet")]
    UnsupportedPlatform { os: &'static str },
    /// Linux with neither `WAYLAND_DISPLAY`/`XDG_SESSION_TYPE=wayland` nor
    /// `DISPLAY`: there is no graphical session to type into.
    #[error("no graphical session found (WAYLAND_DISPLAY and DISPLAY are both unset)")]
    NoSession,
    /// Reading, writing or restoring the clipboard failed.
    #[error("clipboard: {0}")]
    Clipboard(String),
    /// A Win32 call failed; `code` is `GetLastError()`.
    #[error("{api} failed (Win32 error {code})")]
    Win32 { api: &'static str, code: u32 },
    /// The X server refused a request or the connection broke.
    #[error("X11: {0}")]
    X11(String),
    /// The xdg-desktop-portal RemoteDesktop session could not be set up.
    #[error("remote-desktop portal: {0}")]
    Portal(String),
    /// The libei connection failed or the compositor disconnected us.
    #[error("libei: {0}")]
    Ei(String),
}
