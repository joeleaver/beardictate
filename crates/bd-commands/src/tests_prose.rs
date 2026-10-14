//! Adversarial prose: command words used as ordinary words must pass through
//! untouched.

use bd_core::Style;

use crate::{CommandConfig, CommandPass};

/// Prose that must never trigger a structural command.
const STRUCTURAL_PROSE: &[&str] = &[
    "The new paragraph began with a lie.",
    "She started a new paragraph.",
    "Each new paragraph was worse than the last.",
    "He read her new paragraph twice.",
    "It was a whole new line of work.",
    "He drew a new line in the sand.",
    "They opened a new line of credit.",
    "The new line cook quit on Monday.",
    "His brand new line was terrible.",
    "The open quote was never closed.",
    "She deleted the end quote by mistake.",
    "An open quote hung at the start of the line.",
    "Every close quote needs an open quote marks pair.",
    "He used open quote marks everywhere.",
    "He couldn't scratch that itch.",
    "If it itches, scratch that.",
    "I told him to scratch that off the list.",
    "“Scratch that,” she said.",
    "Don't scratch that, it'll scar.",
    "He gave me a quote for the roof.",
    "To quote my father, never again.",
    "It was her favorite quote.",
    "He was quote unquote sick.",
    "He was, quote, unquote, sick.",
    "Quote me on that.",
    "In the end, quote the source.",
    "She would open, quote by quote, the old letters.",
    "The new, paragraph-long note.",
];

#[test]
fn structural_words_in_prose_stay_literal() {
    let pass = CommandPass::default();
    for input in STRUCTURAL_PROSE {
        let out = pass.apply(input, false);
        assert_eq!(out.text, *input, "rewrote prose");
        assert!(out.actions.is_empty(), "{input:?} produced {:?}", out.actions);
    }
}

/// Prose that must never trigger a literal-punctuation command, even with
/// the option on.
const PUNCTUATION_PROSE: &[&str] = &[
    "She put a period at the end.",
    "It was a period of calm.",
    "The period drama ran for years.",
    "She loved period dramas.",
    "A question mark hung over the deal.",
    "His comma splice annoyed the editor.",
    "The comma was misplaced.",
    "The colon is part of the gut.",
    "Use a semicolon here.",
    "She typed an ellipsis.",
    "The exclamation point was too much.",
    "That period of his life was over.",
    "The first period ended at ten.",
];

#[test]
fn punctuation_words_in_prose_stay_literal_even_when_enabled() {
    let pass = CommandPass::new(CommandConfig { literal_punctuation: true }, Style::default());
    for input in PUNCTUATION_PROSE {
        let out = pass.apply(input, false);
        assert_eq!(out.text, *input, "rewrote prose");
        assert!(out.actions.is_empty(), "{input:?} produced {:?}", out.actions);
    }
}

/// Prose that *does* contain a command, mixed with the same words as prose.
const MIXED: &[(&str, &str)] = &[
    (
        "The new paragraph began. New paragraph. It ended.",
        "The new paragraph began.\n\nIt ended.",
    ),
    (
        "He couldn't scratch that itch. Scratch that. He scratched.",
        "He scratched.",
    ),
    (
        "He was quote unquote sick, she said quote fine end quote.",
        "He was quote unquote sick, she said “fine.”",
    ),
    (
        "Tell her open quote I'm coming close quote.",
        "Tell her “I'm coming.”",
    ),
];

#[test]
fn commands_next_to_the_same_words_in_prose() {
    let pass = CommandPass::default();
    for (input, expected) in MIXED {
        assert_eq!(pass.apply(input, false).text, *expected, "input: {input:?}");
    }
}

/// Known limit of the opt-in punctuation mode: a punctuation word used as a
/// bare noun, with no determiner before it and no tell-tale word after it,
/// is indistinguishable from a command. This is why the mode is off by
/// default.
#[test]
fn known_false_positive_of_literal_punctuation() {
    let pass = CommandPass::new(CommandConfig { literal_punctuation: true }, Style::default());
    assert_eq!(pass.apply("He had colon cancer.", false).text, "He had: cancer.");
    assert_eq!(CommandPass::default().apply("He had colon cancer.", false).text, "He had colon cancer.");
}
