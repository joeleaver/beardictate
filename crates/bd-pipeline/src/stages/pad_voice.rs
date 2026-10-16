//! The pad's spoken commands, in Bear Pad mode: "bear, send it", "bear,
//! clear", "bear, undo", "bear, change Smith to Smythe" (or "… to S M Y T H
//! E", spelled), "bear, delete very".
//!
//! Matched like the wake phrases ([`super::wake`]): on the ASR text, word by
//! word, ignoring case and punctuation. A command starts a segment or a
//! sentence and runs to the end of the segment; what comes before it is
//! dictation. A misheard name ("Where, send it") counts only when the
//! command is the whole segment.

use super::wake::{NAMES, SOUND_ALIKES, words};
use crate::PadAction;

/// A pad command found at the end of a segment, and the dictation before it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Heard {
    pub before: String,
    pub action: PadAction,
}

/// Words that may follow "send", "clear" or "undo" without making it prose.
const FILLER: &[&str] = &["it", "that", "this", "now", "please", "the", "pad", "text", "all", "everything", "over"];

pub(super) fn heard(text: &str) -> Option<Heard> {
    let w = words(text);
    (0..w.len()).find_map(|i| {
        let name = w[i].2.as_str();
        let loose = !NAMES.contains(&name);
        if loose && !(i == 0 && SOUND_ALIKES.contains(&name)) {
            return None;
        }
        let before = text[..w[i].0].trim_end_matches([' ', ',', ';', ':', '-', '\u{2014}']);
        if !before.is_empty() && !before.trim_end_matches(['"', '\u{201D}', '\u{2019}']).ends_with(['.', '!', '?', '\u{2026}', '\n']) {
            return None;
        }
        let verb = w.get(i + 1)?.2.as_str();
        let rest = &w[i + 2..];
        let only_filler = rest.iter().all(|x| FILLER.contains(&x.2.as_str()));
        let span = |from: usize, to: usize| text[w[from].0..w[to].1].to_string();
        let action = match verb {
            "send" | "type" | "insert" if only_filler => PadAction::Send,
            "clear" | "empty" if only_filler => PadAction::Clear,
            "start" if rest.first().is_some_and(|x| x.2 == "over") && only_filler => PadAction::Clear,
            "undo" if only_filler => PadAction::Undo,
            "delete" | "cut" | "remove" | "strike" if !rest.is_empty() && !loose => {
                PadAction::Delete(span(i + 2, w.len() - 1))
            }
            "change" | "replace" | "spell" if !loose => {
                let sep: &[&str] = if verb == "replace" { &["with"] } else { &["to", "as", "into"] };
                let j = (i + 3..w.len().saturating_sub(1)).find(|&j| sep.contains(&w[j].2.as_str()))?;
                let to_words = &w[j + 1..];
                let to = if to_words.len() > 1 && to_words.iter().all(|x| x.2.chars().count() == 1) {
                    to_words.iter().map(|x| x.2.as_str()).collect()
                } else {
                    span(j + 1, w.len() - 1)
                };
                PadAction::Change { from: span(i + 2, j - 1), to }
            }
            _ => return None,
        };
        Some(Heard { before: before.to_string(), action })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(text: &str) -> Option<(String, PadAction)> {
        heard(text).map(|h| (h.before, h.action))
    }

    #[test]
    fn send_clear_and_undo() {
        assert_eq!(h("Bear, send it."), Some(("".into(), PadAction::Send)));
        assert_eq!(h("And that was that. Bear, send it."), Some(("And that was that.".into(), PadAction::Send)));
        assert_eq!(h("Bare send."), Some(("".into(), PadAction::Send)));
        assert_eq!(h("Where? Send it."), Some(("".into(), PadAction::Send)));
        assert_eq!(h("Bear, clear the pad."), Some(("".into(), PadAction::Clear)));
        assert_eq!(h("Bear, start over."), Some(("".into(), PadAction::Clear)));
        assert_eq!(h("Bear, undo that."), Some(("".into(), PadAction::Undo)));
    }

    #[test]
    fn edits() {
        let change = |from: &str, to: &str| PadAction::Change { from: from.into(), to: to.into() };
        assert_eq!(h("Bear, change Smith to Smythe."), Some(("".into(), change("Smith", "Smythe"))));
        assert_eq!(h("Bear, change Smith to S-M-Y-T-H-E."), Some(("".into(), change("Smith", "smythe"))));
        assert_eq!(h("Bear, spell Kaitlyn as K A T E L I N."), Some(("".into(), change("Kaitlyn", "katelin"))));
        assert_eq!(h("Bear, replace the old man with Harold."), Some(("".into(), change("the old man", "Harold"))));
        assert_eq!(h("Bear, delete very."), Some(("".into(), PadAction::Delete("very".into()))));
        assert_eq!(h("Bear, change it."), None);
    }

    #[test]
    fn prose_is_left_alone() {
        assert_eq!(h("The bear sent it back."), None);
        assert_eq!(h("Send it, bear."), None);
        assert_eq!(h("I asked the bear, send it to me."), None);
        assert_eq!(h("Bear, send it to the post office."), None);
        assert_eq!(h("Where, delete it."), None, "edits need the name heard right");
        assert_eq!(h("He said where, send it."), None);
    }
}
