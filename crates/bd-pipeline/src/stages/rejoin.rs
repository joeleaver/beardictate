//! Taking back a full stop the listener put at a thinking pause, when the
//! next words show the sentence goes on (and, in list mode, keeping an item
//! together over a pause inside it).
//!
//! When nobody speaks again before its deadline, the listener ends the
//! sentence with a mark of its own ([`super::listen`], [`Ending::Closed`]).
//! A thinking pause can be longer than any tolerable wait (a tester's were
//! 2–8 s), so that mark is provisional: if the next utterance comes soon
//! (within [`WINDOW`]), nothing else was typed or said in between, and the
//! words say the sentence goes on, the mark is taken back and the next
//! words read on in lower case ([`Finisher::reopen`]). The words say so
//! when the one before ended on a word a sentence can't end on ("the",
//! "and", "my") or a comma, or the next one starts with a word that only
//! carries a sentence on ("that", "which"). Nothing else: on the tester's
//! pauses neither the cleanup model nor stock Gemma could tell (`eval/rejoin`).
//!
//! In list mode the same rule keeps an item together instead of starting a
//! new one; and an utterance the speaker went on to before the deadline
//! ([`Ending::WentOn`]) is always the rest of the item, unless the
//! recognizer, hearing what followed, ended the one before with . ? or !.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bd_commands::CommandOutput;

use super::Finisher;
use crate::Event;

/// How soon the next words must come for a closed sentence to reopen.
pub(super) const WINDOW: Duration = Duration::from_secs(10);
/// Utterances seen and not yet decided that are remembered.
const NEXT_KEPT: usize = 8;

/// Words a sentence can't end on: after one, a closed pause was the speaker
/// thinking. (The listener's own list, `UNFINISHED`, also has words that
/// end sentences as often as not: "is", "you", "in".)
const DANGLING: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "nor", "to", "of", "with", "from", "into", "because", "if", "when", "while", "my",
    "your", "our", "their", "his", "its", "than", "very",
];
/// Words that carry a sentence on and hardly ever start one.
const CONTINUES: &[&str] = &["that", "which", "whom", "whose", "than", "whereas"];

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

/// Whether, and why, an utterance carries on the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Join {
    No,
    /// List mode: he went on before the deadline, inside one item.
    InItem,
    /// The one before ended on a word a sentence can't end on.
    Dangling,
    /// It starts with a word that only carries a sentence on.
    Continues,
}

impl Join {
    fn how(self) -> &'static str {
        match self {
            Join::No => "no",
            Join::InItem => "in_item",
            Join::Dangling => "dangling",
            Join::Continues => "continues",
        }
    }
}

/// What an utterance taken in leaves for the one after it.
#[derive(Debug, Clone)]
struct Last {
    id: u64,
    ending: Ending,
    at: Instant,
    /// Its last word, commanded.
    last_word: String,
    /// The . ? or ! it ends with, if any, and whether the speaker said it.
    mark: Option<char>,
    said: bool,
    list: bool,
    /// The delivered piece that holds it: a tidy merges a sentence's
    /// pieces under the first one's id.
    typed_id: u64,
}

/// An utterance after `after`, and what was decided for it.
struct Next {
    id: u64,
    after: Option<Last>,
    join: Option<Join>,
    list: bool,
    /// Its join was carried out (or reported, in list mode).
    done: bool,
}

/// A mark taken back, kept so it goes back if what took it comes to nothing.
struct Reopened {
    id: u64,
    typed_id: u64,
    mark: char,
}

pub(super) struct Rejoin {
    /// See [`crate::PipelineConfig::rejoin`].
    after_close: bool,
    /// See [`crate::PipelineConfig::list_join`].
    list_join: bool,
    last: Option<Last>,
    next: VecDeque<Next>,
    reopened: VecDeque<Reopened>,
}

impl Rejoin {
    pub(super) fn new(after_close: bool, list_join: bool) -> Self {
        Self { after_close, list_join, last: None, next: VecDeque::new(), reopened: VecDeque::new() }
    }

    pub(super) fn configure(&mut self, after_close: bool, list_join: bool) {
        (self.after_close, self.list_join) = (after_close, list_join);
    }

    /// The first word of utterance `id` came in: it follows whatever was
    /// taken in last. (Its words may be handled later, after more is taken in.)
    pub(super) fn seen(&mut self, id: u64) {
        if self.next.iter().any(|n| n.id == id) || self.last.as_ref().is_some_and(|l| l.id == id) {
            return;
        }
        if self.next.len() == NEXT_KEPT {
            self.next.pop_front();
        }
        self.next.push_back(Next { id, after: self.last.clone(), join: None, list: false, done: false });
    }

    /// Utterance `id` was taken in, commanded as `out`: what comes next
    /// reads on from it, if it has words and was said in sentence case.
    pub(super) fn taken(&mut self, id: u64, ending: Ending, out: &CommandOutput, list: bool, sentence_case: bool) {
        if !out.has_words() || !sentence_case {
            self.last = None;
            return;
        }
        let body = out.text.trim_end_matches([' ', '\t']);
        let mark = body.chars().last().filter(|c| matches!(c, '.' | '?' | '!'));
        let said = super::list::said_last_mark(&out.text, &out.actions);
        let last_word = body.split_whitespace().last().unwrap_or("").to_string();
        self.last = Some(Last { id, ending, at: Instant::now(), last_word, mark, said, list, typed_id: id });
    }

    /// Something came between (a new passage, a stop, a sleep or wake):
    /// nothing said after it reopens what was said before. An utterance
    /// whose join was already decided was all said before it (its text was
    /// taken in first) and keeps that decision; the stop, handled in its
    /// turn after it, clears what was typed, so a sentence can't reopen
    /// across it anyway ([`Finisher::take_back_mark`]). One only seen so
    /// far (its words still coming) is decided after it, so it follows
    /// nothing.
    pub(super) fn forget(&mut self) {
        self.last = None;
        for n in self.next.iter_mut().filter(|n| n.join.is_none()) {
            n.after = None;
        }
    }

    /// A tidy merged delivered pieces `ids` into one under `into`.
    pub(super) fn merged(&mut self, ids: &[u64], into: u64) {
        let lasts = self.last.iter_mut().chain(self.next.iter_mut().filter_map(|n| n.after.as_mut()));
        for last in lasts.filter(|l| ids.contains(&l.typed_id)) {
            last.typed_id = into;
        }
        for r in self.reopened.iter_mut().filter(|r| ids.contains(&r.typed_id)) {
            r.typed_id = into;
        }
    }

    /// Whether utterance `id`, its commanded text starting `body`, carries
    /// on the one before it: decided the first time it's asked, from the
    /// utterance it was seen after.
    fn decide(&mut self, id: u64, body: &str, list: bool) -> &mut Next {
        let (after_close, list_join) = (self.after_close, self.list_join);
        if !self.next.iter().any(|n| n.id == id) {
            // Seen only now, with nothing before it to read on from.
            self.next.push_back(Next { id, after: None, join: None, list, done: false });
        }
        let next = self.next.iter_mut().find(|n| n.id == id).expect("just made");
        if next.join.is_none() {
            let join = next.after.as_ref().map_or(Join::No, |last| rule(last, body, list, after_close, list_join));
            (next.join, next.list) = (Some(join), list);
        }
        next
    }

    /// The closed sentence utterance `id` reopens, if one is decided and
    /// not done yet: its id, the piece that holds it, and why.
    fn reopening(&mut self, id: u64) -> Option<(u64, u64, Join)> {
        let next = self.next.iter_mut().find(|n| n.id == id && !n.done && !n.list)?;
        let join = next.join.filter(|j| matches!(j, Join::Dangling | Join::Continues))?;
        let last = next.after.as_ref()?;
        next.done = true;
        Some((last.id, last.typed_id, join))
    }

    /// `id` carries nothing on after all: reopening failed, or it scratched
    /// what it would have carried on.
    pub(super) fn refuse(&mut self, id: u64) {
        match self.next.iter_mut().find(|n| n.id == id) {
            Some(next) => next.join = Some(Join::No),
            None => {
                if self.next.len() == NEXT_KEPT {
                    self.next.pop_front();
                }
                self.next.push_back(Next { id, after: None, join: Some(Join::No), list: false, done: false });
            }
        }
    }

    /// Utterance `id` took back `mark` from the end of delivered piece
    /// `typed_id`.
    fn reopened(&mut self, id: u64, typed_id: u64, mark: char) {
        if self.reopened.len() == NEXT_KEPT {
            self.reopened.pop_front();
        }
        self.reopened.push_back(Reopened { id, typed_id, mark });
    }
}

/// Whether the next utterance, starting `body` (commanded), carries on
/// `last`, which it was said after.
fn rule(last: &Last, body: &str, list: bool, after_close: bool, list_join: bool) -> Join {
    let body = body.trim_start_matches([' ', '\t']);
    // A break, a mark said on its own (it takes our mark's place anyway) or
    // an opening quote starts something; so does a change of mode.
    if last.list != list || last.said || !body.starts_with(char::is_alphanumeric) || last.at.elapsed() > WINDOW {
        return Join::No;
    }
    match last.ending {
        Ending::WentOn if list && list_join && last.mark.is_none() => Join::InItem,
        Ending::Closed if after_close && (list || matches!(last.mark, Some('.' | '?'))) => {
            if dangles(&last.last_word) {
                Join::Dangling
            } else if continues(body) {
                Join::Continues
            } else {
                Join::No
            }
        }
        _ => Join::No,
    }
}

fn bare(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase()
}

fn dangles(word: &str) -> bool {
    word.ends_with(',') || DANGLING.contains(&bare(word).as_str())
}

fn continues(body: &str) -> bool {
    body.split_whitespace().next().is_some_and(|w| CONTINUES.contains(&bare(w).as_str()))
}

impl Finisher {
    /// Whether utterance `id`, commanded as `body`, carries on what was
    /// said before it. In list mode that's the whole of it (no new line);
    /// in sentence case, [`Self::reopen`] takes the mark back.
    pub(super) fn joins_on(&mut self, id: u64, body: &str) -> bool {
        let list = self.list_mode;
        let next = self.rejoin.decide(id, body, list);
        let join = next.join.unwrap_or(Join::No);
        let report = (list && join != Join::No && !next.done).then(|| {
            next.done = true;
            next.after.as_ref().map_or(0, |l| l.id)
        });
        if let Some(onto) = report {
            tracing::info!(segment = id, onto, how = join.how(), "list item goes on");
            self.emit(Event::Rejoined { id, onto, how: join.how(), erased: String::new() });
        }
        join != Join::No
    }

    /// Before utterance `id` is typed: if it reopens the sentence before,
    /// takes back that sentence's closing mark on the page, and tidies the
    /// two as one sentence.
    pub(super) fn reopen(&mut self, id: u64) {
        // Words in a case mode, or in the pad, aren't sentences read on
        // (the mode may have been switched since the sentence before).
        if self.case_mode != crate::CaseMode::Normal || self.pad.is_some() {
            return;
        }
        let Some((onto, typed_id, join)) = self.rejoin.reopening(id) else { return };
        match self.take_back_mark(typed_id, id, join) {
            Ok(mark) => {
                tracing::info!(segment = id, onto, how = join.how(), "sentence reopened");
                self.rejoin.reopened(id, typed_id, mark);
                self.tidy_reopen(typed_id);
                self.emit(Event::Rejoined { id, onto, how: join.how(), erased: mark.to_string() });
            }
            Err(why) => {
                tracing::info!(segment = id, onto, why, "sentence left closed");
                self.rejoin.refuse(id);
            }
        }
    }

    /// Takes the closing . or ? off delivered piece `typed_id`, if it's the
    /// last thing typed, still all there and (after a tidy) still ends on
    /// the word that made it `join`; returns the mark.
    fn take_back_mark(&mut self, typed_id: u64, id: u64, join: Join) -> Result<char, &'static str> {
        let last = self.typed.last().filter(|t| t.id == typed_id).ok_or("typed since")?;
        if last.at.elapsed() > WINDOW {
            return Err("typed too long ago");
        }
        let context = self.composer.context();
        if !context.ends_with(&last.text) {
            return Err("not all there");
        }
        let body = last.text.trim_end_matches([' ', '\t']);
        let mut chars = body.chars().rev();
        let (Some(mark @ ('.' | '?')), Some(before)) = (chars.next(), chars.next()) else { return Err("no mark") };
        if !before.is_alphanumeric() {
            return Err("not a closing mark");
        }
        let at = body.len() - 1;
        // The rule read the words as heard; a tidy may have taken the
        // dangling word out since.
        if join == Join::Dangling && !body[..at].split_whitespace().last().is_some_and(dangles) {
            return Err("tidied to an end");
        }
        let cut = context.len() - last.text.len() + at;
        let rest = last.text[at + 1..].to_string();
        self.rewrite_from(cut, &rest, id, &rest, None)?;
        self.typed.last_mut().expect("checked").text.remove(at);
        Ok(mark)
    }

    /// Utterance `id` came to nothing or was scratched: if it had taken
    /// back the mark of the sentence before, that sentence is closed again.
    pub(super) fn restore_mark(&mut self, id: u64) {
        let Some(i) = self.rejoin.reopened.iter().position(|r| r.id == id) else { return };
        let r = self.rejoin.reopened.remove(i).expect("found");
        let Some(last) = self.typed.last().filter(|t| t.id == r.typed_id) else {
            return tracing::info!(segment = id, "the reopened sentence isn't the last typed: left open");
        };
        let context = self.composer.context();
        if !context.ends_with(&last.text) {
            return tracing::info!(segment = id, "the reopened sentence isn't all there: left open");
        }
        let at = last.text.trim_end_matches([' ', '\t']).len();
        let cut = context.len() - last.text.len() + at;
        let rest = format!("{}{}", r.mark, &last.text[at..]);
        match self.rewrite_from(cut, &rest, id, &rest, None) {
            Ok(()) => {
                self.typed.last_mut().expect("checked").text.insert(at, r.mark);
                tracing::info!(segment = id, "mark put back: nothing carried the sentence on");
            }
            Err(why) => tracing::info!(segment = id, why, "mark not put back"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(ending: Ending, text: &str, list: bool) -> Last {
        let mark = text.chars().last().filter(|c| matches!(c, '.' | '?' | '!'));
        let last_word = text.split_whitespace().last().unwrap_or("").to_string();
        Last { id: 1, ending, at: Instant::now(), last_word, mark, said: false, list, typed_id: 1 }
    }

    fn sentence(a: &str, b: &str) -> Join {
        rule(&last(Ending::Closed, a, false), b, false, true, true)
    }

    #[test]
    fn a_closed_sentence_reopens_after_a_word_it_cant_end_on() {
        assert_eq!(sentence("I went down to the.", "shop on Friday"), Join::Dangling);
        assert_eq!(sentence("We wrote to my.", "Brother last week"), Join::Dangling);
        assert_eq!(sentence("It was very.", "Quiet."), Join::Dangling);
    }

    #[test]
    fn or_before_a_word_that_only_carries_one_on() {
        assert_eq!(sentence("It's the sort of tool.", "That lasts for years."), Join::Continues);
        assert_eq!(sentence("The old mill.", "Which burned down"), Join::Continues);
    }

    #[test]
    fn a_finished_sentence_stays_finished() {
        assert_eq!(sentence("The train was late.", "But nobody minded."), Join::No);
        assert_eq!(sentence("We sold the boat.", "That's all there is to it."), Join::No);
        assert_eq!(sentence("Do you.", "Nobody knows."), Join::No);
        // Breaks, marks said on their own and quotes start something new.
        assert_eq!(sentence("I went to the.", "\n\nNext"), Join::No);
        assert_eq!(sentence("I went to the.", "?"), Join::No);
        assert_eq!(sentence("I went to the.", "\u{201C}Hello"), Join::No);
    }

    #[test]
    fn only_a_closed_sentence_reopens_and_only_when_asked() {
        assert_eq!(rule(&last(Ending::WentOn, "I went to the.", false), "shop", false, true, true), Join::No);
        assert_eq!(rule(&last(Ending::Alone, "I went to the.", false), "shop", false, true, true), Join::No);
        assert_eq!(rule(&last(Ending::Closed, "I went to the.", false), "shop", false, false, true), Join::No);
        // A mark the speaker said is theirs.
        let said = Last { said: true, ..last(Ending::Closed, "I went to the.", false) };
        assert_eq!(rule(&said, "shop", false, true, true), Join::No);
        // An exclamation mark is never the listener's.
        assert_eq!(sentence("I went to the!", "shop"), Join::No);
    }

    #[test]
    fn a_list_item_goes_on_when_he_went_on_in_time() {
        let went_on = |a: &str, b: &str| rule(&last(Ending::WentOn, a, true), b, true, false, true);
        assert_eq!(went_on("a tin of", "tomatoes"), Join::InItem);
        assert_eq!(went_on("Is the van booked?", "Ask Sam"), Join::No);
        assert_eq!(went_on("Pack the red tent.", "Then the stove"), Join::No);
        assert_eq!(rule(&last(Ending::WentOn, "a tin of", true), "be", true, false, false), Join::No);
        // A closed item goes on by the sentence rule, if that's on.
        assert_eq!(rule(&last(Ending::Closed, "drive over to.", true), "the farm", true, true, true), Join::Dangling);
        assert_eq!(rule(&last(Ending::Closed, "drive over to.", true), "the farm", true, false, true), Join::No);
        // Not across a change of mode.
        assert_eq!(rule(&last(Ending::WentOn, "a tin of", false), "be", true, false, true), Join::No);
    }

    #[test]
    fn nothing_reopens_after_the_window() {
        let old = Last { at: Instant::now() - WINDOW - Duration::from_millis(1), ..last(Ending::Closed, "to the.", false) };
        assert_eq!(rule(&old, "shop", false, true, true), Join::No);
    }

    #[test]
    fn an_utterance_follows_what_came_before_its_first_word() {
        let mut r = Rejoin::new(true, true);
        let out = |text: &str| CommandOutput { text: text.into(), actions: Vec::new(), quote_open: false };
        r.seen(1);
        r.taken(1, Ending::Closed, &out("I went to the."), false, true);
        r.seen(2);
        // The next one's text comes in before the second one's words are handled.
        r.taken(2, Ending::Closed, &out("shop."), false, true);
        assert_eq!(r.decide(2, "shop", false).join, Some(Join::Dangling));
        assert_eq!(r.reopening(2).map(|(onto, _, _)| onto), Some(1));
        assert_eq!(r.reopening(2), None, "once");
        r.forget();
        r.seen(3);
        assert_eq!(r.decide(3, "which", false).join, Some(Join::No));
    }
}
