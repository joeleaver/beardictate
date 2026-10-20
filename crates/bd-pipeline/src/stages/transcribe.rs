//! Stage 2: speech to text, a finished segment at a time ([`transcribe`])
//! or as the speaker talks ([`super::listen`]).

use std::time::{Duration, Instant};

use bd_core::{Asr, AudioSegment, BoxError, SAMPLE_RATE};
use crossbeam_channel::{Receiver, Sender};

use super::{Cut, Heard, noise, wake};
use crate::gain::InputGain;
use crate::voice::VoiceLevel;
use crate::{Event, Stage};

fn secs(samples: u64) -> Duration {
    Duration::from_secs_f64(samples as f64 / f64::from(SAMPLE_RATE))
}

/// What both kinds of recognizer report about a finished segment.
pub(super) struct Report {
    pub(super) events: Sender<Event>,
    pub(super) gain: InputGain,
    /// Whose voice a sentence is (`voice.rs`).
    pub(super) voice: VoiceLevel,
    /// Where the previous segment of this session ended, in session samples.
    pub(super) previous_end: Option<u64>,
}

impl Report {
    /// The finished segment's text, or `None` after reporting a failure.
    pub(super) fn finished(&mut self, id: u64, seg: &AudioSegment, used: f32, started: Instant, text: Result<String, BoxError>) -> Option<Heard> {
        let audio = secs(seg.samples.len() as u64);
        let pause = self.previous_end.map(|end| secs(seg.start_sample.saturating_sub(end)));
        self.previous_end = Some(seg.start_sample + seg.samples.len() as u64);
        // How loud it came in from the microphone, before the gain.
        let level = noise::rms(&seg.samples) / used;
        let text = match text {
            Ok(text) => text,
            Err(e) => {
                let _ = self.events.send(Event::Error { stage: Stage::Asr, message: e.to_string() });
                return None;
            }
        };
        let elapsed = started.elapsed();
        let words = text.split_whitespace().count();
        let far = self.voice.judge(level, words, audio, wake::heard(&text).is_some());
        // Someone else's audio stays on this computer.
        crate::dump::segment(id, &seg.samples, &text, !far);
        let text = if noise::is_noise(&text, audio) {
            tracing::info!(segment = id, ms = audio.as_millis(), "dropped a lone {text:?} from a short burst: noise");
            String::new()
        } else {
            text
        };
        let gain = self.gain.learn(level, if far { 0 } else { words }, audio);
        if far {
            tracing::info!(segment = id, words, "someone else's voice ({:.0} dBFS): not delivered", crate::voice::db(level));
        }
        let _ = self.events.send(Event::Transcribed { id, text: text.clone(), audio, level, gain, pause, elapsed, far });
        let text = if far { String::new() } else { text };
        Some(Heard::Text { id, text, pause })
    }

    /// The control messages both stages pass on alike.
    pub(super) fn control(&mut self, cut: Cut, hotwords: impl FnOnce(&[String], f32) -> Result<(), BoxError>) -> Option<Heard> {
        Some(match cut {
            Cut::EndSession => {
                self.previous_end = None;
                // The next dictation starts with the user speaking.
                self.voice.trust_next();
                Heard::EndSession
            }
            Cut::NewPassage => Heard::NewPassage,
            Cut::Awake(awake) => {
                if awake {
                    self.voice.trust_next();
                }
                Heard::Awake(awake)
            }
            Cut::Pad(action) => Heard::Pad(action),
            Cut::Reconfigure(config) => {
                if let Err(e) = hotwords(&config.vocabulary.hotwords(), config.hotword_boost) {
                    let _ = self.events.send(Event::Error { stage: Stage::Asr, message: format!("hotwords: {e}") });
                }
                self.voice.set_on(config.drop_far_voices);
                Heard::Reconfigure(config)
            }
            Cut::Segment(..) | Cut::Live(..) => return None,
        })
    }
}

/// Stage 2 for a recognizer of finished segments.
pub(super) fn transcribe(asr: Box<dyn Asr>, input: Receiver<Cut>, out: Sender<Heard>, events: Sender<Event>, gain: InputGain, voice: VoiceLevel) {
    let mut report = Report { events, gain, voice, previous_end: None };
    for cut in &input {
        let heard = match cut {
            Cut::Segment(id, seg, used) => {
                let started = Instant::now();
                let text = asr.transcribe(&seg);
                report.finished(id, &seg, used, started, text)
            }
            other => report.control(other, |words, boost| asr.set_hotwords(words, boost)),
        };
        if let Some(heard) = heard {
            let _ = out.send(heard);
        }
    }
}
