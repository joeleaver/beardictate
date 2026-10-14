//! Shared helpers for the model-backed integration tests.
//!
//! Tests need real models: set `BD_MODELS_DIR` to a directory containing
//! `asr-parakeet-tdt-0.6b-v3-int8/` (the sherpa-onnx release, including its
//! `test_wavs/`) and `vad-silero/silero_vad.onnx`. Without it they skip.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use bd_core::SAMPLE_RATE;

/// The models directory, or `None` (after saying why) to skip the test.
pub fn models_dir() -> Option<PathBuf> {
    match std::env::var_os("BD_MODELS_DIR") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("skipping: BD_MODELS_DIR is not set");
            None
        }
    }
}

pub fn parakeet_dir(models: &Path) -> PathBuf {
    models.join("asr-parakeet-tdt-0.6b-v3-int8")
}

pub fn vad_model(models: &Path) -> PathBuf {
    models.join("vad-silero").join("silero_vad.onnx")
}

/// Reads a mono or multichannel WAV as 16 kHz mono f32.
pub fn read_wav_16k(path: &Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let spec = r.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.unwrap() as f32 * scale)
                .collect()
        }
    };
    let mut mono = Vec::new();
    bd_audio::downmix_into(&interleaved, usize::from(spec.channels), &mut mono);
    bd_audio::resample_to_16k(&mono, spec.sample_rate).unwrap()
}

pub fn silence(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * SAMPLE_RATE as f32) as usize]
}

/// Lowercase words with punctuation stripped, for WER.
pub fn normalize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect::<String>()
        })
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word error rate of `hyp` against `reference`, after normalization.
pub fn wer(reference: &str, hyp: &str) -> f32 {
    let r = normalize(reference);
    let h = normalize(hyp);
    // Levenshtein distance over words.
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for (i, rw) in r.iter().enumerate() {
        let mut cur = vec![i + 1; h.len() + 1];
        for (j, hw) in h.iter().enumerate() {
            let sub = prev[j] + usize::from(rw != hw);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[h.len()] as f32 / r.len().max(1) as f32
}
