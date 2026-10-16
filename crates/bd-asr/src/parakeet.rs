//! NVIDIA Parakeet through sherpa-onnx's offline transducer recognizer:
//! Parakeet Unified 0.6B English (an RNNT), or the older Parakeet TDT 0.6B v3.

use std::path::Path;
use std::sync::RwLock;

use bd_core::{Asr, AudioSegment, BoxError, SAMPLE_RATE};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

use crate::{AsrError, existing, hotwords};

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
    /// Enables [`ParakeetAsr::set_hotwords`]: decode with beam search and
    /// add this score per matching token to hypotheses that spell a hotword.
    /// `None` decodes greedily and takes no hotwords. With Parakeet Unified,
    /// 22 special words in 16 TTS sentences: 2.0 got 16 (greedy: 8, 1.5: 11)
    /// without changing ordinary words, though it sometimes repeats a
    /// hotword, which the output drops (see `hotwords::collapse_repeats`).
    /// With Parakeet v3 on 22 invented names, 2.0 got 10 (greedy: 4); 2.5–3
    /// got 12–15 but began changing words ("whispered" to "whisper").
    pub hotword_boost: Option<f32>,
}

impl Default for ParakeetConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self {
            threads: cores.clamp(1, 8),
            provider: "cpu".into(),
            debug: false,
            hotword_boost: None,
        }
    }
}

/// The hotword list and its boost, and the two encoded for sherpa-onnx.
struct Hotwords {
    words: Vec<String>,
    boost: f32,
    encoded: String,
}

/// Parakeet transducer ASR.
///
/// The model directory holds `encoder`, `decoder` and `joiner` ONNX files
/// (int8 `*.int8.onnx` preferred, plain `*.onnx` accepted) and `tokens.txt`,
/// as in sherpa-onnx's `sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-non-streaming`
/// and `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` releases.
pub struct ParakeetAsr {
    recognizer: OfflineRecognizer,
    /// The hotwords; `None` when boosting is off.
    hotwords: Option<RwLock<Hotwords>>,
}

/// Score taken from blank in beam search; see [`ParakeetAsr::new`].
const BLANK_PENALTY: f32 = 1.0;

/// Silence added before and after each segment: 0.3 s.
const PAD_SAMPLES: usize = SAMPLE_RATE as usize * 3 / 10;

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
        let tokens = model_dir.join("tokens.txt");
        c.model_config.tokens = Some(existing(&tokens)?);
        c.model_config.model_type = Some("nemo_transducer".into());
        c.model_config.num_threads = i32::try_from(config.threads)
            .map_err(|_| AsrError::InvalidConfig("too many threads".into()))?;
        c.model_config.provider = Some(config.provider.clone());
        c.model_config.debug = config.debug;
        c.decoding_method = Some("greedy_search".into());
        let hotwords = match config.hotword_boost {
            None => None,
            Some(boost) if boost.is_finite() && boost > 0.0 => {
                c.decoding_method = Some("modified_beam_search".into());
                c.max_active_paths = 4;
                // sherpa-onnx's beam search for TDT models prefers to stop:
                // after a sentence's full stop it emitted nothing for the
                // rest of the segment (sherpa-onnx#3267). Charging for each
                // blank evens that out: on 23 segments from a real
                // microphone, 1.0 recovered every word greedy decoding got
                // (0.5 nearly all; 2.0 began inventing words). Parakeet
                // Unified also does better with it: 39 of 40 real segments
                // complete, against 38 with no penalty.
                c.blank_penalty = BLANK_PENALTY;
                c.hotwords_score = boost;
                c.model_config.modeling_unit = Some("bpe".into());
                c.model_config.bpe_vocab = Some(hotwords::bpe_vocab(&tokens)?);
                Some(RwLock::new(Hotwords { words: Vec::new(), boost, encoded: String::new() }))
            }
            Some(boost) => return Err(AsrError::InvalidConfig(format!("hotword boost must be positive, got {boost}"))),
        };
        let recognizer =
            OfflineRecognizer::create(&c).ok_or(AsrError::CreateFailed("Parakeet recognizer"))?;
        Ok(Self { recognizer, hotwords })
    }

    /// Replaces the words and phrases to listen for (names, invented words).
    /// Takes effect from the next segment. Needs
    /// [`ParakeetConfig::hotword_boost`].
    pub fn set_hotwords<S: AsRef<str>>(&self, words: &[S]) -> Result<(), AsrError> {
        self.update(|h| h.words = words.iter().map(|w| w.as_ref().to_string()).collect())
    }

    /// Changes the boost from [`ParakeetConfig::hotword_boost`], from the next
    /// segment. 0 stops listening for the hotwords (decoding stays beam search).
    pub fn set_hotword_boost(&self, boost: f32) -> Result<(), AsrError> {
        if !boost.is_finite() || boost < 0.0 {
            return Err(AsrError::InvalidConfig(format!("hotword boost must be 0 or more, got {boost}")));
        }
        self.update(|h| h.boost = boost)
    }

    fn update(&self, change: impl FnOnce(&mut Hotwords)) -> Result<(), AsrError> {
        let Some(lock) = &self.hotwords else {
            return Err(AsrError::InvalidConfig("hotwords need ParakeetConfig::hotword_boost".into()));
        };
        let mut h = lock.write().unwrap_or_else(|e| e.into_inner());
        change(&mut h);
        h.encoded = hotwords::encode(&h.words, h.boost);
        Ok(())
    }

    /// Transcribes 16 kHz mono samples. Returns an empty string for empty
    /// input or when nothing was recognized.
    pub fn transcribe_samples(&self, samples: &[f32]) -> Result<String, AsrError> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let hotwords = self.hotwords.as_ref().map(|l| l.read().unwrap_or_else(|e| e.into_inner()));
        let hotwords = hotwords.as_ref().filter(|h| !h.encoded.is_empty());
        let stream = match hotwords {
            Some(h) => self.recognizer.create_stream_with_hotwords(&h.encoded),
            None => self.recognizer.create_stream(),
        };
        // Parakeet loses words in audio cut tight to the speech, as the VAD
        // cuts it: a short "new paragraph" came out as "Yeah." or nothing,
        // and beam search dropped whole sentences. Silence either side fixed
        // all 20 of a real microphone's segments that greedy decoding got wrong.
        let pad = vec![0.0; PAD_SAMPLES];
        let padded = [pad.as_slice(), samples, pad.as_slice()].concat();
        stream.accept_waveform(SAMPLE_RATE as i32, &padded);
        self.recognizer.decode(&stream);
        let result = stream.get_result().ok_or(AsrError::NoResult)?;
        let text = result.text.trim();
        Ok(match hotwords {
            Some(h) => hotwords::collapse_repeats(text, &h.words),
            None => text.to_owned(),
        })
    }
}

impl Asr for ParakeetAsr {
    fn transcribe(&self, segment: &AudioSegment) -> Result<String, BoxError> {
        Ok(self.transcribe_samples(&segment.samples)?)
    }

    /// Ignored when boosting is off (no [`ParakeetConfig::hotword_boost`]).
    fn set_hotwords(&self, words: &[String], boost: f32) -> Result<(), BoxError> {
        if self.hotwords.is_some() {
            ParakeetAsr::set_hotwords(self, words)?;
            self.set_hotword_boost(boost)?;
        }
        Ok(())
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
