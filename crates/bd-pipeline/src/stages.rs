//! The stage threads.

use std::collections::VecDeque;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bd_commands::{CommandOutput, CommandPass};
use bd_core::{Asr, AudioSegment, BoxError, Cleanup, CleanupRequest, SAMPLE_RATE, Style};
use bd_vocab::Vocabulary;
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::compose::Composer;
use crate::{Event, Input, Parts, PipelineConfig, Segmenting, Stage};

/// From the segmenter to the ASR.
enum Cut {
    Segment(u64, AudioSegment),
    EndSession,
    NewPassage,
}

/// From the ASR to the finisher.
enum Heard {
    /// A transcript, with the silence before it in this session (`None` for
    /// a session's first segment).
    Text { id: u64, text: String, pause: Option<Duration> },
    EndSession,
    NewPassage,
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
    let finisher = Finisher::new(cleanup, config, events);
    Ok(vec![
        thread::Builder::new().name("bd-segment".into()).spawn(move || segment(segmenter, input, cut_tx))?,
        thread::Builder::new().name("bd-asr".into()).spawn(move || transcribe(asr, cut_rx, heard_tx, asr_events))?,
        thread::Builder::new().name("bd-finish".into()).spawn(move || finisher.run(heard_rx))?,
    ])
}

/// Stage 1: cut audio into utterances. Numbers segments over the pipeline's
/// life. When the input closes mid-session, cuts what is in progress.
fn segment(mut segmenter: Box<dyn Segmenting>, input: Receiver<Input>, out: Sender<Cut>) {
    let mut next_id = 0u64;
    let mut send_all = |segs: Vec<AudioSegment>, out: &Sender<Cut>| {
        for seg in segs {
            let _ = out.send(Cut::Segment(next_id, seg));
            next_id += 1;
        }
    };
    let mut dirty = false;
    for msg in &input {
        match msg {
            Input::Audio(samples) => {
                dirty = true;
                send_all(segmenter.accept(&samples), &out);
            }
            Input::EndSession => {
                dirty = false;
                send_all(segmenter.flush(), &out);
                let _ = out.send(Cut::EndSession);
            }
            Input::NewPassage => {
                let _ = out.send(Cut::NewPassage);
            }
        }
    }
    if dirty {
        send_all(segmenter.flush(), &out);
        let _ = out.send(Cut::EndSession);
    }
}

/// Stage 2: speech to text.
fn transcribe(asr: Box<dyn Asr>, input: Receiver<Cut>, out: Sender<Heard>, events: Sender<Event>) {
    // Where the previous segment of this session ended, in session samples.
    let mut previous_end: Option<u64> = None;
    let secs = |samples: u64| Duration::from_secs_f64(samples as f64 / f64::from(SAMPLE_RATE));
    for cut in &input {
        let heard = match cut {
            Cut::Segment(id, seg) => {
                let started = Instant::now();
                let audio = secs(seg.samples.len() as u64);
                let pause = previous_end.map(|end| secs(seg.start_sample.saturating_sub(end)));
                previous_end = Some(seg.start_sample + seg.samples.len() as u64);
                match asr.transcribe(&seg) {
                    Ok(text) => {
                        let elapsed = started.elapsed();
                        let _ = events.send(Event::Transcribed { id, text: text.clone(), audio, pause, elapsed });
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
    /// Whether a commanded quotation is open after the last segment taken in.
    quote_open: bool,
    /// Start a paragraph after a pause this long (see
    /// [`PipelineConfig::pause_paragraph`]).
    pause_paragraph: Option<Duration>,
    /// Whether this passage has words yet, so a pause before the first
    /// segment doesn't open with a break.
    passage_has_text: bool,
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
            quote_open: false,
            pause_paragraph: config.pause_paragraph,
            passage_has_text: false,
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
            let Some(item) = self.queue.pop_front() else { continue };
            match item {
                Item::EndSession => self.emit(Event::SessionEnded),
                Item::NewPassage => self.composer.clear(),
                Item::Segment(p) => self.finish(p, &input),
            }
        }
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
        let text = self.composer.append(&text);
        if !text.is_empty() {
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
            Heard::NewPassage => {
                self.quote_open = false;
                self.passage_has_text = false;
                return self.queue.push_back(Item::NewPassage);
            }
        };
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

    /// Drops the most recent undelivered segment of this passage, returning
    /// the quotation state from before it. `None` when there is none left.
    fn drop_latest(&mut self) -> Option<bool> {
        for i in (0..self.queue.len()).rev() {
            match &self.queue[i] {
                Item::NewPassage => return None,
                Item::EndSession => continue,
                Item::Segment(_) => {
                    let Some(Item::Segment(p)) = self.queue.remove(i) else { unreachable!() };
                    self.emit(Event::Scratched { id: p.id });
                    return Some(p.quote_before);
                }
            }
        }
        let flight = self.in_flight.as_mut().filter(|f| !f.cancelled)?;
        flight.cancelled = true;
        let (id, quote_before) = (flight.id, flight.quote_before);
        self.emit(Event::Scratched { id });
        Some(quote_before)
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }
}
