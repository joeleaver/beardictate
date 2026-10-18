//! Speech recognition for beardictate, via sherpa-onnx.
//!
//! - [`ParakeetAsr`]: NVIDIA Parakeet Unified 0.6B English (int8 ONNX) behind
//!   [`bd_core::Asr`]. Transcribes one finished segment at a time, with the
//!   model's own casing and punctuation, optionally listening harder for a
//!   list of names and invented words (hotwords).
//! - [`Segmenter`]: Silero VAD that cuts a stream of 16 kHz chunks into
//!   utterance-sized [`bd_core::AudioSegment`]s.
//!
//! Both take model paths directly, so this crate doesn't depend on how the
//! models were downloaded.

mod hotwords;
mod nemotron;
mod parakeet;
mod segmenter;

pub use nemotron::{NemotronAsr, NemotronConfig};
pub use parakeet::{ParakeetAsr, ParakeetConfig};
pub use segmenter::{Segmenter, SegmenterConfig};

use std::path::{Path, PathBuf};

/// ASR and VAD errors.
#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("model file not found: {}", .0.display())]
    MissingModelFile(PathBuf),
    #[error("model path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
    /// sherpa-onnx returned null; the reason is in its log output (usually a
    /// bad model file or an unknown provider).
    #[error("sherpa-onnx could not create the {0}")]
    CreateFailed(&'static str),
    #[error("sherpa-onnx returned no recognition result")]
    NoResult,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

/// `path` as a UTF-8 string for sherpa-onnx, after checking the file exists
/// (sherpa-onnx itself only reports a null handle).
pub(crate) fn existing(path: &Path) -> Result<String, AsrError> {
    if !path.is_file() {
        return Err(AsrError::MissingModelFile(path.to_owned()));
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| AsrError::NonUtf8Path(path.to_owned()))
}
