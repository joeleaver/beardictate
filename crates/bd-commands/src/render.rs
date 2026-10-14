//! Turning matched words and commands back into text.

use bd_core::Style;

use crate::rules::Cmd;
use crate::token::Token;

/// One command the pass applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub kind: ActionKind,
    /// Byte offset into [`crate::CommandOutput::text`] where the command took
    /// effect: the start of the inserted mark or break. Scratch actions use 0.
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    OpenQuote,
    CloseQuote,
    /// A `"\n\n"` was inserted.
    ParagraphBreak,
    /// A `"\n"` was inserted.
    LineBreak,
    /// A spoken punctuation word became this mark.
    Punctuation(&'static str),
    /// `scratch that` began the segment. The caller should discard the most
    /// recent earlier segment if it has not been typed yet (and say so if it
    /// has). Repeated, it reaches one segment further back each time.
    ScratchPreviousSegment,
    /// `scratch that` followed some words in this segment, which were dropped
    /// and are not in the output text.
    ScratchWithinSegment { discarded: String },
}

/// A word kept from the input, or a recognized command.
#[derive(Debug, Clone)]
pub(crate) enum Item {
    Word(Token),
    /// `trail` is the punctuation the ASR attached to the command's last word.
    Cmd { cmd: Cmd, trail: String },
}

/// Marks the ASR may guess at a word's end, which an explicit spoken mark
/// replaces.
const ASR_PUNCT: &[char] = &[',', '.', ';', ':', '!', '?', '\u{2026}'];

/// Renders items into text, returning the actions with their offsets.
pub(crate) fn render(items: &[Item], style: &Style) -> (String, Vec<Action>) {
    let (open_mark, close_mark) = style.quotes.marks();
    let mut out = String::new();
    let mut actions = Vec::new();
    let mut space = false;
    let mut capitalize = false;
    let mut last_was_word = false;
    for item in items {
        match item {
            Item::Word(tok) => {
                if space {
                    out.push(' ');
                }
                let text = tok.text();
                if capitalize {
                    out.push_str(&capitalize_first(&text));
                    capitalize = !text.chars().any(char::is_alphabetic);
                } else {
                    out.push_str(&text);
                }
                space = true;
                last_was_word = true;
                continue;
            }
            Item::Cmd { cmd, trail } => {
                let terminal = trail_terminal(trail);
                let kind = match *cmd {
                    Cmd::OpenQuote => {
                        if space {
                            out.push(' ');
                        }
                        actions.push(Action { kind: ActionKind::OpenQuote, offset: out.len() });
                        out.push(open_mark);
                        space = false;
                        None
                    }
                    Cmd::CloseQuote => {
                        if let (Some(t), true) = (terminal, last_was_word) {
                            end_sentence(&mut out, t);
                        }
                        trim_trailing_spaces(&mut out);
                        actions.push(Action { kind: ActionKind::CloseQuote, offset: out.len() });
                        out.push(close_mark);
                        space = true;
                        None
                    }
                    Cmd::NewParagraph => Some((ActionKind::ParagraphBreak, "\n\n")),
                    Cmd::NewLine => Some((ActionKind::LineBreak, "\n")),
                    Cmd::Punct { mark, terminal: ends } => {
                        if last_was_word {
                            while out.ends_with(ASR_PUNCT) {
                                out.pop();
                            }
                        }
                        trim_trailing_spaces(&mut out);
                        actions.push(Action { kind: ActionKind::Punctuation(mark), offset: out.len() });
                        out.push_str(mark);
                        // An em dash joins the words on both sides.
                        space = mark != "\u{2014}";
                        capitalize |= ends;
                        None
                    }
                    Cmd::Scratch => unreachable!("scratch is resolved before rendering"),
                };
                if let Some((kind, brk)) = kind {
                    if let (Some(t), true) = (terminal, last_was_word) {
                        end_sentence(&mut out, t);
                    }
                    trim_trailing_spaces(&mut out);
                    actions.push(Action { kind, offset: out.len() });
                    out.push_str(brk);
                    space = false;
                    capitalize = true;
                }
                last_was_word = false;
            }
        }
    }
    (out, actions)
}

/// The sentence-ending mark in a command's ASR punctuation, if any.
fn trail_terminal(trail: &str) -> Option<char> {
    trail.chars().find(|c| matches!(c, '.' | '!' | '?'))
}

/// Ends the sentence on the word just written: a trailing comma, colon or
/// semicolon the ASR guessed becomes `mark`. A word that already ends a
/// sentence, or ends in a closing quote or bracket, is left alone.
fn end_sentence(out: &mut String, mark: char) {
    if out.ends_with(['.', '!', '?', '\u{2026}']) || !out.ends_with(|c: char| c.is_alphanumeric() || matches!(c, ',' | ';' | ':')) {
        return;
    }
    while out.ends_with([',', ';', ':']) {
        out.pop();
    }
    out.push(mark);
}

fn trim_trailing_spaces(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
}

/// Uppercases the first letter, skipping leading punctuation such as `“`.
fn capitalize_first(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut done = false;
    for c in word.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
        }
    }
    out
}
