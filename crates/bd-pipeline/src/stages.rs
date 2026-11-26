//! The stage threads.

use std::collections::VecDeque;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bd_commands::{CommandOutput, CommandPass};
use bd_core::{AudioSegment, BoxError, Cleanup, Style};
use bd_vocab::Vocabulary;
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::compose::{Composer, join};
use crate::gain::InputGain;
mod cleaner;
mod deliver;
mod edit;
mod gate;
mod join_on;
mod listen;
mod list;
mod live_type;
mod notes;
mod segment;
mod transcribe;
mod noise;
mod pace;
mod pad_mode;
mod pad_voice;
mod rejoin;
mod rewrite;
mod take_in;
mod tidy;
mod wake;

use gate::Gate;
use crate::{Event, FocusProbe, Input, PadAction, Parts, PipelineConfig, Recognizer};
use pace::Waited;
use pad_mode::PadMode;

/// From the segmenter to the ASR.
enum Cut {
    /// A segment, and the input gain it was raised by.
    Segment(u64, AudioSegment, f32),
    EndSession,
    NewPassage,
    Reconfigure(Box<PipelineConfig>),
    Awake(bool),
    Pad(PadAction),
    /// A live utterance's audio (see [`bd_core::LiveAudio`]), its id, and
    /// the input gain it was raised by.
    Live(u64, bd_core::LiveAudio, f32),
}

/// From the ASR to the finisher.
enum Heard {
    /// A transcript, with the silence before it in this session (`None` for
    /// a session's first segment), and how its sentence was left.
    Text { id: u64, text: String, pause: Option<Duration>, ending: rejoin::Ending },
    /// A streaming recognizer's text so far for utterance `id`; empty when
    /// it turned out not to be speech. `settled` when the utterance has
    /// ended: no word of it changes now but the last one's punctuation.
    Partial { id: u64, text: String, settled: bool },
    EndSession,
    NewPassage,
    Reconfigure(Box<PipelineConfig>),
    Awake(bool),
    Pad(PadAction),
}

pub(crate) fn spawn(
    parts: Parts,
    config: PipelineConfig,
    input: Receiver<Input>,
    events: Sender<Event>,
) -> Result<Vec<JoinHandle<()>>, BoxError> {
    let (cut_tx, cut_rx) = unbounded();
    let (heard_tx, heard_rx) = unbounded();
    let Parts { segmenter, asr, cleanup, focus } = parts;
    let live = matches!(asr, Recognizer::Live(_));
    let asr_events = events.clone();
    let gain = InputGain::new(config.input_gain, config.auto_gain);
    let learner = gain.clone();
    let voice = crate::voice::VoiceLevel::new(config.drop_far_voices);
    let commands = config.commands.clone();
    let finisher = Finisher::new(cleanup, config, events, live, focus);
    Ok(vec![
        thread::Builder::new().name("bd-segment".into()).spawn(move || segment::segment(segmenter, input, cut_tx, gain, live))?,
        thread::Builder::new().name("bd-asr".into()).spawn(move || match asr {
            Recognizer::Segments(asr) => transcribe::transcribe(asr, cut_rx, heard_tx, asr_events, learner, voice),
            Recognizer::Live(asr) => listen::live(asr, cut_rx, heard_tx, asr_events, learner, voice, commands),
        })?,
        thread::Builder::new().name("bd-finish".into()).spawn(move || finisher.run(heard_rx))?,
    ])
}

/// A segment waiting for cleanup.
struct Pending {
    id: u64,
    /// Whether a quotation was open before this segment, to restore if a
    /// "scratch that" drops it.
    quote_before: bool,
    out: CommandOutput,
    /// When it was ready to clean: when it, or the last part joined onto
    /// it, arrived.
    ready: Instant,
    /// A list item, or said in a case mode: typed as it is, not tidied.
    list: bool,
}

enum Item {
    Segment(Pending),
    EndSession,
    NewPassage,
    /// Voice wake woke up here.
    Wake,
    /// Voice wake went to sleep here.
    Sleep,
    /// A streaming recognizer's text so far, and whether it's settled (see
    /// [`Heard::Partial`]).
    Partial(u64, String, bool),
    /// A spoken edit of what's been typed into your app ("bear, change …").
    Edit(PadAction),
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
#[derive(Debug, Clone, PartialEq)]
struct Typed {
    id: u64,
    /// Exactly what was delivered, separator included.
    text: String,
    quote_before: bool,
    /// When it was delivered.
    at: Instant,
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
    /// See [`PipelineConfig::sentence_hold_speaking`].
    sentence_hold_speaking: Duration,
    /// The utterance a streaming recognizer is hearing now, if any.
    speaking: Option<u64>,
    /// See [`PipelineConfig::live_typing`].
    live_typing: bool,
    /// See [`PipelineConfig::context_window`] and
    /// [`PipelineConfig::cleanup_context_chars`].
    context_window: Duration,
    cleanup_context_chars: usize,
    /// See [`PipelineConfig::passage_gap`].
    passage_gap: Duration,
    /// When the last segment came in.
    last_segment_at: Option<Instant>,
    /// See [`Parts::focus`]; and the app the last segment was dictated into.
    focus: Option<FocusProbe>,
    last_focus: Option<String>,
    /// See [`PipelineConfig::blank_line_paragraphs`].
    blank_line_paragraphs: bool,
    gate: Gate,
    /// Bear Pad mode ([`PipelineConfig::pad`]).
    pad: Option<PadMode>,
    /// The recognizer streams: its text so far arrives as [`Item::Partial`].
    live: bool,
    /// Outside the pad, the live utterance being typed as it's spoken.
    typing: Option<live_type::Typing>,
    /// Text typed as it's spoken, tidied in place (the cleanup model moves
    /// here, onto its own thread, the first time).
    tidy: Option<tidy::Tidy>,
    /// Spoken edits in this session, for "bear, undo".
    edits: Vec<edit::Edit>,
    /// See [`PipelineConfig::list_mode`].
    list_mode: bool,
    /// A list just ended: what comes next starts on a line of its own.
    after_list: bool,
    /// See [`PipelineConfig::case_mode`].
    case_mode: crate::CaseMode,
    /// Sentences closed at a pause that the next words may reopen.
    rejoin: rejoin::Rejoin,
}

impl Finisher {
    fn new(cleanup: Option<Box<dyn Cleanup>>, config: PipelineConfig, events: Sender<Event>, live: bool, focus: Option<FocusProbe>) -> Self {
        let pad = config.pad;
        let mut finisher = Self {
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
            sentence_hold_speaking: config.sentence_hold_speaking,
            speaking: None,
            live_typing: config.live_typing,
            context_window: config.context_window,
            cleanup_context_chars: config.cleanup_context_chars,
            passage_gap: config.passage_gap,
            last_segment_at: None,
            focus,
            last_focus: None,
            blank_line_paragraphs: config.blank_line_paragraphs,
            gate: Gate::new(config.voice_wake),
            pad: None,
            live,
            typing: None,
            tidy: None,
            edits: Vec::new(),
            list_mode: config.list_mode,
            after_list: false,
            case_mode: config.case_mode,
            rejoin: rejoin::Rejoin::new(config.rejoin, config.list_join),
        };
        finisher.set_pad(pad);
        finisher
    }

    fn run(mut self, input: Receiver<Heard>) {
        loop {
            if self.pad.is_some() {
                if !self.pad_step(&input) {
                    return;
                }
                continue;
            }
            if self.queue.is_empty() {
                match self.wait(&input, self.tidy_wait()) {
                    Waited::Heard(heard) => self.take_in(heard),
                    Waited::Timeout => {
                        self.tidy_due();
                        continue;
                    }
                    Waited::Closed => return self.tidy_flush(),
                }
            }
            self.take_in_ready(&input);
            if let Some(wait) = self.hold()
                && let Waited::Heard(heard) = self.wait(&input, Some(wait))
            {
                self.take_in(heard);
                continue;
            }
            let Some(item) = self.pop_sentence() else { continue };
            self.handle(item, &input);
        }
    }

    fn handle(&mut self, item: Item, input: &Receiver<Heard>) {
        match item {
            Item::EndSession => {
                self.tidy_flush();
                self.typed.clear();
                self.edits.clear();
                self.emit(Event::SessionEnded);
            }
            Item::NewPassage => {
                self.tidy_drop();
                self.typed.clear();
                self.edits.clear();
                self.composer.clear();
            }
            Item::Wake => self.emit(Event::Woke),
            Item::Sleep => {
                // Like a stop: "scratch that" won't reach back past it.
                self.tidy_flush();
                self.typed.clear();
                self.edits.clear();
                self.emit(Event::Slept);
            }
            Item::Segment(p) => self.finish(p, input),
            Item::Partial(id, text, settled) => self.partial(id, &text, settled),
            Item::Edit(action) => self.edit_typed(action),
        }
    }

    /// The next item, with an unfinished sentence joined to the segments
    /// that finish it, so cleanup sees "…and said, Oh no" whole. "Scratch
    /// that" while it waits still drops one segment at a time; once joined
    /// it is one segment.
    fn pop_sentence(&mut self) -> Option<Item> {
        let mut item = self.queue.pop_front()?;
        if let Item::Segment(p) = &mut item {
            while !self.types_live() && !p.ends_sentence() {
                let Some(Item::Segment(next)) = self.queue.front() else { break };
                if !next.continues() {
                    break;
                }
                let Some(Item::Segment(next)) = self.queue.pop_front() else { unreachable!() };
                tracing::info!(segment = p.id, joined = next.id, "unfinished sentence joined to the next");
                let (take_back, tail) = join(&p.out.text, &self.held_on(&p.out.text, &next));
                for _ in 0..take_back {
                    p.out.text.pop();
                }
                p.out.text.push_str(&tail);
                p.out.quote_open = next.out.quote_open;
                p.ready = next.ready;
            }
        }
        Some(item)
    }

    /// What the recognizer heard in the user's spelling, then with their
    /// special words.
    pub(super) fn own_words(&self, heard: &str) -> String {
        self.vocabulary.apply(&bd_vocab::respell(heard, self.style.spelling, &self.vocabulary.spellings()))
    }

    /// Drops the most recent undelivered segment of this passage, returning
    /// the quotation state from before it. `None` when there is none left.
    fn drop_latest(&mut self) -> Option<bool> {
        if self.pad.is_some() {
            return self.pad_undo();
        }
        // A segment from before a stop may be dropped while it's untyped,
        // but typed text is only erased within the dictation it came from.
        let mut crossed_session = false;
        for i in (0..self.queue.len()).rev() {
            match &self.queue[i] {
                Item::NewPassage => return None,
                Item::EndSession | Item::Sleep => crossed_session = true,
                Item::Wake | Item::Partial(..) | Item::Edit(_) => {}
                Item::Segment(_) => {
                    let Some(Item::Segment(p)) = self.queue.remove(i) else { unreachable!() };
                    self.untype(p.id);
                    self.restore_mark(p.id);
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
        let id = typed.id;
        self.emit(Event::Erase { id, text: typed.text, scratch: true });
        self.restore_mark(id);
        Some(typed.quote_before)
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }
}
