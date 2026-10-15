//! Small models often start their answer by re-typing the end of the
//! previous text ("Story so far") before the cleaned dictation. The words
//! are the context's, not the dictation's, so the guardrail would reject the
//! whole answer and the segment would arrive uncleaned. Cutting the echo off
//! first lets the rest be reviewed on its own.

use super::words::{Tokens, tokenize};

/// An echo shorter than this is more likely the dictation itself.
const MIN_ECHO_WORDS: usize = 4;

/// Marks that belong to what follows an echo and so are kept: breaks
/// (a new paragraph) and opening quotes.
fn keeps(c: char) -> bool {
    matches!(c, '\n' | '\u{201C}' | '\u{2018}' | '"' | '\u{00AB}')
}

/// `output` without a leading copy of the end of `context`: at least
/// [`MIN_ECHO_WORDS`] words matching the context's last words (case and
/// quote marks aside). Unchanged when there's no such copy, or when the
/// dictation itself starts with those words.
pub fn strip_context_echo<'a>(context: &str, input: &str, output: &'a str) -> std::borrow::Cow<'a, str> {
    let ctx: Vec<String> = tokenize(context).words.into_iter().map(|w| w.norm).collect();
    let out = tokenize(output);
    let inp: Vec<String> = tokenize(input).words.into_iter().map(|w| w.norm).collect();
    let longest = ctx.len().min(out.words.len());
    let echo = (MIN_ECHO_WORDS..=longest).rev().find(|&k| {
        out.words[..k].iter().map(|w| &w.norm).eq(ctx[ctx.len() - k..].iter())
            && !inp.iter().take(k).eq(ctx[ctx.len() - k..].iter())
    });
    let Some(k) = echo else { return output.into() };
    let cut = byte_len(&out, k);
    let gap: String = out.gaps[k].text.chars().filter(|&c| keeps(c)).collect();
    let rest = &output[cut + out.gaps[k].text.len()..];
    tracing::debug!(words = k, "cleanup output began with the previous text; dropped it");
    format!("{gap}{rest}").into()
}

/// Bytes of the text up to the end of word `k - 1`.
fn byte_len(t: &Tokens, k: usize) -> usize {
    t.gaps[..k].iter().map(|g| g.text.len()).sum::<usize>() + t.words[..k].iter().map(|w| w.text.len()).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_of_the_previous_text_is_cut_off() {
        let ctx = "\u{201C}Then go,\u{201D} she said. \u{201C}See if I care.\u{201D}";
        let out = "\u{201C}Then go,\u{201D} she said. \u{201C}See if I care.\u{201D}\n\nHe went. The door swung shut.";
        assert_eq!(strip_context_echo(ctx, "He went the door swung shut", out), "\n\nHe went. The door swung shut.");
        // Only the tail needs repeating, and an opening quote after it stays.
        let ctx = "It was late. \u{201C}Who's there?\u{201D} the old man called.";
        let out = "\u{201C}Who's there?\u{201D} the old man called. \u{201C}Only me.\u{201D}";
        assert_eq!(strip_context_echo(ctx, "Only me", out), "\u{201C}Only me.\u{201D}");
    }

    #[test]
    fn short_or_dictated_repeats_stay() {
        // Three words aren't enough to call it an echo.
        assert_eq!(strip_context_echo("He ran away.", "ran away fast", "He ran away fast."), "He ran away fast.");
        // The author really did say them again.
        let ctx = "The road went on and on.";
        assert_eq!(
            strip_context_echo(ctx, "the road went on and on, forever", "The road went on and on, forever."),
            "The road went on and on, forever."
        );
        // Nothing to do.
        assert_eq!(strip_context_echo("", "hello there", "Hello there."), "Hello there.");
    }
}
