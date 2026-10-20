//! Spoken edits of what was just typed into your app, as in the pad:
//! "bear, change Smith to Smythe", "bear, delete very", "bear, undo". The
//! last match in this passage's recent text is changed on the page
//! ([`super::rewrite`]); "bear, undo" takes the last edit back, or, with
//! none to take back, works like "scratch that".

use std::time::Duration;

use super::{Finisher, Typed};
use crate::pad::edit_text;
use crate::{Event, PadAction};

/// Text typed longer ago than this isn't searched: the speaker means
/// something recent, and the cursor may have moved since.
const EDIT_WINDOW: Duration = Duration::from_secs(60);

/// An edit, to take back: the pieces from the one it changed on, before and
/// after it.
#[derive(Debug)]
pub(super) struct Edit {
    before: Vec<Typed>,
    after: Vec<Typed>,
}

impl Finisher {
    /// One of the pad's commands, said while typing into your app.
    pub(super) fn edit_typed(&mut self, action: PadAction) {
        match action {
            PadAction::Change { from, to } => self.change_typed(&from, &to),
            PadAction::Delete(words) => self.change_typed(&words, ""),
            PadAction::Undo => self.undo_edit(),
            // Not taken from the text outside the pad.
            PadAction::Send | PadAction::Clear | PadAction::List(_) | PadAction::Case(_) => {}
        }
    }

    fn change_typed(&mut self, from: &str, to: &str) {
        for j in (0..self.typed.len()).rev() {
            if self.typed[j].at.elapsed() > EDIT_WINDOW {
                break;
            }
            let mut text = self.typed[j].text.clone();
            if !edit_text(&mut text, from, to) {
                continue;
            }
            let before = self.typed[j..].to_vec();
            let mut after = before.clone();
            after[0].text = text;
            if let Err(why) = self.retype_pieces(&before, &after) {
                tracing::info!(why, "voice edit not made");
                return self.emit(Event::PadNotFound { words: from.to_string() });
            }
            self.edits.push(Edit { before, after });
            return;
        }
        self.emit(Event::PadNotFound { words: from.to_string() });
    }

    fn undo_edit(&mut self) {
        if let Some(edit) = self.edits.pop() {
            match self.retype_pieces(&edit.after, &edit.before) {
                Ok(()) => return,
                Err(why) => tracing::info!(why, "can't undo the voice edit; scratching instead"),
            }
        }
        // Handled in order, everything said before it is on the page: the
        // last of that goes, as "scratch that" would take it.
        let Some(last) = self.typed.pop() else { return self.emit(Event::ScratchTooLate) };
        self.composer.retract(&last.text);
        self.tidy_void(&[last.id]);
        self.quote_open = last.quote_before;
        self.emit(Event::Erase { id: last.id, text: last.text });
    }

    /// Replaces the last typed pieces, `old` (which must be what's there),
    /// with `new`, on the page and in [`Finisher::typed`].
    fn retype_pieces(&mut self, old: &[Typed], new: &[Typed]) -> Result<(), &'static str> {
        let k = self.typed.len().checked_sub(old.len()).ok_or("typed since")?;
        if self.typed[k..] != *old {
            return Err("typed since");
        }
        let finished: String = old.iter().map(|t| t.text.as_str()).collect();
        let tail = self.composer.context();
        if !tail.ends_with(&finished) {
            return Err("not what was typed");
        }
        let cut = tail.len() - finished.len();
        let new_rest: String = new.iter().map(|t| t.text.as_str()).collect();
        let id = new.first().or(old.first()).map_or(0, |t| t.id);
        self.rewrite_from(cut, &new_rest, id, &new_rest, None)?;
        let ids: Vec<u64> = old.iter().map(|t| t.id).collect();
        self.tidy_void(&ids);
        self.typed.splice(k.., new.iter().cloned());
        Ok(())
    }
}
