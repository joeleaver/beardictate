//! A breath, a click or room noise that the VAD let through comes back from
//! the ASR as a lone filler word: Parakeet turned half a second of noise into
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
}
