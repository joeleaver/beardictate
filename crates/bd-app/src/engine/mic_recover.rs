//! Always listening: getting the microphone back by itself when it goes
//! away.
//!
//! A Windows tester's USB microphone was invalidated by Windows once or
//! twice a day (WASAPI `AUDCLNT_E_DEVICE_INVALIDATED`), usually after a
//! while unused. The app turned the microphone off in the settings and
//! waited for the menu; the hotkey and the bear only said "the microphone
//! is off" in a notification Windows didn't show, so it looked dead until
//! the tester found the switch in Settings (2026-10-26 to 11-09 telemetry).
//! Now it keeps reopening the microphone until it's back, and says so on
//! the bear, the tray and Settings meanwhile. Only the menu (or Settings)
//! turns the microphone off.
//!
//! A stream that stops delivering audio without an error counts as gone too.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{Ui, Worker};

/// What the tray and Settings say while it's reconnecting.
pub(super) const RECONNECTING: &str = "The microphone stopped working: reconnecting\u{2026}";

/// Back this soon, it carries on as it was (awake stays awake); later, it
/// comes back asleep, as the room may have moved on.
const RESUME_AWAKE_WITHIN: Duration = Duration::from_secs(60);

/// A running stream delivers a chunk every few tens of milliseconds, silent
/// or not; this long without one, it has stopped.
const STALLED_AFTER: Duration = Duration::from_secs(6);

/// The next try after `tries` failed ones: at once (the next tick), then
/// backing off to every 15 seconds for as long as it takes.
fn retry_delay(tries: u32) -> Duration {
    Duration::from_secs(match tries {
        0 => 0,
        1 => 2,
        2 => 5,
        3 => 10,
        _ => 15,
    })
}

#[derive(Debug)]
pub(super) struct Recover {
    since: Instant,
    next: Instant,
    tries: u32,
    /// Whether it was awake when the microphone went.
    awake: bool,
    why: &'static str,
}

impl Recover {
    pub fn new(awake: bool, why: &'static str) -> Self {
        let now = Instant::now();
        Self { since: now, next: now, tries: 0, awake, why }
    }
}

impl<U: Ui> Worker<U> {
    /// The microphone went away while always listening: forget the stream
    /// and keep trying to open it again.
    pub(super) fn lose_mic(&mut self, why: &'static str) {
        let awake = self.listening_since.is_some();
        self.lose_mic_as(why, awake);
    }

    /// [`Self::lose_mic`], coming back awake (if soon) when `awake`.
    pub(super) fn lose_mic_as(&mut self, why: &'static str, awake: bool) {
        if let Some(pipeline) = self.pipeline.as_ref() {
            pipeline.set_awake(false);
            // What was said before it went is still typed.
            pipeline.end_session();
            self.cleaning = true;
        }
        self.standby = false;
        self.listening_since = None;
        self.capture = None;
        self.mic_check = None;
        if self.recover.is_none() {
            tracing::warn!(why, awake, "microphone gone: reconnecting");
            crate::telemetry::record("state", None, || json!({"state": "mic_reconnecting", "why": why}));
            self.recover = Some(Recover::new(awake, why));
        }
        self.publish();
    }

    /// On the engine's tick (and when the hotkey or the bear asks): tries
    /// the microphone again when it's time, or `now`.
    pub(super) fn retry_mic(&mut self, now: bool) {
        let Some(r) = self.recover.as_ref() else { return };
        if !self.wants_standby() {
            self.recover = None;
            return self.publish();
        }
        if self.pipeline.is_none() || self.loading.is_some() || (!now && Instant::now() < r.next) {
            return;
        }
        let mut r = self.recover.take().expect("checked above");
        match self.open_standby() {
            Ok(()) => {
                let away = r.since.elapsed();
                let wake = r.awake && away <= RESUME_AWAKE_WITHIN;
                tracing::info!(tries = r.tries + 1, away_s = away.as_secs(), wake, "microphone back");
                crate::telemetry::record("state", None, || {
                    json!({"state": "mic_back", "why": r.why, "tries": r.tries + 1, "away_ms": crate::telemetry::ms(away)})
                });
                if wake && let Some(pipeline) = self.pipeline.as_ref() {
                    pipeline.set_awake(true);
                }
                self.publish();
            }
            Err(e) => {
                if r.tries == 0 {
                    tracing::warn!("reopening the microphone: {e:#}");
                }
                r.tries += 1;
                r.next = Instant::now() + retry_delay(r.tries);
                self.recover = Some(r);
            }
        }
    }

    /// Whether it's waiting for the microphone to come back.
    pub(super) fn reconnecting(&self) -> bool {
        self.recover.is_some()
    }

    /// On the engine's tick: a running stream that stopped delivering audio
    /// is as good as gone.
    pub(super) fn check_stalled(&mut self) {
        let device = self.capture.as_ref().is_some_and(crate::mic::Mic::is_device);
        if self.standby && device && self.last_chunk.elapsed() >= STALLED_AFTER {
            self.notifier.notify("mic", "Microphone stopped", "No audio is arriving from the microphone; reconnecting.");
            self.lose_mic("stalled");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_at_once_then_backs_off_to_every_fifteen_seconds() {
        assert_eq!(retry_delay(0), Duration::ZERO);
        assert!(retry_delay(1) < retry_delay(2));
        assert!(retry_delay(2) < retry_delay(3));
        assert_eq!(retry_delay(4), Duration::from_secs(15));
        assert_eq!(retry_delay(400), Duration::from_secs(15));
    }
}
