//! A streaming recognizer, end to end: words typed as they're heard, or
//! shown in the pad.

use bd_core::{LiveAudio, StreamingAsr};

use super::*;
use crate::Recognizer;

/// Each chunk is one utterance; its first sample picks the script line.
/// The utterance is handed out as a start, three more pieces and an end.
struct LiveChunks;

impl Segmenting for LiveChunks {
    fn accept(&mut self, _samples: &[f32]) -> Vec<AudioSegment> {
        Vec::new()
    }
    fn flush(&mut self) -> Vec<AudioSegment> {
        Vec::new()
    }
    fn accept_live(&mut self, samples: &[f32]) -> Vec<LiveAudio> {
        let seg = AudioSegment { samples: samples.to_vec(), seq: 0, start_sample: 0 };
        let more = || LiveAudio::More(samples.to_vec());
        vec![LiveAudio::Start(samples.to_vec()), more(), more(), more(), LiveAudio::End(seg)]
    }
}

/// Script lines are the partials in order, then the final text.
struct Script {
    lines: Vec<Vec<&'static str>>,
    line: usize,
    fed: usize,
}

impl StreamingAsr for Script {
    fn start(&mut self) -> Result<(), BoxError> {
        self.fed = 0;
        Ok(())
    }
    fn feed(&mut self, samples: &[f32]) -> Result<Option<String>, BoxError> {
        self.line = samples[0] as usize;
        let partials = &self.lines[self.line];
        self.fed += 1;
        Ok(partials.get(self.fed - 1).filter(|_| self.fed < partials.len()).map(|s| s.to_string()))
    }
    fn finish(&mut self) -> Result<String, BoxError> {
        Ok(self.lines[self.line].last().unwrap().to_string())
    }
}

fn live_rig(lines: Vec<Vec<&'static str>>, config: PipelineConfig) -> Pipeline {
    live_rig_with(lines, config, None, None)
}

fn live_rig_with(
    lines: Vec<Vec<&'static str>>,
    config: PipelineConfig,
    cleanup: Option<Arc<Mutex<Vec<String>>>>,
    focus: Option<crate::FocusProbe>,
) -> Pipeline {
    let cleanup = cleanup.map(|contexts| Box::new(FakeCleanup { contexts, gate: None }) as Box<dyn Cleanup>);
    let parts = Parts {
        segmenter: Box::new(LiveChunks),
        asr: Recognizer::Live(Box::new(Script { lines, line: 0, fed: 0 })),
        cleanup,
        focus,
    };
    Pipeline::spawn(parts, config).unwrap()
}

/// Typed once, cleaned: the words wait for the end of the sentence.
fn typed_once() -> PipelineConfig {
    PipelineConfig { live_typing: false, ..PipelineConfig::default() }
}

/// The page after the events: typed text, take-backs and erasures.
fn page(events: &[Event]) -> String {
    let mut out = String::new();
    for e in events {
        match e {
            Event::Text { take_back, text, .. } => {
                for _ in 0..*take_back {
                    out.pop();
                }
                out.push_str(text);
            }
            Event::Erase { text, .. } => {
                for _ in text.chars() {
                    out.pop();
                }
            }
            _ => {}
        }
    }
    out.trim_end_matches(' ').to_string()
}

#[test]
fn words_are_typed_while_spoken_and_commands_wait_for_the_next_word() {
    let p = live_rig(
        vec![
            vec!["It was", "It was late new", "It was late. New paragraph.", "It was late. New paragraph. Morning came."],
            vec!["She", "She left", "She left the", "She left the room."],
        ],
        PipelineConfig::default(),
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "It was late.\n\nMorning came. She left the room.");
    // "It" was typed alone, from the text so far, and "new" never was.
    assert!(matches!(events.iter().find(|e| matches!(e, Event::Text { .. })), Some(Event::Text { text, .. }) if text == "It"));
    assert!(events.iter().all(|e| !matches!(e, Event::Text { text, .. } if text.contains("new"))));
}

#[test]
fn a_later_word_that_changes_earlier_ones_retypes_them() {
    let p = live_rig(
        vec![vec!["he said quote", "he said quote get down", "he said quote get down end quote", "He said quote get down end quote."]],
        PipelineConfig::default(),
    );
    say(&p, 0);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "He said \u{201C}get down.\u{201D}");
    assert!(events.iter().any(|e| matches!(e, Event::Erase { .. })));
}

#[test]
fn scratch_that_takes_back_a_live_typed_utterance() {
    let p = live_rig(vec![vec!["Wrong words", "Wrong words here."], vec!["Scratch", "Scratch that."]], PipelineConfig::default());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "");
}

#[test]
fn in_the_pad_the_words_show_while_spoken() {
    let config = PipelineConfig { pad: true, ..PipelineConfig::default() };
    let p = live_rig(vec![vec!["Hello", "Hello there", "Hello there friend", "Hello there, friend."]], config);
    say(&p, 0);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert!(events.iter().any(|e| matches!(e, Event::Pad(v) if v.text() == "Hello there" && v.parts.iter().all(|p| !p.clean))));
    assert!(events.iter().any(|e| matches!(e, Event::Pad(v) if v.text() == "Hello there, friend. ")));
    assert!(events.iter().all(|e| !matches!(e, Event::Text { .. })));
}

#[test]
fn with_voice_wake_a_sentence_starting_with_bear_waits_for_the_end() {
    let config = PipelineConfig { voice_wake: true, ..PipelineConfig::default() };
    let p = live_rig(
        vec![
            vec!["Bear wake", "Bear, wake up.", "Bear, wake up.", "Bear, wake up."],
            vec!["It rained. Bear", "It rained. Bear go to", "It rained. Bear, go to sleep", "It rained. Bear, go to sleep."],
        ],
        config,
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "It rained.");
    assert!(events.iter().all(|e| !matches!(e, Event::Text { text, .. } if text.to_lowercase().contains("bear"))));
}

#[test]
fn without_live_typing_each_sentence_is_cleaned_and_typed_once() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let p = live_rig_with(
        vec![vec!["He said", "He said um", "He said, um, wait", "He said, um, wait,"], vec!["Um, we", "Um, we go", "Um, we go now", "Um, we go now."]],
        typed_once(),
        Some(contexts.clone()),
        None,
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    // One sentence, joined, cleaned once, nothing typed or erased while spoken.
    assert_eq!(page(&events), "He said, um, wait, We go now.");
    assert_eq!(contexts.lock().unwrap().len(), 1);
    assert!(events.iter().all(|e| !matches!(e, Event::Erase { .. })));
    assert_eq!(events.iter().filter(|e| matches!(e, Event::Text { .. })).count(), 1);
}

#[test]
fn another_app_starts_a_new_passage() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let apps = Arc::new(Mutex::new(vec!["mail", "mail", "chat"].into_iter()));
    let focus: crate::FocusProbe = Box::new(move || apps.lock().unwrap().next().map(String::from));
    let p = live_rig_with(
        vec![vec!["One", "One."], vec!["Two", "Two."], vec!["Three", "Three."]],
        typed_once(),
        Some(contexts.clone()),
        Some(focus),
    );
    for line in 0..3 {
        say(&p, line);
    }
    let _ = p.finish();
    assert_eq!(*contexts.lock().unwrap(), ["", "One.", ""]);
}

#[test]
fn a_long_silence_starts_a_new_passage() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let config = PipelineConfig { passage_gap: Duration::from_millis(300), ..typed_once() };
    let p = live_rig_with(vec![vec!["One", "One."], vec!["Two", "Two."], vec!["Three", "Three."]], config, Some(contexts.clone()), None);
    let rx = p.events().clone();
    say(&p, 0);
    say(&p, 1);
    let _ = until(&rx, |e| matches!(e, Event::Text { id: 1, .. }));
    std::thread::sleep(Duration::from_millis(400));
    say(&p, 2);
    let _ = p.finish();
    assert_eq!(*contexts.lock().unwrap(), ["", "One.", ""]);
}
