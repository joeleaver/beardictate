//! In list mode, keeping an item together over a pause inside it.
//!
//! The listener holds its stream open over a pause, and when the next words
//! come before its deadline they go on in the same stream ([`super::listen`],
//! [`Ending::WentOn`]). Each utterance would still start an item of its own;
//! one the speaker went on to in time is the rest of the item instead,
//! unless the recognizer, hearing what followed, ended the one before with
//! . ? or ! (`eval/rejoin/FINDINGS.md`: 17 of 18 items split at a pause kept
//! whole, 1 false join in 39 item ends).

use std::collections::VecDeque;

use bd_commands::CommandOutput;

use super::Finisher;
use crate::Event;

/// Utterances seen and not yet decided that are remembered.
const NEXT_KEPT: usize = 8;

/// How an utterance's sentence was left when the recognizer reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ending {
    /// Finished on its own: a recognizer of whole segments, or a failure.
    Alone,
    /// The next utterance's words came into the same stream, so the
    /// recognizer punctuated it knowing what followed.
    WentOn,
    /// Nobody spoke again in time: the stream was finished and the sentence
    /// ended there, with a mark of the listener's where it had none.
    Closed,
}

/// What an utterance taken in leaves for the one after it.
#[derive(Debug, Clone)]
struct Last {
    id: u64,
    ending: Ending,
    /// The . ? or ! it ends with, if any, and whether the speaker said it.
    mark: Option<char>,
    said: bool,
    list: bool,
}

/// An utterance after `after`, and whether it goes on the item of `after`.
struct Next {
    id: u64,
    after: Option<Last>,
    joins: Option<bool>,
    /// Its join was reported.
    reported: bool,
    /// Its text was taken in: all of it was said by then.
    taken: bool,
}

pub(super) struct ListJoin {
    /// See [`crate::PipelineConfig::list_join`].
    on: bool,
    last: Option<Last>,
    next: VecDeque<Next>,
}

impl ListJoin {
    pub(super) fn new(on: bool) -> Self {
        Self { on, last: None, next: VecDeque::new() }
    }

    pub(super) fn configure(&mut self, on: bool) {
        self.on = on;
    }

    fn push(&mut self, next: Next) {
        if self.next.len() == NEXT_KEPT {
            self.next.pop_front();
        }
        self.next.push_back(next);
    }

    /// The first word of utterance `id` came in: it follows whatever was
    /// taken in last. (Its words may be handled later, after more is taken in.)
    pub(super) fn seen(&mut self, id: u64) {
        if self.next.iter().any(|n| n.id == id) || self.last.as_ref().is_some_and(|l| l.id == id) {
            return;
        }
        let after = self.last.clone();
        self.push(Next { id, after, joins: None, reported: false, taken: false });
    }

    /// Utterance `id` was taken in, commanded as `out`: what comes next may
    /// go on from it, if it has words and could be an item (not in the pad,
    /// nor a pad action).
    pub(super) fn taken(&mut self, id: u64, ending: Ending, out: &CommandOutput, list: bool, joinable: bool) {
        if let Some(next) = self.next.iter_mut().find(|n| n.id == id) {
            next.taken = true;
        }
        if !out.has_words() || !joinable {
            self.last = None;
            return;
        }
        let body = out.text.trim_end_matches([' ', '\t']);
        let mark = body.chars().last().filter(|c| matches!(c, '.' | '?' | '!'));
        let said = super::list::said_last_mark(&out.text, &out.actions);
        self.last = Some(Last { id, ending, mark, said, list });
    }

    /// Something came between (a new passage, a stop, a sleep or wake):
    /// nothing said after it goes on from what was said before. An
    /// utterance whose text was taken in first was all said before it, and
    /// keeps what it follows, whether its join is decided yet or not (one
    /// with no words when taken in is decided by its live words, typed
    /// before the stop is handled). One only seen so far (its words still
    /// coming) is decided after it, so it follows nothing.
    pub(super) fn forget(&mut self) {
        self.last = None;
        for n in self.next.iter_mut().filter(|n| n.joins.is_none() && !n.taken) {
            n.after = None;
        }
    }

    /// Whether utterance `id`, its commanded text starting `body`, goes on
    /// the item before it: decided the first time it's asked, from the
    /// utterance it was seen after.
    fn decide(&mut self, id: u64, body: &str, list: bool) -> &mut Next {
        let on = self.on;
        if !self.next.iter().any(|n| n.id == id) {
            // Seen only now, with nothing before it to go on from.
            self.push(Next { id, after: None, joins: None, reported: false, taken: false });
        }
        let next = self.next.iter_mut().find(|n| n.id == id).expect("just made");
        if next.joins.is_none() {
            next.joins = Some(next.after.as_ref().is_some_and(|last| on && list && rule(last, body)));
        }
        next
    }

    /// `id` goes on nothing after all: it scratched what it would have
    /// gone on.
    pub(super) fn refuse(&mut self, id: u64) {
        match self.next.iter_mut().find(|n| n.id == id) {
            Some(next) => next.joins = Some(false),
            None => self.push(Next { id, after: None, joins: Some(false), reported: false, taken: false }),
        }
    }
}

/// Whether the next list item, starting `body` (commanded), is the rest of
/// `last`, which it was said after.
fn rule(last: &Last, body: &str) -> bool {
    let body = body.trim_start_matches([' ', '\t']);
    // A break, a mark said on its own or an opening quote starts something;
    // so does the list starting since, and a mark the speaker said ends it.
    let starts_something = !last.list || last.said || !body.starts_with(char::is_alphanumeric);
    !starts_something && last.ending == Ending::WentOn && last.mark.is_none()
}

impl Finisher {
    /// Whether utterance `id`, commanded as `body`, goes on the list item
    /// before it (no new line); reported once.
    pub(super) fn joins_on(&mut self, id: u64, body: &str) -> bool {
        let next = self.list_join.decide(id, body, self.list_mode);
        let joins = next.joins == Some(true);
        let report = (joins && !next.reported).then(|| {
            next.reported = true;
            next.after.as_ref().map_or(0, |l| l.id)
        });
        if let Some(onto) = report {
            tracing::info!(segment = id, onto, "list item goes on");
            self.emit(Event::Rejoined { id, onto });
        }
        joins
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(ending: Ending, text: &str, list: bool) -> Last {
        let mark = text.chars().last().filter(|c| matches!(c, '.' | '?' | '!'));
        Last { id: 1, ending, mark, said: false, list }
    }

    fn out(text: &str) -> CommandOutput {
        CommandOutput { text: text.into(), actions: Vec::new(), quote_open: false }
    }

    #[test]
    fn a_list_item_goes_on_when_he_went_on_in_time() {
        let went_on = |a: &str, b: &str| rule(&last(Ending::WentOn, a, true), b);
        assert!(went_on("a tin of", "tomatoes"));
        assert!(!went_on("Is the van booked?", "Ask Sam"));
        assert!(!went_on("Pack the red tent.", "Then the stove"));
        // A break or an opening quote starts something.
        assert!(!went_on("a tin of", "\n\nNext"));
        assert!(!went_on("a tin of", "\u{201C}Hello"));
        // Not after a pause the listener closed, nor across a change of mode.
        assert!(!rule(&last(Ending::Closed, "drive over to.", true), "the farm"));
        assert!(!rule(&last(Ending::Alone, "a tin of", true), "tomatoes"));
        assert!(!rule(&last(Ending::WentOn, "a tin of", false), "tomatoes"));
        // A mark the speaker said is theirs.
        let said = Last { said: true, ..last(Ending::WentOn, "a tin of", true) };
        assert!(!rule(&said, "tomatoes"));
    }

    #[test]
    fn only_in_list_mode_and_only_when_on() {
        let mut j = ListJoin::new(true);
        j.seen(1);
        j.taken(1, Ending::WentOn, &out("a tin of"), false, true);
        j.seen(2);
        assert_eq!(j.decide(2, "tomatoes", false).joins, Some(false));
        let mut j = ListJoin::new(false);
        j.seen(1);
        j.taken(1, Ending::WentOn, &out("a tin of"), true, true);
        j.seen(2);
        assert_eq!(j.decide(2, "tomatoes", true).joins, Some(false));
    }

    #[test]
    fn an_utterance_follows_what_came_before_its_first_word() {
        let mut j = ListJoin::new(true);
        j.seen(1);
        j.taken(1, Ending::WentOn, &out("a tin of"), true, true);
        j.seen(2);
        // The next one's text comes in before the second one's words are handled.
        j.taken(2, Ending::WentOn, &out("tomatoes"), true, true);
        assert_eq!(j.decide(2, "tomatoes", true).joins, Some(true));
        j.forget();
        j.seen(3);
        assert_eq!(j.decide(3, "bread", true).joins, Some(false));
    }

    #[test]
    fn an_utterance_taken_in_before_a_stop_still_follows_what_came_before() {
        let mut j = ListJoin::new(true);
        j.seen(1);
        j.taken(1, Ending::WentOn, &out("a tin of"), true, true);
        // Only a break when taken in: its live words decide it later.
        j.seen(2);
        j.taken(2, Ending::WentOn, &out("\n"), true, true);
        j.forget();
        assert_eq!(j.decide(2, "tomatoes", true).joins, Some(true));
    }
}
