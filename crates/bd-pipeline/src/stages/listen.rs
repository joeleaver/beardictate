//! Stage 2 for a streaming recognizer: the text so far while an utterance
//! goes on ([`Heard::Partial`]), its final text when it ends.
//!
//! An utterance ends at a pause, which is as often the middle of a sentence
//! as its end. The recognizer only punctuates a word once it hears what
//! follows, so a stopped utterance is held open ([`StreamingAsr::hold`]) and
//! the next one goes on in the same stream: the stopped one is reported
//! when the next one's first words come in, ending as the recognizer then
//! punctuated it (a full stop, a comma, or nothing when the sentence runs
//! on); when nobody speaks again soon, with a full stop or question mark of
//! ours ([`end_sentence`]). How soon is the user's setting
//! ([`crate::PipelineConfig::sentence_wait`]), longer after a word a sentence
//! doesn't end on; each pause the speaker went on from in time, mid-sentence,
//! is reported ([`Event::Paused`]) to suggest it from.

use std::time::{Duration, Instant};

use bd_commands::{CommandConfig, is_spoken_mark};
use bd_core::{AudioSegment, BoxError, LiveAudio, SAMPLE_RATE, StreamingAsr};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::list_join::Ending;
use super::transcribe::Report;
use super::{Cut, Heard, noise, wake};
use crate::gain::InputGain;
use crate::sentence_wait::{self, Pause};
use crate::voice::VoiceLevel;
use crate::{Event, PipelineConfig, Stage};

/// A stream this long (60 s) is finished at the next pause, not held.
const MAX_HELD: usize = 60 * SAMPLE_RATE as usize;
/// How much of an utterance it takes to tell whose voice it is.
const JUDGE_AFTER: usize = SAMPLE_RATE as usize * 2 / 5;

/// Words a sentence doesn't end on: after one, a pause is the speaker
/// thinking.
const UNFINISHED: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "nor", "to", "of", "with", "for", "in", "on", "at", "from", "by", "into", "about",
    "is", "are", "was", "were", "be", "because", "if", "when", "while", "my", "your", "our", "their", "his", "its", "i",
    "we", "they", "you", "he", "she", "than", "as", "like", "very", "really",
];

/// An utterance that stopped, waiting to hear whether its sentence goes on.
struct Stopped {
    id: u64,
    seg: AudioSegment,
    used: f32,
    started: Instant,
    /// Stream words up to its end.
    end: usize,
    /// When it stopped (the VAD said so), which its wait counts from.
    at: Instant,
    /// It ended on a word a sentence doesn't end on: the longer wait.
    unfinished: bool,
}

/// How the stopped utterance's wait came to an end.
enum Then {
    /// Nobody spoke in time: the stream was finished, the sentence ended at
    /// this stream word.
    Closed(usize),
    /// The next utterance, which started then, under that wait, brought
    /// words.
    WentOn(Instant, Duration),
}

struct Listener {
    asr: Box<dyn StreamingAsr>,
    out: Sender<Heard>,
    report: Report,
    /// Whether a stream is open.
    open: bool,
    /// The stream's text so far.
    text: String,
    /// Stream words already reported.
    done: usize,
    /// Samples fed to the stream.
    fed: usize,
    /// The utterance being spoken, and when it was first heard.
    speaking: Option<(u64, Instant)>,
    stopped: Option<Stopped>,
    /// When the stream was last held.
    held_at: Option<Instant>,
    /// For spoken marks: one said after a pause joins the utterance before.
    commands: CommandConfig,
    /// How long a stopped utterance waits ([`PipelineConfig::sentence_wait`]).
    wait: Duration,
    /// Where its words go: the pad, and typed as they're spoken.
    goes: (bool, bool),
    /// The wait in force when the utterance being spoken started, which
    /// the pause before it was cut off at.
    wait_at_start: Duration,
    /// The utterance being spoken so far: its samples' sum of squares and
    /// count, and the gain they were raised by, to tell whose voice it is.
    loudness: (f64, usize, f32),
}

pub(super) fn live(
    asr: Box<dyn StreamingAsr>,
    input: Receiver<Cut>,
    out: Sender<Heard>,
    events: Sender<Event>,
    gain: InputGain,
    voice: VoiceLevel,
    config: &PipelineConfig,
) {
    let report = Report { events, gain, voice, previous_end: None };
    let mut l = Listener {
        asr,
        out,
        report,
        open: false,
        text: String::new(),
        done: 0,
        fed: 0,
        speaking: None,
        stopped: None,
        held_at: None,
        commands: config.commands.clone(),
        wait: sentence_wait::clamp(config.sentence_wait),
        wait_at_start: sentence_wait::clamp(config.sentence_wait),
        goes: (config.pad, config.live_typing),
        loudness: (0.0, 0, 1.0),
    };
    loop {
        let cut = match l.deadline() {
            Some(at) => match input.recv_deadline(at) {
                Ok(cut) => cut,
                Err(RecvTimeoutError::Timeout) => {
                    l.close(None);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match input.recv() {
                Ok(cut) => cut,
                Err(_) => break,
            },
        };
        l.take(cut);
    }
    l.close(None);
}

impl Listener {
    /// When to stop waiting for the stopped utterance's sentence to go on.
    fn deadline(&self) -> Option<Instant> {
        if self.speaking.is_some() {
            return None;
        }
        match &self.stopped {
            Some(s) => Some(s.at + self.wait_after(s)),
            None => self.open.then(|| self.held_at.unwrap_or_else(Instant::now) + self.wait),
        }
    }

    fn wait_after(&self, s: &Stopped) -> Duration {
        if s.unfinished { sentence_wait::unfinished(self.wait) } else { self.wait }
    }

    /// Whether the stream's word `end` (counting from 1) is one a sentence
    /// doesn't end on.
    fn ends_unfinished(&self, end: usize) -> bool {
        end > 0 && self.text.split_whitespace().nth(end - 1).is_some_and(unfinished)
    }

    fn take(&mut self, cut: Cut) {
        let Cut::Live(id, audio, used) = cut else {
            // A setting changed (the sentence wait slider sends one per
            // step): the sentence waiting goes on waiting, under it, unless
            // its words now go somewhere else (the pad, or typed once), which
            // must not get half of it.
            let stays = match &cut {
                Cut::Reconfigure(config) => {
                    self.commands = config.commands.clone();
                    self.wait = sentence_wait::clamp(config.sentence_wait);
                    let goes = (config.pad, config.live_typing);
                    std::mem::replace(&mut self.goes, goes) == goes
                }
                _ => false,
            };
            if !stays && self.speaking.is_none() {
                self.close(None);
            }
            let asr = &mut self.asr;
            if let Some(heard) = self.report.control(cut, |words, boost| asr.set_hotwords(words, boost)) {
                let _ = self.out.send(heard);
            }
            return;
        };
        if let LiveAudio::Start(_) = audio {
            self.loudness = (0.0, 0, used);
        }
        if let LiveAudio::Start(samples) | LiveAudio::More(samples) = &audio {
            self.loudness.0 += samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
            self.loudness.1 += samples.len();
        }
        match audio {
            LiveAudio::Start(samples) => {
                self.speaking = Some((id, Instant::now()));
                self.wait_at_start = self.wait;
                let started = if self.open {
                    let gap = self.held_at.map_or(Duration::ZERO, |t| t.elapsed());
                    self.asr.resume(gap)
                } else {
                    self.text.clear();
                    (self.done, self.fed) = (0, 0);
                    self.open = true;
                    self.asr.start()
                };
                let text = started.and_then(|()| self.asr.feed(&samples));
                self.fed_in(id, samples.len(), text);
            }
            LiveAudio::More(samples) => {
                let text = self.asr.feed(&samples);
                self.fed_in(id, samples.len(), text);
            }
            LiveAudio::End(seg) => self.end(id, seg, used),
            LiveAudio::Drop => {
                self.speaking = None;
                // What it heard went into the stream: the utterance before
                // it ends there, without what came of the noise.
                let upto = self.stopped.as_ref().map(|s| s.end);
                self.close(upto);
                let _ = self.out.send(Heard::Partial { id, text: String::new(), settled: false });
            }
        }
    }

    fn fed_in(&mut self, id: u64, samples: usize, text: Result<Option<String>, BoxError>) {
        self.fed += samples;
        match text {
            Ok(Some(text)) => {
                self.text = text;
                self.went_on(id);
            }
            Ok(None) => {}
            Err(e) => self.error(e),
        }
    }

    /// The stream's text changed while `id` is spoken: new words settle the
    /// utterance stopped before it, and the rest is `id`'s so far.
    fn went_on(&mut self, id: u64) {
        let n = self.text.split_whitespace().count();
        if self.stopped.as_ref().is_some_and(|s| n > s.end) {
            if self.only_a_mark(n) {
                return;
            }
            let started = self.speaking.map_or_else(Instant::now, |(_, t)| t);
            self.report_stopped(Then::WentOn(started, self.wait_at_start));
        }
        if self.stopped.is_some() {
            return;
        }
        let text = words(&self.text, self.done, n);
        if text.is_empty() || self.far_so_far(&text) {
            return;
        }
        let since_speech = self.speaking.map_or(Duration::ZERO, |(_, t)| t.elapsed());
        let _ = self.report.events.send(Event::Partial { id, text: text.clone(), since_speech });
        let _ = self.out.send(Heard::Partial { id, text, settled: false });
    }

    fn end(&mut self, id: u64, seg: AudioSegment, used: f32) {
        let started = self.speaking.take().map_or_else(Instant::now, |(_, t)| t);
        let held = if self.fed > MAX_HELD { Ok(None) } else { self.asr.hold() };
        let text = match held {
            Ok(Some(text)) => text,
            Ok(None) => {
                // Can't wait for more: finished alone, as it was heard.
                self.stopped = Some(Stopped { id, seg, used, started, end: 0, at: Instant::now(), unfinished: false });
                return self.close(None);
            }
            Err(e) => {
                self.close(None);
                if let Some(heard) = self.report.finished(id, &seg, used, started, Err(e), Ending::Alone) {
                    let _ = self.out.send(heard);
                }
                return;
            }
        };
        self.text = text;
        self.held_at = Some(Instant::now());
        let n = self.text.split_whitespace().count();
        // No new words, but the waiting utterance's last word may read
        // differently now ("river" → "river,"), and with it, how long it waits.
        if let Some(end) = self.stopped.as_ref().map(|s| s.end).filter(|&end| n <= end) {
            let unfinished = self.ends_unfinished(end);
            self.stopped.as_mut().expect("checked").unfinished = unfinished;
        }
        if self.stopped.as_ref().is_some_and(|s| n > s.end) && self.only_a_mark(n) {
            // A mark said on its own ("…so well" and then "exclamation
            // point"): it ends the utterance before, which waits on.
            tracing::info!(segment = id, "a spoken mark joins the utterance before");
            let unfinished = self.ends_unfinished(n);
            let s = self.stopped.as_mut().expect("checked");
            (s.end, s.at, s.unfinished) = (n, Instant::now(), unfinished);
            let (sid, text) = (s.id, words(&self.text, self.done, n));
            let _ = self.out.send(Heard::Partial { id: sid, text, settled: true });
            return;
        }
        if self.stopped.as_ref().is_some_and(|s| n > s.end) {
            self.report_stopped(Then::WentOn(started, self.wait_at_start));
        }
        // An utterance that brought no words leaves the one before waiting.
        if self.stopped.is_some() || n <= self.done {
            tracing::info!(segment = id, "no words heard");
            return;
        }
        let text = words(&self.text, self.done, n);
        // Someone else's voice: nothing to show while it waits.
        let far = self.report.voice.is_far(noise::rms(&seg.samples) / used, wake::heard(&text).is_some());
        let unfinished = self.ends_unfinished(n);
        self.stopped = Some(Stopped { id, seg, used, started, end: n, at: Instant::now(), unfinished });
        if !far {
            let _ = self.out.send(Heard::Partial { id, text, settled: true });
        }
    }

    /// Whether the utterance being spoken, `text` so far, is someone else's
    /// voice (`voice.rs`), once there's enough of it to tell.
    fn far_so_far(&self, text: &str) -> bool {
        let (squares, n, used) = self.loudness;
        if n < JUDGE_AFTER {
            return false;
        }
        let level = (squares / n as f64).sqrt() as f32 / used;
        self.report.voice.is_far(level, wake::heard(text).is_some())
    }

    /// Whether the stream's words after the stopped utterance, up to `n`,
    /// are only a spoken mark or the start of one.
    fn only_a_mark(&self, n: usize) -> bool {
        let Some(s) = &self.stopped else { return false };
        let new: Vec<&str> = self.text.split_whitespace().skip(s.end).take(n - s.end).collect();
        is_spoken_mark(&new, &self.commands)
    }

    /// Reports the stopped utterance: closed, up to that stream word with a
    /// sentence's end added; or, when the speaker went on, as the
    /// recognizer has punctuated it now that the next words came.
    fn report_stopped(&mut self, then: Then) {
        let Some(s) = self.stopped.take() else { return };
        let close = match then {
            Then::Closed(upto) => Some(upto),
            Then::WentOn(..) => None,
        };
        let end = close.unwrap_or(s.end).max(self.done);
        let mut text = words(&self.text, self.done, end);
        if close.is_some() {
            text = end_sentence(text);
        }
        self.done = end;
        let ending = if close.is_some() { Ending::Closed } else { Ending::WentOn };
        let Some(heard) = self.report.finished(s.id, &s.seg, s.used, s.started, Ok(text), ending) else { return };
        if let (Then::WentOn(next, wait), Heard::Text { text, .. }) = (then, &heard) {
            // Someone else's voice (no text) says nothing of the user's pauses.
            let mid_sentence = !text.is_empty() && !text.ends_with(['.', '?', '!']);
            if mid_sentence && !s.unfinished {
                let pause = Pause { pause: next.saturating_duration_since(s.at), wait };
                let _ = self.report.events.send(Event::Paused(pause));
            }
        }
        let _ = self.out.send(heard);
    }

    /// Finishes the stream and reports the utterance waiting on it, ended
    /// there: at stream word `upto`, or after all the words.
    fn close(&mut self, upto: Option<usize>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.held_at = None;
        match self.asr.finish() {
            Ok(text) => self.text = text,
            Err(e) => self.error(e),
        }
        let all = self.text.split_whitespace().count();
        self.report_stopped(Then::Closed(upto.unwrap_or(all).min(all)));
    }

    fn error(&self, e: BoxError) {
        let _ = self.report.events.send(Event::Error { stage: Stage::Asr, message: e.to_string() });
    }
}

/// Words `from..to` of `text`.
fn words(text: &str, from: usize, to: usize) -> String {
    text.split_whitespace().skip(from).take(to.saturating_sub(from)).collect::<Vec<_>>().join(" ")
}

fn unfinished(word: &str) -> bool {
    if word.ends_with(',') {
        return true;
    }
    let w = word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    !word.ends_with(['.', '!', '?']) && UNFINISHED.contains(&w.as_str())
}

/// The end of a sentence the recognizer left unpunctuated (it does with the
/// last words of a stream): a question mark when it reads as a question, a
/// full stop otherwise.
pub(super) fn end_sentence(text: String) -> String {
    let body = text.trim_end();
    match body.chars().last() {
        Some(c) if c.is_alphanumeric() => {
            let start = body.rfind(['.', '!', '?', '\n']).map_or(0, |i| i + 1);
            let mark = if asks(&body[start..]) { '?' } else { '.' };
            format!("{body}{mark}")
        }
        _ => text,
    }
}

/// Verbs that start a question when a subject follows ("can you", "is the").
const AUXILIARIES: &[&str] = &[
    "am", "is", "are", "was", "were", "do", "does", "did", "have", "has", "had", "can", "could", "will", "would", "shall",
    "should", "may", "might", "must", "isn't", "aren't", "wasn't", "weren't", "don't", "doesn't", "didn't", "haven't",
    "hasn't", "can't", "couldn't", "won't", "wouldn't", "shouldn't",
];
const SUBJECTS: &[&str] = &[
    "i", "you", "we", "they", "he", "she", "it", "there", "this", "that", "these", "those", "the", "a", "an", "my", "your",
    "our", "their", "his", "her", "its", "anyone", "anybody", "someone", "somebody", "everyone", "anything", "something",
];
const WH: &[&str] = &["what", "why", "how", "who", "where", "when", "which", "whose"];

/// Whether a sentence reads as a question: it starts with a verb and then
/// its subject ("can you…", "is the…"), or a question word and then a verb
/// ("how do…", "what's…").
fn asks(sentence: &str) -> bool {
    let words: Vec<String> = sentence
        .split_whitespace()
        .take(2)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase().replace('\u{2019}', "'"))
        .collect();
    let (Some(first), second) = (words.first(), words.get(1).map_or("", String::as_str)) else { return false };
    if let Some(wh) = first.strip_suffix("'s").or_else(|| first.strip_suffix("'re")) {
        return WH.contains(&wh);
    }
    if WH.contains(&first.as_str()) {
        // "do" is left out: "it" follows it in orders ("do it now").
        return AUXILIARIES.contains(&second);
    }
    let subject = SUBJECTS.contains(&second) && !(first == "do" && second == "it");
    AUXILIARIES.contains(&first.as_str()) && subject
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_a_statement_with_a_full_stop() {
        assert_eq!(end_sentence("It needs to recognize that I was just".into()), "It needs to recognize that I was just.");
        assert_eq!(end_sentence("Done.".into()), "Done.");
        assert_eq!(end_sentence("Could be worse".into()), "Could be worse.");
        assert_eq!(end_sentence("Do it now".into()), "Do it now.");
        assert_eq!(end_sentence("What I mean is this".into()), "What I mean is this.");
    }

    #[test]
    fn ends_a_question_with_a_question_mark() {
        assert_eq!(end_sentence("I finished. Can you check it before Friday".into()), "I finished. Can you check it before Friday?");
        assert_eq!(end_sentence("Is it possible".into()), "Is it possible?");
        assert_eq!(end_sentence("How do you do that".into()), "How do you do that?");
        assert_eq!(end_sentence("What's the time".into()), "What's the time?");
        assert_eq!(end_sentence("Don't you think".into()), "Don't you think?");
    }

    #[test]
    fn a_dangling_word_waits_longer() {
        assert!(unfinished("the"));
        assert!(unfinished("And"));
        assert!(unfinished("river,"));
        assert!(!unfinished("river"));
        assert!(!unfinished("I."));
    }
}
