//! Spaces in a spelling (testers, 2026-11: "can't put leading spaces in the
//! special words"). A spelling's spaces are typed where it has them: one
//! that starts with a space gets it even where none would come (a passage's
//! start, after a new line or an opening quote); two give two. Beside
//! another word the usual space counts as one of them, so `" Ysolde"` in
//! mid-sentence reads as `"Ysolde"` does. `<space>` is a space you can see,
//! for the list format, whose lines are trimmed:
//!
//! - at the start or end, every space counts (`" Ysolde"`, `"Ysolde<space>"`);
//! - between words, a run of spaces is that many (`"A  B"`, `"A<space><space>B"`);
//! - next to a break only `<space>` counts (`"<new line><space><space>Ysolde"`
//!   indents), so `"Best wishes, <new line> Ysolde"` reads as before;
//! - where a run has a `<space>` in it, only the `<space>`s count.
//!
//! The recognizer, the command pass and the cleanup model all work with the
//! words alone; [`crate::Vocabulary::space`] puts the spaces back in the
//! text about to be typed. A space at that text's very start or end is
//! written [`SPACE`], so joining it to what's typed before and after keeps
//! it rather than trimming it.

use crate::breaks::{LINE, PARAGRAPH};

/// A spelling's own space at the start or end of a piece of text (Unicode
/// private use): joining types it as a space, counting the one it would
/// have put there anyway.
pub const SPACE: char = '\u{E002}';

/// How a space is written so it can be seen.
pub(crate) const SPACE_TOKEN: &str = "<space>";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Lower-cased.
    Word(String),
    Break,
}

/// A spelling with spaces of its own: its words and breaks, and how many
/// spaces go before each part and after the last (0: whatever joining
/// gives).
#[derive(Debug, Clone)]
pub(crate) struct Spaced {
    parts: Vec<Part>,
    gaps: Vec<usize>,
}

enum Lex {
    Word(String),
    Break,
    /// A space typed as one.
    Literal,
    /// `<space>`.
    Explicit,
}

impl Spaced {
    /// `None` for a spelling with no spaces of its own.
    pub(crate) fn new(spelling: &str) -> Option<Self> {
        let mut parts = Vec::new();
        // The run of spaces before each part: (typed, `<space>`).
        let mut runs = vec![(0, 0)];
        for lex in lex(spelling) {
            let run = runs.last_mut().expect("never empty");
            match lex {
                Lex::Literal => run.0 += 1,
                Lex::Explicit => run.1 += 1,
                Lex::Word(w) => {
                    parts.push(Part::Word(w.to_lowercase()));
                    runs.push((0, 0));
                }
                Lex::Break => {
                    parts.push(Part::Break);
                    runs.push((0, 0));
                }
            }
        }
        if !parts.iter().any(|p| matches!(p, Part::Word(_))) {
            return None;
        }
        let gaps: Vec<usize> = runs
            .iter()
            .enumerate()
            .map(|(b, &(typed, explicit))| {
                let prev = b.checked_sub(1).and_then(|i| parts.get(i));
                let next = parts.get(b);
                if explicit > 0 {
                    return explicit;
                }
                match (prev, next) {
                    (Some(Part::Break), _) | (_, Some(Part::Break)) => 0,
                    // One space between words is what joining gives.
                    (Some(_), Some(_)) if typed <= 1 => 0,
                    _ => typed,
                }
            })
            .collect();
        gaps.iter().any(|&g| g > 0).then_some(Self { parts, gaps })
    }
}

fn lex(spelling: &str) -> Vec<Lex> {
    // ASCII lower-casing keeps every byte where it was.
    let lower = spelling.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while i < spelling.len() {
        let rest = &lower[i..];
        if let Some(form) = PARAGRAPH.iter().chain(LINE.iter()).find(|f| rest.starts_with(**f)) {
            out.push(Lex::Break);
            i += form.len();
            continue;
        }
        if rest.starts_with(SPACE_TOKEN) {
            out.push(Lex::Explicit);
            i += SPACE_TOKEN.len();
            continue;
        }
        let c = spelling[i..].chars().next().expect("in bounds");
        if c.is_whitespace() {
            out.push(Lex::Literal);
        } else if let Some(Lex::Word(w)) = out.last_mut() {
            w.push(c);
        } else {
            out.push(Lex::Word(c.to_string()));
        }
        i += c.len_utf8();
    }
    out
}

type Range = std::ops::Range<usize>;

/// A word of the text (a run of anything but spaces) or a break (a run of
/// spaces with a new line in it).
#[derive(Debug, Clone)]
enum Tok {
    Word(Range),
    Break(Range),
}

fn is_space(c: char) -> bool {
    c.is_whitespace() || c == SPACE
}

fn text_tokens(text: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        let space = is_space(c);
        let mut end = start;
        let mut newline = false;
        while let Some(&(i, c)) = chars.peek() {
            if is_space(c) != space {
                break;
            }
            newline |= c == '\n';
            end = i + c.len_utf8();
            chars.next();
        }
        if !space {
            toks.push(Tok::Word(start..end));
        } else if newline {
            toks.push(Tok::Break(start..end));
        }
    }
    toks
}

/// `text` with the spaces of each spelling in `spaced` where it occurs.
pub(crate) fn apply(spaced: &[Spaced], text: &str) -> String {
    let toks = text_tokens(text);
    // Each run of spaces to change, and how many it should have.
    let mut edits: Vec<(Range, usize, bool)> = Vec::new();
    for s in spaced {
        for i in 0..toks.len() {
            if !matches_at(s, text, &toks, i) {
                continue;
            }
            for (b, &want) in s.gaps.iter().enumerate() {
                if want == 0 {
                    continue;
                }
                let (region, beside_word) = if b < s.parts.len() {
                    (before(text, &toks, i + b), matches!(s.parts[b], Part::Word(_)))
                } else {
                    (after(text, &toks, i + b - 1), matches!(s.parts[b - 1], Part::Word(_)))
                };
                let edge = region.start == 0 || region.end == text.len();
                if edge && !beside_word {
                    continue;
                }
                match edits.iter_mut().find(|e| e.0 == region) {
                    Some(e) => e.1 = e.1.max(want),
                    None => edits.push((region, want, edge)),
                }
            }
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    let mut out = text.to_string();
    for (region, want, edge) in edits {
        let with = if edge {
            SPACE.to_string().repeat(want)
        } else {
            " ".repeat(want.max(text[region.clone()].chars().count()))
        };
        out.replace_range(region, &with);
    }
    out
}

fn matches_at(s: &Spaced, text: &str, toks: &[Tok], i: usize) -> bool {
    let n = s.parts.len();
    if i + n > toks.len() {
        return false;
    }
    s.parts.iter().zip(&toks[i..i + n]).enumerate().all(|(k, (part, tok))| match (part, tok) {
        (Part::Break, Tok::Break(_)) => true,
        (Part::Word(want), Tok::Word(r)) => word_matches(&text[r.clone()], want, k == 0, k + 1 == n),
        _ => false,
    })
}

/// Whether a word of the text is the spelling's `want` (lower-cased): at
/// the start of a match an opening mark may come before it, at the end the
/// recognizer's mark or a possessive after it.
fn word_matches(word: &str, want: &str, first: bool, last: bool) -> bool {
    let mut w = word;
    if first && want.starts_with(char::is_alphanumeric) {
        w = w.trim_start_matches(|c: char| !c.is_alphanumeric());
    }
    if last && want.ends_with(char::is_alphanumeric) {
        w = w.trim_end_matches(|c: char| !c.is_alphanumeric());
        if let Some(bare) = w.strip_suffix("'s").or_else(|| w.strip_suffix("\u{2019}s"))
            && bare.to_lowercase() == want
        {
            return true;
        }
    }
    w.to_lowercase() == want
}

/// The spaces before token `k`: after the new line, if a break comes before
/// it; before the new line, if it's a break.
fn before(text: &str, toks: &[Tok], k: usize) -> Range {
    match &toks[k] {
        Tok::Word(r) => match k.checked_sub(1).map(|p| &toks[p]) {
            Some(Tok::Break(b)) => after_last_newline(text, b)..r.start,
            Some(Tok::Word(p)) => p.end..r.start,
            None => 0..r.start,
        },
        Tok::Break(b) => b.start..first_newline(text, b),
    }
}

/// The spaces after token `k`, likewise.
fn after(text: &str, toks: &[Tok], k: usize) -> Range {
    match &toks[k] {
        Tok::Word(r) => match toks.get(k + 1) {
            Some(Tok::Break(b)) => r.end..first_newline(text, b),
            Some(Tok::Word(n)) => r.end..n.start,
            None => r.end..text.len(),
        },
        Tok::Break(b) => after_last_newline(text, b)..b.end,
    }
}

fn first_newline(text: &str, b: &Range) -> usize {
    b.start + text[b.clone()].find('\n').unwrap_or(0)
}

fn after_last_newline(text: &str, b: &Range) -> usize {
    b.start + text[b.clone()].rfind('\n').map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(spellings: &[&str], text: &str) -> String {
        let spaced: Vec<Spaced> = spellings.iter().filter_map(|s| Spaced::new(s)).collect();
        apply(&spaced, text).replace(SPACE, "_")
    }

    #[test]
    fn only_spellings_with_spaces_of_their_own() {
        for s in ["Ysolde", "Port Varrow", "Best wishes, <new line> Ysolde", "A <new line>B"] {
            assert!(Spaced::new(s).is_none(), "{s}");
        }
        for s in [" Ysolde", "Ysolde ", "Ysolde<space>", "A  B", "A<space><space>B", "<new line><space>B"] {
            assert!(Spaced::new(s).is_some(), "{s}");
        }
    }

    #[test]
    fn spaces_at_the_ends_of_the_text_are_marked() {
        assert_eq!(space(&[" Ysolde"], "Ysolde came."), "_Ysolde came.");
        assert_eq!(space(&["Ysolde "], "It was Ysolde."), "It was Ysolde._");
        assert_eq!(space(&["<space><space>Ysolde"], "\u{201C}Ysolde,\u{201D} he said."), "__\u{201C}Ysolde,\u{201D} he said.");
        // Mid-text, the usual space counts as one of them.
        assert_eq!(space(&[" Ysolde"], "Ask Ysolde now."), "Ask Ysolde now.");
        assert_eq!(space(&["  Ysolde"], "Ask Ysolde's ship."), "Ask  Ysolde's ship.");
    }

    #[test]
    fn spaces_inside_and_beside_breaks() {
        assert_eq!(space(&["A  B"], "So a b then."), "So a  b then.");
        assert_eq!(space(&["Regards,<new line><space><space>Ysolde"], "Regards,\nYsolde"), "Regards,\n  Ysolde");
        assert_eq!(space(&["Regards,<new line><space><space>Ysolde"], "Regards,\n\nYsolde"), "Regards,\n\n  Ysolde");
        // After a break, a space at the start of a spelling indents.
        assert_eq!(space(&[" Ysolde"], "Done.\nYsolde came."), "Done.\n Ysolde came.");
        // Words that only look alike aren't it.
        assert_eq!(space(&[" Ysolde"], "Ysoldes came."), "Ysoldes came.");
        assert_eq!(space(&["A  B"], "So a c b."), "So a c b.");
    }
}
