//! The dictation hotkey: registered at start, rebound when the setting
//! changes, and registered again until it works when another program holds
//! the shortcut.
//!
//! A Windows tester's Ctrl+Space was taken at the first start of each day
//! (by something else starting with Windows) and free again later; the app
//! tried once, so the hotkey stayed dead until a restart, and every
//! settings change tried and failed again (2026-11-05 telemetry).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rinch::hotkey::{self, Binding, HotkeyError, HotkeySpec, HotkeyState};
use rinch::prelude::*;
use rinch::reactive::{Effect, untracked};

use crate::engine_send;
use crate::settings::Settings;
use crate::ui::{Shared, check_hotkey, readable};

/// How long to wait before trying a taken shortcut again: soon at first
/// (a program still starting up, or our previous copy still quitting),
/// then twice a minute for as long as it stays taken.
fn retry_after_ms(tries: u32) -> u32 {
    match tries {
        0..=11 => 5_000,
        _ => 30_000,
    }
}

#[derive(Clone)]
struct Wiring {
    text: Signal<String>,
    note: Signal<String>,
    settings: Signal<Settings>,
    slot: Rc<RefCell<Option<hotkey::Hotkey>>>,
    /// A retry is waiting on a timer (only ever one).
    retry_armed: Rc<Cell<bool>>,
    tries: Rc<Cell<u32>>,
}

/// Registers the dictation hotkey and rebinds it when the setting changes.
pub fn wire_hotkey(shared: &Shared) {
    let w = Wiring {
        text: shared.hotkey_text,
        note: shared.hotkey_note,
        settings: shared.settings,
        slot: Rc::new(RefCell::new(None)),
        retry_armed: Rc::new(Cell::new(false)),
        tries: Rc::new(Cell::new(0)),
    };
    // Only the shortcut itself matters here: the engine saves the settings
    // for other reasons (the microphone going away, say), and each save
    // used to re-run the registration.
    let wanted = Memo::new({
        let settings = shared.settings;
        move || settings.with(|st| st.hotkey.clone())
    });
    Effect::new(move || {
        let shortcut = wanted.get();
        untracked(|| {
            w.tries.set(0);
            w.apply(&shortcut);
        });
    });
}

impl Wiring {
    fn wanted(&self) -> String {
        untracked(|| self.settings.with(|st| st.hotkey.clone()))
    }

    fn explain(&self, b: &Binding, wanted: &str) {
        // The desktop (a Wayland portal) keeps its own shortcut: say where to
        // change it when it isn't the one chosen here.
        match b {
            Binding::SystemChosen { description: Some(d) } if !d.eq_ignore_ascii_case(&readable(wanted)) => {
                self.note.set(format!("Your desktop keeps its own shortcut for beardictate ({d}): change it in your desktop\u{2019}s shortcut settings."));
            }
            _ => self.note.set(String::new()),
        }
    }

    fn show(&self, b: &Binding, wanted: &str) {
        let text = match b {
            Binding::Exact(sc) => readable(&sc.to_string()),
            Binding::SystemChosen { description } => description.clone().unwrap_or_else(|| readable(wanted)),
            Binding::Pending => format!("{} (waiting for your approval)", readable(wanted)),
            Binding::Declined => "no hotkey (declined; click the bear)".into(),
            _ => readable(wanted),
        };
        self.text.set(text);
        self.explain(b, wanted);
    }

    /// Makes the hotkey `wanted`: rebinds a registered one, or registers.
    fn apply(&self, wanted: &str) {
        if let Err(why) = check_hotkey(wanted) {
            self.text.set(format!("no hotkey ({why})"));
            return;
        }
        let result = {
            let slot = self.slot.borrow();
            slot.as_ref().map(|hk| hk.rebind(wanted).map(|()| hk.binding()))
        };
        match result {
            Some(Ok(binding)) => {
                engine_send(crate::engine::Command::ShortcutTaken(None));
                self.show(&binding, wanted);
            }
            // The old binding stays in force.
            Some(Err(e)) => self.failed(wanted, &e),
            None => self.register(wanted),
        }
    }

    fn register(&self, wanted: &str) {
        let spec = HotkeySpec::new(wanted.to_string()).id("dictate").description("Start or stop dictating");
        match hotkey::register(spec, |ev| {
            // A toggle: act on the press. Bindings are always two keys or
            // more, so a release never needs telling apart from a chord.
            tracing::info!(state = ?ev.state, "hotkey");
            if ev.state == HotkeyState::Pressed {
                engine_send(crate::engine::Command::Toggle);
            }
        }) {
            Ok(hk) => {
                let tries = self.tries.replace(0);
                // A shortcut that was taken works now.
                engine_send(crate::engine::Command::ShortcutTaken(None));
                tracing::info!(backend = ?hotkey::backend(), binding = ?hk.binding(), tries, "hotkey registered");
                self.show(&hk.binding(), wanted);
                let w = self.clone();
                hk.on_binding_change(move |b| w.show(b, &w.wanted()));
                *self.slot.borrow_mut() = Some(hk);
            }
            Err(e) => self.failed(wanted, &e),
        }
    }

    fn failed(&self, wanted: &str, e: &HotkeyError) {
        let tries = self.tries.get();
        // Logged once per shortcut, not every retry.
        if tries == 0 {
            tracing::warn!("hotkey {wanted}: {e}");
        }
        let taken = matches!(e, HotkeyError::Conflict(_));
        if taken {
            // On the bear, in the tray and here: the notification may be
            // silenced, and the tester never looked at Settings.
            self.text.set(format!("{} (in use by another program)", readable(wanted)));
            self.note.set(format!(
                "Another program is using {}, so it won\u{2019}t work here until that program lets it go. beardictate keeps trying; or choose another shortcut.",
                readable(wanted)
            ));
            engine_send(crate::engine::Command::ShortcutTaken(Some(readable(wanted))));
            self.retry(wanted.to_string());
        } else {
            self.text.set(format!("{} unavailable: {e}", readable(wanted)));
        }
    }

    /// Tries a taken shortcut again later, while it's still the one wanted.
    fn retry(&self, wanted: String) {
        if self.retry_armed.replace(true) {
            return;
        }
        let delay = retry_after_ms(self.tries.get());
        let w = self.clone();
        set_timeout(delay, move || {
            w.retry_armed.set(false);
            if w.wanted() != wanted {
                return; // A new shortcut was chosen; its own attempt ran.
            }
            w.tries.set(w.tries.get() + 1);
            w.apply(&wanted);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_soon_then_twice_a_minute() {
        assert_eq!(retry_after_ms(0), 5_000);
        assert_eq!(retry_after_ms(11), 5_000);
        assert_eq!(retry_after_ms(12), 30_000);
        assert_eq!(retry_after_ms(1000), 30_000);
    }
}
