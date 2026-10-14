//! Shared types and the seams between pipeline stages.
//!
//! Every stage is a trait so the pipeline can be driven headless (CLI, tests)
//! and each backend can be swapped: Parakeet vs Whisper for [`Asr`], a local
//! GGUF vs an OpenAI-compatible server for [`Cleanup`], SendInput vs libei vs
//! clipboard for [`Injector`]. Nothing here depends on audio, models, UI, or
//! an OS.

pub mod style;

pub use style::{PunctuationPlacement, QuoteStyle, Style};

/// Errors crossing a trait boundary. Each backend crate has its own concrete
/// error type and boxes it here, so callers can report it without knowing the
/// backend.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Sample rate every stage after capture works in.
pub const SAMPLE_RATE: u32 = 16_000;

/// One utterance of mono f32 PCM at [`SAMPLE_RATE`], as cut by the VAD or by
/// the user stopping dictation.
#[derive(Debug, Clone, Default)]
pub struct AudioSegment {
    pub samples: Vec<f32>,
    /// Monotonic sequence number within a dictation session, starting at 0.
    pub seq: u64,
}

impl AudioSegment {
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / SAMPLE_RATE as f32
    }
}

/// Speech-to-text for one finished segment.
pub trait Asr: Send {
    /// Returns the transcript with the model's own casing and punctuation.
    /// An empty string means "no speech".
    fn transcribe(&mut self, segment: &AudioSegment) -> Result<String, BoxError>;
}

/// What the cleanup model is asked to do with one segment.
#[derive(Debug, Clone)]
pub struct CleanupRequest<'a> {
    /// Our own previously emitted text (most recent last), for speaker and
    /// open-quote continuity. Never text read back from the target app.
    pub context: &'a str,
    /// The segment after the command pass: ASR text with spoken commands
    /// already turned into literal marks.
    pub raw: &'a str,
    pub style: &'a Style,
}

/// Turns a raw segment into clean prose.
pub trait Cleanup: Send {
    fn clean(&mut self, request: &CleanupRequest<'_>) -> Result<String, BoxError>;
}

/// How text actually reached the target app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectOutcome {
    /// Synthesized keystrokes.
    Typed,
    /// Placed on the clipboard and pasted with the platform paste chord; the
    /// previous clipboard content was restored.
    Pasted,
    /// Could not deliver (e.g. the focused window is elevated on Windows), so
    /// the text was left on the clipboard for the user to paste.
    LeftOnClipboard { reason: String },
}

/// Delivers text into whatever application has keyboard focus.
pub trait Injector {
    fn inject(&mut self, text: &str) -> Result<InjectOutcome, BoxError>;
}
