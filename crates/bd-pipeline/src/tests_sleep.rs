//! Going to sleep (the hotkey, or the app) while a sentence is still being
//! said, or waiting to hear whether it goes on: it ends there and is typed,
//! as a stop would type it, before the bear sleeps.

use std::time::Instant;

use super::*;

/// An utterance goes on while chunks of speech come (first sample: the
/// script line, as with [`LiveChunks`]) and ends at a chunk of silence (a
/// negative first sample), or when cut.
#[derive(Default)]
struct OpenUtterance {
    /// The utterance in progress: where it started, and its audio.
    open: Option<(u64, Vec<f32>)>,
    pos: u64,
    seq: u64,
}

impl OpenUtterance {
    /// Takes in a chunk: whether it's speech, and the utterance it ended.
    fn take(&mut self, samples: &[f32]) -> (bool, Option<AudioSegment>) {
        let at = self.pos;
        self.pos += samples.len() as u64;
        if samples[0] < 0.0 {
            return (false, self.end());
        }
        self.open.get_or_insert_with(|| (at, Vec::new())).1.extend_from_slice(samples);
        (true, None)
    }

    fn end(&mut self) -> Option<AudioSegment> {
        let (start_sample, samples) = self.open.take()?;
        self.seq += 1;
        Some(AudioSegment { samples, seq: self.seq - 1, start_sample })
    }
}

impl Segmenting for OpenUtterance {
    fn accept(&mut self, samples: &[f32]) -> Vec<AudioSegment> {
        self.take(samples).1.into_iter().collect()
    }
    fn flush(&mut self) -> Vec<AudioSegment> {
        let out = self.cut();
        *self = Self::default();
        out
    }
    fn cut(&mut self) -> Vec<AudioSegment> {
        self.end().into_iter().collect()
    }
    fn accept_live(&mut self, samples: &[f32]) -> Vec<LiveAudio> {
        let started = self.open.is_none();
        match self.take(samples) {
            (true, _) if started => vec![LiveAudio::Start(samples.to_vec())],
            (true, _) => vec![LiveAudio::More(samples.to_vec())],
            (false, ended) => ended.map(LiveAudio::End).into_iter().collect(),
        }
    }
    fn flush_live(&mut self) -> Vec<LiveAudio> {
        self.flush().into_iter().map(LiveAudio::End).collect()
    }
    fn cut_live(&mut self) -> Vec<LiveAudio> {
        self.cut().into_iter().map(LiveAudio::End).collect()
    }
}

/// One stream across utterances: each line is its whole text after each
/// piece fed; holding or finishing gives the line's last.
struct Stream {
    lines: Vec<Vec<&'static str>>,
    line: usize,
    fed: usize,
    text: &'static str,
}

impl StreamingAsr for Stream {
    fn start(&mut self) -> Result<(), BoxError> {
        (self.fed, self.text) = (0, "");
        Ok(())
    }
    fn feed(&mut self, samples: &[f32]) -> Result<Option<String>, BoxError> {
        if samples[0] as usize != self.line {
            (self.line, self.fed) = (samples[0] as usize, 0);
        }
        let line = &self.lines[self.line];
        self.text = line[self.fed.min(line.len() - 1)];
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

fn awake_with(config: PipelineConfig) -> PipelineConfig {
    PipelineConfig { voice_wake: true, ..config }
}

fn rig(lines: Vec<Vec<&'static str>>, config: PipelineConfig) -> Pipeline {
    let parts = Parts {
        segmenter: Box::new(OpenUtterance::default()),
        asr: Recognizer::Live(Box::new(Stream { lines, line: 0, fed: 0, text: "" })),
        cleanup: None,
        focus: None,
    };
    let p = Pipeline::spawn(parts, awake_with(config)).unwrap();
    p.set_awake(true);
    p
}

fn silence(p: &Pipeline) {
    p.push_audio(vec![-1.0; 160]);
}

/// Where in `events` the first that matches is.
fn at(events: &[Event], what: impl Fn(&Event) -> bool) -> usize {
    events.iter().position(what).expect("no such event")
}

fn slept(e: &Event) -> bool {
    matches!(e, Event::Slept)
}

/// The words were heard, and typed, before the bear went to sleep.
fn typed_before_sleep(events: &[Event], heard: &str) {
    let sleep = at(events, slept);
    let transcribed = at(events, |e| matches!(e, Event::Transcribed { text, .. } if text == heard));
    assert!(transcribed < sleep, "{events:?}");
    let Event::Transcribed { id, .. } = events[transcribed] else { unreachable!() };
    let typed = |e: &Event| matches!(e, Event::Text { id: i, .. } if *i == id);
    let last = events.iter().rposition(typed).expect("typed");
    assert!(last < sleep, "typed after sleeping: {events:?}");
}

#[test]
fn one_word_then_sleep_is_typed() {
    let p = rig(vec![vec!["Yes", "Yes"]], PipelineConfig::default());
    say(&p, 0);
    // The hotkey, straight after the word: the VAD hasn't heard its end.
    p.set_awake(false);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "Yes.");
    typed_before_sleep(&events, "Yes.");
}

#[test]
fn a_sentence_waiting_to_go_on_ends_when_the_bear_sleeps() {
    let config = PipelineConfig { sentence_wait: crate::sentence_wait::MAX, ..PipelineConfig::default() };
    let p = rig(vec![vec!["I'm going to", "I'm going to say a", "I'm going to say a sentence"]], config);
    say(&p, 0);
    say(&p, 0);
    silence(&p);
    p.set_awake(false);
    let asleep = Instant::now();
    let events = until(p.events(), slept);
    // At once, not after the 3 s wait.
    assert!(asleep.elapsed() < Duration::from_millis(1500), "took {:?}", asleep.elapsed());
    assert_eq!(page(&events), "I'm going to say a sentence.");
    typed_before_sleep(&events, "I'm going to say a sentence.");
}

#[test]
fn words_typed_live_stay_typed_when_the_bear_sleeps_mid_sentence() {
    let p = rig(
        vec![
            vec!["It was", "It was late", "It was late at", "It was late at night"],
            vec!["On the phone", "On the phone now"],
            vec!["Hello", "Hello"],
        ],
        PipelineConfig::default(),
    );
    for _ in 0..3 {
        say(&p, 0);
    }
    p.set_awake(false);
    // Said asleep: not typed.
    say(&p, 1);
    silence(&p);
    p.set_awake(true);
    say(&p, 2);
    silence(&p);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "It was late at night. Hello.");
    typed_before_sleep(&events, "It was late at night.");
    // "It was late" was typed while spoken, and nothing was taken back.
    let sleep = at(&events, slept);
    assert!(events[..sleep].iter().any(|e| matches!(e, Event::Text { text, .. } if text.contains("late"))));
    assert!(events.iter().all(|e| !matches!(e, Event::Erase { .. })), "{events:?}");
}

#[test]
fn typed_once_a_sentence_cut_by_sleep_is_cleaned_and_typed() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let parts = Parts {
        segmenter: Box::new(OpenUtterance::default()),
        asr: Recognizer::Live(Box::new(Stream { lines: vec![vec!["Um, we", "Um, we go"]], line: 0, fed: 0, text: "" })),
        cleanup: Some(Box::new(FakeCleanup { contexts: contexts.clone(), gate: None })),
        focus: None,
    };
    let p = Pipeline::spawn(parts, awake_with(typed_once())).unwrap();
    p.set_awake(true);
    say(&p, 0);
    p.set_awake(false);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "We go.");
    assert_eq!(contexts.lock().unwrap().len(), 1);
    typed_before_sleep(&events, "Um, we go.");
}

#[test]
fn a_segment_recognizer_hears_an_utterance_cut_by_sleep() {
    let parts = Parts {
        segmenter: Box::new(OpenUtterance::default()),
        asr: Recognizer::Segments(Box::new(ScriptAsr(vec!["Yes."]))),
        cleanup: None,
        focus: None,
    };
    let p = Pipeline::spawn(parts, awake_with(PipelineConfig::default())).unwrap();
    p.set_awake(true);
    say(&p, 0);
    p.set_awake(false);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "Yes.");
    typed_before_sleep(&events, "Yes.");
}

#[test]
fn turning_voice_wake_on_mid_sentence_types_it_whole() {
    let lines = vec![vec!["It was", "It was late", "It was late at night"], vec!["On the phone", "On the phone now"]];
    let parts = Parts {
        segmenter: Box::new(OpenUtterance::default()),
        asr: Recognizer::Live(Box::new(Stream { lines, line: 0, fed: 0, text: "" })),
        cleanup: None,
        focus: None,
    };
    let p = Pipeline::spawn(parts, PipelineConfig::default()).unwrap();
    say(&p, 0);
    say(&p, 0);
    // Turned on, it starts asleep: what is being said ends there.
    p.reconfigure(awake_with(PipelineConfig::default()));
    say(&p, 1);
    silence(&p);
    let events: Vec<Event> = p.finish().try_iter().collect();
    assert_eq!(page(&events), "It was late at night.");
    assert!(events.iter().all(|e| !matches!(e, Event::Erase { .. })), "{events:?}");
}
