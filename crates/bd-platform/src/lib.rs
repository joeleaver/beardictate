//! Delivers dictated text into whatever application has keyboard focus.
//!
//! [`PlatformInjector`] implements [`bd_core::Injector`] with two strategies
//! per OS, **type** (synthetic keystrokes) and **paste** (clipboard + paste
//! chord, then the previous clipboard is put back), chosen by [`Strategy`]
//! and an optional per-app [`AppOverride`]:
//!
//! | Platform | Type | Paste |
//! |---|---|---|
//! | Windows | `SendInput` + `KEYEVENTF_UNICODE` | Win32 clipboard (all memory formats) + `SendInput` chord |
//! | Linux X11 | XTest, characters bound to spare keycodes | arboard (X11) + XTest chord |
//! | Linux Wayland | libei via the RemoteDesktop portal, keymap reverse-lookup (or `ei_text` when offered) | arboard (`wlr`/`ext` data-control) + libei chord |
//! | macOS, others | [`PlatformError::UnsupportedPlatform`] | |
//!
//! When the text can't be delivered (an elevated window on Windows, a held
//! modifier, a refused portal), it is left on the clipboard and the outcome
//! is [`InjectOutcome::LeftOnClipboard`] with a reason for the user.
//!
//! Hotkeys, the tray and notifications are rinch's job, not this crate's.

mod config;
// On platforms without a backend only the fallible constructor is reachable.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod driver;
mod error;
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod text;
#[cfg_attr(not(windows), allow(dead_code))]
mod win_keys;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(windows, target_os = "linux")))]
mod unsupported;
#[cfg(windows)]
mod windows;

use bd_core::{BoxError, InjectOutcome, Injector};

pub use config::{
    AppOverride, AppPolicy, ForegroundApp, InjectorConfig, NewlineKey, PasteChord,
    RestoreTokenSink, Strategy,
};
pub use error::PlatformError;
/// Value of `KEYBDINPUT::dwExtraInfo` on every event we inject, so a
/// low-level keyboard hook can tell our keystrokes from the user's.
#[cfg(windows)]
pub use windows::INJECTED_MARKER;

#[cfg(target_os = "linux")]
use linux::LinuxBackend as Imp;
#[cfg(not(any(windows, target_os = "linux")))]
use unsupported::Unsupported as Imp;
#[cfg(windows)]
use windows::WindowsBackend as Imp;

/// The text injector for the OS this was built for.
pub struct PlatformInjector {
    config: InjectorConfig,
    backend: Imp,
}

impl PlatformInjector {
    /// Picks the backend for this OS (on Linux: Wayland or X11 from the
    /// session environment). Cheap: no permission prompt happens here; see
    /// [`PlatformInjector::prepare`].
    pub fn new(config: InjectorConfig) -> Result<Self, PlatformError> {
        let backend = Imp::new(&config)?;
        tracing::info!(backend = backend.name(), "text injector ready");
        Ok(Self { config, backend })
    }

    /// `"windows"`, `"x11"` or `"wayland"`.
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    pub fn config(&self) -> &InjectorConfig {
        &self.config
    }

    /// Applies new settings (e.g. after the settings window changed them).
    pub fn set_config(&mut self, config: InjectorConfig) {
        self.config = config;
    }

    /// Does any one-time setup that may need the user: on Wayland this asks
    /// the RemoteDesktop portal for keyboard access, which shows a
    /// permission dialog the first time. Call it from first-run onboarding
    /// so the dialog doesn't interrupt the first dictation. A no-op on
    /// Windows and X11. Calling it again after a refusal asks again.
    pub fn prepare(&mut self) -> Result<(), PlatformError> {
        self.backend.prepare(&self.config)
    }

    /// The app with keyboard focus, if the platform can tell (not on Wayland).
    pub fn foreground_app(&self) -> Option<ForegroundApp> {
        self.backend.foreground_app()
    }

    /// Like [`Injector::inject`] but with the concrete error type.
    pub fn inject_text(&mut self, text: &str) -> Result<InjectOutcome, PlatformError> {
        driver::inject(&mut self.backend, &self.config, text)
    }
}

impl Injector for PlatformInjector {
    fn prepare(&mut self) -> Result<(), BoxError> {
        Ok(PlatformInjector::prepare(self)?)
    }

    fn inject(&mut self, text: &str) -> Result<InjectOutcome, BoxError> {
        Ok(self.inject_text(text)?)
    }
}
