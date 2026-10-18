//! Pipeline behaviour with scripted stages: one pushed chunk is one segment,
//! and its first sample picks the line the fake ASR "hears". A chunk whose
//! first sample is negative is silence: it only moves the clock on.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bd_core::{Asr, AudioSegment, BoxError, Cleanup, CleanupRequest};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};

use crate::{Event, Parts, Pipeline, PipelineConfig, Segmenting, Stage};

struct ChunkSegmenter {
    seq: u64,
    offset: u64,
}

impl Segmenting for ChunkSegmenter {
    fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment> {
        let start_sample = self.offset;
        self.offset += samples.len() as u64;
        if samples[0] < 0.0 {
            return Vec::new();
        }
        let seg = AudioSegment { samples: samples.to_vec(), seq: self.seq, start_sample };
        self.seq += 1;
        vec![seg]
    }

    fn flush(&mut self) -> Vec<AudioSegment> {
        self.seq = 0;
        self.offset = 0;
        Vec::new()
    }
}

struct ScriptAsr(Vec<&'static str>);

impl Asr for ScriptAsr {
    fn transcribe(&self, segment: &AudioSegment) -> Result<String, BoxError> {
        match self.0[segment.samples[0] as usize] {
            "<fail>" => Err("decoder fell over".into()),
            line => Ok(line.to_string()),
        }
    }
}

/// Marks its output so delivery shows it ran; records the context it was
/// given; optionally waits for the test before answering.
struct FakeCleanup {
    contexts: Arc<Mutex<Vec<String>>>,
    gate: Option<(Sender<()>, Receiver<()>)>,
}

impl Cleanup for FakeCleanup {
    fn clean(&mut self, request: &CleanupRequest<'_>) -> Result<String, BoxError> {
        self.contexts.lock().unwrap().push(request.context.to_string());
        if let Some((started, go)) = &self.gate {
            started.send(()).unwrap();
            go.recv().unwrap();
        }
        if request.raw.contains("explode") {
            return Err("model crashed".into());
        }
        Ok(request.raw.replace("Um, w", "W"))
    }
}

struct Rig {
    pipeline: Pipeline,
    contexts: Arc<Mutex<Vec<String>>>,
    started: Receiver<()>,
    go: Sender<()>,
}

fn rig(script: Vec<&'static str>, cleanup: bool, gated: bool) -> Rig {
    rig_with(script, cleanup, gated, PipelineConfig::default())
}

fn rig_with(script: Vec<&'static str>, cleanup: bool, gated: bool, config: PipelineConfig) -> Rig {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started) = unbounded();
    let (go, go_rx) = bounded(0);
    let cleanup: Option<Box<dyn Cleanup>> = cleanup.then(|| {
        Box::new(FakeCleanup { contexts: contexts.clone(), gate: gated.then_some((started_tx, go_rx)) }) as _
    });
    let parts = Parts { segmenter: Box::new(ChunkSegmenter { seq: 0, offset: 0 }), asr: crate::Recognizer::Segments(Box::new(ScriptAsr(script))), cleanup };
    let pipeline = Pipeline::spawn(parts, config).unwrap();
    Rig { pipeline, contexts, started, go }
}

fn say(p: &Pipeline, line: usize) {
    p.push_audio(vec![line as f32; 160]);
}

fn next(rx: &Receiver<Event>) -> Event {
    rx.recv_timeout(Duration::from_secs(5)).expect("pipeline went quiet")
}

/// Reads events up to and including the first that matches.
fn until(rx: &Receiver<Event>, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let e = next(rx);
        let stop = done(&e);
        seen.push(e);
        if stop {
            return seen;
        }
    }
}

fn texts(events: &[Event]) -> String {
    // What ends up on the page: each text after its take-back, without the
    // invisible space after the last one.
    let mut out = String::new();
    for e in events {
        if let Event::Text { take_back, text, .. } = e {
            for _ in 0..*take_back {
                out.pop();
            }
            out.push_str(text);
        }
    }
    out.trim_end_matches(' ').to_string()
}

#[test]
fn segments_are_cleaned_joined_and_delivered_in_order() {
    let r = rig(vec!["It was late.", "Um, we stayed in.", "New paragraph. Morning came."], true, false);
    for i in 0..3 {
        say(&r.pipeline, i);
    }
    r.pipeline.end_session();
    let events = until(r.pipeline.events(), |e| *e == Event::SessionEnded);
    assert_eq!(texts(&events), "It was late. We stayed in.\n\nMorning came.");
    assert_eq!(*r.contexts.lock().unwrap(), ["", "It was late.", "It was late. We stayed in."]);
    let transcribed = events.iter().filter(|e| matches!(e, Event::Transcribed { .. })).count();
    assert_eq!(transcribed, 3);
}

#[test]
fn without_cleanup_the_commanded_text_is_delivered() {
    let r = rig(vec!["Open quote. Hello. Close quote.", "She waved."], false, false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "\u{201C}Hello.\u{201D} She waved.");
    assert!(events.iter().all(|e| !matches!(e, Event::Text { cleanup: Some(_), .. })));
    assert_eq!(events.last(), Some(&Event::SessionEnded));
}

#[test]
fn scratch_that_drops_a_queued_segment() {
    let r = rig(vec!["First.", "Second.", "Scratch that. Third."], true, true);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    r.started.recv().unwrap();
    say(&r.pipeline, 1);
    say(&r.pipeline, 2);
    until(&rx, |e| matches!(e, Event::Transcribed { id: 2, .. }));
    r.go.send(()).unwrap();
    r.started.recv().unwrap();
    r.go.send(()).unwrap();
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert!(events.contains(&Event::Scratched { id: 1 }));
    assert_eq!(texts(&events), "First. Third.");
}

#[test]
fn scratch_that_drops_the_segment_being_cleaned() {
    let r = rig(vec!["Wrong words.", "Scratch that."], true, true);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    r.started.recv().unwrap();
    say(&r.pipeline, 1);
    until(&rx, |e| matches!(e, Event::Transcribed { id: 1, .. }));
    r.go.send(()).unwrap();
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert!(events.contains(&Event::Scratched { id: 0 }));
    assert_eq!(texts(&events), "");
}

#[test]
fn scratch_that_after_delivery_erases_within_the_session() {
    let r = rig(vec!["First.", "Second.", "Scratch that.", "Scratch that.", "Third."], true, false);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    until(&rx, |e| matches!(e, Event::Text { id: 0, .. }));
    say(&r.pipeline, 1);
    until(&rx, |e| matches!(e, Event::Text { id: 1, .. }));
    say(&r.pipeline, 2);
    let events = until(&rx, |e| matches!(e, Event::Erase { .. }));
    assert_eq!(events.last(), Some(&Event::Erase { id: 1, text: "Second. ".into() }));
    say(&r.pipeline, 3);
    let events = until(&rx, |e| matches!(e, Event::Erase { .. }));
    assert_eq!(events.last(), Some(&Event::Erase { id: 0, text: "First. ".into() }));
    say(&r.pipeline, 4);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    // Everything was taken back, so the next sentence starts the text.
    assert_eq!(texts(&events), "Third.");
    assert_eq!(r.contexts.lock().unwrap().last().map(String::as_str), Some(""));
}

#[test]
fn scratch_that_never_reaches_into_an_earlier_dictation() {
    let r = rig(vec!["Typed before the stop.", "Scratch that."], false, false);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    r.pipeline.end_session();
    until(&rx, |e| *e == Event::SessionEnded);
    say(&r.pipeline, 1);
    let events = until(&rx, |e| *e == Event::ScratchTooLate);
    assert!(!events.iter().any(|e| matches!(e, Event::Erase { .. })));
}

#[test]
fn scratching_restores_the_quotation_state() {
    // Segment 1 opens a quotation and is scratched while still queued, so
    // segment 2 must be read as if no quotation were open.
    let line = "Scratch that. She said quote hello end quote.";
    let pass = bd_commands::CommandPass::default();
    let (closed, open) = (pass.apply(line, false).text, pass.apply(line, true).text);
    assert_ne!(closed, open, "the line must read differently inside a quotation");
    let r = rig(vec!["Plain text.", "Open quote. Oops", line], true, true);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    r.started.recv().unwrap();
    say(&r.pipeline, 1);
    say(&r.pipeline, 2);
    until(&rx, |e| matches!(e, Event::Transcribed { id: 2, .. }));
    r.go.send(()).unwrap();
    r.started.recv().unwrap();
    r.go.send(()).unwrap();
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert!(events.contains(&Event::Scratched { id: 1 }));
    assert_eq!(texts(&events), format!("Plain text. {closed}"));
}

#[test]
fn failures_are_reported_and_the_pipeline_carries_on() {
    let r = rig(vec!["<fail>", "This will explode.", "Fine."], true, false);
    for i in 0..3 {
        say(&r.pipeline, i);
    }
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert!(events.iter().any(|e| matches!(e, Event::Error { stage: Stage::Asr, .. })));
    assert!(events.iter().any(|e| matches!(e, Event::Error { stage: Stage::Cleanup, .. })));
    assert_eq!(texts(&events), "This will explode. Fine.");
}

#[test]
fn a_new_passage_forgets_the_context() {
    let r = rig(vec!["Before.", "After."], true, false);
    say(&r.pipeline, 0);
    r.pipeline.new_passage();
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(*r.contexts.lock().unwrap(), ["", ""]);
    assert_eq!(texts(&events), "Before. After.");
}

#[test]
fn finishing_without_audio_since_the_last_session_adds_no_session_end() {
    let r = rig(vec!["Done."], false, false);
    say(&r.pipeline, 0);
    r.pipeline.end_session();
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(events.iter().filter(|e| **e == Event::SessionEnded).count(), 1);
}

#[test]
fn the_vocabulary_fixes_words_before_commands_and_cleanup() {
    let vocabulary = bd_vocab::Vocabulary::parse("wyrmstone: worm stone\nYsolde: Ussel").unwrap();
    let config = PipelineConfig { vocabulary, ..PipelineConfig::default() };
    let r = rig_with(vec!["Open quote. Ussel, take the worm stone. Close quote."], true, false, config);
    say(&r.pipeline, 0);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "\u{201C}Ysolde, take the wyrmstone.\u{201D}");
}

fn pause(p: &Pipeline, secs: f32) {
    p.push_audio(vec![-1.0; (secs * bd_core::SAMPLE_RATE as f32) as usize]);
}

#[test]
fn a_long_pause_starts_a_paragraph() {
    let config = PipelineConfig { pause_paragraph: Some(Duration::from_secs(2)), ..PipelineConfig::default() };
    let r = rig_with(vec!["It was late.", "We stayed in.", "Morning came.", "Fresh start."], false, false, config);
    pause(&r.pipeline, 3.0);
    say(&r.pipeline, 0);
    pause(&r.pipeline, 1.0);
    say(&r.pipeline, 1);
    pause(&r.pipeline, 2.5);
    say(&r.pipeline, 2);
    // A new session and a new passage start without a break.
    r.pipeline.end_session();
    r.pipeline.new_passage();
    pause(&r.pipeline, 5.0);
    say(&r.pipeline, 3);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "It was late. We stayed in.\n\nMorning came. Fresh start.");
    let pauses: Vec<Option<Duration>> = events
        .iter()
        .filter_map(|e| if let Event::Transcribed { pause, .. } = e { Some(*pause) } else { None })
        .collect();
    assert_eq!(pauses[0], None);
    assert_eq!(pauses[2], Some(Duration::from_secs_f32(2.5)));
    assert_eq!(pauses[3], None);
}

#[test]
fn pauses_add_no_paragraphs_unless_asked() {
    let r = rig(vec!["One.", "Two."], false, false);
    say(&r.pipeline, 0);
    pause(&r.pipeline, 10.0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "One. Two.");
}

#[test]
fn an_unfinished_sentence_waits_and_is_cleaned_with_the_rest() {
    let r = rig(vec!["The boy saw the tax man and said,", "Oh no, he's coming.", "He ran."], true, false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    say(&r.pipeline, 2);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "The boy saw the tax man and said, Oh no, he's coming. He ran.");
    // One cleanup call saw the whole sentence.
    assert_eq!(*r.contexts.lock().unwrap(), ["", "The boy saw the tax man and said, Oh no, he's coming."]);
    let ids: Vec<u64> = events.iter().filter_map(|e| if let Event::Text { id, .. } = e { Some(*id) } else { None }).collect();
    assert_eq!(ids, [0, 2]);
}

#[test]
fn an_unfinished_sentence_goes_on_alone_after_the_hold() {
    let config = PipelineConfig { sentence_hold: Duration::from_millis(50), ..PipelineConfig::default() };
    let r = rig_with(vec!["And then,", "Later."], true, false, config);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    let events = until(&rx, |e| matches!(e, Event::Text { .. }));
    assert_eq!(texts(&events), "And then,");
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "Later.");
}

#[test]
fn a_paragraph_break_is_not_joined_across() {
    let r = rig(vec!["He hesitated,", "New paragraph. Morning came."], true, false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "He hesitated,\n\nMorning came.");
    assert_eq!(r.contexts.lock().unwrap().len(), 2);
}

#[test]
fn scratch_that_drops_a_held_half_sentence() {
    let r = rig(vec!["The boy said,", "Scratch that.", "He left."], true, false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    say(&r.pipeline, 2);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert!(events.contains(&Event::Scratched { id: 0 }));
    assert_eq!(texts(&events), "He left.");
}

#[test]
fn reconfiguring_mid_dictation_changes_the_next_segment() {
    let r = rig(vec!["The worm stone glowed.", "The worm stone dimmed."], false, false);
    let rx = r.pipeline.events().clone();
    say(&r.pipeline, 0);
    until(&rx, |e| matches!(e, Event::Text { id: 0, .. }));
    let vocabulary = bd_vocab::Vocabulary::parse("wyrmstone: worm stone").unwrap();
    r.pipeline.reconfigure(PipelineConfig { vocabulary, ..PipelineConfig::default() });
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "The wyrmstone dimmed.");
}

#[test]
fn paragraphs_can_be_one_return() {
    let config = PipelineConfig { blank_line_paragraphs: false, ..PipelineConfig::default() };
    let r = rig_with(vec!["It was late.", "New paragraph. Morning came. New line. Rain."], true, false, config);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "It was late.\nMorning came.\nRain.");
}

#[test]
fn voice_wake_delivers_only_what_is_said_while_awake() {
    let config = PipelineConfig { voice_wake: true, ..PipelineConfig::default() };
    let script = vec!["On the phone.", "Bear, wake up. It was late.", "He left. Bear, go to sleep.", "Still on the phone.", "Hello."];
    let r = rig_with(script, false, false, config);
    for i in 0..4 {
        say(&r.pipeline, i);
    }
    r.pipeline.set_awake(true);
    say(&r.pipeline, 4);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    assert_eq!(texts(&events), "It was late. He left. Hello.");
    let marks: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Woke => Some("woke"),
            Event::Slept => Some("slept"),
            Event::Text { .. } => Some("text"),
            _ => None,
        })
        .collect();
    assert_eq!(marks, ["woke", "text", "text", "slept", "woke", "text"]);
}

#[test]
fn spaces_trail_and_are_taken_back_before_a_break() {
    let r = rig(vec!["It was late.", "New paragraph. Morning came."], false, false);
    say(&r.pipeline, 0);
    say(&r.pipeline, 1);
    let events: Vec<Event> = r.pipeline.finish().try_iter().collect();
    let deliveries: Vec<(usize, String)> = events
        .iter()
        .filter_map(|e| if let Event::Text { take_back, text, .. } = e { Some((*take_back, text.clone())) } else { None })
        .collect();
    // A space after each sentence, so one the author starts with Enter
    // begins clean; taken back before a dictated break.
    assert_eq!(deliveries, [(0, "It was late. ".to_string()), (1, "\n\nMorning came. ".to_string())]);
}

#[path = "tests_pad.rs"]
mod pad;

#[path = "tests_live.rs"]
mod live;
