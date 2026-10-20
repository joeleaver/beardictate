//! Whose voice it is. Always listening hears everyone in the room: an alpha
//! tester's microphone, left awake, typed and recorded a video and other
//! people at -45 to -76 dBFS while their own voice came in at -25 to -37.
//! The user's speech level is learned from what they dictate, and a
//! sentence far quieter than that is someone else's: not typed, its audio
//! not shared, and the gain not learned from it
//! ([`PipelineConfig::drop_far_voices`](crate::PipelineConfig)).
//!
//! A sentence with "bear, wake up" or "bear, go to sleep" in it is the
//! user's whatever its level, and so is the first sentence after the hotkey
//! wakes or a dictation starts: either sets the level afresh, so someone
//! who moved away from the microphone isn't shut out.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A sentence this much quieter than the user's level is someone else's.
pub const FAR_BELOW_DB: f32 = 15.0;
/// How far one sentence moves the level towards its own: quickly up (the
/// nearest voice is the user's), slowly down.
const RISE: f32 = 0.5;
const FALL: f32 = 0.15;
/// A quieter sentence pulls the level down as if at most this much below.
const MAX_FALL_DB: f32 = 6.0;
/// Sentences shorter than this, or with fewer words, don't teach it.
const MIN_AUDIO: Duration = Duration::from_millis(1000);
const MIN_WORDS: usize = 2;

pub(crate) fn db(level: f32) -> f32 {
    20.0 * level.max(1e-9).log10()
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct State {
    on: bool,
    /// The user's speech level, dBFS; `None` until a sentence teaches it.
    user_db: Option<f32>,
    /// The next sentence with words is the user's.
    trust_next: bool,
}

/// Shared by the recognizer's stage (which judges) and its live text.
#[derive(Debug, Clone)]
pub(crate) struct VoiceLevel(Arc<Mutex<State>>);

impl VoiceLevel {
    pub fn new(on: bool) -> Self {
        Self(Arc::new(Mutex::new(State { on, user_db: None, trust_next: false })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Settings changed: switching it on starts learning afresh.
    pub fn set_on(&self, on: bool) {
        let mut st = self.state();
        if st.on != on {
            *st = State { on, user_db: None, trust_next: false };
        }
    }

    /// The user just woke it by hand or started dictating.
    pub fn trust_next(&self) {
        self.state().trust_next = true;
    }

    /// Whether speech at `level` (RMS before gain) is someone else's, so
    /// far; `switch`: it holds a wake or sleep phrase. Learns nothing.
    pub fn is_far(&self, level: f32, switch: bool) -> bool {
        let st = self.state();
        st.on && !switch && !st.trust_next && st.user_db.is_some_and(|u| db(level) < u - FAR_BELOW_DB)
    }

    /// Judges a finished sentence, as [`Self::is_far`], and learns the
    /// user's level from it when it's theirs.
    pub fn judge(&self, level: f32, words: usize, audio: Duration, switch: bool) -> bool {
        let mut st = self.state();
        if !st.on || words == 0 {
            return false;
        }
        let here = db(level);
        let teaches = words >= MIN_WORDS && audio >= MIN_AUDIO && level > 0.0;
        if switch || std::mem::take(&mut st.trust_next) {
            if teaches || switch {
                st.user_db = Some(here);
            }
            return false;
        }
        let far = st.user_db.is_some_and(|u| here < u - FAR_BELOW_DB);
        if !far && teaches {
            st.user_db = Some(match st.user_db {
                None => here,
                Some(u) if here > u => u + RISE * (here - u),
                Some(u) => u + FALL * (here.max(u - MAX_FALL_DB) - u),
            });
        }
        far
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTENCE: Duration = Duration::from_millis(2500);

    fn rms(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    #[test]
    fn a_voice_far_below_the_users_is_someone_elses() {
        let v = VoiceLevel::new(true);
        for d in [-31.0, -29.0, -34.0, -32.0] {
            assert!(!v.judge(rms(d), 8, SENTENCE, false));
        }
        // Softer, but still the user.
        assert!(!v.judge(rms(-40.0), 6, SENTENCE, false));
        // The room, as the alpha tester's microphone heard it.
        for d in [-48.0, -55.0, -62.0, -75.0] {
            assert!(v.is_far(rms(d), false), "{d}");
            assert!(v.judge(rms(d), 8, SENTENCE, false), "{d}");
        }
        // Many far sentences don't drag the user's level down.
        for _ in 0..50 {
            v.judge(rms(-60.0), 10, SENTENCE, false);
        }
        assert!(!v.judge(rms(-32.0), 8, SENTENCE, false));
        assert!(v.judge(rms(-50.0), 8, SENTENCE, false));
    }

    #[test]
    fn the_nearest_voice_wins_and_a_quiet_start_is_outgrown() {
        // A video was the first thing heard after starting.
        let v = VoiceLevel::new(true);
        assert!(!v.judge(rms(-52.0), 8, SENTENCE, false));
        // Then the user speaks, much louder: theirs at once, and the level follows.
        for _ in 0..3 {
            assert!(!v.judge(rms(-30.0), 8, SENTENCE, false));
        }
        assert!(v.judge(rms(-52.0), 8, SENTENCE, false));
    }

    #[test]
    fn a_wake_phrase_or_the_hotkey_sets_the_level_afresh() {
        let v = VoiceLevel::new(true);
        v.judge(rms(-25.0), 8, SENTENCE, false);
        // The user moved away: "bear, wake up" is theirs, and so is what follows.
        assert!(!v.judge(rms(-45.0), 3, SENTENCE, true));
        assert!(!v.judge(rms(-46.0), 8, SENTENCE, false));
        // The hotkey: the next sentence is the user's, however quiet.
        v.judge(rms(-25.0), 8, SENTENCE, true);
        v.trust_next();
        assert!(!v.is_far(rms(-50.0), false));
        assert!(!v.judge(rms(-50.0), 8, SENTENCE, false));
        assert!(!v.judge(rms(-52.0), 8, SENTENCE, false));
        // Only once.
        assert!(v.judge(rms(-70.0), 8, SENTENCE, false));
    }

    #[test]
    fn short_blips_teach_nothing_and_off_judges_nothing() {
        let v = VoiceLevel::new(true);
        assert!(!v.judge(rms(-60.0), 1, SENTENCE, false));
        assert!(!v.judge(rms(-60.0), 5, Duration::from_millis(400), false));
        assert!(!v.is_far(rms(-90.0), false), "nothing learned yet");
        let off = VoiceLevel::new(false);
        off.judge(rms(-20.0), 8, SENTENCE, false);
        assert!(!off.judge(rms(-80.0), 8, SENTENCE, false));
        // Switching it on starts afresh.
        v.judge(rms(-20.0), 8, SENTENCE, false);
        v.set_on(false);
        v.set_on(true);
        assert!(!v.judge(rms(-80.0), 8, SENTENCE, false));
    }
}
