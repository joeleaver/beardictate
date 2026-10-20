//! For diagnosing what the recognizer hears: with `BD_SAVE_SEGMENTS` set to a
//! folder (or one given to [`save_to`]), each segment's audio is saved there exactly as the ASR gets it
//! (after the microphone gain), with what was heard and typed listed in
//! `segments.tsv`. Off unless set; it records the user's words, so it is
//! never on by default.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use bd_core::SAMPLE_RATE;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Something else that wants each segment's audio (alpha telemetry).
type Sink = Box<dyn Fn(u64, &[f32]) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();

/// Hands every segment's audio, as the ASR gets it, to `sink` from now on.
pub fn audio_to(sink: Sink) {
    let _ = SINK.set(sink);
}

/// Saves segments to `dir` from now on, when `BD_SAVE_SEGMENTS` isn't set.
pub fn save_to(dir: PathBuf) {
    let _ = DIR.set(dir);
}

fn dir() -> Option<PathBuf> {
    std::env::var_os("BD_SAVE_SEGMENTS").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| DIR.get().cloned())
}

fn stamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Saves segment `id`'s audio as `<time>-<id>.wav` and lists what the ASR
/// made of it; `share`: hands it to the sink too.
pub fn segment(id: u64, samples: &[f32], heard: &str, share: bool) {
    if share && let Some(sink) = SINK.get() {
        sink(id, samples);
    }
    let Some(dir) = dir() else { return };
    let name = format!("{}-{id:04}.wav", stamp());
    let result = fs::create_dir_all(&dir)
        .and_then(|()| fs::write(dir.join(&name), wav(samples)))
        .and_then(|()| line(&dir, &format!("{name}\theard\t{heard:?}")));
    if let Err(e) = result {
        tracing::warn!("saving segment {id}: {e}");
    }
}

/// Lists what was typed for segment `id`.
pub fn typed(id: u64, text: &str) {
    if let Some(dir) = dir()
        && let Err(e) = line(&dir, &format!("{}-{id:04}\ttyped\t{text:?}", stamp()))
    {
        tracing::warn!("noting segment {id}: {e}");
    }
}

fn line(dir: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(dir.join("segments.tsv"))?;
    writeln!(f, "{text}")
}

/// 16-bit mono PCM at the pipeline's rate.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}
