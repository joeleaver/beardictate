//! Stage 2: speech to text, a finished segment at a time ([`transcribe`])
//! or as the speaker talks ([`live`]).

use std::time::{Duration, Instant};

use bd_core::{Asr, AudioSegment, BoxError, LiveAudio, SAMPLE_RATE, StreamingAsr};
use crossbeam_channel::{Receiver, Sender};

use super::{Cut, Heard, noise};
use crate::gain::InputGain;
use crate::{Event, Stage};

fn secs(samples: u64) -> Duration {
    Duration::from_secs_f64(samples as f64 / f64::from(SAMPLE_RATE))
}

/// What both kinds of recognizer report about a finished segment.
struct Report {
    events: Sender<Event>,
    gain: InputGain,
    /// Where the previous segment of this session ended, in session samples.
    previous_end: Option<u64>,
}

impl Report {
    /// The finished segment's text, or `None` after reporting a failure.
    fn finished(&mut self, id: u64, seg: &AudioSegment, used: f32, started: Instant, text: Result<String, BoxError>) -> Option<Heard> {
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
        crate::dump::segment(id, &seg.samples, &text);
        let text = if noise::is_noise(&text, audio) {
            tracing::info!(segment = id, ms = audio.as_millis(), "dropped a lone {text:?} from a short burst: noise");
            String::new()
        } else {
            text
        };
        let gain = self.gain.learn(level, text.split_whitespace().count(), audio);
        let _ = self.events.send(Event::Transcribed { id, text: text.clone(), audio, level, gain, pause, elapsed });
        Some(Heard::Text { id, text, pause })
    }

    /// The control messages both stages pass on alike.
    fn control(&mut self, cut: Cut, hotwords: impl FnOnce(&[String], f32) -> Result<(), BoxError>) -> Option<Heard> {
        Some(match cut {
            Cut::EndSession => {
                self.previous_end = None;
                Heard::EndSession
            }
            Cut::NewPassage => Heard::NewPassage,
            Cut::Awake(awake) => Heard::Awake(awake),
            Cut::Pad(action) => Heard::Pad(action),
            Cut::Reconfigure(config) => {
                if let Err(e) = hotwords(&config.vocabulary.hotwords(), config.hotword_boost) {
                    let _ = self.events.send(Event::Error { stage: Stage::Asr, message: format!("hotwords: {e}") });
                }
                Heard::Reconfigure(config)
            }
            Cut::Segment(..) | Cut::Live(..) => return None,
        })
    }
}

/// Stage 2 for a recognizer of finished segments.
pub(super) fn transcribe(asr: Box<dyn Asr>, input: Receiver<Cut>, out: Sender<Heard>, events: Sender<Event>, gain: InputGain) {
    let mut report = Report { events, gain, previous_end: None };
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

/// Stage 2 for a streaming recognizer: the text so far while an utterance
/// goes on ([`Heard::Partial`]), its final text when it ends.
pub(super) fn live(mut asr: Box<dyn StreamingAsr>, input: Receiver<Cut>, out: Sender<Heard>, events: Sender<Event>, gain: InputGain) {
    let mut report = Report { events, gain, previous_end: None };
    // When the utterance in progress started being heard.
    let mut started = Instant::now();
    for cut in &input {
        let heard = match cut {
            Cut::Live(id, audio, used) => {
                let partial = match audio {
                    LiveAudio::Start(samples) => {
                        started = Instant::now();
                        asr.start().and_then(|()| asr.feed(&samples))
                    }
                    LiveAudio::More(samples) => asr.feed(&samples),
                    LiveAudio::End(seg) => {
                        let text = asr.finish();
                        if let Some(heard) = report.finished(id, &seg, used, started, text) {
                            let _ = out.send(heard);
                        }
                        continue;
                    }
                    LiveAudio::Drop => asr.finish().map(|_| Some(String::new())),
                };
                match partial {
                    Ok(Some(text)) => Some(Heard::Partial { id, text }),
                    Ok(None) => None,
                    Err(e) => {
                        let _ = report.events.send(Event::Error { stage: Stage::Asr, message: e.to_string() });
                        None
                    }
                }
            }
            other => report.control(other, |words, boost| asr.set_hotwords(words, boost)),
        };
        if let Some(heard) = heard {
            let _ = out.send(heard);
        }
    }
}
