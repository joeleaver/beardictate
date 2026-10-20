//! The stage threads.

use std::collections::VecDeque;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bd_commands::{CommandOutput, CommandPass};
use bd_core::{AudioSegment, BoxError, Cleanup, CleanupRequest, Style};
use bd_vocab::Vocabulary;
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::compose::{Composer, join, single_return_paragraphs};
use crate::gain::InputGain;
mod cleaner;
mod edit;
mod gate;
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
mod rewrite;
mod tidy;
mod wake;

use gate::{Gate, Step};
use crate::{Event, FocusProbe, Input, PadAction, Parts, PipelineConfig, Recognizer, Stage};
use pace::{BACKLOG, Waited};
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
    /// a session's first segment).
    Text { id: u64, text: String, pause: Option<Duration> },
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
    /// A list item: typed as it is, not tidied.
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
                let (take_back, tail) = join(&p.out.text, &next.out.text);
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

    fn finish(&mut self, p: Pending, input: &Receiver<Heard>) {
        self.in_flight = Some(InFlight { id: p.id, quote_before: p.quote_before, cancelled: false });
        let ends_sentence = p.ends_sentence();
        let list = p.list;
        let commanded = p.out.text;
        let behind = self.queue.iter().any(|item| matches!(item, Item::Segment(_)));
        let (text, cleanup) = if self.types_live() || list {
            // Typed as dictated now; tidied in place once it comes back.
            (commanded.clone(), None)
        } else if behind && self.cleanup.is_some() && p.ready.elapsed() > BACKLOG {
            tracing::info!(segment = p.id, waited_ms = p.ready.elapsed().as_millis() as u64, "cleanup is behind; delivered as dictated");
            (commanded.clone(), None)
        } else {
            self.clean(p.id, &commanded)
        };
        // A "scratch that" spoken while this was cleaning still drops it.
        self.take_in_ready(input);
        let flight = self.in_flight.take();
        if flight.is_some_and(|f| f.cancelled) {
            return;
        }
        let text = if self.blank_line_paragraphs { text } else { single_return_paragraphs(&text) };
        let base = self.composer.context().to_string();
        let (take_back, text) = self.composer.append(&text);
        if take_back > 0
            && let Some(last) = self.typed.last_mut()
        {
            // That space is gone from the page, so "scratch that" mustn't count it.
            for _ in 0..take_back {
                last.text.pop();
            }
        }
        if self.types_live() {
            self.typed_live(p.id, p.quote_before, &base, take_back, text);
            if list {
                return;
            }
            return self.tidy_typed(p.id, ends_sentence);
        }
        if text.is_empty() {
            tracing::info!(segment = p.id, "nothing to type (cleaned to nothing)");
        } else {
            if self.typed.len() == MAX_ERASABLE {
                self.typed.remove(0);
            }
            self.typed.push(Typed { id: p.id, text: text.clone(), quote_before: p.quote_before, at: Instant::now() });
            self.emit(Event::Text { id: p.id, take_back, text, commanded, cleanup });
        }
    }

    /// What the recognizer heard in the user's spelling, then with their
    /// special words.
    pub(super) fn own_words(&self, heard: &str) -> String {
        self.vocabulary.apply(&bd_vocab::respell(heard, self.style.spelling, &self.vocabulary.spellings()))
    }

    fn clean(&mut self, id: u64, commanded: &str) -> (String, Option<Duration>) {
        let Some(cleanup) = self.cleanup.as_mut() else {
            return (commanded.to_string(), None);
        };
        let started = Instant::now();
        // Without the trailing space typed for the next segment.
        let context = self.composer.recent(self.context_window, self.cleanup_context_chars).trim_end_matches(' ');
        let names = self.vocabulary.spellings();
        let request = CleanupRequest { context, raw: commanded, style: &self.style, names: &names };
        let result = cleanup.clean(&request);
        let elapsed = started.elapsed();
        let report = cleanup.last_report();
        let context = context.to_string();
        match result.map(|text| bd_vocab::respell(&text, self.style.spelling, &names)) {
            Ok(text) => {
                self.note_cleaned(vec![id], context, commanded, &text, elapsed, report, false);
                (text, Some(elapsed))
            }
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
        if let Heard::Text { id, .. } = &heard
            && self.speaking == Some(*id)
        {
            self.speaking = None;
        }
        let (id, text, pause) = match heard {
            Heard::Text { id, text, pause } => (id, text, pause),
            Heard::EndSession => return self.queue.push_back(Item::EndSession),
            Heard::Reconfigure(config) => return self.reconfigure(*config),
            Heard::Partial { id, text, settled } => {
                self.speaking = (!text.is_empty()).then_some(id);
                // Typed a sentence at a time, it only says someone's speaking.
                if self.types_live() || self.pad.is_some() {
                    self.queue.push_back(Item::Partial(id, text, settled));
                }
                return;
            }
            Heard::Pad(action) => return self.pad_action(action),
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

    fn take_in_text(&mut self, id: u64, heard: String, pause: Option<Duration>) {
        if self.pad.is_none() {
            self.new_passage_if_moved_on();
        }
        let text = self.own_words(&heard);
        let (text, pad_action) = self.pad_command(text);
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
        self.note_commanded(id, &heard, &text, &out, pad_action.as_ref());
        let list = self.list_mode && out.has_words();
        if list {
            out.text = list::item(&out.text, &out.actions, self.passage_has_text, &self.vocabulary.spellings());
        } else if std::mem::take(&mut self.after_list) && out.has_words() && !out.text.starts_with('\n') {
            out.text.insert(0, '\n');
        }
        let long_pause = !list && matches!((pause, self.pause_paragraph), (Some(p), Some(min)) if p >= min);
        if long_pause && self.passage_has_text && !out.text.starts_with('\n') && out.has_words() {
            out.text.insert_str(0, "\n\n");
        }
        if out.has_words() {
            self.passage_has_text = true;
        }
        // Live text may be on the page already: an utterance that comes to
        // nothing still has to take it back.
        let typed_live = self.types_live();
        if !out.text.is_empty() || typed_live {
            if self.pad.is_some() {
                self.pad_push(id, out.text, quote_before);
            } else {
                self.queue.push_back(Item::Segment(Pending { id, quote_before, out, ready: Instant::now(), list }));
            }
        }
        match pad_action {
            Some(PadAction::List(on)) => self.set_list(on),
            Some(action) if self.pad.is_some() => self.pad_action(action),
            // In order, after what was said before it.
            Some(action) => self.queue.push_back(Item::Edit(action)),
            None => {}
        }
    }

    /// Segments already taken in keep the settings they were read with.
    fn reconfigure(&mut self, config: PipelineConfig) {
        self.vocabulary = config.vocabulary;
        self.pass = CommandPass::new(config.commands, config.style.clone());
        self.style = config.style;
        self.pause_paragraph = config.pause_paragraph;
        self.sentence_hold = config.sentence_hold;
        self.sentence_hold_speaking = config.sentence_hold_speaking;
        self.live_typing = config.live_typing;
        self.list_mode = config.list_mode;
        if !(self.live && self.live_typing && !config.pad) {
            self.tidy_stop();
        }
        self.context_window = config.context_window;
        self.cleanup_context_chars = config.cleanup_context_chars;
        self.passage_gap = config.passage_gap;
        self.blank_line_paragraphs = config.blank_line_paragraphs;
        self.gate.configure(config.voice_wake);
        self.set_pad(config.pad);
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
