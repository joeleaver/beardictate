//! The stage threads.

use std::collections::VecDeque;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bd_commands::{CommandOutput, CommandPass};
use bd_core::{Asr, AudioSegment, BoxError, Cleanup, CleanupRequest, SAMPLE_RATE, Style};
use bd_vocab::Vocabulary;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};

use crate::compose::{Composer, join, single_return_paragraphs};
use crate::gain::InputGain;
mod gate;
mod segment;
mod noise;
mod wake;

use gate::{Gate, Step};
use crate::{Event, Input, Parts, PipelineConfig, Stage};

/// From the segmenter to the ASR.
enum Cut {
    /// A segment, and the input gain it was raised by.
    Segment(u64, AudioSegment, f32),
    EndSession,
    NewPassage,
    Reconfigure(Box<PipelineConfig>),
    Awake(bool),
}

/// From the ASR to the finisher.
enum Heard {
    /// A transcript, with the silence before it in this session (`None` for
    /// a session's first segment).
    Text { id: u64, text: String, pause: Option<Duration> },
    EndSession,
    NewPassage,
    Reconfigure(Box<PipelineConfig>),
    Awake(bool),
}

pub(crate) fn spawn(
    parts: Parts,
    config: PipelineConfig,
    input: Receiver<Input>,
    events: Sender<Event>,
) -> Result<Vec<JoinHandle<()>>, BoxError> {
    let (cut_tx, cut_rx) = unbounded();
    let (heard_tx, heard_rx) = unbounded();
    let Parts { segmenter, asr, cleanup } = parts;
    let asr_events = events.clone();
    let gain = InputGain::new(config.input_gain, config.auto_gain);
    let learner = gain.clone();
    let finisher = Finisher::new(cleanup, config, events);
    Ok(vec![
        thread::Builder::new().name("bd-segment".into()).spawn(move || segment::segment(segmenter, input, cut_tx, gain))?,
        thread::Builder::new().name("bd-asr".into()).spawn(move || transcribe(asr, cut_rx, heard_tx, asr_events, learner))?,
        thread::Builder::new().name("bd-finish".into()).spawn(move || finisher.run(heard_rx))?,
    ])
}

/// Stage 2: speech to text.
fn transcribe(asr: Box<dyn Asr>, input: Receiver<Cut>, out: Sender<Heard>, events: Sender<Event>, gain: InputGain) {
    // Where the previous segment of this session ended, in session samples.
    let mut previous_end: Option<u64> = None;
    let secs = |samples: u64| Duration::from_secs_f64(samples as f64 / f64::from(SAMPLE_RATE));
    for cut in &input {
        let heard = match cut {
            Cut::Segment(id, seg, used) => {
                let started = Instant::now();
                let audio = secs(seg.samples.len() as u64);
                let pause = previous_end.map(|end| secs(seg.start_sample.saturating_sub(end)));
                previous_end = Some(seg.start_sample + seg.samples.len() as u64);
                // How loud it came in from the microphone, before the gain.
                let level = noise::rms(&seg.samples) / used;
                match asr.transcribe(&seg) {
                    Ok(text) => {
                        let elapsed = started.elapsed();
                        let text = if noise::is_noise(&text, audio) {
                            tracing::info!(segment = id, ms = audio.as_millis(), "dropped a lone {text:?} from a short burst: noise");
                            String::new()
                        } else {
                            text
                        };
                        let gain = gain.learn(level, text.split_whitespace().count(), audio);
                        let _ = events.send(Event::Transcribed { id, text: text.clone(), audio, level, gain, pause, elapsed });
                        Heard::Text { id, text, pause }
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error { stage: Stage::Asr, message: e.to_string() });
                        continue;
                    }
                }
            }
            Cut::EndSession => {
                previous_end = None;
                Heard::EndSession
            }
            Cut::NewPassage => Heard::NewPassage,
            Cut::Awake(awake) => Heard::Awake(awake),
            Cut::Reconfigure(config) => {
                if let Err(e) = asr.set_hotwords(&config.vocabulary.hotwords(), config.hotword_boost) {
                    let _ = events.send(Event::Error { stage: Stage::Asr, message: format!("hotwords: {e}") });
                }
                Heard::Reconfigure(config)
            }
        };
        let _ = out.send(heard);
    }
}

/// A segment waiting for cleanup.
struct Pending {
    id: u64,
    /// Whether a quotation was open before this segment, to restore if a
    /// "scratch that" drops it.
    quote_before: bool,
    out: CommandOutput,
}

enum Item {
    Segment(Pending),
    EndSession,
    NewPassage,
    /// Voice wake woke up here.
    Wake,
    /// Voice wake went to sleep here.
    Sleep,
}

impl Pending {
    /// Whether this ends a sentence, so cleanup can see it whole. A segment
    /// ending in a comma, a dash or mid-word ("…and said,") is waiting for
    /// its other half, which is often the line of dialog to quote.
    fn ends_sentence(&self) -> bool {
        let text = self.out.text.trim_end_matches([' ', '\t']);
        if text.ends_with('\n') || !self.out.has_words() {
            return true;
        }
        let text = text.trim_end_matches(['\u{201D}', '\u{2019}', '"', '\'', ')', ']', '\u{00BB}']);
        text.ends_with(['.', '!', '?', '\u{2026}'])
    }

    /// Whether this can be read on from the segment before it: not the
    /// start of a paragraph.
    fn continues(&self) -> bool {
        !self.out.text.trim_start_matches([' ', '\t']).starts_with('\n')
    }
}

/// How many delivered segments "scratch that" can reach back through.
const MAX_ERASABLE: usize = 20;

/// A segment delivered in the current dictation session.
struct Typed {
    id: u64,
    /// Exactly what was delivered, separator included.
    text: String,
    quote_before: bool,
}

/// The segment being cleaned: past the queue, not yet delivered.
struct InFlight {
    id: u64,
    quote_before: bool,
    cancelled: bool,
}

/// Stage 3: the user's vocabulary, spoken commands, cleanup, joining,
/// delivery.
///
/// Commands run as soon as a segment arrives, so a "scratch that" can drop
/// queued segments and the one being cleaned. Cleanup runs one segment at a
/// time, with the delivered text as context.
struct Finisher {
    vocabulary: Vocabulary,
    pass: CommandPass,
    cleanup: Option<Box<dyn Cleanup>>,
    style: Style,
    composer: Composer,
    events: Sender<Event>,
    queue: VecDeque<Item>,
    in_flight: Option<InFlight>,
    /// This session's delivered segments, oldest first, for "scratch that".
    typed: Vec<Typed>,
    /// Whether a commanded quotation is open after the last segment taken in.
    quote_open: bool,
    /// Start a paragraph after a pause this long (see
    /// [`PipelineConfig::pause_paragraph`]).
    pause_paragraph: Option<Duration>,
    /// Whether this passage has words yet, so a pause before the first
    /// segment doesn't open with a break.
    passage_has_text: bool,
    /// See [`PipelineConfig::sentence_hold`].
    sentence_hold: Duration,
    /// See [`PipelineConfig::blank_line_paragraphs`].
    blank_line_paragraphs: bool,
    gate: Gate,
}

impl Finisher {
    fn new(cleanup: Option<Box<dyn Cleanup>>, config: PipelineConfig, events: Sender<Event>) -> Self {
        Self {
            vocabulary: config.vocabulary,
            pass: CommandPass::new(config.commands, config.style.clone()),
            cleanup,
            style: config.style,
            composer: Composer::new(config.context_chars),
            events,
            queue: VecDeque::new(),
            in_flight: None,
            typed: Vec::new(),
            quote_open: false,
            pause_paragraph: config.pause_paragraph,
            passage_has_text: false,
            sentence_hold: config.sentence_hold,
            blank_line_paragraphs: config.blank_line_paragraphs,
            gate: Gate::new(config.voice_wake),
        }
    }

    fn run(mut self, input: Receiver<Heard>) {
        loop {
            if self.queue.is_empty() {
                match input.recv() {
                    Ok(heard) => self.take_in(heard),
                    Err(_) => return,
                }
            }
            self.take_in_ready(&input);
            if self.waiting_for_rest() {
                match input.recv_timeout(self.sentence_hold) {
                    Ok(heard) => {
                        self.take_in(heard);
                        continue;
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
                }
            }
            let Some(item) = self.pop_sentence() else { continue };
            match item {
                Item::EndSession => {
                    self.typed.clear();
                    self.emit(Event::SessionEnded);
                }
                Item::NewPassage => {
                    self.typed.clear();
                    self.composer.clear();
                }
                Item::Wake => self.emit(Event::Woke),
                Item::Sleep => {
                    // Like a stop: "scratch that" won't reach back past it.
                    self.typed.clear();
                    self.emit(Event::Slept);
                }
                Item::Segment(p) => self.finish(p, &input),
            }
        }
    }

    /// Whether everything queued is one sentence still being spoken.
    fn waiting_for_rest(&self) -> bool {
        let mut segments = self.queue.iter().enumerate().map(|(i, item)| match item {
            Item::Segment(p) if i == 0 || p.continues() => Some(p),
            _ => None,
        });
        segments.all(|p| p.is_some())
            && matches!(self.queue.back(), Some(Item::Segment(p)) if !p.ends_sentence())
    }

    /// The next item, with an unfinished sentence joined to the segments
    /// that finish it, so cleanup sees "…and said, Oh no" whole. "Scratch
    /// that" while it waits still drops one segment at a time; once joined
    /// it is one segment.
    fn pop_sentence(&mut self) -> Option<Item> {
        let mut item = self.queue.pop_front()?;
        if let Item::Segment(p) = &mut item {
            while !p.ends_sentence() {
                let Some(Item::Segment(next)) = self.queue.front() else { break };
                if !next.continues() {
                    break;
                }
                let Some(Item::Segment(next)) = self.queue.pop_front() else { unreachable!() };
                tracing::info!(segment = p.id, joined = next.id, "unfinished sentence joined to the next");
                let tail = join(&p.out.text, &next.out.text);
                p.out.text.push_str(&tail);
                p.out.quote_open = next.out.quote_open;
            }
        }
        Some(item)
    }

    fn finish(&mut self, p: Pending, input: &Receiver<Heard>) {
        self.in_flight = Some(InFlight { id: p.id, quote_before: p.quote_before, cancelled: false });
        let commanded = p.out.text;
        let (text, cleanup) = self.clean(&commanded);
        // A "scratch that" spoken while this was cleaning still drops it.
        self.take_in_ready(input);
        let flight = self.in_flight.take();
        if flight.is_some_and(|f| f.cancelled) {
            return;
        }
        let text = if self.blank_line_paragraphs { text } else { single_return_paragraphs(&text) };
        let text = self.composer.append(&text);
        if text.is_empty() {
            tracing::info!(segment = p.id, "nothing to type (cleaned to nothing)");
        } else {
            if self.typed.len() == MAX_ERASABLE {
                self.typed.remove(0);
            }
            self.typed.push(Typed { id: p.id, text: text.clone(), quote_before: p.quote_before });
            self.emit(Event::Text { id: p.id, text, commanded, cleanup });
        }
    }

    fn clean(&mut self, commanded: &str) -> (String, Option<Duration>) {
        let Some(cleanup) = self.cleanup.as_mut() else {
            return (commanded.to_string(), None);
        };
        let started = Instant::now();
        let request = CleanupRequest { context: self.composer.context(), raw: commanded, style: &self.style };
        let result = cleanup.clean(&request);
        let elapsed = started.elapsed();
        match result {
            Ok(text) => (text, Some(elapsed)),
            Err(e) => {
                self.emit(Event::Error { stage: Stage::Cleanup, message: e.to_string() });
                (commanded.to_string(), Some(elapsed))
            }
        }
    }

    fn take_in_ready(&mut self, input: &Receiver<Heard>) {
        while let Ok(heard) = input.try_recv() {
            self.take_in(heard);
        }
    }

    fn take_in(&mut self, heard: Heard) {
        let (id, text, pause) = match heard {
            Heard::Text { id, text, pause } => (id, text, pause),
            Heard::EndSession => return self.queue.push_back(Item::EndSession),
            Heard::Reconfigure(config) => return self.reconfigure(*config),
            Heard::Awake(awake) => {
                let step = self.gate.set(awake);
                return self.follow(step, 0, None);
            }
            Heard::NewPassage => {
                self.quote_open = false;
                self.passage_has_text = false;
                return self.queue.push_back(Item::NewPassage);
            }
        };
        let words = text.split_whitespace().count();
        let step = self.gate.pass(text);
        let kept = step.pass.as_deref().map_or(0, |t| t.split_whitespace().count());
        if kept < words {
            tracing::info!(segment = id, words, kept, woke = step.woke, sleeps = step.sleep_after, "voice wake kept only part");
        }
        self.follow(step, id, pause);
    }

    /// Carries out what the voice wake gate decided.
    fn follow(&mut self, step: Step, id: u64, pause: Option<Duration>) {
        if step.woke {
            if step.fresh {
                self.take_in(Heard::NewPassage);
            }
            self.queue.push_back(Item::Wake);
        }
        if let Some(text) = step.pass {
            self.take_in_text(id, text, pause);
        }
        if step.sleep_after {
            self.queue.push_back(Item::Sleep);
        }
    }

    fn take_in_text(&mut self, id: u64, text: String, pause: Option<Duration>) {
        let text = self.vocabulary.apply(&text);
        let mut out = self.pass.apply(&text, self.quote_open);
        let scratches = out.scratch_previous();
        if scratches > 0 {
            let mut restored = None;
            for _ in 0..scratches {
                match self.drop_latest() {
                    Some(quote_before) => restored = Some(quote_before),
                    None => {
                        self.emit(Event::ScratchTooLate);
                        break;
                    }
                }
            }
            if let Some(quote) = restored {
                self.quote_open = quote;
                out = self.pass.apply(&text, quote);
            }
        }
        let quote_before = self.quote_open;
        self.quote_open = out.quote_open;
        let long_pause = matches!((pause, self.pause_paragraph), (Some(p), Some(min)) if p >= min);
        if long_pause && self.passage_has_text && !out.text.starts_with('\n') && out.has_words() {
            out.text.insert_str(0, "\n\n");
        }
        if out.has_words() {
            self.passage_has_text = true;
        }
        if !out.text.is_empty() {
            self.queue.push_back(Item::Segment(Pending { id, quote_before, out }));
        }
    }

    /// Segments already taken in keep the settings they were read with.
    fn reconfigure(&mut self, config: PipelineConfig) {
        self.vocabulary = config.vocabulary;
        self.pass = CommandPass::new(config.commands, config.style.clone());
        self.style = config.style;
        self.pause_paragraph = config.pause_paragraph;
        self.sentence_hold = config.sentence_hold;
        self.blank_line_paragraphs = config.blank_line_paragraphs;
        self.gate.configure(config.voice_wake);
    }

    /// Drops the most recent undelivered segment of this passage, returning
    /// the quotation state from before it. `None` when there is none left.
    fn drop_latest(&mut self) -> Option<bool> {
        // A segment from before a stop may be dropped while it's untyped,
        // but typed text is only erased within the dictation it came from.
        let mut crossed_session = false;
        for i in (0..self.queue.len()).rev() {
            match &self.queue[i] {
                Item::NewPassage => return None,
                Item::EndSession | Item::Sleep => crossed_session = true,
                Item::Wake => {}
                Item::Segment(_) => {
                    let Some(Item::Segment(p)) = self.queue.remove(i) else { unreachable!() };
                    self.emit(Event::Scratched { id: p.id });
                    return Some(p.quote_before);
                }
            }
        }
        if let Some(flight) = self.in_flight.as_mut().filter(|f| !f.cancelled) {
            flight.cancelled = true;
            let (id, quote_before) = (flight.id, flight.quote_before);
            self.emit(Event::Scratched { id });
            return Some(quote_before);
        }
        if crossed_session {
            return None;
        }
        let typed = self.typed.pop()?;
        self.composer.retract(&typed.text);
        self.emit(Event::Erase { id: typed.id, text: typed.text });
        Some(typed.quote_before)
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }
}
