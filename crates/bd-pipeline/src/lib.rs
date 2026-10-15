//! The dictation pipeline, headless: 16 kHz audio in, text to deliver out.
//!
//! ```text
//!  push_audio ─▶ [segment] ─▶ [ASR] ─▶ [commands → cleanup → join] ─▶ events
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
//! "scratch that" drops earlier segments that have not reached that point;
//! for one that has, the pipeline sends [`Event::ScratchTooLate`].
//!
//! The context the cleanup model sees is the pipeline's own delivered text
//! for the current passage (see [`Pipeline::new_passage`]), never text read
//! back from the target application.

mod compose;
mod stages;

#[cfg(test)]
mod tests;

use std::thread::JoinHandle;
use std::time::Duration;

use bd_commands::CommandConfig;
use bd_core::{Asr, AudioSegment, BoxError, Cleanup, Style};
use crossbeam_channel::{Receiver, Sender, unbounded};

pub use compose::{Composer, join};

/// Cuts a 16 kHz mono stream into utterances. Implemented for
/// [`bd_asr::Segmenter`]; tests use a simpler one.
pub trait Segmenting: Send {
    /// Feeds audio; returns the utterances it completed.
    fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment>;
    /// Ends the dictation session: returns the utterance in progress, if
    /// any, and starts the next session's numbering from 0.
    fn flush(&mut self) -> Vec<AudioSegment>;
}

impl Segmenting for bd_asr::Segmenter {
    fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment> {
        bd_asr::Segmenter::accept(self, samples)
    }

    fn flush(&mut self) -> Vec<AudioSegment> {
        bd_asr::Segmenter::flush(self)
    }
}

/// The stages a pipeline runs. Each moves onto its own thread.
pub struct Parts {
    pub segmenter: Box<dyn Segmenting>,
    pub asr: Box<dyn Asr>,
    /// `None` runs without a cleanup model: ASR text with spoken commands
    /// applied.
    pub cleanup: Option<Box<dyn Cleanup>>,
}

/// Settings fixed for the life of a pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub style: Style,
    pub commands: CommandConfig,
    /// Characters of delivered text kept as context. The cleanup prompt
    /// trims this further to its own budget.
    pub context_chars: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self { style: Style::default(), commands: CommandConfig::default(), context_chars: 4000 }
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
    Transcribed { id: u64, text: String, audio: Duration, elapsed: Duration },
    /// Text to deliver now, separator included: append it to what was
    /// delivered before.
    Text {
        id: u64,
        text: String,
        /// The segment after the command pass: what the cleanup model was
        /// given, and what is delivered when it fails or is off.
        commanded: String,
        /// Time spent in the cleanup model; `None` when it was not called.
        cleanup: Option<Duration>,
    },
    /// A spoken "scratch that" dropped this segment before delivery.
    Scratched { id: u64 },
    /// "scratch that" came after the segment it meant had been delivered.
    ScratchTooLate,
    /// Everything spoken before [`Pipeline::end_session`] has been
    /// delivered.
    SessionEnded,
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
