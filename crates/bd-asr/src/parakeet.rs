//! Parakeet TDT 0.6B v3 through sherpa-onnx's offline transducer recognizer.

use std::path::Path;

use bd_core::{Asr, AudioSegment, BoxError, SAMPLE_RATE};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

use crate::{AsrError, existing};

/// Runtime options for [`ParakeetAsr`].
#[derive(Debug, Clone)]
pub struct ParakeetConfig {
    /// ONNX Runtime intra-op threads.
    pub threads: usize,
    /// ONNX Runtime execution provider: `"cpu"`, or e.g. `"cuda"` /
    /// `"coreml"` when the linked sherpa-onnx build supports it.
    pub provider: String,
    /// Let sherpa-onnx print model metadata and timing to stderr.
    pub debug: bool,
}

impl Default for ParakeetConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self {
            threads: cores.clamp(1, 8),
            provider: "cpu".into(),
            debug: false,
        }
    }
}

/// Parakeet TDT transducer ASR.
///
/// The model directory holds `encoder`, `decoder` and `joiner` ONNX files
/// (int8 `*.int8.onnx` preferred, plain `*.onnx` accepted) and `tokens.txt`,
/// as in the `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` release.
pub struct ParakeetAsr {
    recognizer: OfflineRecognizer,
}

impl ParakeetAsr {
    /// Loads the model from `model_dir`. Takes a few seconds: the encoder is
    /// about 650 MB.
    pub fn new(model_dir: &Path, config: &ParakeetConfig) -> Result<Self, AsrError> {
        if config.threads == 0 {
            return Err(AsrError::InvalidConfig("threads must be at least 1".into()));
        }
        let mut c = OfflineRecognizerConfig::default();
        c.feat_config.sample_rate = SAMPLE_RATE as i32;
        c.feat_config.feature_dim = 80;
        c.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(onnx_file(model_dir, "encoder")?),
            decoder: Some(onnx_file(model_dir, "decoder")?),
            joiner: Some(onnx_file(model_dir, "joiner")?),
        };
        c.model_config.tokens = Some(existing(&model_dir.join("tokens.txt"))?);
        c.model_config.model_type = Some("nemo_transducer".into());
        c.model_config.num_threads = i32::try_from(config.threads)
            .map_err(|_| AsrError::InvalidConfig("too many threads".into()))?;
        c.model_config.provider = Some(config.provider.clone());
        c.model_config.debug = config.debug;
        c.decoding_method = Some("greedy_search".into());
        let recognizer =
            OfflineRecognizer::create(&c).ok_or(AsrError::CreateFailed("Parakeet recognizer"))?;
        Ok(Self { recognizer })
    }

    /// Transcribes 16 kHz mono samples. Returns an empty string for empty
    /// input or when nothing was recognized.
    pub fn transcribe_samples(&self, samples: &[f32]) -> Result<String, AsrError> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.recognizer.decode(&stream);
        let result = stream.get_result().ok_or(AsrError::NoResult)?;
        Ok(result.text.trim().to_owned())
    }
}

impl Asr for ParakeetAsr {
    fn transcribe(&self, segment: &AudioSegment) -> Result<String, BoxError> {
        Ok(self.transcribe_samples(&segment.samples)?)
    }
}

/// `<dir>/<stem>.int8.onnx` if present, else `<dir>/<stem>.onnx`.
fn onnx_file(dir: &Path, stem: &str) -> Result<String, AsrError> {
    let int8 = dir.join(format!("{stem}.int8.onnx"));
    if int8.is_file() {
        return existing(&int8);
    }
    existing(&dir.join(format!("{stem}.onnx")))
}
