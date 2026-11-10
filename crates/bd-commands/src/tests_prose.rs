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
    let pass = CommandPass::new(CommandConfig { literal_punctuation: true, ..CommandConfig::default() }, Style::default());
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
    let pass = CommandPass::new(CommandConfig { literal_punctuation: true, ..CommandConfig::default() }, Style::default());
    assert_eq!(pass.apply("He had colon cancer.", false).text, "He had: cancer.");
    assert_eq!(CommandPass::default().apply("He had colon cancer.", false).text, "He had colon cancer.");
}

/// "dash" and "hyphen" as ordinary words, with spoken marks on: none of
/// these may become a mark.
const DASH_PROSE: &[&str] = &[
    "He made a dash for the door.",
    "They made a mad dash for it.",
    "They made a mad dash home.",
    "It was a last-minute dash.",
    "Add a dash of salt.",
    "A dash of lime, then stir.",
    "Just a dash.",
    "I need to dash off a note.",
    "Dash off a quick reply.",
    "I'll dash to the shop.",
    "We had to dash across the road.",
    "She will dash his hopes.",
    "It could dash their chances.",
    "Don't dash out into the road.",
    "Dash it all!",
    "Dash said he'd come.",
    "I told Dash to wait.",
    "Ask Dash and Violet.",
    "Dash.Wait",
    "The dashboard lit up.",
    "His dash cam caught it.",
    "Dash cam footage helped.",
    "The hundred yard dash was close.",
    "Note the dash.",
    "Items without an extra dash are the ones I don't remember.",
    "I don't get where the trailing dash comes from.",
    "I did not ask for a dash, but it was putting a dash in.",
    "Gotta put dash in the punctuation.",
    "Is it a dash or a hyphen?",
    "Morse is dot and dash.",
    "Dash is working now.",
    "Use a hyphen here.",
    "The hyphen key is stuck.",
    "Spell it with hyphen marks.",
    "Each hyphen was wrong.",
    "Hyphen is working now.",
    "No hyphen, please.",
    "Hyphen and dash differ.",
    "The em dash and the hyphen differ.",
];

#[test]
fn dash_and_hyphen_in_prose_stay_literal_even_when_enabled() {
    let pass = CommandPass::new(CommandConfig { literal_punctuation: true, ..CommandConfig::default() }, Style::default());
    for input in DASH_PROSE {
        let out = pass.apply(input, false);
        assert_eq!(out.text, *input, "rewrote prose");
        assert!(out.actions.is_empty(), "{input:?} produced {:?}", out.actions);
    }
}

/// Numbers that look like years but are said outside a date stay as said,
/// with the style guide on too.
const YEARLIKE_PROSE: &[&str] = &[
    "Read John nineteen twenty three.",
    "Zechariah twelve ten.",
    "Back in nineteen ninety seven.",
    "In nineteen eighty four we moved.",
    "Those nineteen fifties chairs.",
    "The soldiers march twenty miles.",
    "Then march twenty paces.",
    "We were at ten fifteen and climbing.",
    "Twenty twenty vision.",
];

#[test]
fn year_like_numbers_outside_dates_stay_as_said() {
    let pass = CommandPass::new(CommandConfig { numbers: crate::NumberStyle::StyleGuide, ..CommandConfig::default() }, Style::default());
    for input in YEARLIKE_PROSE {
        assert_eq!(pass.apply(input, false).text, *input, "rewrote prose");
    }
    for input in YEARLIKE_PROSE {
        assert_eq!(CommandPass::default().apply(input, false).text, *input, "as said by default");
    }
}
