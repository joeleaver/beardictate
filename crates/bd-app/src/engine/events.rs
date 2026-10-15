//! What the engine does with each pipeline event: type, erase, report.

use std::time::Instant;

use bd_pipeline::Event;

use super::inject::TypeJob;
use super::{Ui, Worker};

impl<U: Ui> Worker<U> {
    pub(super) fn on_pipeline(&mut self, event: Event) {
        match event {
            Event::Text { id, text, .. } if self.testing => {
                tracing::info!(segment = id, chars = text.chars().count(), "microphone test: not typed");
            }
            Event::Text { id, take_back, text, .. } => {
                tracing::info!(segment = id, chars = text.chars().count(), "typing");
                if take_back > 0 {
                    // The trailing space before a break or a closing mark.
                    let _ = self.type_text.send(TypeJob::Erase(" ".repeat(take_back)));
                }
                let _ = self.type_text.send(TypeJob::Type(text));
            }
            Event::SessionEnded => {
                self.cleaning = false;
                if std::mem::take(&mut self.reopen_mic) && !self.standby {
                    // The next dictation opens the newly chosen microphone.
                    self.capture = None;
                }
                if self.reload_when_idle && !self.busy() {
                    self.start_load();
                    return;
                }
                self.publish();
            }
            Event::Erase { id, text } => {
                tracing::info!(segment = id, chars = text.chars().count(), "erasing (scratch that)");
                let _ = self.type_text.send(TypeJob::Erase(text));
            }
            Event::ScratchTooLate => {
                self.notifier.notify(
                    "scratch",
                    "Nothing to scratch",
                    "\"Scratch that\" only takes back text from the current dictation; earlier text is left as is.",
                );
            }
            Event::Error { stage, message } => {
                self.notifier.notify("pipeline", &format!("{stage:?} failed on a sentence"), &message);
            }
            Event::Woke => {
                tracing::info!("voice wake: awake");
                self.listening_since = Some(Instant::now());
                self.publish();
            }
            Event::Slept => {
                tracing::info!("voice wake: asleep");
                self.listening_since = None;
                self.last_dictation = Some(Instant::now());
                if self.reload_when_idle {
                    self.start_load();
                    return;
                }
                self.publish();
            }
            // Counts only, never the words: enough to see in the log which
            // sentence went missing, and when.
            Event::Transcribed { id, text, audio, level, gain, .. } => {
                let db = 20.0 * level.max(1e-9).log10();
                let words = text.split_whitespace().count();
                tracing::info!(segment = id, words, ms = audio.as_millis(), gain = format!("{:.1}", gain), "heard ({db:.0} dBFS)");
                self.note_speech_level(level, words);
                self.note_gain(gain);
                if self.testing {
                    self.ui.heard(text, level);
                }
            }
            Event::Scratched { id } => tracing::info!(segment = id, "scratched"),
        }
    }

    // ---- settings -----------------------------------------------------------
}
