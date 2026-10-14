//! Parakeet against the real model. Skips unless `BD_MODELS_DIR` is set.

mod common;

use std::path::Path;
use std::time::Instant;

use bd_asr::{AsrError, ParakeetAsr, ParakeetConfig};
use bd_core::{Asr, AudioSegment, SAMPLE_RATE};
use common::*;

/// What `test_wavs/en.wav` in the sherpa-onnx release says.
const EN_TEXT: &str =
    "Ask not what your country can do for you, ask what you can do for your country.";

#[test]
fn transcribes_known_sentence() {
    let Some(models) = models_dir() else { return };
    let mut asr = ParakeetAsr::new(&parakeet_dir(&models), &ParakeetConfig::default()).unwrap();
    // en.wav is 24 kHz, so this also exercises bd-audio's resampler.
    let samples = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    let text = asr.transcribe(&AudioSegment { samples, seq: 0 }).unwrap();
    eprintln!("transcript: {text}");
    assert_eq!(wer(EN_TEXT, &text), 0.0, "got {text:?}");
    // Native punctuation and casing survive.
    assert!(
        text.starts_with("Ask") && text.ends_with('.'),
        "got {text:?}"
    );
}

#[test]
fn empty_and_silent_segments_give_empty_text() {
    let Some(models) = models_dir() else { return };
    let mut asr = ParakeetAsr::new(&parakeet_dir(&models), &ParakeetConfig::default()).unwrap();
    assert_eq!(asr.transcribe(&AudioSegment::default()).unwrap(), "");
    let quiet = AudioSegment {
        samples: silence(1.0),
        seq: 0,
    };
    assert_eq!(asr.transcribe(&quiet).unwrap(), "");
}

#[test]
fn missing_model_is_a_clear_error() {
    let err = ParakeetAsr::new(
        Path::new("/nonexistent/parakeet"),
        &ParakeetConfig::default(),
    )
    .err()
    .expect("should fail");
    assert!(matches!(err, AsrError::MissingModelFile(_)), "{err}");
}

/// Real-time factor on CPU. Run with
/// `cargo test --release -p bd-asr --test parakeet -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn real_time_factor() {
    let Some(models) = models_dir() else { return };
    let en = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    // ~4 s and ~20 s segments: the short one is the typical dictated sentence.
    let mut long = Vec::new();
    for _ in 0..5 {
        long.extend_from_slice(&en);
        long.extend(silence(0.3));
    }
    for threads in [4, 8] {
        let config = ParakeetConfig {
            threads,
            ..ParakeetConfig::default()
        };
        let asr = ParakeetAsr::new(&parakeet_dir(&models), &config).unwrap();
        asr.transcribe_samples(&en).unwrap(); // warm-up
        for (name, clip) in [("4 s", &en), ("20 s", &long)] {
            let runs = 3;
            let t = Instant::now();
            for _ in 0..runs {
                asr.transcribe_samples(clip).unwrap();
            }
            let secs = t.elapsed().as_secs_f32() / runs as f32;
            let audio = clip.len() as f32 / SAMPLE_RATE as f32;
            eprintln!(
                "threads {threads}, {name} clip: {secs:.3} s per decode, RTF {:.3}",
                secs / audio
            );
        }
    }
}
