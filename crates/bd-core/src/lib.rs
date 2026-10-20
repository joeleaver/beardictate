//! Shared types and the seams between pipeline stages.
//!
//! Every stage is a trait so the pipeline can be driven headless (CLI, tests)
//! and each backend can be swapped: Nemotron or another for [`StreamingAsr`], a local
//! GGUF vs an OpenAI-compatible server for [`Cleanup`], SendInput vs libei vs
//! clipboard for [`Injector`]. Nothing here depends on audio, models, UI, or
//! an OS.

pub mod style;

pub use style::{PunctuationPlacement, QuoteStyle, Style};

/// Errors crossing a trait boundary. Each backend crate has its own concrete
/// error type and boxes it here, so callers can report it without knowing the
/// backend.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A line break a special word's spelling asks for (`<new line>`), carried
/// from the vocabulary pass to the command pass as this character (Unicode
/// private use), a whitespace-separated word of its own. The command pass
/// types it as "new line" would be, whatever the command settings.
pub const LINE_BREAK: char = '\u{E000}';
/// A paragraph break a spelling asks for (`<paragraph>`), likewise.
pub const PARAGRAPH_BREAK: char = '\u{E001}';

/// Sample rate every stage after capture works in.
pub const SAMPLE_RATE: u32 = 16_000;

/// One utterance of mono f32 PCM at [`SAMPLE_RATE`], as cut by the VAD or by
/// the user stopping dictation.
#[derive(Debug, Clone, Default)]
pub struct AudioSegment {
    pub samples: Vec<f32>,
    /// Monotonic sequence number within a dictation session, starting at 0.
    pub seq: u64,
    /// Offset of the first sample from the start of the dictation session's
    /// audio stream, for aligning segments and spotting gaps between them.
    pub start_sample: u64,
}

impl AudioSegment {
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / SAMPLE_RATE as f32
    }
}

/// Speech-to-text for one finished segment.
pub trait Asr: Send {
    /// Returns the transcript with the model's own casing and punctuation.
    /// An empty string means "no speech". Takes `&self`: a recognizer keeps
    /// no state between segments.
    fn transcribe(&self, segment: &AudioSegment) -> Result<String, BoxError>;

    /// Replaces the names and invented words to listen for, and how strongly
    /// (0: not at all), from the next segment. Recognizers that can't take
    /// hotwords ignore them.
    fn set_hotwords(&self, _words: &[String], _boost: f32) -> Result<(), BoxError> {
        Ok(())
    }
}

/// Speech-to-text as the speaker talks: one utterance at a time, fed audio
/// as it arrives, with the text so far after each piece.
pub trait StreamingAsr: Send {
    /// Starts an utterance, dropping any unfinished one.
    fn start(&mut self) -> Result<(), BoxError>;

    /// Feeds 16 kHz mono samples of the current utterance. Returns the whole
    /// utterance's text so far when it changed.
    fn feed(&mut self, samples: &[f32]) -> Result<Option<String>, BoxError>;

    /// Ends the utterance and returns its final text.
    fn finish(&mut self) -> Result<String, BoxError>;

    /// The utterance stopped at a pause: brings out its last word and keeps
    /// the stream open, so that the next utterance goes on from it
    /// ([`resume`](Self::resume)) and the recognizer punctuates the pause
    /// knowing what came after it. Returns the whole stream's text so far,
    /// or `None` when this recognizer can't (the caller then finishes).
    fn hold(&mut self) -> Result<Option<String>, BoxError> {
        Ok(None)
    }

    /// Goes on with a held stream after `gap` of silence; starts one when
    /// none is held. [`feed`](Self::feed) and [`finish`](Self::finish) then
    /// return the text of everything since [`start`](Self::start).
    fn resume(&mut self, _gap: std::time::Duration) -> Result<(), BoxError> {
        self.start()
    }

    /// As [`Asr::set_hotwords`], from the next utterance.
    fn set_hotwords(&mut self, _words: &[String], _boost: f32) -> Result<(), BoxError> {
        Ok(())
    }
}

/// Audio for a [`StreamingAsr`], as the segmenter hears it.
#[derive(Debug, Clone)]
pub enum LiveAudio {
    /// Speech began: an utterance's audio so far, from just before it.
    Start(Vec<f32>),
    /// More of the utterance.
    More(Vec<f32>),
    /// The utterance ended; the same audio as one finished segment.
    End(AudioSegment),
    /// What looked like speech never became an utterance.
    Drop,
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
    /// The user's special words (names, invented words), as they should be
    /// spelled, for a model trained to restore them where the ASR misheard.
    pub names: &'a [String],
}

/// Turns a raw segment into clean prose.
pub trait Cleanup: Send {
    fn clean(&mut self, request: &CleanupRequest<'_>) -> Result<String, BoxError>;

    /// What happened in the last [`Cleanup::clean`], for diagnostics.
    fn last_report(&self) -> Option<CleanupReport> {
        None
    }
}

/// What a cleanup model did with one request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupReport {
    /// The model's answer, tidied, before the guardrail.
    pub model_output: Option<String>,
    /// `accepted`, `repaired` or `fell_back`; empty when the model wasn't asked.
    pub verdict: String,
    /// Why the guardrail rejected it.
    pub reason: Option<String>,
    /// Misheard special words allowed through: (heard, word).
    pub respelled: Vec<(String, String)>,
    /// Words the model dropped, as allowed (fillers, repeats).
    pub removed: Vec<String>,
    pub prompt_tokens: usize,
    pub reused_tokens: usize,
    pub generated_tokens: usize,
    pub prefill: std::time::Duration,
    pub decode: std::time::Duration,
}

/// How text actually reached the target app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectOutcome {
    /// Synthesized keystrokes.
    Typed,
    /// Placed on the clipboard and pasted with the platform paste chord.
    /// `restored` says whether the previous clipboard content was put back;
    /// it isn't when restoring is turned off, the clipboard changed while
    /// pasting, or the restore itself failed.
    Pasted { restored: bool },
    /// Could not deliver (e.g. the focused window is elevated on Windows), so
    /// the text was left on the clipboard for the user to paste.
    LeftOnClipboard { reason: String },
}

/// Delivers text into whatever application has keyboard focus.
pub trait Injector {
    /// One-time setup that may need the user, such as a permission dialog.
    /// Call it during onboarding so the dialog doesn't interrupt the first
    /// dictation. Calling it again after a refusal asks again.
    fn prepare(&mut self) -> Result<(), BoxError> {
        Ok(())
    }

    fn inject(&mut self, text: &str) -> Result<InjectOutcome, BoxError>;
}
