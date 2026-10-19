//! Alpha telemetry: what happened in each dictation, sent to the telemetry
//! server for debugging testers' problems and training the next cleanup
//! model. The format and the rules are in `docs/telemetry.md`.
//!
//! Only with the `alpha` feature. Without it nothing is recorded, and a
//! `telemetry` folder an alpha build left behind is deleted at start.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::settings::Paths;

#[cfg(feature = "alpha")]
mod logs;
#[cfg(feature = "alpha")]
mod spool;
#[cfg(feature = "alpha")]
mod system;
#[cfg(feature = "alpha")]
mod upload;

#[cfg(feature = "alpha")]
pub use logs::Layer;

/// Where events and audio wait to be sent.
pub fn dir(paths: &Paths) -> PathBuf {
    paths.log.parent().map_or_else(|| PathBuf::from("telemetry"), |d| d.join("telemetry"))
}

/// Starts recording and sending (alpha), or removes what an alpha build
/// left behind (release).
pub fn start(paths: &Paths) {
    #[cfg(feature = "alpha")]
    spool::start(paths);
    #[cfg(not(feature = "alpha"))]
    remove(&dir(paths));
}

/// True the first time an alpha build starts on this computer (once).
pub fn first_start(paths: &Paths) -> bool {
    if !cfg!(feature = "alpha") {
        return false;
    }
    let Some(marker) = paths.log.parent().map(|d| d.join("alpha-notice-shown")) else { return false };
    if marker.exists() {
        return false;
    }
    let _ = std::fs::write(&marker, "");
    true
}

/// Records one event. `data` is only built when telemetry is on.
pub fn record(kind: &'static str, segment: Option<u64>, data: impl FnOnce() -> Value) {
    #[cfg(feature = "alpha")]
    spool::record(kind, segment, data());
    #[cfg(not(feature = "alpha"))]
    let _ = (kind, segment, data);
}

/// Sends what's waiting, briefly, before the app quits.
pub fn stop() {
    #[cfg(feature = "alpha")]
    spool::stop();
}

#[cfg_attr(feature = "alpha", allow(dead_code))]
fn remove(dir: &Path) {
    if dir.exists()
        && let Err(e) = std::fs::remove_dir_all(dir)
    {
        tracing::warn!("removing {}: {e}", dir.display());
    }
}

static DICTATING: AtomicBool = AtomicBool::new(false);

/// Whether the user is dictating (listening, or finishing the last
/// sentences). Only then are what's heard and its audio recorded: always
/// listening hears everything said near the microphone while asleep, and
/// that never leaves the computer.
pub fn set_dictating(on: bool) {
    DICTATING.store(on, Ordering::Relaxed);
}

pub fn dictating() -> bool {
    DICTATING.load(Ordering::Relaxed)
}

/// Milliseconds of a duration, for event data.
pub fn ms(d: std::time::Duration) -> u64 {
    d.as_millis().try_into().unwrap_or(u64::MAX)
}
