//! Silero VAD segmentation against the real model, checked end to end with
//! Parakeet. Skips unless `BD_MODELS_DIR` is set.

mod common;

use std::path::Path;
use std::time::Duration;

use bd_asr::{AsrError, ParakeetAsr, ParakeetConfig, Segmenter, SegmenterConfig};
use bd_core::{AudioSegment, SAMPLE_RATE};
use common::*;

const EN_TEXT: &str =
    "Ask not what your country can do for you, ask what you can do for your country.";

/// Feeds `audio` in odd-sized chunks, as capture would, then flushes.
/// Returns (segments emitted while streaming, segments from the flush).
fn run(seg: &mut Segmenter, audio: &[f32]) -> (Vec<AudioSegment>, Vec<AudioSegment>) {
    let mut live = Vec::new();
    for chunk in audio.chunks(480) {
        live.extend(seg.accept(chunk));
    }
    (live, seg.flush())
}

fn secs(n: usize) -> f32 {
    n as f32 / SAMPLE_RATE as f32
}

#[test]
fn gap_splits_two_sentences() {
    let Some(models) = models_dir() else { return };
    let en = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    let mut audio = silence(0.5);
    audio.extend(&en);
    audio.extend(silence(1.5));
    audio.extend(&en);
    audio.extend(silence(1.0));

    let mut seg = Segmenter::new(&vad_model(&models), SegmenterConfig::default()).unwrap();
    let (live, flushed) = run(&mut seg, &audio);
    let all: Vec<_> = live.iter().chain(&flushed).collect();
    assert_eq!(
        all.len(),
        2,
        "lengths {:?}",
        all.iter()
            .map(|s| secs(s.samples.len()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        live.len(),
        2,
        "both end in >600 ms of silence, so neither waits for flush"
    );
    assert_eq!((all[0].seq, all[1].seq), (0, 1));

    let asr = ParakeetAsr::new(&parakeet_dir(&models), &ParakeetConfig::default()).unwrap();
    for s in all {
        let text = asr.transcribe_samples(&s.samples).unwrap();
        assert_eq!(wer(EN_TEXT, &text), 0.0, "segment {}: {text:?}", s.seq);
    }
}

#[test]
fn short_pause_does_not_split() {
    let Some(models) = models_dir() else { return };
    let en = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    let mut audio = en.clone();
    audio.extend(silence(0.3));
    audio.extend(&en);
    audio.extend(silence(1.0));
    let mut seg = Segmenter::new(&vad_model(&models), SegmenterConfig::default()).unwrap();
    let (live, flushed) = run(&mut seg, &audio);
    assert_eq!(live.len() + flushed.len(), 1);
}

#[test]
fn flush_returns_speech_in_progress_and_restarts_numbering() {
    let Some(models) = models_dir() else { return };
    let en = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    let mut seg = Segmenter::new(&vad_model(&models), SegmenterConfig::default()).unwrap();
    // The user hits stop mid-sentence: no trailing silence at all.
    let cut = en.len() * 3 / 4;
    let mut live = Vec::new();
    for chunk in en[..cut].chunks(512) {
        live.extend(seg.accept(chunk));
    }
    assert!(live.is_empty());
    let flushed = seg.flush();
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].seq, 0);
    // Nothing of what was said is lost at the end.
    let tail = &en[cut - 1600..cut];
    assert!(flushed[0].samples.ends_with(tail));

    // A new session starts from seq 0 again.
    let (live, flushed) = run(&mut seg, &en);
    let all: Vec<_> = live.into_iter().chain(flushed).collect();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].seq, 0);
    assert!(all[0].start_sample < en.len() as u64, "offsets restart too");
}

#[test]
fn silence_yields_nothing() {
    let Some(models) = models_dir() else { return };
    let mut seg = Segmenter::new(&vad_model(&models), SegmenterConfig::default()).unwrap();
    let (live, flushed) = run(&mut seg, &silence(5.0));
    assert!(live.is_empty() && flushed.is_empty());
}

#[test]
fn long_run_on_is_cut_without_losing_audio() {
    let Some(models) = models_dir() else { return };
    let en = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    // ~12 s of speech with pauses too short to close an utterance.
    let mut audio = silence(0.5);
    let speech_start = audio.len();
    for _ in 0..3 {
        audio.extend(&en);
        audio.extend(silence(0.25));
    }
    let speech_end = audio.len();
    audio.extend(silence(1.0));
    // Faint deterministic noise makes every stretch of the input unique, so
    // segments can be located in it exactly.
    let mut lcg = 0x2545_f491_u32;
    for s in &mut audio {
        lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s += (lcg >> 8) as f32 / (1u32 << 24) as f32 * 2e-4 - 1e-4;
    }

    let max = Duration::from_secs(5);
    let config = SegmenterConfig {
        max_segment: max,
        ..SegmenterConfig::default()
    };
    let mut seg = Segmenter::new(&vad_model(&models), config).unwrap();
    let (live, flushed) = run(&mut seg, &audio);
    let all: Vec<_> = live.into_iter().chain(flushed).collect();
    let lens: Vec<f32> = all.iter().map(|s| secs(s.samples.len())).collect();
    assert!(all.len() >= 3, "expected forced cuts, got {lens:?}");
    for (i, s) in all.iter().enumerate() {
        assert_eq!(s.seq, i as u64);
        assert!(
            s.samples.len() <= SAMPLE_RATE as usize * 5,
            "segment too long: {lens:?}"
        );
    }
    // Forced cuts are back to back: each segment is an exact slice of the
    // input starting where the previous one ended, and together they cover
    // all the speech.
    let locate = |seg: &[f32]| {
        let at = audio
            .windows(64)
            .position(|w| w == &seg[..64])
            .expect("segment comes from the input");
        assert_eq!(
            &audio[at..at + seg.len()],
            seg,
            "segment is not a contiguous slice"
        );
        (at, at + seg.len())
    };
    let spans: Vec<(usize, usize)> = all.iter().map(|s| locate(&s.samples)).collect();
    for (s, span) in all.iter().zip(&spans) {
        assert_eq!(s.start_sample, span.0 as u64, "start_sample is off");
    }
    eprintln!("spans {spans:?}, speech {speech_start}..{speech_end}");
    for pair in spans.windows(2) {
        assert_eq!(
            pair[0].1, pair[1].0,
            "gap or overlap between cuts: {spans:?}"
        );
    }
    assert!(spans[0].0 <= speech_start);
    assert!(spans[spans.len() - 1].1 >= speech_end - SAMPLE_RATE as usize / 4);

    // Cutting at the quietest point keeps words intact.
    let asr = ParakeetAsr::new(&parakeet_dir(&models), &ParakeetConfig::default()).unwrap();
    let text: Vec<String> = all
        .iter()
        .map(|s| asr.transcribe_samples(&s.samples).unwrap())
        .collect();
    let reference = [EN_TEXT; 3].join(" ");
    let w = wer(&reference, &text.join(" "));
    eprintln!("cuts at {lens:?} -> {text:?} (WER {w:.3})");
    assert!(w <= 0.1, "WER {w}");
}

#[test]
fn bad_config_and_missing_model_are_errors() {
    let err = Segmenter::new(
        Path::new("/nonexistent/silero_vad.onnx"),
        SegmenterConfig::default(),
    )
    .err()
    .expect("should fail");
    assert!(matches!(err, AsrError::MissingModelFile(_)), "{err}");
    let Some(models) = models_dir() else { return };
    let config = SegmenterConfig {
        max_segment: Duration::from_millis(500),
        ..Default::default()
    };
    assert!(matches!(
        Segmenter::new(&vad_model(&models), config).err(),
        Some(AsrError::InvalidConfig(_))
    ));
}
