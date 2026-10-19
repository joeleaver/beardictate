//! Typing a streaming recognizer's words as they are spoken, outside the
//! pad: the finisher's side of [`crate::Recognizer::Live`].
//!
//! Each update types only what changed. The words typed are the ones that
//! can't change any more: all but the word still being spoken, minus a
//! trailing run that may yet turn out to be a spoken command ("new" before
//! "paragraph", "open" before "quote"), and minus a "bear, …" at the start
//! of a sentence (it may be "bear, go to sleep", or an edit). They
//! go through the user's vocabulary and the command pass first, as the
//! finished utterance will. When a later word changes how earlier ones read
//! (a bare "quote" that an "end quote" makes an opening mark), the page is
//! taken back to where the two differ and typed again from there. The
//! utterance's final text is reconciled the same way.

use bd_commands::{ActionKind, command_tail};

use super::wake::NAMES;
use super::{Finisher, MAX_ERASABLE, Typed};
use crate::Event;
use crate::compose::join;

/// The utterance being typed live.
pub(super) struct Typing {
    id: u64,
    /// Characters of the text before it taken back (a trailing space before
    /// a break or a closing mark).
    pub take_back: usize,
    /// What has been typed of it.
    pub typed: String,
}

impl Finisher {
    /// A streaming recognizer's text so far for utterance `id`.
    /// `settled` once it has ended: its last word is typed too.
    pub(super) fn partial(&mut self, id: u64, text: &str, settled: bool) {
        if !self.gate.awake() {
            return;
        }
        if self.pad.is_some() {
            let shown = self.vocabulary.apply(text);
            return self.pad_live(&shown);
        }
        let Some(body) = self.stable(text, settled) else { return };
        let base = self.composer.context().to_string();
        let (take_back, delta) = join(&base, &body);
        // The space after it comes with the next words, or at the end.
        self.retype(id, &base, take_back, delta.trim_end_matches(' '));
    }

    /// The finished utterance's text, `take_back` and `text` as
    /// [`crate::Composer::append`] gave them after `base`: makes the page
    /// match, and remembers it for "scratch that".
    pub(super) fn typed_live(&mut self, id: u64, quote_before: bool, base: &str, take_back: usize, text: String) {
        self.retype(id, base, take_back, &text);
        self.typing = None;
        if !text.is_empty() {
            if self.typed.len() == MAX_ERASABLE {
                self.typed.remove(0);
            }
            self.typed.push(Typed { id, text, quote_before, at: std::time::Instant::now() });
        }
    }

    /// Utterance `id` was scratched before it finished: drops its live text
    /// still queued and takes back what of it was typed.
    pub(super) fn untype(&mut self, id: u64) {
        self.queue.retain(|item| !matches!(item, super::Item::Partial(i, ..) if *i == id));
        if self.typing.as_ref().is_some_and(|t| t.id == id) {
            let base = self.composer.context().to_string();
            self.retype(id, &base, 0, "");
            self.typing = None;
        }
    }

    /// The part of `text` that won't change, vocabulary and commands
    /// applied; `None` when nothing is sure yet. All of it when `settled`.
    fn stable(&self, text: &str, settled: bool) -> Option<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut n = if settled { words.len() } else { words.len().saturating_sub(1) };
        // A sentence starting "bear, …" may be voice wake's, or an edit
        // ("bear, change Smith to Smythe"): it waits for the utterance's end.
        if let Some(i) = name_at_sentence_start(&words[..n])
        {
            n = i;
        }
        n -= command_tail(&words[..n], &self.pass.config);
        if n == 0 {
            return None;
        }
        let out = self.pass.apply(&self.vocabulary.apply(&words[..n].join(" ")), self.quote_open);
        let scratches = out.actions.iter().any(|a| {
            matches!(a.kind, ActionKind::ScratchPreviousSegment | ActionKind::ScratchWithinSegment { .. })
        });
        // A "scratch that" takes effect when the utterance ends.
        let body = out.text.trim_end_matches([' ', '\t']).to_string();
        (!scratches && (body.chars().any(char::is_alphanumeric) || body.contains('\n'))).then_some(body)
    }

    /// Makes the page after `base` read `base` less `take_back` characters,
    /// then `text`: takes back what differs from what's there, types the rest.
    fn retype(&mut self, id: u64, base: &str, take_back: usize, text: &str) {
        let t = match self.typing.take() {
            Some(t) if t.id == id => t,
            _ => Typing { id, take_back: 0, typed: String::new() },
        };
        let k = t.take_back.max(take_back);
        let tail: Vec<char> = base.chars().rev().take(k).collect::<Vec<_>>().into_iter().rev().collect();
        let keep = |n: usize| tail[..tail.len().saturating_sub(n)].iter().collect::<String>();
        let on_page = keep(t.take_back) + &t.typed;
        let want = keep(take_back) + text;
        let common = on_page.chars().zip(want.chars()).take_while(|(a, b)| a == b).map(|(c, _)| c.len_utf8()).sum::<usize>();
        let (gone, added) = (&on_page[common..], &want[common..]);
        if !gone.is_empty() {
            tracing::info!(segment = id, chars = gone.chars().count(), "retyping live text");
            self.emit(Event::Erase { id, text: gone.to_string() });
        }
        if !added.is_empty() {
            self.emit(Event::Text { id, take_back: 0, text: added.to_string(), commanded: added.to_string(), cleanup: None });
        }
        self.typing = Some(Typing { id, take_back, typed: text.to_string() });
    }
}

/// Where a sentence starts with "bear" (as the recognizer spells it).
fn name_at_sentence_start(words: &[&str]) -> Option<usize> {
    (0..words.len()).find(|&i| {
        let w = words[i].trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        NAMES.contains(&w.as_str()) && (i == 0 || words[i - 1].ends_with(['.', '!', '?']))
    })
}
