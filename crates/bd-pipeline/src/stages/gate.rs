//! The voice wake gate: whether segments are delivered or dropped, in
//! always-listening mode.

use std::time::{Duration, Instant};

use super::wake::{self, Switch};

/// Waking after a sleep this long starts a new passage, like a dictation
/// started that long after the last.
const NEW_PASSAGE_AFTER: Duration = Duration::from_secs(60);

/// What the finisher does with one segment or switch.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Step {
    /// Just woke: emit [`crate::Event::Woke`].
    pub woke: bool,
    /// ...after a long sleep, so start a new passage first.
    pub fresh: bool,
    /// Text to take in as dictation.
    pub pass: Option<String>,
    /// Go to sleep once `pass` is delivered.
    pub sleep_after: bool,
}

pub(super) struct Gate {
    on: bool,
    awake: bool,
    asleep_since: Option<Instant>,
}

impl Gate {
    pub fn new(on: bool) -> Self {
        Self { on, awake: !on, asleep_since: on.then(Instant::now) }
    }

    /// Turning voice wake on starts asleep; off, everything passes.
    pub fn configure(&mut self, on: bool) {
        if on != self.on {
            *self = Self::new(on);
        }
    }

    /// Segments are delivered (always, with voice wake off).
    pub fn awake(&self) -> bool {
        self.awake
    }

    /// A transcribed segment.
    pub fn pass(&mut self, text: String) -> Step {
        if !self.on {
            return Step { pass: Some(text), ..Step::default() };
        }
        match wake::heard(&text) {
            Some(h) if h.switch == Switch::Wake => {
                let after = h.after.to_string();
                let step = if self.awake { Step::default() } else { self.wake() };
                Step { pass: Some(after), ..step }
            }
            Some(h) if self.awake => {
                let before = h.before.to_string();
                self.sleep();
                Step { pass: Some(before), sleep_after: true, ..Step::default() }
            }
            _ if self.awake => Step { pass: Some(text), ..Step::default() },
            _ => Step::default(),
        }
    }

    /// The hotkey, or the app, switching by hand.
    pub fn set(&mut self, awake: bool) -> Step {
        match (self.on, self.awake, awake) {
            (true, false, true) => self.wake(),
            (true, true, false) => {
                self.sleep();
                Step { sleep_after: true, ..Step::default() }
            }
            _ => Step::default(),
        }
    }

    fn wake(&mut self) -> Step {
        self.awake = true;
        let fresh = self.asleep_since.take().is_some_and(|t| t.elapsed() > NEW_PASSAGE_AFTER);
        Step { woke: true, fresh, ..Step::default() }
    }

    fn sleep(&mut self) {
        self.awake = false;
        self.asleep_since = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed(step: &Step) -> Option<&str> {
        step.pass.as_deref()
    }

    #[test]
    fn off_lets_everything_through() {
        let mut g = Gate::new(false);
        assert_eq!(passed(&g.pass("Bear, go to sleep.".into())), Some("Bear, go to sleep."));
        assert_eq!(g.set(false), Step::default());
    }

    #[test]
    fn only_what_is_said_while_awake_passes() {
        let mut g = Gate::new(true);
        assert_eq!(g.pass("Talking on the phone.".into()), Step::default());
        let woke = g.pass("Bear, wake up. It was late.".into());
        assert!(woke.woke && !woke.fresh);
        assert_eq!(passed(&woke), Some("It was late."));
        assert_eq!(passed(&g.pass("The bear went to sleep.".into())), Some("The bear went to sleep."));
        let slept = g.pass("He left. Bear, go to sleep.".into());
        assert_eq!((passed(&slept), slept.sleep_after), (Some("He left."), true));
        assert_eq!(g.pass("More phone.".into()), Step::default());
        // The hotkey works too, and switching to the state it's in does nothing.
        assert!(g.set(true).woke);
        assert_eq!(g.set(true), Step::default());
        assert!(g.set(false).sleep_after);
        // Turning it off and on again starts asleep.
        g.configure(false);
        g.configure(true);
        assert_eq!(g.pass("Hello.".into()), Step::default());
    }
}
