//! A breath, a click or room noise that the VAD let through comes back from
//! the ASR as a lone filler word: Parakeet turns half a second of noise into
//! "Yeah." or "Uh". Such a segment is dropped as noise. A real one-word
//! reply survives when it's longer than a burst, and dialog almost always
//! comes with more words ("Yeah," he said.) in the same segment.

use std::time::Duration;

/// Words the ASR makes out of noise.
const FILLERS: &[&str] = &["yeah", "uh", "um", "mm", "hmm", "mhm", "mm-hmm", "er", "ah"];

/// A segment this short holding only a filler is noise.
const BURST: Duration = Duration::from_millis(900);

/// Whether `text`, transcribed from `audio` of speech, is the ASR's guess at
/// noise rather than something said.
pub(super) fn is_noise(text: &str, audio: Duration) -> bool {
    let word: String = text.trim().trim_end_matches(['.', ',', '!', '?', '\u{2026}']).to_lowercase();
    audio < BURST && FILLERS.contains(&word.as_str())
}

/// Root-mean-square of `samples`, 0 to 1: how loud a segment was.
pub(super) fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// The RMS speech is brought up to before the ASR (about -23 dBFS). A
/// quiet microphone (Joe's came in near -48 dBFS) makes Parakeet drop words
/// and guess fillers; at a normal level it hears them.
const TARGET_RMS: f32 = 0.07;
/// At most this much boost (30 dB), so near-silence isn't blown up.
const MAX_GAIN: f32 = 31.6;

/// `samples` brought up towards a normal speech level, never turned down.
pub(super) fn level_up(samples: &mut [f32]) {
    let level = rms(samples);
    if level <= 0.0 || level >= TARGET_RMS {
        return;
    }
    let gain = (TARGET_RMS / level).min(MAX_GAIN);
    for s in samples {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_filler_from_a_short_burst_is_noise() {
        let short = Duration::from_millis(600);
        assert!(is_noise("Yeah.", short));
        assert!(is_noise("Uh", short));
        assert!(is_noise(" mm-hmm ", short));
        // Longer, it was probably said on purpose.
        assert!(!is_noise("Yeah.", Duration::from_millis(1200)));
        // Anything more than a filler is speech.
        assert!(!is_noise("Yes.", short));
        assert!(!is_noise("Yeah, he said.", short));
        assert!(!is_noise("No.", short));
    }

    #[test]
    fn quiet_speech_is_brought_up_and_loud_left_alone() {
        let mut quiet = vec![0.004, -0.004, 0.004, -0.004];
        level_up(&mut quiet);
        assert!((rms(&quiet) - TARGET_RMS).abs() < 1e-3);
        let mut hiss = vec![0.0001, -0.0001];
        level_up(&mut hiss);
        assert!((rms(&hiss) - 0.0001 * MAX_GAIN).abs() < 1e-5, "capped");
        let mut loud = vec![0.3, -0.3];
        level_up(&mut loud);
        assert_eq!(loud, [0.3, -0.3]);
    }
}
