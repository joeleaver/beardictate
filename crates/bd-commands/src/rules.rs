//! The command vocabulary and the context rules that tell a spoken command
//! from the same words used in prose.

use crate::token::{Token, joined};

/// What a matched phrase does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cmd {
    OpenQuote,
    CloseQuote,
    NewParagraph,
    NewLine,
    Scratch,
    /// A literal punctuation mark; `terminal` marks end a sentence.
    Punct { mark: &'static str, terminal: bool },
}

struct Phrase {
    words: &'static [&'static str],
    cmd: Cmd,
    /// Literal-punctuation phrases only match when the config enables them.
    opt_in: bool,
}

const fn p(words: &'static [&'static str], cmd: Cmd) -> Phrase {
    Phrase { words, cmd, opt_in: false }
}

const fn punct(words: &'static [&'static str], mark: &'static str, terminal: bool) -> Phrase {
    Phrase { words, cmd: Cmd::Punct { mark, terminal }, opt_in: true }
}

/// Longest phrases first, so `end of quote` wins over a shorter prefix.
const PHRASES: &[Phrase] = &[
    p(&["end", "of", "quote"], Cmd::CloseQuote),
    p(&["open", "quote"], Cmd::OpenQuote),
    p(&["open", "quotes"], Cmd::OpenQuote),
    p(&["begin", "quote"], Cmd::OpenQuote),
    p(&["close", "quote"], Cmd::CloseQuote),
    p(&["close", "quotes"], Cmd::CloseQuote),
    p(&["end", "quote"], Cmd::CloseQuote),
    p(&["end", "quotes"], Cmd::CloseQuote),
    p(&["unquote"], Cmd::CloseQuote),
    p(&["new", "paragraph"], Cmd::NewParagraph),
    p(&["new", "line"], Cmd::NewLine),
    p(&["scratch", "that"], Cmd::Scratch),
    punct(&["dot", "dot", "dot"], "\u{2026}", true),
    punct(&["question", "mark"], "?", true),
    punct(&["exclamation", "point"], "!", true),
    punct(&["exclamation", "mark"], "!", true),
    punct(&["full", "stop"], ".", true),
    punct(&["semi", "colon"], ";", false),
    punct(&["em", "dash"], "\u{2014}", false),
    punct(&["period"], ".", true),
    punct(&["comma"], ",", false),
    punct(&["colon"], ":", false),
    punct(&["semicolon"], ";", false),
    punct(&["ellipsis"], "\u{2026}", true),
];

/// Whether `words` (lowercase) spell any command, for hyphen splitting.
pub(crate) fn is_command_phrase(words: &[&str]) -> bool {
    PHRASES.iter().any(|ph| ph.words == words)
}

/// Words that, directly before `open quote` / `end quote`, make it a noun
/// phrase ("the open quote", "an end quote") rather than a command. Kept
/// short on purpose: "told her open quote …" is a command.
const QUOTE_BLOCKERS: &[&str] = &[
    "a", "an", "the", "this", "these", "those", "each", "every", "another", "no", "any",
];

/// Words that, directly before `new paragraph`, `new line` or a punctuation
/// word, make it prose ("the new paragraph", "a period"). Also checked one
/// word further back across a [`MODIFIERS`] word ("his whole new line").
const NOUN_BLOCKERS: &[&str] = &[
    "a", "an", "the", "this", "that", "these", "those", "my", "your", "his", "her", "its",
    "our", "their", "each", "every", "another", "no", "any", "some", "one", "whose", "which",
    "what", "first", "last", "next",
];

/// Adjectives that can sit between a determiner and a command phrase used as
/// a noun ("a brand new line", "the final period").
const MODIFIERS: &[&str] = &[
    "whole", "brand", "entire", "single", "second", "third", "final", "fresh", "short", "long",
    "separate", "very", "lone", "stray", "extra", "missing", "same", "little", "tiny",
];

/// Words that, directly after a command phrase, show it is a noun in prose
/// ("new line of credit", "open quote marks", "comma splice").
const NOUN_FOLLOWERS: &[&str] = &[
    "of", "mark", "marks", "character", "characters", "symbol", "symbols", "splice", "splices",
    "break", "breaks", "drama", "dramas", "piece", "pieces", "key", "keys",
];

/// Words that keep a bare `quote` from opening a `quote … end quote` pair
/// ("to quote him", "a quote").
const BARE_QUOTE_BLOCKERS: &[&str] = &[
    "to", "a", "an", "the", "this", "that", "his", "her", "my", "your", "our", "their", "its",
    "one", "favorite", "favourite", "famous",
];

/// A command recognized at a token position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Match {
    pub cmd: Cmd,
    /// Number of tokens the phrase covers.
    pub len: usize,
}

/// Tries every phrase at token `i` and returns the first whose words match
/// and whose surroundings say "command" rather than "prose".
/// `after_command` is true when the previous token was itself consumed by a
/// command, which counts as a clause boundary.
pub(crate) fn match_at(toks: &[Token], i: usize, literal_punct: bool, after_command: bool) -> Option<Match> {
    PHRASES
        .iter()
        .filter(|ph| literal_punct || !ph.opt_in)
        .find(|ph| words_match(toks, i, ph.words) && context_ok(toks, i, ph, after_command))
        .map(|ph| Match { cmd: ph.cmd, len: ph.words.len() })
}

fn words_match(toks: &[Token], i: usize, words: &[&str]) -> bool {
    if i + words.len() > toks.len() {
        return false;
    }
    let span = &toks[i..i + words.len()];
    span.iter().zip(words).all(|(t, w)| t.lower == *w)
        && span.windows(2).all(|w| joined(&w[0], &w[1]))
}

fn context_ok(toks: &[Token], i: usize, ph: &Phrase, after_command: bool) -> bool {
    let end = i + ph.words.len();
    let first = &toks[i];
    let last = &toks[end - 1];
    let next = toks.get(end).filter(|n| joined(last, n));
    let next_is_noun = next.is_some_and(|n| NOUN_FOLLOWERS.contains(&n.lower.as_str()));
    match ph.cmd {
        Cmd::OpenQuote | Cmd::CloseQuote => {
            let blocked = prev_words(toks, i, 1)
                .iter()
                .any(|w| QUOTE_BLOCKERS.contains(&w.as_str()));
            !blocked && !next_is_noun && !(ph.words == ["unquote"] && preceded_by_bare_quote(toks, i))
        }
        Cmd::NewParagraph | Cmd::NewLine => !noun_blocked(toks, i) && !next_is_noun,
        Cmd::Scratch => {
            let left = i == 0 || after_command || toks[i - 1].ends_sentence() || toks[i - 1].is_bare_punct();
            let right = end == toks.len()
                || !last.trail.is_empty()
                || toks[end].is_bare_punct()
                || match_at(toks, end, false, true).is_some();
            left && right && first.lead.is_empty()
        }
        Cmd::Punct { terminal, .. } => {
            if noun_blocked(toks, i) || next_is_noun {
                return false;
            }
            // After a spoken "period", the next sentence must visibly start:
            // end of segment, ASR punctuation, or a capital letter. This keeps
            // "she loved period dramas" literal.
            !terminal
                || next.is_none()
                || next.is_some_and(|n| n.core.chars().next().is_some_and(char::is_uppercase))
        }
    }
}

/// True for `quote unquote`, the spoken scare-quote idiom, which stays
/// literal rather than producing an empty pair of marks.
fn preceded_by_bare_quote(toks: &[Token], i: usize) -> bool {
    i > 0 && toks[i - 1].lower == "quote" && toks[i - 1].lead.is_empty() && joined(&toks[i - 1], &toks[i])
        || i > 0 && toks[i - 1].lower == "quote" && toks[i - 1].trail == ","
}

fn noun_blocked(toks: &[Token], i: usize) -> bool {
    let prev = prev_words(toks, i, 2);
    let is_det = |w: &String| NOUN_BLOCKERS.contains(&w.as_str());
    match prev.as_slice() {
        [first, ..] if is_det(first) => true,
        [first, second] => MODIFIERS.contains(&first.as_str()) && is_det(second),
        _ => false,
    }
}

/// Up to `n` words directly before token `i`, nearest first, stopping at any
/// punctuation boundary.
fn prev_words(toks: &[Token], i: usize, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut k = i;
    while k > 0 && out.len() < n && joined(&toks[k - 1], &toks[k]) {
        out.push(toks[k - 1].lower.clone());
        k -= 1;
    }
    out
}

/// Whether the word token can be the `quote` of a `quote … end quote` pair.
/// `prev` is the word before it, if no punctuation separates them.
pub(crate) fn can_open_bare_quote(tok: &Token, prev: Option<&Token>) -> bool {
    tok.lower == "quote"
        && tok.lead.is_empty()
        && !prev.is_some_and(|p| joined(p, tok) && BARE_QUOTE_BLOCKERS.contains(&p.lower.as_str()))
}
