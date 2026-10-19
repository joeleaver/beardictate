//! A pause in the middle of a sentence: the recognizer's stream is held
//! open over it, and the next words say how the sentence goes on.

use std::time::Instant;

use super::*;

/// One stream across the utterances. Each line is the stream's whole text
/// after each piece of that utterance; holding gives its last, and the
/// stream finishes as it was.
struct Stream {
    lines: Vec<Vec<&'static str>>,
    line: usize,
    fed: usize,
    text: &'static str,
}

impl StreamingAsr for Stream {
    fn start(&mut self) -> Result<(), BoxError> {
        self.text = "";
        Ok(())
    }
    fn feed(&mut self, samples: &[f32]) -> Result<Option<String>, BoxError> {
        if samples[0] as usize != self.line || self.fed == 0 {
            (self.line, self.fed) = (samples[0] as usize, 0);
        }
        self.text = self.lines[self.line][self.fed.min(self.lines[self.line].len() - 1)];
        self.fed += 1;
        Ok(Some(self.text.to_string()))
    }
    fn finish(&mut self) -> Result<String, BoxError> {
        Ok(self.text.to_string())
    }
    fn hold(&mut self) -> Result<Option<String>, BoxError> {
        self.fed = 0;
        self.text = self.lines[self.line].last().unwrap();
        Ok(Some(self.text.to_string()))
    }
    fn resume(&mut self, _gap: Duration) -> Result<(), BoxError> {
        Ok(())
    }
}

fn rig(lines: Vec<Vec<&'static str>>, config: PipelineConfig, contexts: Option<Arc<Mutex<Vec<String>>>>) -> Pipeline {
    let parts = Parts {
        segmenter: Box::new(LiveChunks),
        asr: Recognizer::Live(Box::new(Stream { lines, line: 0, fed: 0, text: "" })),
        cleanup: contexts.map(|contexts| Box::new(FakeCleanup { contexts, gate: None }) as Box<dyn Cleanup>),
        focus: None,
    };
    Pipeline::spawn(parts, config).unwrap()
}

const PAUSED: [&str; 4] = [
    "I'm going to",
    "I'm going to say a",
    "I'm going to say a sentence with a",
    "I'm going to say a sentence with a pause",
];

#[test]
fn a_pause_mid_sentence_gets_no_full_stop() {
    let p = rig(
        vec![
            PAUSED.to_vec(),
            vec![
                "I'm going to say a sentence with a pause in",
                "I'm going to say a sentence with a pause in the",
                "I'm going to say a sentence with a pause in the middle of",
                "I'm going to say a sentence with a pause in the middle of it",
            ],
        ],
        PipelineConfig::default(),
        None,
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I'm going to say a sentence with a pause in the middle of it.");
    // Nothing was typed to be taken back: no full stop after "pause".
    assert!(events.iter().all(|e| !matches!(e, Event::Erase { .. })));
}

#[test]
fn the_recognizer_says_where_a_sentence_ended() {
    let p = rig(
        vec![
            vec!["I finished", "I finished the", "I finished the report", "I finished the report"],
            vec!["I finished the report. Can", "I finished the report. Can you check it", "I finished the report. Can you check it before Friday"],
        ],
        PipelineConfig::default(),
        None,
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I finished the report. Can you check it before Friday?");
}

#[test]
fn a_last_utterance_ends_its_sentence_after_a_while() {
    let p = rig(vec![PAUSED.to_vec()], PipelineConfig::default(), None);
    say(&p, 0);
    let done = Instant::now();
    let events = until(p.events(), |e| matches!(e, Event::Text { text, .. } if text.contains('.')));
    assert!(done.elapsed() >= Duration::from_millis(1000), "waited {:?}", done.elapsed());
    assert_eq!(page(&events), "I'm going to say a sentence with a pause.");
}

#[test]
fn typed_once_the_halves_are_cleaned_as_one_sentence() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let config = PipelineConfig { live_typing: false, ..PipelineConfig::default() };
    let p = rig(
        vec![PAUSED.to_vec(), vec!["I'm going to say a sentence with a pause in the middle"]],
        config,
        Some(contexts.clone()),
    );
    say(&p, 0);
    say(&p, 1);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "I'm going to say a sentence with a pause in the middle.");
    assert_eq!(contexts.lock().unwrap().len(), 1);
}

#[test]
fn a_mark_said_after_a_pause_takes_the_place_of_the_full_stop() {
    let mut config = PipelineConfig::default();
    config.commands.literal_punctuation = true;
    let p = rig(
        vec![
            vec!["So", "So necessary", "So necessary", "So necessary"],
            vec!["So necessary. Exclamation", "So necessary. Exclamation point", "So necessary. Exclamation point"],
            vec!["So necessary. Exclamation point. Then", "So necessary. Exclamation point. Then we left"],
        ],
        config,
        None,
    );
    say(&p, 0);
    say(&p, 1);
    say(&p, 2);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "So necessary! Then we left.");
}
