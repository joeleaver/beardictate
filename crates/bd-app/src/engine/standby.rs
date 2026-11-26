//! Always listening ("voice wake"): the microphone stays on and the
//! pipeline's gate decides what is typed. "Bear, wake up", "bear, go to
//! sleep" and the hotkey switch typing on and off; only the tray menu turns
//! the microphone itself off and on (`Settings::microphone_on`); when it
//! goes away by itself, `mic_recover.rs` gets it back.
//!
//! Awake, it goes back to sleep after `Settings::sleep_after_mins` (10 by
//! default) with nothing said to it (someone else's voice doesn't count: `bd_pipeline::FAR_BELOW_DB`), as
//! a dictation started by the hotkey stops: an alpha tester's, left awake,
//! typed and recorded the room for an hour and a half.

use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::json;

use super::{Ui, Worker};

/// Whether a dictation that last heard the user at `last_heard` has gone on
/// `minutes` unspoken to; 0 never has.
pub(super) fn unspoken_to(last_heard: Instant, now: Instant, minutes: u32) -> bool {
    minutes > 0 && now.saturating_duration_since(last_heard) >= Duration::from_secs(u64::from(minutes) * 60)
}

impl<U: Ui> Worker<U> {
    /// On the engine's tick: sends a dictation nobody is speaking to back
    /// to sleep, or stops it.
    pub(super) fn sleep_if_unspoken_to(&mut self) {
        let minutes = self.settings.sleep_after_mins;
        if self.listening_since.is_none() || self.testing || !unspoken_to(self.last_heard, Instant::now(), minutes) {
            return;
        }
        let Some(pipeline) = self.pipeline.as_ref() else { return };
        tracing::info!("nothing said to it for {minutes} minutes: going to sleep");
        crate::telemetry::record("state", None, || json!({"state": "unspoken_to", "minutes": minutes}));
        self.last_heard = Instant::now();
        if self.standby {
            // The pipeline says when it's asleep (Event::Slept).
            pipeline.set_awake(false);
        } else {
            self.stop();
        }
    }

    /// Whether the settings ask for the microphone to be running.
    pub(super) fn wants_standby(&self) -> bool {
        self.settings.voice_wake && self.settings.microphone_on
    }

    /// Starts the microphone, asleep, or awake when `wake`.
    pub(super) fn stand_by(&mut self, wake: bool) {
        let Some(pipeline) = self.pipeline.as_ref() else { return };
        if wake {
            pipeline.set_awake(true);
        }
        if self.standby {
            return;
        }
        match self.open_standby() {
            Ok(()) => tracing::info!(wake, "always listening: microphone on"),
            Err(e) => {
                tracing::warn!("opening the microphone: {e:#}");
                // Not off in the settings: it keeps trying (mic_recover.rs),
                // and says so on the bear meanwhile.
                self.lose_mic_as("open_failed", wake);
                return;
            }
        }
        self.publish();
    }

    /// Opens and starts the microphone for always listening.
    pub(super) fn open_standby(&mut self) -> anyhow::Result<()> {
        let started = self
            .open_mic()
            .and_then(|()| self.capture.as_mut().expect("opened above").start().context("starting the microphone"));
        if let Err(e) = started {
            self.capture = None;
            return Err(e);
        }
        self.standby = true;
        self.recover = None;
        self.last_chunk = Instant::now();
        Ok(())
    }

    /// Turns the microphone off: what was said so far is still typed.
    pub(super) fn leave_standby(&mut self) {
        if !self.standby {
            return;
        }
        self.standby = false;
        if let Some(pipeline) = self.pipeline.as_ref() {
            pipeline.set_awake(false);
            pipeline.end_session();
            self.cleaning = true;
        }
        if let Some(capture) = self.capture.as_mut() {
            let _ = capture.stop();
        }
        // Release the device, so the desktop stops showing it in use.
        self.capture = None;
        self.silence.reset();
        self.listening_since = None;
        tracing::info!("always listening: microphone off");
        self.publish();
    }

    /// The hotkey: wake up or go to sleep. With the microphone off it says
    /// where to turn it on, rather than turning it on itself.
    pub(super) fn voice_toggle(&mut self) {
        if self.standby && self.silence.showing() && self.listening_since.is_none() {
            // Asleep: try now, and wake, as it was pressed to talk. (Awake,
            // the hotkey sends it to sleep as usual.)
            self.reopen_silent();
            if self.standby {
                self.pipeline.as_ref().expect("standing by").set_awake(true);
                return self.publish();
            }
        }
        if self.reconnecting() {
            // Try now rather than at the next retry; once it's back, the
            // hotkey wakes it as usual.
            self.retry_mic(true);
            if self.reconnecting() {
                self.notifier.notify("mic", "The microphone isn't back yet", "beardictate keeps trying. Check that it's plugged in, or pick another in Settings › Microphone.");
                return;
            }
        }
        if self.standby {
            let awake = self.listening_since.is_some();
            self.pipeline.as_ref().expect("standing by").set_awake(!awake);
        } else if self.loading.is_none() {
            self.notifier.notify(
                "mic",
                "The microphone is off",
                "Turn it on from the beardictate menu in the system tray.",
            );
        }
    }

    /// Always listening or the microphone switched in the settings. Turning
    /// the microphone on starts dictating straight away.
    pub(super) fn switch_mic(&mut self, was_on: bool) {
        if self.wants_standby() {
            let wake = !was_on && self.settings.microphone_on;
            if !self.standby && self.listening_since.is_some() {
                // Hotkey dictation in progress: carry on under the new mode.
                self.stop();
            }
            self.stand_by(wake);
        } else {
            self.recover = None;
            self.silence.reset();
            self.leave_standby();
        }
        self.publish();
    }

    /// Records the microphone as on or off in the settings, when the app
    /// rather than the menu switched it (it went away; the app started).
    pub(super) fn record_mic(&mut self, on: bool) {
        if self.settings.microphone_on == on {
            return self.publish();
        }
        self.settings.microphone_on = on;
        if let Err(e) = self.paths.save(&self.settings) {
            tracing::warn!("saving settings: {e:#}");
        }
        self.settings_mtime = super::mtime(&self.paths.settings);
        self.ui.settings_changed(self.settings.clone());
        self.publish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_minutes_unspoken_to_is_enough() {
        let start = Instant::now();
        assert!(!unspoken_to(start, start + Duration::from_secs(8 * 60), 10));
        assert!(unspoken_to(start, start + Duration::from_secs(10 * 60), 10));
        assert!(unspoken_to(start, start + Duration::from_secs(5 * 60), 5));
        // A clock that went backwards isn't a long silence.
        assert!(!unspoken_to(start + Duration::from_secs(5), start, 10));
    }

    #[test]
    fn never_means_never() {
        let start = Instant::now();
        assert!(!unspoken_to(start, start + Duration::from_secs(24 * 3600), 0));
    }
}
