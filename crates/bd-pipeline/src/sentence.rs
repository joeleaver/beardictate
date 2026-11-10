//! Reading on from what's typed: a piece that continues an open sentence
//! starts in lower case (a tester, 2026-11: "It needs to not capitalize
//! after a comma, even if it's starting again with a new dictation"). The
//! recognizer capitalizes the start of every utterance, and the cleanup
//! model often keeps it.
//!
//! Only within a passage: after a new one (a long silence, another app, a
//! dictation started later) the page could be anywhere, so the capital stays.

use crate::compose::OPENERS;

/// Whether `before`, the passage typed so far, ends in an open sentence: a
/// comma, a semicolon, a dash or a word, not a sentence's end, a colon, a
/// new line or an opening mark. A closing quote or bracket is looked past
/// ("“Go,” " is open, "“Go.” " isn't).
pub(crate) fn is_open(before: &str) -> bool {
    let ending = before.trim_end_matches([' ', '\t']);
    let bare = ending.trim_end_matches(['\u{201D}', '\u{2019}', '\u{00BB}', ')', ']']);
    let Some(last) = bare.chars().last() else { return false };
    // A straight quote could be opening or closing: left alone.
    !(".!?\u{2026}:\"'\n".contains(last) || OPENERS.contains(&last))
}

/// `text` with its first word in lower case when it reads on from `before`
/// ([`is_open`]) and starts with a word (not a quote or a bracket), and the
/// word isn't one that keeps its capital (see [`lower_first_word`]).
pub(crate) fn read_on(before: &str, text: &str, names: &[String]) -> String {
    let body = text.trim_start_matches([' ', '\t']);
    if !body.starts_with(char::is_alphabetic) || !is_open(before) {
        return text.to_string();
    }
    let mut out = text.to_string();
    lower_first_word(&mut out, names);
    out
}

/// The first word of `body` in lower case, unless it's "I" ("I'm"), has
/// capitals after its first letter (NASA, McAllister), is a special word
/// (the first word of one of `names`) or is known as a name
/// ([`bd_vocab::is_name`]: Sarah, Monday).
pub(crate) fn lower_first_word(body: &mut String, names: &[String]) {
    let Some(start) = body.find(char::is_alphabetic) else { return };
    let word: String = body[start..].chars().take_while(|c| c.is_alphanumeric() || *c == '\'' || *c == '\u{2019}').collect();
    let first = word.chars().next().unwrap_or(' ');
    let rest_upper = word.chars().skip(1).any(char::is_uppercase);
    let bare = word.trim_end_matches("'s").trim_end_matches("\u{2019}s");
    let keep = word == "I"
        || word.starts_with("I'")
        || word.starts_with("I\u{2019}")
        || rest_upper
        || bd_vocab::is_name(bare)
        || names.iter().any(|n| n.split_whitespace().next() == Some(bare));
    if keep || !first.is_uppercase() {
        return;
    }
    let lower: String = first.to_lowercase().collect();
    body.replace_range(start..start + first.len_utf8(), &lower);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(before: &str, text: &str) -> String {
        read_on(before, text, &["Ysolde".to_string(), "Port Varrow".to_string()])
    }

    #[test]
    fn an_open_sentence_reads_on_in_lower_case() {
        assert_eq!(on("Hello, ", "How are you?"), "how are you?");
        assert_eq!(on("We tried; ", "It failed."), "it failed.");
        assert_eq!(on("And then we ", "Went home."), "went home.");
        assert_eq!(on("Wait \u{2014} ", "No."), "no.");
        assert_eq!(on("\u{201C}Go,\u{201D} ", "She said."), "she said.");
    }

    #[test]
    fn a_closed_one_keeps_the_capital() {
        for before in ["", "Done. ", "Really? ", "Wow! ", "Hm\u{2026} ", "Note: ", "Line\n", "He said, \u{201C}", "(", "\u{201C}Go.\u{201D} "] {
            assert_eq!(on(before, "Then we left."), "Then we left.", "{before:?}");
        }
        // Only a word is lowered, not a quote it starts with.
        assert_eq!(on("He said, ", "\u{201C}Go.\u{201D}"), "\u{201C}Go.\u{201D}");
        assert_eq!(on("Hello, ", "\nNew line."), "\nNew line.");
    }

    #[test]
    fn names_and_capitals_keep_theirs() {
        for text in ["I think so.", "I'm here.", "NASA called.", "McAllister came.", "Ysolde's ship.", "Port Varrow.", "Sarah said.", "Monday then."] {
            assert_eq!(on("Hello, ", text), text);
        }
    }
}
