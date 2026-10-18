//! Commands as Parakeet is likely to render them.

use bd_core::{QuoteStyle, Style};

use crate::{ActionKind, CommandConfig, CommandPass, command_tail};

fn pass() -> CommandPass {
    CommandPass::default()
}

fn punct_pass() -> CommandPass {
    CommandPass::new(CommandConfig { literal_punctuation: true, ..CommandConfig::default() }, Style::default())
}

/// (ASR text, expected output) with structural commands only.
const STRUCTURAL: &[(&str, &str)] = &[
    // Bare, as in the seed eval (no ASR punctuation at all).
    (
        "open quote get down close quote Tom hissed new paragraph she dropped to her knees",
        "“get down” Tom hissed\n\nShe dropped to her knees",
    ),
    // Parakeet punctuating the pauses around commands.
    ("Open quote. Get down. Close quote. Tom hissed.", "“Get down.” Tom hissed."),
    ("Open quote, get down, close quote, Tom hissed.", "“get down,” Tom hissed."),
    ("He said, open quote, I'm leaving, close quote.", "He said, “I'm leaving.”"),
    ("Open quote, where are you going? Close quote, she asked.", "“where are you going?” she asked."),
    ("She whispered open quote run close quote and ran.", "She whispered “run” and ran."),
    // Hyphenated and capitalized variants.
    ("Open-quote run, close-quote.", "“run.”"),
    ("OPEN QUOTE run END QUOTE", "“run”"),
    ("Begin quote run unquote.", "“run.”"),
    ("Open quotes run close quotes", "“run”"),
    ("He said open quote run end of quote", "He said “run”"),
    // quote … end quote.
    ("He said quote get down end quote and left.", "He said “get down” and left."),
    ("Quote. Get down. End quote.", "“Get down.”"),
    (
        "To quote my father, quote, never again, end quote.",
        "To quote my father, “never again.”",
    ),
    // A segment that starts or ends mid-quote (state carried by the caller).
    ("I'm not going. Close quote. She folded her arms.", "I'm not going.” She folded her arms."),
    ("He turned. Open quote, wait.", "He turned. “wait."),
    // Paragraph and line breaks, with the ASR's sentence end moved back.
    ("Tom hissed new paragraph. She dropped.", "Tom hissed.\n\nShe dropped."),
    ("Tom hissed, new paragraph, she dropped.", "Tom hissed,\n\nShe dropped."),
    ("Tom hissed. New paragraph. She dropped.", "Tom hissed.\n\nShe dropped."),
    ("New paragraph. The rain stopped.", "\n\nThe rain stopped."),
    ("The rain stopped. New paragraph.", "The rain stopped.\n\n"),
    ("Dear John, new line, I miss you.", "Dear John,\nI miss you."),
    ("new-paragraph the end", "\n\nThe end"),
    (
        "Close quote. New paragraph. Open quote. Fine.",
        "”\n\n“Fine.",
    ),
    ("New paragraph. Open quote, no, close quote, he said.", "\n\n“No,” he said."),
    // Scratch that, within a segment.
    ("He went left. Scratch that. He went right.", "He went right."),
    ("He went left, scratch that.", "He went left, scratch that."),
    ("He went left. Scratch that.", ""),
    ("Scratch that, he went right.", "he went right."),
];

#[test]
fn structural_commands() {
    for (input, expected) in STRUCTURAL {
        let open_at_start = input.starts_with("I'm not going");
        let out = pass().apply(input, open_at_start);
        assert_eq!(out.text, *expected, "input: {input:?}");
    }
}

/// (ASR text, expected output) with literal punctuation enabled.
const PUNCTUATION: &[(&str, &str)] = &[
    ("To the river comma he said period", "To the river, he said."),
    ("To the river, comma, he said. Period.", "To the river, he said."),
    ("Where question mark", "Where?"),
    ("Stop exclamation point She ran.", "Stop! She ran."),
    ("Stop exclamation mark", "Stop!"),
    ("He had one rule colon never look back", "He had one rule: never look back"),
    ("He waited dot dot dot", "He waited…"),
    ("She paused em dash then ran", "She paused—then ran"),
    ("Done full stop", "Done."),
    ("He left semicolon she stayed", "He left; she stayed"),
    ("It was over period. New paragraph. Open quote, done comma close quote he said.", "It was over.\n\n“Done,” he said."),
];

#[test]
fn literal_punctuation_when_enabled() {
    for (input, expected) in PUNCTUATION {
        let out = punct_pass().apply(input, false);
        assert_eq!(out.text, *expected, "input: {input:?}");
    }
}

#[test]
fn literal_punctuation_is_off_by_default() {
    for (input, _) in PUNCTUATION.iter().take(3) {
        let out = pass().apply(input, false);
        assert!(out.text.contains("comma") || out.text.contains("question") || out.text.contains("Period"), "{input:?} -> {:?}", out.text);
    }
}

#[test]
fn quote_marks_follow_style() {
    let cases = [
        (QuoteStyle::CurlyDouble, "“run”"),
        (QuoteStyle::CurlySingle, "‘run’"),
        (QuoteStyle::Straight, "\"run\""),
        (QuoteStyle::StraightSingle, "'run'"),
    ];
    for (quotes, expected) in cases {
        let style = Style { quotes, ..Style::default() };
        let out = CommandPass::new(CommandConfig::default(), style).apply("open quote run close quote", false);
        assert_eq!(out.text, expected);
    }
}

#[test]
fn actions_carry_offsets_into_the_output() {
    let out = pass().apply("Open quote, get down, close quote, Tom hissed. New paragraph. She ran.", false);
    let kinds: Vec<_> = out.actions.iter().map(|a| (a.kind.clone(), a.offset)).collect();
    let text = &out.text;
    assert_eq!(text, "“get down,” Tom hissed.\n\nShe ran.");
    assert_eq!(kinds[0], (ActionKind::OpenQuote, 0));
    assert_eq!(&text[kinds[1].1..kinds[1].1 + '”'.len_utf8()], "”");
    assert_eq!(kinds[1].0, ActionKind::CloseQuote);
    assert_eq!(kinds[2].0, ActionKind::ParagraphBreak);
    assert_eq!(&text[kinds[2].1..kinds[2].1 + 2], "\n\n");
    assert!(!out.quote_open);
}

#[test]
fn quote_state_carries_across_segments() {
    let first = pass().apply("He said, open quote, I'm not going.", false);
    assert!(first.quote_open);
    // With the quote already open, a closer doesn't reach back for "quote".
    let second = pass().apply("I won't quote him on that, end quote.", first.quote_open);
    assert_eq!(second.text, "I won't quote him on that.”");
    assert!(!second.quote_open);
    // Without it, the bare quote would open.
    let cold = pass().apply("He said quote run end quote", false);
    assert_eq!(cold.text, "He said “run”");
}

#[test]
fn scratch_previous_segment_is_an_action() {
    let out = pass().apply("Scratch that. He went right.", false);
    assert_eq!(out.text, "He went right.");
    assert_eq!(out.scratch_previous(), 1);
    let twice = pass().apply("Scratch that. Scratch that.", false);
    assert_eq!(twice.text, "");
    assert_eq!(twice.scratch_previous(), 2);
    assert!(!twice.has_words());
}

#[test]
fn scratch_within_segment_reports_what_it_dropped() {
    let out = pass().apply("He went left. Scratch that. He went right.", false);
    assert_eq!(out.scratch_previous(), 0);
    assert_eq!(
        out.actions[0].kind,
        ActionKind::ScratchWithinSegment { discarded: "He went left.".into() }
    );
    // A second scratch with nothing left in the segment reaches back further.
    let out = pass().apply("He went left. Scratch that. Scratch that.", false);
    assert_eq!(out.scratch_previous(), 1);
    assert_eq!(out.text, "");
}

#[test]
fn scratch_discards_commands_too() {
    let out = pass().apply("Open quote, run. Scratch that. He walked.", false);
    assert_eq!(out.text, "He walked.");
    assert!(!out.quote_open);
}

#[test]
fn empty_and_whitespace_input() {
    for input in ["", "   ", "\n"] {
        let out = pass().apply(input, false);
        assert_eq!(out.text, "");
        assert!(out.actions.is_empty());
    }
}

#[test]
fn text_without_commands_is_unchanged() {
    for input in [
        "The rain had not stopped for three days, and Mara was starting to think it never would.",
        "He didn't look up — not once.",
        "“Already quoted,” she said.",
        "Um, I, I think so.",
    ] {
        assert_eq!(pass().apply(input, false).text, input);
    }
}

#[test]
fn break_commands_can_be_turned_off() {
    let off = CommandPass::new(CommandConfig { break_commands: false, ..CommandConfig::default() }, Style::default());
    assert_eq!(off.apply("It ended. New paragraph. Morning came.", false).text, "It ended. New paragraph. Morning came.");
    let on = CommandPass::default();
    assert_eq!(on.apply("It ended. New paragraph. Morning came.", false).text, "It ended.\n\nMorning came.");
}

#[test]
fn speech_running_on_into_a_new_paragraph() {
    let line = "Open quote. It rose all night. New paragraph. And by morning it was gone. Close quote.";
    // Book style: no closing mark on the first paragraph; the next opens.
    assert_eq!(pass().apply(line, false).text, "\u{201C}It rose all night.\n\n\u{201C}And by morning it was gone.\u{201D}");
    // Closing each paragraph instead.
    let closed = CommandPass::new(
        CommandConfig::default(),
        Style { close_continued_speech: true, ..Style::default() },
    );
    assert_eq!(closed.apply(line, false).text, "\u{201C}It rose all night.\u{201D}\n\n\u{201C}And by morning it was gone.\u{201D}");
    // Across segments, and a spoken "open quote" after the break isn't doubled.
    let first = pass().apply("Open quote. It rose all night. New paragraph.", false);
    assert_eq!(first.text, "\u{201C}It rose all night.\n\n\u{201C}");
    assert!(first.quote_open);
    let second = pass().apply("Open quote. And by morning it was gone.", first.quote_open);
    assert_eq!(second.text, "And by morning it was gone.");
    assert_eq!(pass().apply("It rose. New paragraph. Open quote, gone.", true).text, "It rose.\n\n\u{201C}Gone.");
    // Outside a quotation a break is just a break.
    assert_eq!(pass().apply("It ended. New paragraph. Morning came.", false).text, "It ended.\n\nMorning came.");
}

#[test]
fn the_tail_that_may_still_be_a_command_is_held() {
    let c = CommandConfig::default();
    let tail = |s: &str| command_tail(&s.split_whitespace().collect::<Vec<_>>(), &c);
    assert_eq!(tail("It was late. New"), 1);
    assert_eq!(tail("It was late. New paragraph."), 2, "complete, but the next word decides");
    assert_eq!(tail("and then, end of"), 2);
    assert_eq!(tail("she said open"), 1);
    assert_eq!(tail("It was late."), 0);
    assert_eq!(tail("a period"), 0, "literal punctuation is off by default");
    let lit = CommandConfig { literal_punctuation: true, ..CommandConfig::default() };
    assert_eq!(command_tail(&["a", "period"], &lit), 1);
}
