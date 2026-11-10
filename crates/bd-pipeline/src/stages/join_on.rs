//! A piece's last touches before it's joined to what's been typed: it reads
//! on from an open sentence in lower case ([`crate::sentence`]), and the
//! special words in it get their own spaces back ([`bd_vocab::Vocabulary::space`]).

use super::Finisher;

impl Finisher {
    /// `text` as it should be joined after `before` (the passage typed so
    /// far). `as_said`: a list item or said in a case mode, whose first
    /// word's case is already what it should be.
    pub(super) fn join_on(&self, before: &str, text: &str, as_said: bool) -> String {
        let text = if as_said { text.to_string() } else { crate::sentence::read_on(before, text, &self.vocabulary.spellings()) };
        self.vocabulary.space(&text)
    }

    /// The text of `next`, a segment joined to the unfinished sentence
    /// `before` before cleanup: read on in lower case, unless the cleanup
    /// model, seeing the sentence whole, is to say ("and said, Oh no" may be
    /// a line to quote).
    pub(super) fn held_on(&self, before: &str, next: &super::Pending) -> String {
        if next.list || self.cleanup.is_some() {
            return next.out.text.clone();
        }
        crate::sentence::read_on(before, &next.out.text, &self.vocabulary.spellings())
    }

    /// Whether what's said now is a list item or in a case mode.
    pub(super) fn as_said(&self) -> bool {
        self.list_mode || self.case_mode != crate::CaseMode::Normal
    }
}
