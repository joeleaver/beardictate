//! NVIDIA Nemotron Speech Streaming 0.6B English through sherpa-onnx's
//! online (streaming) transducer recognizer: text while the speaker talks.
//!
//! The 560 ms export: on 40 real microphone segments it got 39 complete and
//! 0.4% word errors with beam search, decodes at about a quarter of real time
//! on 8 CPU threads, and finishes 0.1 s after the speech does (eval notes,
//! 2026-10-18: 1120 ms was slower to show words and no more accurate on
//! that audio; 160 ms couldn't keep up on the CPU).
//!
//! Hotwords need beam search, which sherpa-onnx's NeMo streaming transducer
//! gained after 1.13.8 (sherpa-onnx#3895). Asking a library without it for
//! beam search fails, so it is behind the `nemotron-hotwords` feature,
//! for builds linked against a library that has it; otherwise decoding is
//! greedy and hotwords are ignored.

use std::path::Path;

use bd_core::{BoxError, SAMPLE_RATE, StreamingAsr};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig};

use crate::{AsrError, existing, hotwords};

/// Silence before an utterance and after it, as the model was evaluated:
/// 0.3 s ahead, 0.8 s behind (so the last chunk is complete).
const LEAD_SAMPLES: usize = SAMPLE_RATE as usize * 3 / 10;
const TAIL_SAMPLES: usize = SAMPLE_RATE as usize * 8 / 10;

/// Runtime options for [`NemotronAsr`].
#[derive(Debug, Clone)]
pub struct NemotronConfig {
    pub threads: usize,
    pub provider: String,
    /// Beam search with hotwords at this score per token (with the
    /// `nemotron-hotwords` feature; ignored without it).
    pub hotword_boost: Option<f32>,
}

impl Default for NemotronConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self { threads: cores.clamp(1, 8), provider: "cpu".into(), hotword_boost: None }
    }
}

/// The streaming recognizer, one utterance at a time.
pub struct NemotronAsr {
    recognizer: OnlineRecognizer,
    stream: Option<OnlineStream>,
    /// Encoded hotwords for the next utterance; `None` when greedy.
    hotwords: Option<String>,
    boost: f32,
    last: String,
}

impl NemotronAsr {
    /// Loads the model from `model_dir` (`encoder`, `decoder`, `joiner`
    /// `.int8.onnx` and `tokens.txt`, as in sherpa-onnx's
    /// `sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8` release).
    pub fn new(model_dir: &Path, config: &NemotronConfig) -> Result<Self, AsrError> {
        let mut c = OnlineRecognizerConfig::default();
        c.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(existing(&model_dir.join("encoder.int8.onnx"))?),
            decoder: Some(existing(&model_dir.join("decoder.int8.onnx"))?),
            joiner: Some(existing(&model_dir.join("joiner.int8.onnx"))?),
        };
        let tokens = model_dir.join("tokens.txt");
        c.model_config.tokens = Some(existing(&tokens)?);
        c.model_config.num_threads = i32::try_from(config.threads.max(1))
            .map_err(|_| AsrError::InvalidConfig("too many threads".into()))?;
        c.model_config.provider = Some(config.provider.clone());
        c.decoding_method = Some("greedy_search".into());
        let beam = cfg!(feature = "nemotron-hotwords").then_some(config.hotword_boost).flatten();
        if let Some(boost) = beam.filter(|b| b.is_finite() && *b > 0.0) {
            c.decoding_method = Some("modified_beam_search".into());
            c.max_active_paths = 4;
            c.blank_penalty = 1.0;
            c.hotwords_score = boost;
            c.model_config.modeling_unit = Some("bpe".into());
            c.model_config.bpe_vocab = Some(hotwords::bpe_vocab(&tokens)?);
        } else if config.hotword_boost.is_some() {
            tracing::info!("Nemotron decodes greedily: this build has no beam search for it (special words aren't boosted)");
        }
        let greedy = c.decoding_method.as_deref() == Some("greedy_search");
        let recognizer = OnlineRecognizer::create(&c).ok_or(AsrError::CreateFailed("Nemotron recognizer"))?;
        Ok(Self { recognizer, stream: None, hotwords: (!greedy).then(String::new), boost: config.hotword_boost.unwrap_or(0.0), last: String::new() })
    }

    fn decode(&mut self) -> Option<String> {
        let stream = self.stream.as_ref()?;
        while self.recognizer.is_ready(stream) {
            self.recognizer.decode(stream);
        }
        let text = self.recognizer.get_result(stream).map(|r| r.text.trim().to_string()).unwrap_or_default();
        (text != self.last).then(|| {
            self.last.clone_from(&text);
            text
        })
    }
}

impl StreamingAsr for NemotronAsr {
    fn start(&mut self) -> Result<(), BoxError> {
        let stream = match self.hotwords.as_deref().filter(|h| !h.is_empty()) {
            Some(h) => self.recognizer.create_stream_with_hotwords(h),
            None => self.recognizer.create_stream(),
        };
        stream.accept_waveform(SAMPLE_RATE as i32, &vec![0.0; LEAD_SAMPLES]);
        self.stream = Some(stream);
        self.last.clear();
        Ok(())
    }

    fn feed(&mut self, samples: &[f32]) -> Result<Option<String>, BoxError> {
        let Some(stream) = &self.stream else { return Ok(None) };
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        Ok(self.decode())
    }

    fn finish(&mut self) -> Result<String, BoxError> {
        let Some(stream) = &self.stream else { return Ok(String::new()) };
        stream.accept_waveform(SAMPLE_RATE as i32, &vec![0.0; TAIL_SAMPLES]);
        stream.input_finished();
        self.decode();
        self.stream = None;
        Ok(std::mem::take(&mut self.last))
    }

    fn set_hotwords(&mut self, words: &[String], boost: f32) -> Result<(), BoxError> {
        if let Some(h) = self.hotwords.as_mut() {
            self.boost = boost;
            *h = if boost > 0.0 { hotwords::encode(words, boost) } else { String::new() };
        }
        Ok(())
    }
}
