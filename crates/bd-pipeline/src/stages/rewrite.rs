//! Changing text already typed: the page is taken back to where the old and
//! the new first differ and typed again from there. For a sentence tidied in
//! place ([`super::tidy`]) and for a spoken edit ([`super::edit`]).
//!
//! The page can't be read, only what was typed onto it remembered, so the
//! callers check that what they change is still what was typed (in this
//! passage, recent enough); here, that the app it went to is still in front
//! (where the platform can tell) and that it's not too much to take back.

use std::time::Duration;

use super::Finisher;
use crate::Event;

/// A change that would take back more than this many characters is left
/// undone: too slow to watch, and too much to lose if the page isn't what we
/// think.
const MAX_ERASE: usize = 400;

impl Finisher {
    /// Makes what was delivered read `new_rest` from byte `cut` on (the live
    /// words being typed kept after it), on the page and in the context.
    /// `id` and `commanded` label the text typed.
    pub(super) fn rewrite_from(&mut self, cut: usize, new_rest: &str, id: u64, commanded: &str, cleanup: Option<Duration>) -> Result<(), &'static str> {
        let app = self.focus.as_mut().and_then(|probe| probe());
        if matches!((&app, &self.last_focus), (Some(now), Some(then)) if now != then) {
            return Err("another app is in front");
        }
        let tail = self.composer.context();
        let (live_back, live) = self.typing.as_ref().map_or((0, ""), |t| (t.take_back, t.typed.as_str()));
        let old_page = format!("{}{live}", drop_last(&tail[cut..], live_back));
        let new_page = format!("{}{live}", drop_last(new_rest, live_back));
        let common = old_page.chars().zip(new_page.chars()).take_while(|(a, b)| a == b).map(|(c, _)| c.len_utf8()).sum::<usize>();
        let (gone, added) = (&old_page[common..], &new_page[common..]);
        if gone.chars().count() > MAX_ERASE {
            return Err("too much to take back");
        }
        tracing::info!(segment = id, erased = gone.chars().count(), typed = added.chars().count(), "rewriting typed text");
        if !gone.is_empty() {
            self.emit(Event::Erase { id, text: gone.to_string() });
        }
        if !added.is_empty() {
            self.emit(Event::Text { id, take_back: 0, text: added.to_string(), commanded: commanded.to_string(), cleanup });
        }
        self.composer.replace_from(cut, new_rest);
        Ok(())
    }
}

/// `text` without its last `n` characters.
pub(super) fn drop_last(text: &str, n: usize) -> &str {
    let keep = text.chars().count().saturating_sub(n);
    text.char_indices().nth(keep).map_or(text, |(i, _)| &text[..i])
}
