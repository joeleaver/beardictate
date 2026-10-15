//! Straight double quotes from the model, turned into the style's marks.
//!
//! Stock models often answer with `"…"` even when the prompt and the command
//! pass use curly marks. Which way each one faces follows from what is
//! before it, as in any "smart quotes" feature.

use bd_core::{QuoteStyle, Style};

/// Characters after which a `"` opens a quotation.
const BEFORE_OPENING: &[char] = &['(', '[', '\u{2014}', '\u{2013}', '\u{201C}', '\u{2018}'];

/// Replaces each straight `"` in `text` with the style's opening or closing
/// dialog mark. A `"` at the very start closes only if `previous`, the text
/// delivered before this segment, leaves a quotation open. Unchanged for
/// [`QuoteStyle::Straight`].
pub fn smarten(previous: &str, text: &str, style: &Style) -> String {
    if style.quotes == QuoteStyle::Straight || !text.contains('"') {
        return text.to_string();
    }
    let (open, close) = style.quotes.marks();
    let mut before: Option<char> = None;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let mark = match (c, before) {
            ('"', None) if open_at_end(previous, open, close) => close,
            ('"', None) => open,
            ('"', Some(b)) if b.is_whitespace() || BEFORE_OPENING.contains(&b) => open,
            ('"', Some(_)) => close,
            (other, _) => other,
        };
        out.push(mark);
        before = Some(mark);
    }
    out
}

/// Whether `text` ends inside a quotation: its last opening mark comes
/// after its last closing mark.
fn open_at_end(text: &str, open: char, close: char) -> bool {
    match (text.rfind(open), text.rfind(close)) {
        (Some(o), Some(c)) => o > c,
        (Some(_), None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bd_core::PunctuationPlacement;

    #[test]
    fn straight_quotes_face_the_right_way() {
        let us = Style::default();
        assert_eq!(smarten("", "\"Go,\" she said. \"Now.\"", &us), "\u{201C}Go,\u{201D} she said. \u{201C}Now.\u{201D}");
        assert_eq!(smarten("", "He said (\"no\").", &us), "He said (\u{201C}no\u{201D}).");
        assert_eq!(smarten("", "Don't \"panic.\"", &us), "Don't \u{201C}panic.\u{201D}");
    }

    #[test]
    fn a_leading_quote_reads_the_previous_text() {
        let us = Style::default();
        assert_eq!(smarten("\u{201C}Wait", "\" she said.", &us), "\u{201D} she said.");
        assert_eq!(smarten("Done.", "\"Next.\"", &us), "\u{201C}Next.\u{201D}");
        assert_eq!(smarten("\u{201C}Go.\u{201D}", "\"Next.\"", &us), "\u{201C}Next.\u{201D}");
    }

    #[test]
    fn styles_pick_the_marks() {
        let uk = Style { quotes: QuoteStyle::CurlySingle, punctuation: PunctuationPlacement::Logical, ..Style::default() };
        assert_eq!(smarten("", "\"Go\", she said.", &uk), "\u{2018}Go\u{2019}, she said.");
        let straight = Style { quotes: QuoteStyle::Straight, ..Style::default() };
        assert_eq!(smarten("", "\"Go.\"", &straight), "\"Go.\"");
    }
}
