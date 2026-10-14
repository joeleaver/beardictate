//! Platforms without an injector yet (macOS, BSDs). Construction fails with
//! [`PlatformError::UnsupportedPlatform`]; nothing here panics.

use std::time::Duration;

use crate::config::{ForegroundApp, InjectorConfig, PasteChord, Settings};
use crate::driver::{Backend, Chord, Target, Typed};
use crate::error::PlatformError;
use crate::text::Token;

/// Uninhabited: [`Unsupported::new`] always fails, so no value exists and
/// the methods below can never run.
pub(crate) enum Unsupported {}

impl Unsupported {
    pub(crate) fn new(_config: &InjectorConfig) -> Result<Self, PlatformError> {
        Err(PlatformError::UnsupportedPlatform {
            os: std::env::consts::OS,
        })
    }

    pub(crate) fn name(&self) -> &'static str {
        match *self {}
    }

    pub(crate) fn prepare(&mut self, _config: &InjectorConfig) -> Result<(), PlatformError> {
        match *self {}
    }

    pub(crate) fn foreground_app(&self) -> Option<ForegroundApp> {
        match *self {}
    }
}

impl Backend for Unsupported {
    type Snapshot = ();

    fn target(&mut self) -> Result<Target, PlatformError> {
        match *self {}
    }
    fn wait_modifiers_released(&mut self, _: Duration) -> Result<bool, PlatformError> {
        match *self {}
    }
    fn type_tokens(&mut self, _: &[Token], _: &Settings) -> Result<Typed, PlatformError> {
        match *self {}
    }
    fn send_paste_chord(&mut self, _: PasteChord, _: &Settings) -> Result<Chord, PlatformError> {
        match *self {}
    }
    fn clipboard_snapshot(&mut self) -> Result<(), PlatformError> {
        match *self {}
    }
    fn clipboard_set(&mut self, _: &str, _: bool) -> Result<(), PlatformError> {
        match *self {}
    }
    fn clipboard_still_ours(&mut self, _: &str) -> Result<bool, PlatformError> {
        match *self {}
    }
    fn clipboard_restore(&mut self, _: ()) -> Result<(), PlatformError> {
        match *self {}
    }
}
