//! A sentence the listener closed at a thinking pause, opened again when
//! the next words carry it on (`stages/rejoin.rs`). The script recognizer
//! can't hold its stream, so every utterance is closed as it ends.

use bd_core::{AudioSegment, LiveAudio};

use super::*;

fn rejoining() -> PipelineConfig {
    PipelineConfig { rejoin: true, ..PipelineConfig::default() }
}

const TO_THE: [&str; 4] = ["I went", "I went down", "I went down to", "I went down to the"];
const SHOP: [&str; 4] = ["Shop", "Shop on", "Shop on Friday", "Shop on Friday"];

fn rejoined(events: &[Event]) -> Vec<(&'static str, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Rejoined { how, erased, .. } => Some((*how, erased.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_sentence_closed_on_a_word_it_cant_end_on_goes_on() {
    let p = live_rig(vec![TO_THE.to_vec(), SHOP.to_vec()], rejoining());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the shop on Friday.");
    assert_eq!(rejoined(&events), vec![("dangling", ".".to_string())]);
}

#[test]
fn or_when_the_next_words_only_carry_one_on() {
    let p = live_rig(vec![vec!["It's the", "It's the sort of", "It's the sort of tool"], vec!["That", "That lasts", "That lasts for years", "That lasts for years."]], rejoining());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "It's the sort of tool that lasts for years.");
    assert_eq!(rejoined(&events), vec![("continues", ".".to_string())]);
}

#[test]
fn off_by_default() {
    let p = live_rig(vec![TO_THE.to_vec(), SHOP.to_vec()], PipelineConfig::default());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the. Shop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_finished_sentence_stays_finished() {
    let p = live_rig(
        vec![vec!["The train", "The train was", "The train was late"], vec!["But", "But nobody", "But nobody minded"]],
        rejoining(),
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "The train was late. But nobody minded.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_break_said_between_leaves_it_closed() {
    let p = live_rig(vec![TO_THE.to_vec(), vec!["New", "New paragraph", "New paragraph."], SHOP.to_vec()], rejoining());
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.\n\nShop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_scratched_sentence_is_not_reopened() {
    let p = live_rig(
        vec![TO_THE.to_vec(), vec!["Scratch", "Scratch that", "Scratch that."], SHOP.to_vec()],
        rejoining(),
    );
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "Shop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn the_mark_goes_back_when_what_reopened_it_comes_to_nothing() {
    let p = live_rig(vec![TO_THE.to_vec(), vec!["Shop", "Shop on", "Shop on", ""]], rejoining());
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.");
}

#[test]
fn and_when_what_reopened_it_is_scratched() {
    let p = live_rig(vec![TO_THE.to_vec(), SHOP.to_vec(), vec!["Scratch", "Scratch that."]], rejoining());
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.");
}

#[test]
fn a_sentence_tidied_to_an_end_is_left_closed() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let config = PipelineConfig { rejoin: true, ..typed_once() };
    let p = live_rig_with(
        vec![vec!["We sailed", "We sailed over", "We sailed over the"], vec!["Lighthouse", "Lighthouse", "Lighthouse"]],
        config,
        Some(contexts),
        None,
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    // The model took "over the" out: "sailed" ends a sentence.
    assert_eq!(page(&events), "We sailed. Lighthouse.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn the_tidy_of_the_closed_sentence_alone_is_void_and_the_two_are_tidied_whole() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started) = unbounded();
    let (go, go_rx) = bounded(0);
    let p = live_rig_gated(
        vec![vec!["Um, we", "Um, we went", "Um, we went to the"], SHOP.to_vec()],
        rejoining(),
        Some(contexts.clone()),
        None,
        Some((started_tx, go_rx)),
    );
    say(&p, 0);
    // The closed sentence is with the model when the next words come.
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    say(&p, 1);
    std::thread::sleep(Duration::from_millis(100));
    go.send(()).unwrap();
    // Then the whole sentence's turn.
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    go.send(()).unwrap();
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "We went to the shop on Friday.");
    assert_eq!(contexts.lock().unwrap().len(), 2);
    assert_eq!(rejoined(&events).len(), 1);
}

#[test]
fn typed_once_the_mark_is_taken_back_before_the_next_words() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let config = PipelineConfig { rejoin: true, ..typed_once() };
    let p = live_rig_with(vec![TO_THE.to_vec(), SHOP.to_vec()], config, Some(contexts), None);
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the shop on Friday.");
    assert_eq!(rejoined(&events), vec![("dangling", ".".to_string())]);
}

#[test]
fn the_mark_goes_back_when_what_reopened_it_turns_out_to_be_a_break() {
    // "New pair" read on from "to the"; the utterance ended "new paragraph".
    let lines = vec![TO_THE.to_vec(), vec!["New", "New pair", "New pair of", "New paragraph."], SHOP.to_vec()];
    let p = live_rig(lines, rejoining());
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.\n\nShop on Friday.");
}

#[test]
fn and_when_what_reopened_it_is_scratched_while_with_the_model() {
    let (started_tx, started) = unbounded();
    let (go, go_rx) = bounded(0);
    let config = PipelineConfig { rejoin: true, ..typed_once() };
    let lines = vec![TO_THE.to_vec(), SHOP.to_vec(), vec!["Scratch", "Scratch that."]];
    let p = live_rig_gated(lines, config, Some(Arc::new(Mutex::new(Vec::new()))), None, Some((started_tx, go_rx)));
    say(&p, 0);
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    go.send(()).unwrap();
    say(&p, 1);
    // The mark is taken back before the next words go to the model.
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    say(&p, 2);
    std::thread::sleep(Duration::from_millis(100));
    go.send(()).unwrap();
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.");
    assert_eq!(rejoined(&events).len(), 1);
}

#[test]
fn typed_once_the_mark_goes_back_when_what_reopened_it_is_cleaned_to_nothing() {
    let config = PipelineConfig { rejoin: true, ..typed_once() };
    let lines = vec![TO_THE.to_vec(), vec!["Oh", "Oh well", "Oh well"]];
    let p = live_rig_with(lines, config, Some(Arc::new(Mutex::new(Vec::new()))), None);
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.");
    assert_eq!(rejoined(&events).len(), 1);
}

#[test]
fn nothing_said_after_going_to_sleep_reopens_what_came_before() {
    let config = PipelineConfig { rejoin: true, voice_wake: true, list_mode: true, ..PipelineConfig::default() };
    let lines = vec![
        vec!["Bear, wake up."],
        vec!["I went down to the. Bear, go to sleep."],
        vec!["Bear, wake up. Shop on Friday"],
    ];
    let p = live_rig(lines, config);
    for line in 0..3 {
        say(&p, line);
    }
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert!(rejoined(&events).is_empty(), "{events:?}");
    assert_eq!(page(&events), "I went down to the\nshop on Friday");
}

/// Like [`LiveChunks`], but a chunk of negative samples is silence that long.
struct PausedChunks {
    at: u64,
}

impl Segmenting for PausedChunks {
    fn accept(&mut self, _samples: &[f32]) -> Vec<AudioSegment> {
        Vec::new()
    }
    fn flush(&mut self) -> Vec<AudioSegment> {
        Vec::new()
    }
    fn accept_live(&mut self, samples: &[f32]) -> Vec<LiveAudio> {
        let start = self.at;
        self.at += samples.len() as u64;
        if samples[0] < 0.0 {
            return Vec::new();
        }
        let seg = AudioSegment { samples: samples.to_vec(), seq: 0, start_sample: start };
        let more = || LiveAudio::More(samples.to_vec());
        vec![LiveAudio::Start(samples.to_vec()), more(), more(), more(), LiveAudio::End(seg)]
    }
}

fn paused_rig(lines: Vec<Vec<&'static str>>, config: PipelineConfig, gate: Option<(Sender<()>, Receiver<()>)>) -> Pipeline {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let parts = Parts {
        segmenter: Box::new(PausedChunks { at: 0 }),
        asr: Recognizer::Live(Box::new(Script { lines, line: 0, fed: 0 })),
        cleanup: Some(Box::new(FakeCleanup { contexts, gate })),
        focus: None,
    };
    Pipeline::spawn(parts, config).unwrap()
}

fn silence(p: &Pipeline, secs: f32) {
    p.push_audio(vec![-1.0; (secs * bd_core::SAMPLE_RATE as f32) as usize]);
}

fn paragraphs_at_pauses() -> PipelineConfig {
    PipelineConfig { rejoin: true, pause_paragraph: Some(Duration::from_secs(2)), ..typed_once() }
}

#[test]
fn a_long_pause_starts_a_paragraph_when_the_sentence_isnt_reopened_after_all() {
    let lines = vec![vec!["We sailed", "We sailed over", "We sailed over the"], vec!["Lighthouse", "Lighthouse", "Lighthouse"]];
    let p = paused_rig(lines, paragraphs_at_pauses(), None);
    say(&p, 0);
    silence(&p, 3.0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    // The model took "over the" out, so the sentence stays closed.
    assert_eq!(page(&events), "We sailed.\n\nLighthouse.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn and_when_a_case_mode_came_on_before_it_was_typed() {
    let (started_tx, started) = unbounded();
    let (go, go_rx) = bounded(0);
    let lines = vec![TO_THE.to_vec(), SHOP.to_vec(), vec!["Bear, all", "Bear, all caps."]];
    let p = paused_rig(lines, paragraphs_at_pauses(), Some((started_tx, go_rx)));
    say(&p, 0);
    // While the first sentence is with the model, the next words come
    // after a long pause, then "bear, all caps".
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    silence(&p, 3.0);
    say(&p, 1);
    say(&p, 2);
    std::thread::sleep(Duration::from_millis(100));
    go.send(()).unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    go.send(()).unwrap();
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the.\n\nShop on Friday.");
    assert!(rejoined(&events).is_empty());
}

#[test]
fn a_reopened_sentence_runs_on_over_a_long_pause() {
    let p = paused_rig(vec![TO_THE.to_vec(), SHOP.to_vec()], paragraphs_at_pauses(), None);
    say(&p, 0);
    silence(&p, 3.0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I went down to the shop on Friday.");
}
