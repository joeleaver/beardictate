//! Taking out paragraph breaks and quote marks the author didn't dictate.

use super::align;
use super::words::{Gap, tokenize};

/// `output` with every line or paragraph break the model added turned back
/// into a space, keeping those the dictation had ("new paragraph", "new
/// line"). For [`bd_core::Style::dialog_paragraphs`] off. `output` should
/// already have passed the guardrail; if it can't be aligned with `input`,
/// it is returned unchanged.
pub fn keep_dictated_breaks(context: &str, input: &str, output: &str) -> String {
    if !output.contains('\n') {
        return output.to_string();
    }
    let a = tokenize(input);
    let b = tokenize(output);
    let tail: Vec<String> = tokenize(context).words.into_iter().map(|w| w.norm).collect();
    let tail = &tail[tail.len().saturating_sub(4)..];
    let Some(alignment) = align::align(&a, &b, tail) else {
        return output.to_string();
    };
    let kept = &alignment.kept;
    let last = b.gaps.len() - 1;
    let mut out = String::with_capacity(output.len());
    for (g, gap) in b.gaps.iter().enumerate() {
        let from = if g == 0 { 0 } else { kept[g - 1] + 1 };
        let to = if g == kept.len() { a.words.len() } else { kept[g] };
        let dictated = a.gaps[from..=to].iter().map(Gap::break_level).max().unwrap_or(0);
        let text = if gap.break_level() > dictated { lower_break(&gap.text, dictated, g == 0, g == last) } else { gap.text.clone() };
        out.push_str(&text);
        if let Some(word) = b.words.get(g) {
            out.push_str(&word.text);
        }
    }
    out
}

/// `output` with the quote marks the model added taken out, keeping those
/// the dictation had ("open quote", or marks the recognizer wrote). For
/// [`bd_core::Style::dialog`] off: the model, trained on novels, quotes a
/// short line as if a character said it. `output` should already have
/// passed the guardrail; if it can't be aligned with `input`, every quote
/// mark is taken out when the dictation had none, else it's unchanged.
pub fn keep_dictated_quotes(context: &str, input: &str, output: &str) -> String {
    let b = tokenize(output);
    if b.gaps.iter().all(|g| g.quotes() == 0) {
        return output.to_string();
    }
    let a = tokenize(input);
    let tail: Vec<String> = tokenize(context).words.into_iter().map(|w| w.norm).collect();
    let tail = &tail[tail.len().saturating_sub(4)..];
    let Some(alignment) = align::align(&a, &b, tail) else {
        if a.gaps.iter().all(|g| g.quotes() == 0) {
            let last = b.gaps.len() - 1;
            return rebuild(&b, |g, gap| {
                let bare: String = gap.text.chars().filter(|c| !super::words::is_quote(*c)).collect();
                if bare == gap.text { bare } else { respace(&bare, g == 0, g == last) }
            });
        }
        return output.to_string();
    };
    let kept = &alignment.kept;
    let last = b.gaps.len() - 1;
    rebuild(&b, |g, gap| {
        let from = if g == 0 { 0 } else { kept[g - 1] + 1 };
        let to = if g == kept.len() { a.words.len() } else { kept[g] };
        let mut dictated: Vec<char> = a.gaps[from..=to].iter().flat_map(|d| d.text.chars()).filter(|c| super::words::is_quote(*c)).collect();
        if gap.quotes() == 0 {
            return gap.text.clone();
        }
        // Each mark the dictation had there stays; the others go.
        let kept: String = gap
            .text
            .chars()
            .filter(|c| match dictated.iter().position(|d| d == c) {
                Some(i) => {
                    dictated.remove(i);
                    true
                }
                None => !super::words::is_quote(*c),
            })
            .collect();
        if kept == gap.text { kept } else { respace(&kept, g == 0, g == last) }
    })
}

/// The text of `tokens`, each gap as `gap` makes it.
fn rebuild(tokens: &super::words::Tokens, gap: impl Fn(usize, &Gap) -> String) -> String {
    let mut out = String::new();
    for (g, gp) in tokens.gaps.iter().enumerate() {
        out.push_str(&gap(g, gp));
        if let Some(word) = tokens.words.get(g) {
            out.push_str(&word.text);
        }
    }
    out
}

/// A gap that lost quote marks, its spaces as they'd be without them.
fn respace(gap: &str, at_start: bool, at_end: bool) -> String {
    let mut out = String::with_capacity(gap.len());
    for c in gap.chars() {
        if c == ' ' && (out.ends_with([' ', '\n']) || (out.is_empty() && at_start)) {
            continue;
        }
        out.push(c);
    }
    if at_end {
        out.truncate(out.trim_end_matches(' ').len());
    } else if out.is_empty() && !at_start {
        out.push(' ');
    }
    out
}

/// Rewrites the whitespace run holding the newlines in `gap` to `level`'s
/// break (a space for 0, unless the gap opens or closes the text).
fn lower_break(gap: &str, level: u8, at_start: bool, at_end: bool) -> String {
    let Some(first_nl) = gap.find('\n') else { return gap.to_string() };
    let last_nl = gap.rfind('\n').unwrap_or(first_nl);
    let start = gap[..first_nl].trim_end_matches([' ', '\t']).len();
    let end = last_nl + 1 + (gap[last_nl + 1..].len() - gap[last_nl + 1..].trim_start_matches([' ', '\t']).len());
    let replacement = match level {
        2.. => "\n\n",
        1 => "\n",
        _ if at_start || at_end => "",
        _ => " ",
    };
    format!("{}{replacement}{}", &gap[..start], &gap[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_quotes_come_out() {
        assert_eq!(keep_dictated_quotes("", "Is it possible?", "\u{201C}Is it possible?\u{201D}"), "Is it possible?");
        assert_eq!(
            keep_dictated_quotes("", "No, old son, Jeff said. I think you got it.", "\u{201C}No, old son,\u{201D} Jeff said. \u{201C}I think you got it.\u{201D}"),
            "No, old son, Jeff said. I think you got it."
        );
        // Dictated ones stay.
        let said = "He said \u{201C}go now.\u{201D} Then he left.";
        assert_eq!(keep_dictated_quotes("", said, "He said, \u{201C}Go now.\u{201D} \u{201C}Then he left.\u{201D}"), "He said, \u{201C}Go now.\u{201D} Then he left.");
        // Apostrophes are words' own.
        assert_eq!(keep_dictated_quotes("", "the dogs' bowl isn't here", "The dogs' bowl isn't here."), "The dogs' bowl isn't here.");
    }

    #[test]
    fn added_breaks_become_spaces() {
        let input = "Where? she asked. Here, he said.";
        let output = "\u{201C}Where?\u{201D} she asked.\n\n\u{201C}Here,\u{201D} he said.";
        assert_eq!(keep_dictated_breaks("", input, output), "\u{201C}Where?\u{201D} she asked. \u{201C}Here,\u{201D} he said.");
        assert_eq!(keep_dictated_breaks("Before.", "she ran", "\n\nShe ran."), "She ran.");
    }

    #[test]
    fn dictated_breaks_stay() {
        let input = "It ended.\n\nMorning came. She woke.";
        let output = "It ended.\n\nMorning came.\n\nShe woke.";
        assert_eq!(keep_dictated_breaks("", input, output), "It ended.\n\nMorning came. She woke.");
        assert_eq!(keep_dictated_breaks("", "line one\nline two", "Line one.\n\nLine two."), "Line one.\nLine two.");
        assert_eq!(keep_dictated_breaks("", "\n\nshe ran", "\n\nShe ran."), "\n\nShe ran.");
    }
}
