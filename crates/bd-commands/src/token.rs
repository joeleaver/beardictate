//! Splitting ASR text into words with the punctuation the ASR attached.

use std::ops::Range;

/// One whitespace-separated word of ASR output, split into the punctuation
/// the ASR glued to its front and back and the word itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    /// Punctuation before the first alphanumeric character, e.g. `“` or `(`.
    pub lead: String,
    /// From the first to the last alphanumeric character, keeping inner
    /// apostrophes and hyphens (`didn't`, `well-known`).
    pub core: String,
    /// `core` lowercased with apostrophes normalized, for matching.
    pub lower: String,
    /// Punctuation after the last alphanumeric character, e.g. `.` or `,”`.
    pub trail: String,
    /// Byte range of the whole token in the input.
    pub span: Range<usize>,
}

impl Token {
    /// The token as it appeared, punctuation included.
    pub fn text(&self) -> String {
        format!("{}{}{}", self.lead, self.core, self.trail)
    }

    /// True when the token has no letters or digits at all (a lone `—`).
    pub fn is_bare_punct(&self) -> bool {
        self.core.is_empty()
    }

    /// True when a sentence-ending mark (`.`, `!`, `?`, `…`) trails the word.
    pub fn ends_sentence(&self) -> bool {
        self.trail.chars().any(|c| matches!(c, '.' | '!' | '?' | '…'))
    }
}

/// True when no punctuation separates `a` from the token after it, which is
/// what a multi-word command or a noun phrase needs.
pub(crate) fn joined(a: &Token, b: &Token) -> bool {
    a.trail.is_empty() && b.lead.is_empty() && !a.is_bare_punct() && !b.is_bare_punct()
}

/// Splits `text` on whitespace into [`Token`]s. Hyphenated words whose parts
/// spell a command (`open-quote`, `new-paragraph`) are split into one token
/// per part, so the matcher sees the same words either way. `is_command`
/// reports whether a lowercase word sequence is a command phrase.
pub(crate) fn tokenize(text: &str, is_command: impl Fn(&[&str]) -> bool) -> Vec<Token> {
    let mut out = Vec::new();
    for (start, word) in split_whitespace_indices(text) {
        let token = split_token(word, start);
        let parts: Vec<&str> = token.lower.split('-').collect();
        if parts.len() > 1 && parts.iter().all(|p| !p.is_empty()) && is_command(&parts) {
            let core_parts: Vec<&str> = token.core.split('-').collect();
            let mut offset = start + token.lead.len();
            let last = core_parts.len() - 1;
            for (k, part) in core_parts.iter().enumerate() {
                let lead = if k == 0 { token.lead.clone() } else { String::new() };
                let trail = if k == last { token.trail.clone() } else { String::new() };
                let begin = if k == 0 { start } else { offset };
                let end = if k == last { token.span.end } else { offset + part.len() };
                out.push(Token {
                    lead,
                    core: (*part).to_string(),
                    lower: normalize(part),
                    trail,
                    span: begin..end,
                });
                offset += part.len() + 1;
            }
        } else {
            out.push(token);
        }
    }
    out
}

/// Lowercases and maps typographic apostrophes to `'`.
pub(crate) fn normalize(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            '\u{2019}' | '\u{2018}' | '\u{02BC}' => '\'',
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn split_token(word: &str, start: usize) -> Token {
    let first = word.char_indices().find(|(_, c)| c.is_alphanumeric());
    let last = word.char_indices().rev().find(|(_, c)| c.is_alphanumeric());
    match (first, last) {
        (Some((a, _)), Some((b, cb))) => {
            let end = b + cb.len_utf8();
            let core = &word[a..end];
            Token {
                lead: word[..a].to_string(),
                core: core.to_string(),
                lower: normalize(core),
                trail: word[end..].to_string(),
                span: start..start + word.len(),
            }
        }
        _ => Token {
            lead: word.to_string(),
            core: String::new(),
            lower: String::new(),
            trail: String::new(),
            span: start..start + word.len(),
        },
    }
}

fn split_whitespace_indices(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut rest = text;
    let mut base = 0;
    std::iter::from_fn(move || {
        let skip = rest.len() - rest.trim_start().len();
        base += skip;
        rest = &rest[skip..];
        if rest.is_empty() {
            return None;
        }
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let item = (base, &rest[..len]);
        base += len;
        rest = &rest[len..];
        Some(item)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Token> {
        tokenize(s, |p| p == ["open", "quote"])
    }

    #[test]
    fn splits_punctuation_off_the_word() {
        let t = toks("“Open quote.” didn't —");
        assert_eq!(t.len(), 4);
        assert_eq!((t[0].lead.as_str(), t[0].core.as_str()), ("“", "Open"));
        assert_eq!((t[1].core.as_str(), t[1].trail.as_str()), ("quote", ".”"));
        assert_eq!(t[2].lower, "didn't");
        assert!(t[3].is_bare_punct());
        assert_eq!(t[1].span, 8..17);
    }

    #[test]
    fn splits_hyphenated_commands_only() {
        let t = toks("Open-quote, well-known");
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].core, "Open");
        assert_eq!(t[1].core, "quote");
        assert_eq!(t[1].trail, ",");
        assert_eq!(t[2].core, "well-known");
        assert_eq!(t[0].span, 0..4);
        assert_eq!(t[1].span, 5..11);
    }

    #[test]
    fn normalizes_curly_apostrophes() {
        assert_eq!(normalize("Didn’t"), "didn't");
    }
}
