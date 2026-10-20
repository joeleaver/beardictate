//! Case modes (a tester asked, 2026-10-20): "bear, title case", "bear, all
//! caps", "bear, lower case" and "bear, normal case", or the bear's menu.
//! Like list mode, a mode lasts until switched back, isn't saved, and what's
//! said in it isn't tidied (the model would put the capitals back). The
//! special words keep their own spelling in every mode.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseMode {
    /// As the recognizer and the command pass write it: sentence case.
    #[default]
    Normal,
    /// Each Word Capitalized, but for Short Words like "of" and "the".
    Title,
    /// all lower case.
    Lower,
    /// ALL CAPITALS.
    Upper,
}

/// Words a title keeps in lower case, but for its first and last.
const SMALL: &[&str] = &[
    "a", "an", "the", "and", "but", "or", "nor", "for", "so", "yet", "as", "at", "by", "in", "of", "off", "on", "per",
    "to", "up", "via", "vs", "from", "into", "onto", "over", "with",
];

/// One segment's text in `mode`. `names` (the special words) keep their
/// spelling. In title case the recognizer's full stop at the end comes off,
/// as a heading has none; `said_last` keeps a mark the speaker said.
pub(crate) fn apply(text: &str, mode: CaseMode, names: &[String], said_last: bool) -> String {
    let keep: Vec<String> = names.iter().flat_map(|n| n.split_whitespace().map(str::to_string).collect::<Vec<_>>()).collect();
    let kept = |w: &str| {
        let bare = w.trim_end_matches("'s").trim_end_matches("\u{2019}s");
        keep.iter().any(|k| k == bare)
    };
    let words = words(text);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (n, &(start, end)) in words.iter().enumerate() {
        out.push_str(&text[copied..start]);
        let word = &text[start..end];
        let new = if kept(word) {
            word.to_string()
        } else {
            match mode {
                CaseMode::Normal => word.to_string(),
                CaseMode::Upper => word.to_uppercase(),
                CaseMode::Lower => word.to_lowercase(),
                CaseMode::Title => title_word(word, n == 0 || n + 1 == words.len()),
            }
        };
        out.push_str(&new);
        copied = end;
    }
    out.push_str(&text[copied..]);
    if mode == CaseMode::Title && !said_last {
        let body = out.trim_end_matches([' ', '\t']);
        let bare = body.trim_end_matches('.').len();
        let tail = out[body.len()..].to_string();
        out.truncate(bare);
        out.push_str(&tail);
    }
    out
}

/// A word in a title: capitalized, unless it's a small word inside it.
/// Words with capitals of their own (NASA, iPhone) stay; so do the parts
/// of "well-known" each get a capital.
fn title_word(word: &str, end: bool) -> String {
    if word.chars().skip(1).any(char::is_uppercase) {
        return word.to_string();
    }
    let lower = word.to_lowercase();
    if !end && SMALL.contains(&lower.as_str()) {
        return lower;
    }
    lower.split('-').map(capitalize).collect::<Vec<_>>().join("-")
}

fn capitalize(part: &str) -> String {
    let mut c = part.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Byte ranges of the words: letters and digits, with the apostrophes and
/// hyphens inside them.
fn words(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_alphanumeric() {
            i += 1;
            continue;
        }
        let start = chars[i].0;
        while i < chars.len() {
            let c = chars[i].1;
            let inner = matches!(c, '\'' | '\u{2019}' | '-')
                && chars.get(i + 1).is_some_and(|n| n.1.is_alphanumeric())
                && i > 0
                && chars[i - 1].1.is_alphanumeric();
            if !(c.is_alphanumeric() || inner) {
                break;
            }
            i += 1;
        }
        out.push((start, chars.get(i).map_or(text.len(), |c| c.0)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(text: &str, mode: CaseMode) -> String {
        apply(text, mode, &["Ysolde".into(), "beardictate".into()], false)
    }

    #[test]
    fn the_four_modes() {
        assert_eq!(a("The lord of the rings.", CaseMode::Normal), "The lord of the rings.");
        assert_eq!(a("The lord of the rings.", CaseMode::Title), "The Lord of the Rings");
        assert_eq!(a("The Lord of the Rings, again.", CaseMode::Lower), "the lord of the rings, again.");
        assert_eq!(a("Don't stop the music.", CaseMode::Upper), "DON'T STOP THE MUSIC.");
    }

    #[test]
    fn titles_keep_their_own_capitals_and_end_words() {
        assert_eq!(a("a well-known guide to NASA and the iPhone.", CaseMode::Title), "A Well-Known Guide to NASA and the iPhone");
        assert_eq!(a("what it is all about.", CaseMode::Title), "What It Is All About");
        assert_eq!(a("something to think of.", CaseMode::Title), "Something to Think Of");
        assert_eq!(apply("Is it over?", CaseMode::Title, &[], true), "Is It Over?");
    }

    #[test]
    fn special_words_keep_their_spelling() {
        assert_eq!(a("Ysolde uses beardictate.", CaseMode::Lower), "Ysolde uses beardictate.");
        assert_eq!(a("Ysolde uses beardictate.", CaseMode::Upper), "Ysolde USES beardictate.");
        assert_eq!(a("Ysolde's book on beardictate.", CaseMode::Title), "Ysolde's Book on beardictate");
    }
}
