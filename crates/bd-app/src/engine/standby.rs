//! Always listening ("voice wake"): the microphone stays on and the
//! pipeline's gate decides what is typed. "Bear, wake up", "bear, go to
//! sleep" and the hotkey switch typing on and off; only the tray menu turns
//! the microphone itself off and on (`Settings::microphone_on`).

use anyhow::Context;

use super::{Ui, Worker};

impl<U: Ui> Worker<U> {
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
        let started = self
            .open_mic()
            .and_then(|()| self.capture.as_mut().expect("opened above").start().context("starting the microphone"));
        match started {
            Ok(()) => {
                tracing::info!(wake, "always listening: microphone on");
                self.standby = true;
            }
            Err(e) => {
                self.notifier.notify("mic", "Microphone problem", &format!("{e:#}"));
                self.capture = None;
                // Off in the settings too, so the menu offers to try again.
                return self.record_mic(false);
            }
        }
        self.publish();
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
        self.listening_since = None;
        tracing::info!("always listening: microphone off");
        self.publish();
    }

    /// The hotkey: wake up or go to sleep. With the microphone off it says
    /// where to turn it on, rather than turning it on itself.
    pub(super) fn voice_toggle(&mut self) {
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
