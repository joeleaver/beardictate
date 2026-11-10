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
    // An ellipsis trails off mid-sentence as often as it ends one.
    punct(&["dot", "dot", "dot"], "\u{2026}", false),
    punct(&["question", "mark"], "?", true),
    punct(&["exclamation", "point"], "!", true),
    punct(&["exclamation", "mark"], "!", true),
    punct(&["full", "stop"], ".", true),
    punct(&["semi", "colon"], ";", false),
    punct(&["em", "dash"], "\u{2014}", false),
    // As the recognizer writes "em dash" more often than not.
    punct(&["m", "dash"], "\u{2014}", false),
    punct(&["emdash"], "\u{2014}", false),
    punct(&["en", "dash"], "\u{2013}", false),
    punct(&["n", "dash"], "\u{2013}", false),
    punct(&["endash"], "\u{2013}", false),
    // "en dash" as the recognizer sometimes hears it.
    punct(&["end", "dash"], "\u{2013}", false),
    // A hyphen joins the words on both sides ("well hyphen known"). "dash"
    // alone is an ordinary word too, so it has stricter context rules
    // ([`hyphen_ok`]).
    punct(&["hyphen"], "-", false),
    punct(&["dash"], "-", false),
    punct(&["period"], ".", true),
    punct(&["comma"], ",", false),
    punct(&["colon"], ":", false),
    punct(&["semicolon"], ";", false),
    punct(&["ellipsis"], "\u{2026}", false),
];

/// Whether `words` (lowercase) spell any command, for hyphen splitting.
pub(crate) fn is_command_phrase(words: &[&str]) -> bool {
    PHRASES.iter().any(|ph| ph.words == words)
}

/// Whether `words` (lowercase, no marks) could still turn out to be a
/// command when more words follow: they begin a command phrase, or are one
/// (whose context rules look at the word after it).
pub(crate) fn may_start_command(words: &[&str], literal_punctuation: bool) -> bool {
    !words.is_empty()
        && PHRASES
            .iter()
            .filter(|ph| literal_punctuation || !ph.opt_in)
            .any(|ph| ph.words.len() >= words.len() && ph.words[..words.len()] == *words)
}

/// Whether `words` (lowercase, no marks) are a spoken punctuation mark,
/// or its start.
pub(crate) fn may_be_mark(words: &[&str]) -> bool {
    !words.is_empty()
        && PHRASES
            .iter()
            .filter(|ph| matches!(ph.cmd, Cmd::Punct { .. }))
            .any(|ph| ph.words.len() >= words.len() && ph.words[..words.len()] == *words)
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

/// Words that, directly after "new line" or "new paragraph", show someone
/// is talking about the command ("new line is working pretty well").
const ABOUT_A_COMMAND: &[&str] = &["is", "was", "isn't", "wasn't", "works", "worked", "working", "command", "commands", "feature", "doesn't", "didn't"];

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
        Cmd::NewParagraph | Cmd::NewLine => {
            // "new line is working": talk about the command, not the command.
            let talked_about = next.is_some_and(|n| ABOUT_A_COMMAND.contains(&n.lower.as_str()));
            !noun_blocked(toks, i) && !next_is_noun && !talked_about
        }
        Cmd::Scratch => {
            // A clause starts here: the segment's start, a sentence's end
            // before it, or a capital the recognizer gave it mid-sentence
            // ("…apples Scratch that I…"). A comma isn't enough: "if it
            // itches, scratch that" is prose.
            let capital = first.core.starts_with(|c: char| c.is_uppercase());
            let left = i == 0
                || after_command
                || toks[i - 1].ends_sentence()
                || toks[i - 1].is_bare_punct()
                || capital;
            let right = end == toks.len()
                || !last.trail.is_empty()
                || toks[end].is_bare_punct()
                || match_at(toks, end, false, true).is_some()
                || (capital && toks[end].core.starts_with(|c: char| c.is_uppercase()));
            left && right && first.lead.is_empty()
        }
        Cmd::Punct { mark: "-", .. } => hyphen_ok(toks, i, ph.words == ["dash"], after_command),
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
                // Another spoken command: "question mark, new paragraph",
                // or marks said in a row.
                || match_at(toks, end, true, true).is_some()
        }
    }
}

/// Words that, directly after "hyphen" or "dash", show it is a noun in
/// prose ("hyphen key", "dash mark"). Like [`NOUN_FOLLOWERS`] without "of",
/// which a hyphen joins ("mother hyphen in hyphen law", "jack hyphen of").
const HYPHEN_NOUN_FOLLOWERS: &[&str] = &[
    "mark", "marks", "character", "characters", "symbol", "symbols", "key", "keys", "sign", "signs",
];

/// Words that, directly before "hyphen" or "dash", show someone is talking
/// about the mark ("spelled with hyphen", "no dash").
const HYPHEN_TALK: &[&str] = &["with", "without", "using", "no"];

/// Words that, directly before "dash", make it the verb or the noun of a
/// run ("to dash", "will dash", "a mad dash", "the hundred yard dash"),
/// one of a pair ("dot and dash"), or someone asking for the mark ("add
/// dash", "put dash in").
const DASH_BLOCKERS: &[&str] = &[
    "to", "will", "would", "could", "can", "can't", "cannot", "must", "might", "should", "shall", "did",
    "didn't", "don't", "won't", "i", "we", "they", "you", "he", "she", "mad", "quick", "frantic",
    "desperate", "sudden", "wild", "headlong", "final", "late", "short", "yard", "metre", "meter", "add",
    "put", "type", "insert", "and", "or", "little", "trailing", "leading", "extra", "double", "single",
];

/// Words that, directly after "dash", make it the verb ("dash off a note",
/// "dash to the shop", "dash his hopes", "dash it all") or the noun ("dash
/// of salt", "dash cam", "dash and dot"). A compound whose second part is
/// one of these ("follow-up", "add-on") needs "hyphen".
const DASH_FOLLOWERS: &[&str] = &[
    "of", "for", "off", "to", "out", "away", "across", "into", "through", "back", "home", "over",
    "around", "round", "down", "up", "past", "after", "in", "inside", "outside", "along", "from",
    "toward", "towards", "onto", "on", "at", "by", "between", "with", "it", "his", "her", "their", "my",
    "our", "your", "its", "all", "them", "him", "me", "us", "the", "a", "an", "any", "this", "that",
    "these", "those", "and", "or", "cam", "cams", "camera", "cameras", "board", "boards", "light",
    "lights",
];

/// Whether "hyphen" (or, with `dash`, "dash") at token `i` is the mark.
///
/// Both: said on its own (the whole segment), or after a word of the same
/// sentence (or another spoken mark); not a noun ("a hyphen", "the dash",
/// "hyphen key") and not talked about ("dash is working", "with hyphen").
///
/// "dash" also needs a word after it in the same sentence, with no mark
/// between (a sentence ending on "dash" is prose: "a last-minute dash."),
/// and is never the verb or a run ([`DASH_BLOCKERS`], [`DASH_FOLLOWERS`])
/// nor a name: a capital "Dash" mid-sentence ("I told Dash") is someone,
/// unless the words before it are spelled-out letters ("J O T Dash").
fn hyphen_ok(toks: &[Token], i: usize, dash: bool, after_command: bool) -> bool {
    let tok = &toks[i];
    if toks.len() == 1 {
        return true;
    }
    let prev = i.checked_sub(1).map(|k| &toks[k]);
    let next = toks.get(i + 1);
    let joined_next = next.filter(|n| joined(tok, n));
    let prev_word = prev.filter(|p| joined(p, tok)).map(|p| p.lower.as_str());
    let next_word = joined_next.map(|n| n.lower.as_str());
    let left = after_command || prev.is_some_and(|p| !p.ends_sentence());
    let noun = noun_blocked(toks, i)
        || next_word.is_some_and(|w| HYPHEN_NOUN_FOLLOWERS.contains(&w) || ABOUT_A_COMMAND.contains(&w))
        || prev_word.is_some_and(|w| HYPHEN_TALK.contains(&w));
    if !left || noun {
        return false;
    }
    if !dash {
        return true;
    }
    let capital = tok.core.starts_with(|c: char| c.is_uppercase());
    let spelled = prev.is_some_and(|p| p.core.chars().count() == 1 && p.core.chars().all(char::is_uppercase));
    let name = capital && !after_command && !spelled;
    let word_after = joined_next.is_some() || (tok.trail.is_empty() && next.is_some_and(|n| n.is_bare_punct()));
    word_after
        && !name
        && !prev_word.is_some_and(|w| DASH_BLOCKERS.contains(&w))
        && !next_word.is_some_and(|w| DASH_FOLLOWERS.contains(&w))
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
