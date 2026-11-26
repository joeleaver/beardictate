//! What the engine tells the UI about itself: its state, what that state
//! says, and the warning to show on the bear, the tray and Settings.

use std::time::Instant;

use super::{Ui, Worker, inject, load, mic_check, mic_recover, mic_silence};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Loading,
    Idle,
    /// Always listening: the microphone is open, waiting for "bear, wake up"
    /// or the hotkey.
    Asleep,
    /// Always listening, with the microphone turned off from the menu.
    MicOff,
    /// Always listening, but the microphone went away, or sends only
    /// silence: trying to reopen it (see `engine/mic_recover.rs` and
    /// `engine/mic_silence.rs`).
    Reconnecting,
    Listening,
    /// Stopped listening; the last sentences are still being cleaned.
    Cleaning,
    NeedsYou,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub state: State,
    /// The cleanup model's name, or what needs attention.
    pub detail: String,
    /// When listening started.
    pub since: Option<Instant>,
    /// Always listening: the hotkey wakes and sends to sleep.
    pub voice: bool,
    /// Something the user should fix, shown on the bear, in the tray's
    /// tooltip and menu and in Settings (a notification alone can be
    /// silenced by the system).
    pub warning: Option<Warning>,
}

/// Something the user should know and fix, besides the state.
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// Speech keeps arriving quiet (`mic_check.rs`).
    Quiet,
    /// Words couldn't be typed: this many characters are on the clipboard
    /// (`inject.rs`).
    Clipboard(usize),
    /// Another program holds the shortcut (as people write it); it's being
    /// tried again (`hotkey_wiring.rs`).
    ShortcutTaken(String),
    /// Dictating by the hotkey, the microphone sends only digital silence
    /// (`mic_silence.rs`; always listening shows `State::Reconnecting`).
    Silent,
}

impl Warning {
    /// What the tooltip and Settings say.
    pub fn text(&self) -> String {
        match self {
            Warning::Quiet => mic_check::QUIET_WARNING.to_string(),
            Warning::Clipboard(chars) => inject::clipboard_warning(*chars),
            Warning::Silent => mic_silence::SILENT_WARNING.to_string(),
            Warning::ShortcutTaken(sc) => format!("Another program is using {sc}, so it won\u{2019}t work here: beardictate keeps trying, or choose another shortcut in Settings \u{203A} General"),
        }
    }
}

impl<U: Ui> Worker<U> {
    pub(super) fn publish(&self) {
        let (state, detail) = if self.loading.is_some() {
            (State::Loading, self.loading_detail.clone())
        } else if let Some(e) = self.load_error.as_ref().or(self.injector_error.as_ref()) {
            (State::NeedsYou, e.clone())
        } else if self.standby && self.silence.showing() {
            // Awake or asleep: nothing can be heard until it's back.
            (State::Reconnecting, mic_silence::REOPENING.to_string())
        } else if self.listening_since.is_some() {
            let detail = match (self.reload_when_idle, load::cleanup_label(&self.settings)) {
                (true, Some(next)) => format!("{} · {next} loads when you stop dictating", self.cleanup_name),
                (true, None) => format!("{} · post-processing turns off when you stop dictating", self.cleanup_name),
                _ => self.cleanup_name.clone(),
            };
            (State::Listening, detail)
        } else if self.cleaning {
            (State::Cleaning, self.cleanup_name.clone())
        } else if self.recover.is_some() {
            (State::Reconnecting, mic_recover::RECONNECTING.to_string())
        } else if self.standby {
            (State::Asleep, self.cleanup_name.clone())
        } else if self.settings.voice_wake {
            (State::MicOff, self.cleanup_name.clone())
        } else {
            (State::Idle, self.cleanup_name.clone())
        };
        crate::telemetry::set_dictating(matches!(state, State::Listening | State::Cleaning));
        let warning = self.warning();
        self.ui.status(Status { state, detail, since: self.listening_since, voice: self.settings.voice_wake, warning });
        self.pad.look.set(state);
        self.sync_pad();
    }

    /// The warning to show on the bear, the tray and Settings, if any: the
    /// most pressing one.
    pub(super) fn warning(&self) -> Option<Warning> {
        if let Some((chars, _)) = self.on_clipboard {
            Some(Warning::Clipboard(chars))
        } else if let Some(sc) = &self.shortcut_taken {
            Some(Warning::ShortcutTaken(sc.clone()))
        } else if self.silence.showing() && !self.standby {
            Some(Warning::Silent)
        } else {
            self.quiet.showing.then_some(Warning::Quiet)
        }
    }

    /// The UI says the shortcut is taken, or works again.
    pub(super) fn shortcut_taken(&mut self, taken: Option<String>) {
        if taken == self.shortcut_taken {
            return;
        }
        if let Some(sc) = &taken {
            self.notifier.notify("hotkey", &format!("{sc} is taken"), &Warning::ShortcutTaken(sc.clone()).text());
        }
        self.shortcut_taken = taken;
        self.publish();
    }

    pub(super) fn busy(&self) -> bool {
        self.listening_since.is_some() || self.cleaning
    }
}
