//! The dictation pipeline, headless: 16 kHz audio in, text to deliver out.
//!
//! ```text
//!  push_audio ─▶ [segment] ─▶ [ASR] ─▶ [vocabulary → commands → cleanup → join] ─▶ events
//! ```
//!
//! Each bracket is a thread, linked by channels, so segment N is cleaned
//! while segment N+1 is still being spoken and transcribed. The pipeline
//! delivers nothing itself: every piece of text arrives as an
//! [`Event::Text`], and the caller types it, prints it, or holds it (for
//! example until a held hotkey is released, since an injector waits for
//! modifiers to come up).
//!
//! A segment counts as delivered once its [`Event::Text`] is sent. A spoken
//! "scratch that" drops the latest segment: before delivery by not
//! delivering it ([`Event::Scratched`]); after, within the same dictation
//! session, by asking the caller to erase it ([`Event::Erase`]). Text from
//! an earlier session stays ([`Event::ScratchTooLate`]): the user may have
//! moved the cursor since.
//!
//! The context the cleanup model sees is the pipeline's own delivered text
//! for the current passage (see [`Pipeline::new_passage`]), never text read
//! back from the target application.

mod compose;
pub mod dump;
pub mod gain;
pub mod pad;
pub mod setup;
mod stages;

#[cfg(test)]
mod tests;

use std::thread::JoinHandle;
use std::time::Duration;

use bd_commands::CommandConfig;
use bd_core::{Asr, AudioSegment, BoxError, Cleanup, LiveAudio, StreamingAsr, Style};
use bd_vocab::Vocabulary;
use crossbeam_channel::{Receiver, Sender, unbounded};

pub use compose::{Composer, join};
pub use pad::{PadAction, PadPart, PadView};

/// Cuts a 16 kHz mono stream into utterances. Implemented for
/// [`bd_asr::Segmenter`]; tests use a simpler one.
pub trait Segmenting: Send {
    /// Feeds audio; returns the utterances it completed.
    fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment>;
    /// Ends the dictation session: returns the utterance in progress, if
    /// any, and starts the next session's numbering from 0.
    fn flush(&mut self) -> Vec<AudioSegment>;

    /// [`accept`](Self::accept) for a streaming recognizer: the default
    /// hands out each finished utterance whole.
    fn accept_live(&mut self, samples: &[f32]) -> Vec<LiveAudio> {
        whole(self.accept(samples))
    }

    /// [`flush`](Self::flush) for [`accept_live`](Self::accept_live).
    fn flush_live(&mut self) -> Vec<LiveAudio> {
        whole(self.flush())
    }
}

fn whole(segments: Vec<AudioSegment>) -> Vec<LiveAudio> {
    segments.into_iter().flat_map(|s| [LiveAudio::Start(s.samples.clone()), LiveAudio::End(s)]).collect()
}

impl Segmenting for bd_asr::Segmenter {
    fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment> {
        bd_asr::Segmenter::accept(self, samples)
    }

    fn flush(&mut self) -> Vec<AudioSegment> {
        bd_asr::Segmenter::flush(self)
    }

    fn accept_live(&mut self, samples: &[f32]) -> Vec<LiveAudio> {
        bd_asr::Segmenter::accept_live(self, samples)
    }

    fn flush_live(&mut self) -> Vec<LiveAudio> {
        bd_asr::Segmenter::flush_live(self)
    }
}

/// The speech recognizer a pipeline runs.
pub enum Recognizer {
    /// A finished segment at a time (Parakeet): text arrives when the
    /// speaker pauses.
    Segments(Box<dyn Asr>),
    /// As the speaker talks (Nemotron): with [`PipelineConfig::pad`] off,
    /// the words are typed as they come, with no cleanup; with it on, they
    /// show in the pad.
    Live(Box<dyn StreamingAsr>),
}

/// The stages a pipeline runs. Each moves onto its own thread.
pub struct Parts {
    pub segmenter: Box<dyn Segmenting>,
    pub asr: Recognizer,
    /// `None` runs without a cleanup model: ASR text with spoken commands
    /// applied.
    pub cleanup: Option<Box<dyn Cleanup>>,
}

/// Settings for a pipeline. All but `context_chars` can be changed while it
/// runs, with [`Feeder::reconfigure`].
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub style: Style,
    pub commands: CommandConfig,
    /// The user's names and invented words, fixed up in the ASR text before
    /// the command pass. (Their hotwords are the ASR's business.)
    pub vocabulary: Vocabulary,
    /// How strongly the ASR listens for the vocabulary's words: a score per
    /// matching token, 0 for not at all. Only [`Feeder::reconfigure`] reads
    /// it; the ASR starts with what it was loaded with.
    pub hotword_boost: f32,
    /// Start a new paragraph when the speaker paused at least this long
    /// before a segment, within one dictation session. `None`: only
    /// dictated breaks and (with [`Style::dialog_paragraphs`]) the cleanup
    /// model's.
    pub pause_paragraph: Option<Duration>,
    /// Characters of delivered text kept as context. The cleanup prompt
    /// trims this further to its own budget.
    pub context_chars: usize,
    /// How long a segment that stops mid-sentence ("…and said,") waits for
    /// the next one, so cleanup sees the sentence whole and can quote the
    /// line that follows a tag. Long enough to speak and transcribe the
    /// next utterance; when it runs out, the half sentence goes on alone.
    pub sentence_hold: Duration,
    /// Paragraphs separated by a blank line (two returns). Off, one return:
    /// for editors that space paragraphs themselves (Word, Scrivener,
    /// Google Docs). A "new line" is one return either way.
    pub blank_line_paragraphs: bool,
    /// Always-listening mode: audio flows all the time, and only what's said
    /// between "bear, wake up" and "bear, go to sleep" is delivered. Starts
    /// asleep. Off, everything pushed is delivered.
    pub voice_wake: bool,
    /// Microphone gain before the VAD, as a factor (1 to [`gain::MAX_GAIN`]).
    /// With `auto_gain` it's where learning starts; without, it's fixed.
    pub input_gain: f32,
    /// Learn the gain from recognized speech ([`gain`]).
    pub auto_gain: bool,
    /// Bear Pad ([`pad`]): segments collect in a pad, cleaned there as the
    /// speaker pauses, and are delivered only when the user sends it
    /// ([`Event::PadSend`]). Off, each segment is cleaned and delivered.
    pub pad: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self { style: Style::default(), commands: CommandConfig::default(), vocabulary: Vocabulary::default(), hotword_boost: setup::DEFAULT_HOTWORD_BOOST, pause_paragraph: None, context_chars: 4000, sentence_hold: Duration::from_secs(8), blank_line_paragraphs: true, voice_wake: false, input_gain: 1.0, auto_gain: true, pad: false }
    }
}

/// Which stage an [`Event::Error`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Asr,
    Cleanup,
}

/// What the pipeline reports, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The ASR finished a segment. `id` counts segments over the pipeline's
    /// whole life, from 0. Empty `text` means it heard no words.
    /// `pause` is the silence before it in this session (`None` for the
    /// session's first segment).
    /// `level` is the segment's RMS loudness from the microphone (before
    /// gain), 0 to 1; `gain` the input gain in use after it.
    Transcribed { id: u64, text: String, audio: Duration, level: f32, gain: f32, pause: Option<Duration>, elapsed: Duration },
    /// Text to deliver now: first take back `take_back` characters (the
    /// trailing space before a break or a closing mark), then append `text`,
    /// which ends with the space the next segment needs.
    Text {
        id: u64,
        take_back: usize,
        text: String,
        /// The segment after the command pass: what the cleanup model was
        /// given, and what is delivered when it fails or is off.
        commanded: String,
        /// Time spent in the cleanup model; `None` when it was not called.
        cleanup: Option<Duration>,
    },
    /// A spoken "scratch that" dropped this segment before delivery.
    Scratched { id: u64 },
    /// "scratch that" for a segment already delivered in this dictation
    /// session: take `text` back (with Backspace, one per character typed;
    /// the cursor is still right after it, since the user is talking, not
    /// typing). Repeated, it reaches further back within the session.
    Erase { id: u64, text: String },
    /// "scratch that" with nothing left to take back in this session (the
    /// text it meant was typed in an earlier dictation, where the cursor
    /// may have moved).
    ScratchTooLate,
    /// Everything spoken before [`Pipeline::end_session`] has been
    /// delivered.
    SessionEnded,
    /// With [`PipelineConfig::voice_wake`]: "bear, wake up" (or
    /// [`Feeder::set_awake`]); segments are delivered from now on.
    Woke,
    /// With [`PipelineConfig::voice_wake`]: "bear, go to sleep" (or
    /// [`Feeder::set_awake`]), after everything before it was delivered.
    /// Segments are transcribed but dropped until [`Event::Woke`].
    Slept,
    /// With [`PipelineConfig::pad`]: what the pad holds now, after each
    /// change.
    Pad(PadView),
    /// With [`PipelineConfig::pad`]: "bear, send it" (or
    /// [`Feeder::pad`]`(Send)`): deliver this, the whole pad, cleaned. The
    /// pad is empty now.
    PadSend { text: String },
    /// With [`PipelineConfig::pad`]: "bear, change …" or "bear, delete …"
    /// named words that aren't in the pad.
    PadNotFound { words: String },
    /// A stage failed on one segment. The pipeline keeps going: an ASR
    /// failure loses that segment, a cleanup failure delivers the
    /// `commanded` text instead.
    Error { stage: Stage, message: String },
}

/// Control and audio going into the first stage.
pub(crate) enum Input {
    Audio(Vec<f32>),
    EndSession,
    NewPassage,
    Reconfigure(Box<PipelineConfig>),
    Awake(bool),
    Pad(PadAction),
}

/// Feeds a running pipeline from any thread. Cheap to clone. The stages
/// drain and stop once the [`Pipeline`] and every feeder are gone.
#[derive(Clone)]
pub struct Feeder {
    input: Sender<Input>,
}

impl Feeder {
    /// Feeds 16 kHz mono samples from the current session.
    pub fn push_audio(&self, samples: Vec<f32>) {
        self.send(Input::Audio(samples));
    }

    /// The user stopped dictating: cut the utterance in progress. An
    /// [`Event::SessionEnded`] follows once it is delivered.
    pub fn end_session(&self) {
        self.send(Input::EndSession);
    }

    /// Forgets the context and any open quotation, so the next segment
    /// starts fresh text (for example after the focus moved to another
    /// window). Takes effect after everything already pushed.
    pub fn new_passage(&self) {
        self.send(Input::NewPassage);
    }

    /// Changes the vocabulary (hotwords too), style, commands and paragraph
    /// rules. Takes effect from the next segment the ASR finishes after
    /// everything already pushed, so it can arrive mid-dictation.
    pub fn reconfigure(&self, config: PipelineConfig) {
        self.send(Input::Reconfigure(Box::new(config)));
    }

    /// With [`PipelineConfig::voice_wake`]: wakes or sends to sleep as the
    /// spoken phrases do, in order with the audio already pushed.
    pub fn set_awake(&self, awake: bool) {
        self.send(Input::Awake(awake));
    }

    /// With [`PipelineConfig::pad`]: does to the pad what the spoken command
    /// would, in order with the audio already pushed.
    pub fn pad(&self, action: PadAction) {
        self.send(Input::Pad(action));
    }

    fn send(&self, msg: Input) {
        // The stages run until every sender is dropped, so a send fails only
        // if a stage panicked; that is reported when the pipeline finishes.
        let _ = self.input.send(msg);
    }
}

/// A running pipeline. Dropping it, or calling [`Pipeline::finish`], ends
/// the current session and lets every stage drain.
pub struct Pipeline {
    feeder: Option<Feeder>,
    events: Receiver<Event>,
    threads: Vec<JoinHandle<()>>,
}

impl Pipeline {
    /// Starts the stage threads.
    pub fn spawn(parts: Parts, config: PipelineConfig) -> Result<Self, BoxError> {
        let (input, input_rx) = unbounded();
        let (events_tx, events) = unbounded();
        let threads = stages::spawn(parts, config, input_rx, events_tx)?;
        Ok(Self { feeder: Some(Feeder { input }), events, threads })
    }

    /// A handle for feeding audio from another thread.
    pub fn feeder(&self) -> Feeder {
        self.feeder_ref().clone()
    }

    /// See [`Feeder::push_audio`].
    pub fn push_audio(&self, samples: Vec<f32>) {
        self.feeder_ref().push_audio(samples);
    }

    /// See [`Feeder::end_session`].
    pub fn end_session(&self) {
        self.feeder_ref().end_session();
    }

    /// See [`Feeder::reconfigure`].
    pub fn reconfigure(&self, config: PipelineConfig) {
        self.feeder_ref().reconfigure(config);
    }

    /// See [`Feeder::set_awake`].
    pub fn set_awake(&self, awake: bool) {
        self.feeder_ref().set_awake(awake);
    }

    /// See [`Feeder::pad`].
    pub fn pad(&self, action: PadAction) {
        self.feeder_ref().pad(action);
    }

    /// See [`Feeder::new_passage`].
    pub fn new_passage(&self) {
        self.feeder_ref().new_passage();
    }

    /// The event stream. Clone it to read from another thread; it closes
    /// once every stage has drained.
    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    /// Ends the session (with an [`Event::SessionEnded`] if audio came after
    /// the last [`Pipeline::end_session`]) and waits for this pipeline's
    /// stages to drain. Returns the event stream, now closed, holding any
    /// events no one has read yet. If a [`Feeder`] is still alive, waits for
    /// it to be dropped too.
    pub fn finish(mut self) -> Receiver<Event> {
        self.shutdown();
        self.events.clone()
    }

    fn feeder_ref(&self) -> &Feeder {
        self.feeder.as_ref().expect("the feeder is only taken by shutdown")
    }

    fn shutdown(&mut self) {
        // Dropping the sender lets the first stage cut any utterance in
        // progress and then close, which closes each stage after it.
        drop(self.feeder.take());
        for t in self.threads.drain(..) {
            if t.join().is_err() {
                tracing::error!("a pipeline stage panicked");
            }
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shutdown();
    }
}
