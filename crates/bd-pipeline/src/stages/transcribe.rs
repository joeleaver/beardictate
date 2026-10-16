//! Stage 2: speech to text.

use std::time::{Duration, Instant};

use bd_core::{Asr, SAMPLE_RATE};
use crossbeam_channel::{Receiver, Sender};

use super::{Cut, Heard, noise};
use crate::gain::InputGain;
use crate::{Event, Stage};

/// Stage 2: speech to text.
pub(super) fn transcribe(asr: Box<dyn Asr>, input: Receiver<Cut>, out: Sender<Heard>, events: Sender<Event>, gain: InputGain) {
    // Where the previous segment of this session ended, in session samples.
    let mut previous_end: Option<u64> = None;
    let secs = |samples: u64| Duration::from_secs_f64(samples as f64 / f64::from(SAMPLE_RATE));
    for cut in &input {
        let heard = match cut {
            Cut::Segment(id, seg, used) => {
                let started = Instant::now();
                let audio = secs(seg.samples.len() as u64);
                let pause = previous_end.map(|end| secs(seg.start_sample.saturating_sub(end)));
                previous_end = Some(seg.start_sample + seg.samples.len() as u64);
                // How loud it came in from the microphone, before the gain.
                let level = noise::rms(&seg.samples) / used;
                match asr.transcribe(&seg) {
                    Ok(text) => {
                        let elapsed = started.elapsed();
                        crate::dump::segment(id, &seg.samples, &text);
                        let text = if noise::is_noise(&text, audio) {
                            tracing::info!(segment = id, ms = audio.as_millis(), "dropped a lone {text:?} from a short burst: noise");
                            String::new()
                        } else {
                            text
                        };
                        let gain = gain.learn(level, text.split_whitespace().count(), audio);
                        let _ = events.send(Event::Transcribed { id, text: text.clone(), audio, level, gain, pause, elapsed });
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
            Cut::Awake(awake) => Heard::Awake(awake),
            Cut::Pad(action) => Heard::Pad(action),
            Cut::Reconfigure(config) => {
                if let Err(e) = asr.set_hotwords(&config.vocabulary.hotwords(), config.hotword_boost) {
                    let _ = events.send(Event::Error { stage: Stage::Asr, message: format!("hotwords: {e}") });
                }
                Heard::Reconfigure(config)
            }
        };
        let _ = out.send(heard);
    }
}
