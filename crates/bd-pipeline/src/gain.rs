//! Microphone gain before the VAD, learned the way a call app does it: only
//! from sentences the ASR actually turned into words, nudged a little each
//! time, and held through silence, breaths and room noise. A quiet
//! microphone otherwise loses soft word onsets in the VAD ("Mara" heard as
//! "I") and gets fillers guessed for whole sentences.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The speech level gain aims for (RMS about -23 dBFS).
pub const TARGET_RMS: f32 = 0.07;
/// The most gain: 30 dB.
pub const MAX_GAIN: f32 = 31.6;
/// How far one sentence moves the gain towards what it wants.
const STEP: f32 = 0.3;
/// Sentences shorter than this, or with fewer words, don't teach it.
const MIN_AUDIO: Duration = Duration::from_millis(1000);
const MIN_WORDS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
struct State {
    gain: f32,
    auto: bool,
}

/// Shared between the stage that applies the gain and the one that learns it.
#[derive(Debug, Clone)]
pub(crate) struct InputGain(Arc<Mutex<State>>);

impl InputGain {
    pub fn new(gain: f32, auto: bool) -> Self {
        Self(Arc::new(Mutex::new(State { gain: gain.clamp(1.0, MAX_GAIN), auto })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new starting gain and mode (settings changed).
    pub fn set(&self, gain: f32, auto: bool) {
        *self.state() = State { gain: gain.clamp(1.0, MAX_GAIN), auto };
    }

    /// Raises `samples` by the current gain; returns the gain used.
    pub fn apply(&self, samples: &mut [f32]) -> f32 {
        let gain = self.state().gain;
        if gain > 1.0 {
            for s in samples.iter_mut() {
                *s = (*s * gain).clamp(-1.0, 1.0);
            }
        }
        gain
    }

    /// Learns from a recognized sentence that came in at `level` (RMS before
    /// any gain). Returns the gain now in use.
    pub fn learn(&self, level: f32, words: usize, audio: Duration) -> f32 {
        let mut st = self.state();
        if st.auto && words >= MIN_WORDS && audio >= MIN_AUDIO && level > 0.0 {
            let wanted = (TARGET_RMS / level).clamp(1.0, MAX_GAIN);
            st.gain += STEP * (wanted - st.gain);
        }
        st.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTENCE: Duration = Duration::from_millis(2500);

    #[test]
    fn learns_from_speech_only_and_holds() {
        let g = InputGain::new(1.0, true);
        // Quiet speech (-48 dBFS): a few sentences bring the gain most of the way.
        for _ in 0..6 {
            g.learn(0.004, 8, SENTENCE);
        }
        let learned = g.learn(0.004, 0, SENTENCE);
        assert!(learned > 15.0 && learned <= MAX_GAIN, "{learned}");
        // Noise, a one-word burst and a short blip teach nothing.
        assert_eq!(g.learn(0.0001, 0, SENTENCE), learned);
        assert_eq!(g.learn(0.0001, 1, SENTENCE), learned);
        assert_eq!(g.learn(0.0001, 5, Duration::from_millis(400)), learned);
        // The gain is applied to what comes in.
        let mut chunk = [0.004, -0.004];
        assert_eq!(g.apply(&mut chunk), learned);
        assert!((chunk[0] - 0.004 * learned).abs() < 1e-6);
    }

    #[test]
    fn manual_gain_stays_put() {
        let g = InputGain::new(4.0, false);
        assert_eq!(g.learn(0.004, 8, SENTENCE), 4.0);
        g.set(100.0, false);
        assert_eq!(g.learn(0.1, 8, SENTENCE), MAX_GAIN, "clamped");
        // Loud speech never turns it below unity.
        let g = InputGain::new(1.0, true);
        assert_eq!(g.learn(0.5, 8, SENTENCE), 1.0);
    }
}
