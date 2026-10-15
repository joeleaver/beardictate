//! The user's own words: character names, places, invented words.
//!
//! Two uses:
//! - [`Vocabulary::hotwords`] goes to the ASR, which then listens harder for
//!   these spellings (sherpa-onnx hotword boosting).
//! - [`Vocabulary::apply`] runs on ASR text before the command pass. It
//!   replaces what the ASR is known to hear instead ("worm stone" for
//!   *wyrmstone*) and fixes the casing of the words themselves. It is plain
//!   whole-word matching, so it only ever changes what the list says to.
//!
//! The list format, one word per line:
//!
//! ```text
//! # comments and blank lines are ignored
//! Ysolde: Isolde, Ussel
//! wyrmstone: worm stone, wormstone
//! Port Varrow
//! ```
//!
//! Left of the colon is the spelling; right of it, comma-separated, what the
//! ASR writes instead. A heard form that is an ordinary word ("train" for
//! *Thrane*) is replaced everywhere, so list only real mishearings.

#[cfg(test)]
mod tests;

/// One of the user's words.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Word {
    /// The spelling to produce.
    pub text: String,
    /// What the ASR writes instead, in any casing.
    #[serde(default)]
    pub heard: Vec<String>,
}

/// A word list, ready to apply.
#[derive(Debug, Clone, Default)]
pub struct Vocabulary {
    words: Vec<Word>,
    /// Longest pattern first, so "Port Varrow" wins over "Varrow".
    rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
struct Rule {
    /// Lowercased words of what to match.
    pattern: Vec<String>,
    /// Index into `words`.
    word: usize,
}

/// A line of the list file that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl Vocabulary {
    pub fn new(words: Vec<Word>) -> Self {
        let mut rules = Vec::new();
        for (i, w) in words.iter().enumerate() {
            for form in std::iter::once(&w.text).chain(&w.heard) {
                let pattern: Vec<String> = tokens(form).map(|t| form[t].to_lowercase()).collect();
                if !pattern.is_empty() && !rules.iter().any(|r: &Rule| r.pattern == pattern) {
                    rules.push(Rule { pattern, word: i });
                }
            }
        }
        rules.sort_by_key(|r| std::cmp::Reverse(r.pattern.len()));
        Self { words, rules }
    }

    /// Reads the list format described in the crate docs.
    pub fn parse(list: &str) -> Result<Self, ParseError> {
        let mut words = Vec::new();
        for (n, line) in list.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (text, heard) = line.split_once(':').unwrap_or((line, ""));
            let text = text.trim();
            if tokens(text).next().is_none() {
                return Err(ParseError { line: n + 1, message: format!("no word in {line:?}") });
            }
            let heard = heard.split(',').map(str::trim).filter(|h| !h.is_empty()).map(String::from).collect();
            words.push(Word { text: text.to_string(), heard });
        }
        Ok(Self::new(words))
    }

    pub fn words(&self) -> &[Word] {
        &self.words
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The spellings, for the ASR's hotword list.
    pub fn hotwords(&self) -> Vec<String> {
        self.words.iter().map(|w| w.text.clone()).collect()
    }

    /// Replaces heard forms with the user's spellings in `text`.
    pub fn apply(&self, text: &str) -> String {
        if self.rules.is_empty() {
            return text.to_string();
        }
        let toks: Vec<Range> = tokens(text).collect();
        let lower: Vec<String> = toks.iter().map(|t| text[t.clone()].to_lowercase()).collect();
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        let mut i = 0;
        while i < toks.len() {
            let Some((rule, suffix)) = self.rules.iter().find_map(|r| matches(r, text, &toks, &lower, i).map(|s| (r, s)))
            else {
                i += 1;
                continue;
            };
            let start = toks[i].start;
            let end = toks[i + rule.pattern.len() - 1].end;
            out.push_str(&text[copied..start]);
            let spelling = &self.words[rule.word].text;
            out.push_str(&cased(spelling, &text[start..end], sentence_start(&text[..start])));
            out.push_str(&text[end - suffix..end]);
            copied = end;
            i += rule.pattern.len();
        }
        out.push_str(&text[copied..]);
        out
    }
}

type Range = std::ops::Range<usize>;

/// Whether `rule` matches the tokens starting at `i`. Returns the length in
/// bytes of a possessive `'s` left on the last token, which is kept.
fn matches(rule: &Rule, text: &str, toks: &[Range], lower: &[String], i: usize) -> Option<usize> {
    let n = rule.pattern.len();
    if i + n > toks.len() {
        return None;
    }
    for k in 1..n {
        // Only spaces may separate the words of a match, not a comma or a
        // sentence end.
        if !text[toks[i + k - 1].end..toks[i + k].start].chars().all(char::is_whitespace) {
            return None;
        }
    }
    for k in 0..n - 1 {
        if lower[i + k] != rule.pattern[k] {
            return None;
        }
    }
    let last = &lower[i + n - 1];
    let want = &rule.pattern[n - 1];
    if last == want {
        return Some(0);
    }
    ["'s", "\u{2019}s"]
        .iter()
        .find(|s| last.strip_suffix(*s) == Some(want.as_str()))
        .map(|s| s.len())
}

/// Word tokens: runs of letters and digits, joined by an apostrophe or
/// hyphen with letters on both sides.
fn tokens(text: &str) -> impl Iterator<Item = Range> + '_ {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < chars.len() && !chars[i].1.is_alphanumeric() {
            i += 1;
        }
        if i >= chars.len() {
            return None;
        }
        let start = chars[i].0;
        while i < chars.len() {
            let c = chars[i].1;
            let joiner = matches!(c, '\'' | '\u{2019}' | '-')
                && chars.get(i + 1).is_some_and(|n| n.1.is_alphanumeric())
                && i > 0
                && chars[i - 1].1.is_alphanumeric();
            if !(c.is_alphanumeric() || joiner) {
                break;
            }
            i += 1;
        }
        let end = chars.get(i).map_or(text.len(), |c| c.0);
        Some(start..end)
    })
}

/// True when the text before a match ends a sentence (or is empty).
fn sentence_start(before: &str) -> bool {
    let trimmed = before.trim_end_matches(|c: char| c.is_whitespace() || "\u{201C}\u{2018}\"(".contains(c));
    if trimmed.len() < before.len() && before[trimmed.len()..].contains('\n') {
        return true;
    }
    trimmed.chars().last().is_none_or(|c| ".!?\u{2026}".contains(c))
}

/// The spelling as it should appear: names keep their own capitals; an
/// all-lowercase word is capitalized only where the ASR capitalized it at
/// the start of a sentence.
fn cased(spelling: &str, heard: &str, at_sentence_start: bool) -> String {
    if spelling.chars().any(char::is_uppercase) {
        return spelling.to_string();
    }
    if at_sentence_start && heard.chars().next().is_some_and(char::is_uppercase) {
        let mut c = spelling.chars();
        return c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    }
    spelling.to_string()
}
