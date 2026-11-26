//! A microphone that opens but sends only digital silence: reopened until
//! sound arrives, and said so on the bear, the tray and Settings meanwhile.
//!
//! A Windows tester's USB microphone (an Orca 001) delivered nothing but
//! silence after about 30 of the computer's first starts of the day since
//! 2026-10-22 (telemetry). The stream wasn't lost or stalled: its chunks kept
//! coming, all below -100 dBFS. The silence ended on the device's own
//! schedule, seconds to 40 minutes later, usually when Windows reset the
//! device (`lost` or `stalled`), sometimes in the same stream. Opening it
//! again sometimes stayed silent (two reopens at about 13 minutes did; a
//! restart at 5 minutes did), so a single retry isn't enough: it keeps
//! reopening, backing off to once a minute, and picks the sound up as soon
//! as the device sends it. The `mic-silent` notification was the only sign,
//! and Windows hid it.
//!
//! Only a stream that has never sent sound counts. A microphone in a quiet
//! room has a noise floor far above [`SILENT`], and a stream that sent sound
//! once (a noise-gated microphone, someone speaking) is live: its silences
//! are ordinary quiet, never reopened, so nothing that was dictated is cut.
//! A silent stream can't be mid-dictation, so it's reopened awake or asleep.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{Ui, Worker};

/// Below this a chunk's RMS is digital silence: no microphone's noise floor
/// is this quiet (about -100 dBFS; the tester's live sessions never were).
pub(super) const SILENT: f32 = 1e-5;

/// What the tray and Settings say while it's reopening a silent microphone.
pub(super) const REOPENING: &str = "The microphone is on but sends no sound: reopening it\u{2026}";

/// What Settings and the tray say when dictating by the hotkey (the
/// microphone is opened afresh at the next dictation).
pub(super) const SILENT_WARNING: &str = "The microphone sends no sound: check that it isn\u{2019}t muted, or pick another in Settings \u{203A} Microphone";

/// How long a stream must be silent before it's judged: 8 s, the length of
/// the first-seconds check that found the tester's silence.
const JUDGE_AFTER: Duration = Duration::from_secs(8);

/// How long a reopened stream that's still silent waits before the next
/// reopen, after `reopens` of them: soon at first, then once a minute for
/// as long as it takes (the tester's lasted up to 40 minutes).
fn reopen_after(reopens: u32) -> Duration {
    Duration::from_secs(match reopens {
        0 | 1 => 10,
        2 => 20,
        3 => 30,
        _ => 60,
    })
}

/// A run of reopens with no sound yet.
#[derive(Debug)]
struct Episode {
    since: Instant,
    reopens: u32,
}

/// What happened, for the engine to act on.
#[derive(Debug, PartialEq)]
pub(super) enum Heard {
    /// Sound, ending a silent episode: after this many reopens, this long.
    Back { reopens: u32, silent: Duration },
}

/// Watches the open stream for digital silence.
#[derive(Debug, Default)]
pub(super) struct Silence {
    /// When the stream now open went silent, if it has sent nothing but.
    silent_since: Option<Instant>,
    /// The stream now open has sent sound: it's live.
    heard: bool,
    episode: Option<Episode>,
}

impl Silence {
    /// A new stream was opened.
    pub fn opened(&mut self) {
        self.silent_since = None;
        self.heard = false;
    }

    /// The stream stopped between dictations (the hotkey): the time until
    /// the next one isn't silence it sent.
    pub fn paused(&mut self) {
        self.silent_since = None;
    }

    /// Forgets everything: the microphone was turned off or another chosen.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Notes a chunk at `rms`; `Some` when sound ends a silent episode.
    pub fn chunk(&mut self, rms: f32, now: Instant) -> Option<Heard> {
        if rms < SILENT {
            if !self.heard {
                self.silent_since.get_or_insert(now);
            }
            return None;
        }
        self.heard = true;
        self.silent_since = None;
        self.episode.take().map(|e| Heard::Back { reopens: e.reopens, silent: now.saturating_duration_since(e.since) })
    }

    /// Whether the stream has sent only silence long enough to act on.
    pub fn due(&self, now: Instant) -> bool {
        let wait = self.episode.as_ref().map_or(JUDGE_AFTER, |e| reopen_after(e.reopens));
        !self.heard && self.silent_since.is_some_and(|s| now.saturating_duration_since(s) >= wait)
    }

    /// The stream is being given up on (and reopened, when always
    /// listening); true when this starts an episode.
    pub fn give_up(&mut self, now: Instant) -> bool {
        let since = self.silent_since.take().unwrap_or(now);
        match self.episode.as_mut() {
            Some(e) => {
                e.reopens += 1;
                false
            }
            None => {
                self.episode = Some(Episode { since, reopens: 1 });
                true
            }
        }
    }

    /// Whether a silent microphone is being reopened (or warned about).
    pub fn showing(&self) -> bool {
        self.episode.is_some()
    }
}

impl<U: Ui> Worker<U> {
    /// On each chunk from a real microphone.
    pub(super) fn note_silence(&mut self, rms: f32) {
        if !self.capture.as_ref().is_some_and(crate::mic::Mic::is_device) {
            return;
        }
        if let Some(Heard::Back { reopens, silent }) = self.silence.chunk(rms, Instant::now()) {
            tracing::info!(reopens, silent_s = silent.as_secs(), "the microphone sends sound again");
            crate::telemetry::record("state", None, || {
                json!({"state": "mic_sound", "reopens": reopens, "silent_ms": crate::telemetry::ms(silent)})
            });
            self.publish();
        }
    }

    /// On the engine's tick: a stream that has sent only silence for a while
    /// is reopened (always listening), or warned about (the hotkey).
    pub(super) fn check_silent(&mut self) {
        let device = self.capture.as_ref().filter(|c| c.is_device()).map(crate::mic::Mic::name);
        let Some(device) = device else { return };
        let now = Instant::now();
        if !self.silence.due(now) {
            return;
        }
        if self.silence.give_up(now) {
            tracing::warn!(device = %device, standby = self.standby, "the microphone sends only silence");
            crate::telemetry::record("state", None, || json!({"state": "mic_silent", "device": device}));
            self.notifier.notify(
                "mic-silent",
                "No sound from the microphone",
                &format!("{device} is open but sends silence. beardictate keeps reopening it; check that it isn\u{2019}t muted, or pick another microphone in Settings."),
            );
        }
        if self.standby {
            self.reopen_silent();
        }
        self.publish();
    }

    /// Opens the silent microphone afresh, staying awake or asleep; if it
    /// won't open, it's gone (`mic_recover.rs` keeps trying).
    pub(super) fn reopen_silent(&mut self) {
        let awake = self.listening_since.is_some();
        self.capture = None;
        if let Err(e) = self.open_standby() {
            tracing::warn!("reopening the silent microphone: {e:#}");
            self.lose_mic_as("silent", awake);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: f32 = 3e-4; // about -70 dBFS: a quiet room's noise floor
    const CHUNK: Duration = Duration::from_millis(32);

    /// Feeds `secs` of chunks at `rms` from `at`, returning the time after.
    fn feed(s: &mut Silence, at: Instant, secs: u64, rms: f32) -> (Instant, Option<Heard>) {
        let mut now = at;
        let mut heard = None;
        while now < at + Duration::from_secs(secs) {
            heard = heard.or(s.chunk(rms, now));
            now += CHUNK;
        }
        (now, heard)
    }

    #[test]
    fn a_silent_stream_is_given_up_on_after_eight_seconds() {
        let mut s = Silence::default();
        s.opened();
        let t0 = Instant::now();
        let (t, _) = feed(&mut s, t0, 7, 0.0);
        assert!(!s.due(t), "not yet");
        let (t, _) = feed(&mut s, t, 2, 0.0);
        assert!(s.due(t));
        assert!(s.give_up(t), "starts an episode");
        assert!(s.showing());
    }

    #[test]
    fn a_quiet_room_is_not_silence() {
        let mut s = Silence::default();
        s.opened();
        let (t, heard) = feed(&mut s, Instant::now(), 600, ROOM);
        assert_eq!(heard, None);
        assert!(!s.due(t) && !s.showing());
    }

    #[test]
    fn a_stream_that_sent_sound_is_live_through_any_silence() {
        // A noise-gated microphone: speech, then exact zeros for an hour.
        let mut s = Silence::default();
        s.opened();
        let (t, _) = feed(&mut s, Instant::now(), 3, 0.05);
        let (t, _) = feed(&mut s, t, 3600, 0.0);
        assert!(!s.due(t), "never reopened mid-use");
    }

    #[test]
    fn reopens_back_off_to_once_a_minute_until_sound_comes() {
        let mut s = Silence::default();
        s.opened();
        let (mut t, _) = feed(&mut s, Instant::now(), 9, 0.0);
        assert!(s.due(t) && s.give_up(t));
        let mut gaps = Vec::new();
        for _ in 0..6 {
            s.opened();
            let start = t;
            while !s.due(t) {
                s.chunk(0.0, t);
                t += CHUNK;
            }
            gaps.push((t - start).as_secs());
            assert!(!s.give_up(t), "the same episode");
        }
        assert!(gaps.windows(2).all(|w| w[0] <= w[1]), "{gaps:?}");
        assert_eq!(*gaps.last().unwrap(), 60);
        // Sound at last: the episode ends, with how many reopens and how long.
        s.opened();
        let heard = s.chunk(ROOM, t);
        let Some(Heard::Back { reopens, silent }) = heard else { panic!("{heard:?}") };
        assert_eq!(reopens, 7);
        assert!(silent >= Duration::from_secs(8 + 10 + 20 + 30 + 60 + 60));
        assert!(!s.showing());
    }

    #[test]
    fn the_time_between_dictations_is_not_silence() {
        let mut s = Silence::default();
        s.opened();
        let (t, _) = feed(&mut s, Instant::now(), 5, 0.0);
        s.paused();
        let (t, _) = feed(&mut s, t + Duration::from_secs(3600), 1, 0.0);
        assert!(!s.due(t), "one second into the next dictation");
    }

    #[test]
    fn no_chunks_is_a_stall_not_silence() {
        // check_stalled handles a stream that sends nothing at all.
        let mut s = Silence::default();
        s.opened();
        assert!(!s.due(Instant::now() + Duration::from_secs(60)));
    }

    #[test]
    fn turning_the_microphone_off_ends_the_episode_quietly() {
        let mut s = Silence::default();
        s.opened();
        let (t, _) = feed(&mut s, Instant::now(), 9, 0.0);
        s.give_up(t);
        s.reset();
        assert!(!s.showing());
        assert_eq!(s.chunk(ROOM, t), None, "no `mic_sound` for an episode that was dropped");
    }
}
