//! A copy left running in the tray never restarts, so a downloaded update
//! waited for weeks (testers stayed on 0.1.13 with 0.1.17 out). With
//! automatic updates on, the app restarts into a waiting update itself once
//! nothing has gone on for [`IDLE_FOR`] (Joe, 2026-10-20): not dictating,
//! cleaning or testing the microphone, no text in the Bear Pad, and not in
//! list mode (which isn't saved). Asleep while always listening counts as
//! idle. A start normally switches dictation on; after this restart, which
//! nobody asked for, the new version starts asleep (or idle) instead, told
//! by a [`QUIET_START`] file in the updates folder.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{Ui, Worker};

/// How long nothing must have gone on before it restarts to update.
pub(super) const IDLE_FOR: Duration = Duration::from_secs(30 * 60);

/// Left in the updates folder by an idle restart, for the next start.
pub const QUIET_START: &str = "quiet-start";

/// At start: whether the last run restarted itself to update (and so this
/// one shouldn't start dictating). Removes the marker.
pub(super) fn take_quiet_start(updates: &Path) -> bool {
    fs::remove_file(updates.join(QUIET_START)).is_ok()
}

/// Whether a restart now loses nothing and interrupts no one.
pub(super) fn may_restart(ready: bool, auto: bool, busy: bool, keep: bool, active_at: Instant, now: Instant) -> bool {
    ready && auto && !busy && !keep && now.saturating_duration_since(active_at) >= IDLE_FOR
}

impl<U: Ui> Worker<U> {
    /// On the engine's tick: restarts into a waiting update after a long
    /// idle stretch.
    pub(super) fn restart_if_idle_for_update(&mut self) {
        let now = Instant::now();
        let busy = self.busy() || self.testing || self.loading.is_some();
        if busy {
            self.active_at = now;
        }
        let keep = !self.pad.view.is_empty() || self.settings.list_mode;
        if !may_restart(crate::update::ready(), self.settings.auto_update, busy, keep, self.active_at, now) {
            return;
        }
        tracing::info!("idle for {} minutes with an update waiting: restarting to install it", IDLE_FOR.as_secs() / 60);
        crate::telemetry::record("update", None, || json!({"event": "idle_restart"}));
        if let Err(e) = fs::create_dir_all(&self.paths.updates).and_then(|()| fs::write(self.paths.updates.join(QUIET_START), b"")) {
            // Starting with dictation on, unasked, is worse than waiting.
            tracing::warn!("not restarting to update: {e}");
            self.active_at = now;
            return;
        }
        // Once only: the restart takes a moment on the UI thread.
        self.active_at = now;
        self.ui.act(crate::menu::Act::RestartToUpdate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_only_when_ready_switched_on_and_long_idle() {
        let start = Instant::now();
        let later = start + IDLE_FOR;
        assert!(may_restart(true, true, false, false, start, later));
        assert!(!may_restart(true, true, false, false, start, start + Duration::from_secs(29 * 60)));
        assert!(!may_restart(false, true, false, false, start, later), "nothing waiting");
        assert!(!may_restart(true, false, false, false, start, later), "automatic updates off");
        assert!(!may_restart(true, true, true, false, start, later), "dictating");
        assert!(!may_restart(true, true, false, true, start, later), "text in the pad");
    }

    #[test]
    fn the_quiet_start_marker_is_read_once() {
        let dir = std::env::temp_dir().join(format!("bd-quiet-start-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(!take_quiet_start(&dir));
        fs::write(dir.join(QUIET_START), b"").unwrap();
        assert!(take_quiet_start(&dir));
        assert!(!take_quiet_start(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }
}
