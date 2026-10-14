//! Platform-neutral text preparation: what gets typed, in what bursts.

/// One unit of typing. Each backend turns a token into a self-contained group
/// of key events (all keys pressed are released again within the group), so
/// cutting the stream between tokens never leaves a key stuck down or splits
/// a UTF-16 surrogate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Token {
    /// A printable character.
    Char(char),
    /// `'\n'`, `"\r\n"` or a lone `'\r'`: typed as the configured newline key.
    Newline,
    /// `'\t'`: typed as the Tab key.
    Tab,
}

/// Splits `text` into tokens. Line endings are normalized to one
/// [`Token::Newline`]; other control characters (C0, DEL, C1) are dropped,
/// because typing them yields shortcuts (Ctrl+letter) or nothing.
pub(crate) fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(Token::Newline);
            }
            '\n' => out.push(Token::Newline),
            '\t' => out.push(Token::Tab),
            c if c.is_control() => {
                tracing::debug!(codepoint = c as u32, "dropping control character");
            }
            c => out.push(Token::Char(c)),
        }
    }
    out
}

/// Turns tokens back into text, e.g. the undelivered rest after typing was
/// cut short, to leave it on the clipboard.
pub(crate) fn detokenize(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|t| match t {
            Token::Char(c) => *c,
            Token::Newline => '\n',
            Token::Tab => '\t',
        })
        .collect()
}

/// The distinct characters among `tokens` for which `typeable` is false, in
/// first-seen order. Used to explain why typing is impossible.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn untypeable(tokens: &[Token], mut typeable: impl FnMut(&Token) -> bool) -> Vec<char> {
    let mut missing = Vec::new();
    for t in tokens {
        if !typeable(t) {
            let c = match t {
                Token::Char(c) => *c,
                Token::Newline => '\n',
                Token::Tab => '\t',
            };
            if !missing.contains(&c) {
                missing.push(c);
            }
        }
    }
    missing
}

/// Human-readable list of characters for log/notification reasons.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn describe_chars(chars: &[char]) -> String {
    chars
        .iter()
        .map(|c| match c {
            '\n' => "newline".to_owned(),
            '\t' => "tab".to_owned(),
            c => format!("'{c}' (U+{:04X})", *c as u32),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use Token::*;

    #[test]
    fn line_endings_normalize_to_one_newline() {
        assert_eq!(
            tokenize("a\r\nb\rc\nd"),
            vec![
                Char('a'),
                Newline,
                Char('b'),
                Newline,
                Char('c'),
                Newline,
                Char('d')
            ]
        );
        assert_eq!(tokenize("\r\n\r\n"), vec![Newline, Newline]);
    }

    #[test]
    fn controls_dropped_tab_kept() {
        assert_eq!(
            tokenize("a\u{7}\tb\u{7f}\u{85}"),
            vec![Char('a'), Tab, Char('b')]
        );
    }

    #[test]
    fn non_bmp_and_typography_are_single_tokens() {
        assert_eq!(
            tokenize("“Hi”—😀"),
            vec![
                Char('“'),
                Char('H'),
                Char('i'),
                Char('”'),
                Char('—'),
                Char('😀')
            ]
        );
    }

    #[test]
    fn roundtrip_after_normalization() {
        let t = tokenize("“Go,” she said.\r\nThen\tleft 😀");
        assert_eq!(detokenize(&t), "“Go,” she said.\nThen\tleft 😀");
    }

    #[test]
    fn untypeable_lists_each_once_in_order() {
        let t = tokenize("“a” “b”");
        let missing = untypeable(&t, |t| !matches!(t, Char('“') | Char('”')));
        assert_eq!(missing, vec!['“', '”']);
        assert_eq!(describe_chars(&missing), "'“' (U+201C), '”' (U+201D)");
    }
}
