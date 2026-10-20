//! Tidying text typed as it's spoken, in place: each finished sentence goes
//! to the cleanup model on a thread of its own while typing goes on, and
//! when the cleaned sentence comes back the page is taken back to where the
//! two differ and typed again from there, words typed since included.
//!
//! A sentence is swapped only while what was typed is sure to be what's
//! there: every piece of it still delivered in this passage (not scratched
//! or edited, no new passage or stop since), typed in the last
//! [`REWRITE_WINDOW`], and as [`super::rewrite`] checks. Otherwise it stays
//! as typed.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, never};

use super::cleaner::{Cleaned, Cleaner};
use super::rewrite::drop_last;
use super::{Finisher, Typed};
use crate::compose::{join, single_return_paragraphs};
use crate::{Event, Stage};

/// A sentence typed longer ago than this stays as typed: the speaker may
/// have moved on, clicked, or typed.
const REWRITE_WINDOW: Duration = Duration::from_secs(15);

pub(super) struct Tidy {
    cleaner: Cleaner,
    /// Utterances typed since the last sentence went to the model, and when
    /// the first of them was typed.
    open: Vec<u64>,
    open_since: Option<Instant>,
    /// Sentences waiting for the model, oldest first.
    waiting: VecDeque<Vec<u64>>,
    in_flight: Option<Sent>,
}

/// A sentence with the model.
struct Sent {
    ids: Vec<u64>,
    raw: String,
    context: String,
    /// Edited by voice since it was sent: the answer is for old words.
    void: bool,
}

impl Finisher {
    /// Utterance `id` was just typed as spoken: queue it for tidying, as a
    /// sentence once one ends. Does nothing without a cleanup model.
    pub(super) fn tidy_typed(&mut self, id: u64, ends_sentence: bool) {
        if self.tidy.is_none() {
            let Some(engine) = self.cleanup.take() else { return };
            let Some(cleaner) = Cleaner::spawn(engine) else { return };
            self.tidy = Some(Tidy { cleaner, open: Vec::new(), open_since: None, waiting: VecDeque::new(), in_flight: None });
        }
        let tidy = self.tidy.as_mut().expect("just made");
        if tidy.open.is_empty() {
            tidy.open_since = Some(Instant::now());
        }
        tidy.open.push(id);
        if ends_sentence {
            self.close_sentence();
        }
        self.tidy_send();
    }

    /// The model's answers, to wait on with the input.
    pub(super) fn tidy_results(&self) -> Receiver<Cleaned> {
        self.tidy.as_ref().map_or_else(never, |t| t.cleaner.results.clone())
    }

    /// How long until an unfinished sentence goes to the model anyway.
    pub(super) fn tidy_wait(&self) -> Option<Duration> {
        let since = self.tidy.as_ref()?.open_since?;
        let limit = if self.speaking.is_some() { self.sentence_hold_speaking } else { self.sentence_hold };
        Some((since + limit).saturating_duration_since(Instant::now()))
    }

    /// Sends an unfinished sentence that has waited long enough.
    pub(super) fn tidy_due(&mut self) {
        if self.tidy_wait().is_some_and(|left| left.is_zero()) {
            self.close_sentence();
            self.tidy_send();
        }
    }

    /// One of the model's answers.
    pub(super) fn tidied(&mut self, (result, elapsed, report): Cleaned) {
        let Some(sent) = self.tidy.as_mut().and_then(|t| t.in_flight.take()) else { return };
        match result {
            Ok(text) => {
                self.note_cleaned(sent.ids.clone(), sent.context.clone(), &sent.raw, &text, elapsed, report, false);
                let swapped = if sent.void { Err("edited since") } else { self.rewrite(&sent, &text, elapsed) };
                if let Err(why) = swapped {
                    tracing::info!(segment = sent.ids[0], why, "tidied sentence left as typed");
                }
            }
            Err(message) => self.emit(Event::Error { stage: Stage::Cleanup, message }),
        }
        self.tidy_send();
    }

    /// Before typed text is forgotten (a stop, a sleep, the end): tidies
    /// everything typed so far, waiting for the model.
    pub(super) fn tidy_flush(&mut self) {
        self.close_sentence();
        loop {
            self.tidy_send();
            let Some(tidy) = self.tidy.as_ref() else { return };
            if tidy.in_flight.is_none() {
                return;
            }
            match tidy.cleaner.results.recv() {
                Ok(cleaned) => self.tidied(cleaned),
                Err(_) => return,
            }
        }
    }

    /// A new passage: what's typed so far belongs to the old one (perhaps
    /// another app), so it stays as it is.
    pub(super) fn tidy_drop(&mut self) {
        if let Some(tidy) = self.tidy.as_mut() {
            tidy.open.clear();
            tidy.open_since = None;
            tidy.waiting.clear();
        }
    }

    /// Typed pieces `ids` were edited by voice: a tidy of the old words
    /// mustn't undo that.
    pub(super) fn tidy_void(&mut self, ids: &[u64]) {
        let Some(tidy) = self.tidy.as_mut() else { return };
        if let Some(sent) = tidy.in_flight.as_mut()
            && sent.ids.iter().any(|id| ids.contains(id))
        {
            sent.void = true;
        }
    }

    /// Stops tidying (the pad, or typing a sentence at a time, took over)
    /// and hands the model back.
    pub(super) fn tidy_stop(&mut self) {
        if let Some(tidy) = self.tidy.take() {
            self.cleanup = tidy.cleaner.stop();
        }
    }

    fn close_sentence(&mut self) {
        if let Some(tidy) = self.tidy.as_mut()
            && !tidy.open.is_empty()
        {
            tidy.waiting.push_back(std::mem::take(&mut tidy.open));
            tidy.open_since = None;
        }
    }

    /// Gives the model the next waiting sentence, if it's free.
    fn tidy_send(&mut self) {
        loop {
            let Some(tidy) = self.tidy.as_mut() else { return };
            if tidy.in_flight.is_some() {
                return;
            }
            let Some(mut ids) = tidy.waiting.pop_front() else { return };
            // Scratched since: what's left of the sentence.
            ids.retain(|id| self.typed.iter().any(|t| t.id == *id));
            let Some((k, finished)) = self.typed_from(&ids) else { continue };
            let raw: String = self.typed[k..k + ids.len()].iter().map(|t| t.text.as_str()).collect();
            let raw = raw.trim_end_matches([' ', '\t']).to_string();
            if !raw.chars().any(char::is_alphanumeric) {
                continue;
            }
            let end = self.composer.context().len() - finished.len();
            let context = self.composer.recent_before(end, self.context_window, self.cleanup_context_chars).trim_end_matches(' ').to_string();
            let job = (context.clone(), raw.clone(), self.style.clone(), self.vocabulary.spellings());
            let tidy = self.tidy.as_mut().expect("checked above");
            if tidy.cleaner.jobs.send(job).is_ok() {
                tidy.in_flight = Some(Sent { ids, raw, context, void: false });
            }
            return;
        }
    }

    /// Where the typed pieces `ids` start in [`Finisher::typed`], one after
    /// another, with everything typed from there on; `None` unless that is
    /// all there, at the end of what's been delivered.
    fn typed_from(&self, ids: &[u64]) -> Option<(usize, String)> {
        let k = self.typed.iter().position(|t| Some(&t.id) == ids.first())?;
        let run = self.typed.get(k..k + ids.len())?;
        if !run.iter().map(|t| t.id).eq(ids.iter().copied()) {
            return None;
        }
        let finished: String = self.typed[k..].iter().map(|t| t.text.as_str()).collect();
        self.composer.context().ends_with(&finished).then_some((k, finished))
    }

    /// Swaps the tidied sentence in on the page, if that's still safe.
    fn rewrite(&mut self, sent: &Sent, cleaned: &str, elapsed: Duration) -> Result<(), &'static str> {
        let (k, finished) = self.typed_from(&sent.ids).ok_or("not all there any more")?;
        if self.typed[k].at.elapsed() > REWRITE_WINDOW {
            return Err("typed too long ago");
        }
        let tail = self.composer.context();
        let base = &tail[..tail.len() - finished.len()];
        let cleaned = if self.blank_line_paragraphs { cleaned.to_string() } else { single_return_paragraphs(cleaned) };
        let (back, delta) = join(base, &cleaned);
        // Where the old page and the new can first differ: in the text
        // before, when the new sentence takes its trailing space back.
        let cut = drop_last(base, back).len();
        let later: String = self.typed[k + sent.ids.len()..].iter().map(|t| t.text.as_str()).collect();
        // A mark said after the sentence ("…ready" and then "question
        // mark") took the place of the one it ended with: the tidied
        // sentence doesn't bring that back ("ready. ?").
        let delta = if later.starts_with([',', '.', ';', ':', '!', '?', '\u{2026}']) {
            delta.trim_end_matches(' ').trim_end_matches(['.', ',', ';', ':', '!', '?', '\u{2026}']).to_string()
        } else {
            delta
        };
        let id = sent.ids[0];
        self.rewrite_from(cut, &format!("{delta}{later}"), id, &sent.raw, Some(elapsed))?;
        if back > 0
            && let Some(before) = k.checked_sub(1).and_then(|i| self.typed.get_mut(i))
        {
            for _ in 0..back {
                before.text.pop();
            }
        }
        let first = &self.typed[k];
        let merged = Typed { id, text: delta, quote_before: first.quote_before, at: first.at };
        self.typed.splice(k..k + sent.ids.len(), (!merged.text.is_empty()).then_some(merged));
        Ok(())
    }
}
