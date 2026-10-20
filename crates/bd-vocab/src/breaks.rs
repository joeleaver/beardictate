//! Breaks in a spelling: `<new line>` (also `<newline>`, `<line>`, `\n`) and
//! `<paragraph>` (also `<new paragraph>`, `\n\n`), so a special word can
//! stand for a sign-off on lines of its own. Testers asked (2026-10-20).
//!
//! In the vocabulary pass's output a break is a word of its own,
//! [`bd_core::LINE_BREAK`] or [`bd_core::PARAGRAPH_BREAK`], which the
//! command pass types as a spoken "new line" or "new paragraph" would be.

use bd_core::{LINE_BREAK, PARAGRAPH_BREAK};

const PARAGRAPH: [&str; 3] = ["<paragraph>", "<new paragraph>", "\\n\\n"];
const LINE: [&str; 4] = ["<new line>", "<newline>", "<line>", "\\n"];

/// The spelling with each break as its mark, set apart by spaces.
pub(crate) fn marked(spelling: &str) -> String {
    replace_breaks(spelling, &format!(" {PARAGRAPH_BREAK} "), &format!(" {LINE_BREAK} "))
}

/// The spelling's words alone, breaks taken out: what the recognizer
/// listens for and what the cleanup must keep.
pub(crate) fn plain(spelling: &str) -> String {
    replace_breaks(spelling, " ", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text from the vocabulary pass with its break marks as the breaks
/// themselves, for showing it before the command pass (the Bear Pad's live
/// line).
pub fn breaks_as_text(text: &str) -> String {
    if !text.contains([LINE_BREAK, PARAGRAPH_BREAK]) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split(' ').filter(|w| !w.is_empty()).enumerate() {
        let brk = match word {
            w if w == PARAGRAPH_BREAK.to_string() => Some("\n\n"),
            w if w == LINE_BREAK.to_string() => Some("\n"),
            _ => None,
        };
        match brk {
            Some(b) => out.push_str(b),
            None => {
                if i > 0 && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(word);
            }
        }
    }
    out
}

/// Replaces each way of writing a break, ignoring case; paragraphs first,
/// so `\n\n` isn't read as two lines.
fn replace_breaks(spelling: &str, paragraph: &str, line: &str) -> String {
    let mut out = spelling.to_string();
    for (forms, with) in [(&PARAGRAPH[..], paragraph), (&LINE[..], line)] {
        for form in forms {
            out = replace_ignoring_case(&out, form, with);
        }
    }
    out
}

fn replace_ignoring_case(text: &str, form: &str, with: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in lower.match_indices(form) {
        out.push_str(&text[last..at]);
        out.push_str(with);
        last = at + form.len();
    }
    out.push_str(&text[last..]);
    out
}
