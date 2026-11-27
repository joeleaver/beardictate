//! Settings changes, applied to the running engine: from the Settings
//! window, or the settings file edited by hand (watched).

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use super::{Ui, Worker, mic_check, notes, pipeline_config};
use crate::settings::Settings;

impl<U: Ui> Worker<U> {
    pub(super) fn apply(&mut self, new: Settings) {
        if new != self.settings {
            notes::settings(&new);
        }
        let old = std::mem::replace(&mut self.settings, new);
        let s = &self.settings;
        self.notifier.set_enabled(s.notifications);
        if (old.voice_wake, old.microphone_on) != (s.voice_wake, s.microphone_on) {
            self.apply_rest(&old);
            return self.switch_mic(old.microphone_on);
        }
        if old.microphone != s.microphone
            && let Some(pipeline) = &self.pipeline
        {
            // Each microphone has its own learned gain.
            pipeline.reconfigure(pipeline_config(s, self.starting_gain()));
            self.gain = self.starting_gain();
            self.quiet = mic_check::Quiet::default();
        }
        let s = &self.settings;
        if old.microphone != s.microphone {
            // A silent episode was about the other one.
            self.silence.reset();
        }
        if self.standby && old.microphone != s.microphone {
            // A different microphone: reopen it, staying asleep or awake.
            let awake = self.listening_since.is_some();
            self.leave_standby();
            self.stand_by(awake);
        } else if old.microphone != s.microphone || old.keep_mic_open != s.keep_mic_open {
            if self.busy() {
                tracing::info!("microphone change applies after this dictation");
                self.reopen_mic = true;
            } else {
                self.capture = None;
            }
        }
        self.apply_rest(&old);
    }

    /// The model and live pipeline settings.
    fn apply_rest(&mut self, old: &Settings) {
        let s = &self.settings;
        if old.models() != s.models() {
            self.request_reload();
        } else if old.live() != s.live()
            && let Some(pipeline) = &self.pipeline
        {
            tracing::info!("settings applied");
            pipeline.reconfigure(pipeline_config(s, self.starting_gain()));
        }
    }

    pub(super) fn request_reload(&mut self) {
        if self.loading.is_some() {
            // One load at a time: two at once hold two copies of the model.
            // The one running is stale; load again when it lands.
            self.reload_after_load = true;
            return;
        }
        // Always listening is awake for hours; waiting for "go to sleep"
        // would keep the old model all that time. Reload now and wake again
        // after (speech during the few seconds of loading isn't typed).
        let waits = self.cleaning || (self.listening_since.is_some() && !self.standby);
        if waits {
            self.reload_when_idle = true;
            self.publish();
        } else {
            self.start_load();
        }
    }

    pub(super) fn watch_files(&mut self) {
        let now = mtime(&self.paths.settings);
        if now != self.settings_mtime {
            self.settings_mtime = now;
            // List and case modes aren't in the file: they carry over.
            let loaded = self.paths.load().map(|mut s| {
                s.list_mode = self.settings.list_mode;
                s.case_mode = self.settings.case_mode;
                s
            });
            match loaded {
                Ok(s) if s != self.settings => {
                    tracing::info!("settings file changed");
                    self.ui.settings_changed(s.clone());
                    self.apply(s);
                }
                Ok(_) => {}
                Err(e) => self.notifier.notify("settings", "Settings file has a mistake", &format!("{e:#}")),
            }
        }
    }
}

pub(super) fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}
