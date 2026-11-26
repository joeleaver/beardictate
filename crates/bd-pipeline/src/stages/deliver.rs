//! Delivering a segment: taking back the mark of a sentence it reopens,
//! cleaning it (or typing it as dictated), dropping it if a "scratch that"
//! came while it was with the model, and typing it.

use std::time::{Duration, Instant};

use bd_core::CleanupRequest;
use crossbeam_channel::Receiver;

use super::pace::BACKLOG;
use super::{Finisher, Heard, InFlight, Item, MAX_ERASABLE, Pending, Typed};
use crate::compose::single_return_paragraphs;
use crate::{Event, Stage};

impl Finisher {
    pub(super) fn finish(&mut self, mut p: Pending, input: &Receiver<Heard>) {
        // Before it's cleaned, so the model sees the sentence it goes on.
        if !self.reopen(p.id) && p.paragraph_unless_reopened {
            p.out.text.insert_str(0, "\n\n");
        }
        self.in_flight = Some(InFlight { id: p.id, quote_before: p.quote_before, cancelled: false });
        let ends_sentence = p.ends_sentence();
        let list = p.list;
        let commanded = p.out.text;
        let behind = self.queue.iter().any(|item| matches!(item, Item::Segment(_)));
        let (text, cleanup) = if self.types_live() || list {
            // Typed as dictated now; tidied in place once it comes back.
            (commanded.clone(), None)
        } else if behind && self.cleanup.is_some() && p.ready.elapsed() > BACKLOG {
            tracing::info!(segment = p.id, waited_ms = p.ready.elapsed().as_millis() as u64, "cleanup is behind; delivered as dictated");
            (commanded.clone(), None)
        } else {
            self.clean(p.id, &commanded)
        };
        // A "scratch that" spoken while this was cleaning still drops it.
        self.take_in_ready(input);
        let flight = self.in_flight.take();
        if flight.is_some_and(|f| f.cancelled) {
            // Scratched: its live words go too, and the sentence it had
            // reopened is closed again.
            self.untype(p.id);
            return self.restore_mark(p.id);
        }
        let text = if self.blank_line_paragraphs { text } else { single_return_paragraphs(&text) };
        if !text.chars().any(char::is_alphanumeric) {
            // It came to nothing, or to a break: if its live words had
            // reopened the sentence before, that's closed again before
            // what's left of it is typed after it.
            self.restore_mark(p.id);
        }
        let base = self.composer.context().to_string();
        let text = self.join_on(&base, &text, list);
        let (take_back, text) = self.composer.append(&text);
        if take_back > 0
            && let Some(last) = self.typed.last_mut()
        {
            // That space is gone from the page, so "scratch that" mustn't count it.
            for _ in 0..take_back {
                last.text.pop();
            }
        }
        if self.types_live() {
            self.typed_live(p.id, p.quote_before, &base, take_back, text);
            if list {
                return;
            }
            return self.tidy_typed(p.id, ends_sentence);
        }
        if text.is_empty() {
            tracing::info!(segment = p.id, "nothing to type (cleaned to nothing)");
        } else {
            if self.typed.len() == MAX_ERASABLE {
                self.typed.remove(0);
            }
            self.typed.push(Typed { id: p.id, text: text.clone(), quote_before: p.quote_before, at: Instant::now() });
            self.emit(Event::Text { id: p.id, take_back, text, commanded, cleanup });
        }
    }

    fn clean(&mut self, id: u64, commanded: &str) -> (String, Option<Duration>) {
        let Some(cleanup) = self.cleanup.as_mut() else {
            return (commanded.to_string(), None);
        };
        let started = Instant::now();
        // Without the trailing space typed for the next segment.
        let context = self.composer.recent(self.context_window, self.cleanup_context_chars).trim_end_matches(' ');
        let names = self.vocabulary.spellings();
        let request = CleanupRequest { context, raw: commanded, style: &self.style, names: &names };
        let result = cleanup.clean(&request);
        let elapsed = started.elapsed();
        let report = cleanup.last_report();
        let context = context.to_string();
        match result.map(|text| bd_vocab::respell(&text, self.style.spelling, &names)) {
            Ok(text) => {
                self.note_cleaned(vec![id], context, commanded, &text, elapsed, report, false);
                (text, Some(elapsed))
            }
            Err(e) => {
                self.emit(Event::Error { stage: Stage::Cleanup, message: e.to_string() });
                (commanded.to_string(), Some(elapsed))
            }
        }
    }
}
