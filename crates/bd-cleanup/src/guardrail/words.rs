//! Splitting text into comparable words and the gaps between them.

/// One word, as the guardrail compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The word as written (`Didn’t`).
    pub text: String,
    /// Lowercased, typographic apostrophes folded to `'` (`didn't`). Two
    /// words are "the same word" when their `norm`s are equal.
    pub norm: String,
    /// A bracketed noise tag such as `[cough]`, kept whole.
    pub tag: bool,
    /// A cut-off fragment: the word ends in a hyphen followed by a space or
    /// the end of the text (`th- the`).
    pub fragment: bool,
}

/// What lies between two words: punctuation, quote marks, whitespace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gap {
    pub text: String,
}

impl Gap {
    /// Quotation marks in the gap. Straight `'` is not counted: in a gap it
    /// is far more often an apostrophe (`goin'`) than a quote.
    pub fn quotes(&self) -> usize {
        self.text.chars().filter(|c| is_quote(*c)).count()
    }

    /// 2 for a paragraph break (two newlines, possibly with spaces between),
    /// 1 for a line break, 0 otherwise.
    pub fn break_level(&self) -> u8 {
        match self.text.chars().filter(|c| *c == '\n').count() {
            0 => 0,
            1 => 1,
            _ => 2,
        }
    }

    /// True when the gap holds a mark that cuts speech off: a dash or an
    /// ellipsis.
    pub fn is_cut_off(&self) -> bool {
        self.text.contains(['-', '\u{2013}', '\u{2014}', '\u{2026}']) || self.text.contains("..")
    }

    pub fn has_comma(&self) -> bool {
        self.text.contains(',')
    }
}

/// Words with a gap before each and one after the last:
/// `gaps.len() == words.len() + 1`, and `gaps[k]` comes before `words[k]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    pub words: Vec<Word>,
    pub gaps: Vec<Gap>,
}

const QUOTES: &[char] = &[
    '"', '\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}', '\u{201E}', '\u{201F}', '\u{00AB}',
    '\u{00BB}', '\u{2039}', '\u{203A}',
];

pub(crate) fn is_quote(c: char) -> bool {
    QUOTES.contains(&c)
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '\u{02BC}')
}

/// Folds case and apostrophes so that casing and `’`/`'` never count as a
/// change.
pub fn normalize(word: &str) -> String {
    word.chars()
        .map(|c| if is_apostrophe(c) { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Splits `text` into words and gaps.
///
/// A word is a run of letters and digits, with apostrophes kept only
/// *between* two letters or digits (`didn't`, `rock'n'roll`). Everything
/// else separates words, including hyphens and dashes, so `well-known` and
/// `well known` compare equal, and a quote mark next to an apostrophe
/// (`‘Tis`, `dogs’`) never changes the word. `[...]` is a single tag word.
pub fn tokenize(text: &str) -> Tokens {
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new();
    let mut gaps = Vec::new();
    let mut gap = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' && let Some(len) = chars[i..].iter().position(|&c| c == ']') {
            let body: String = chars[i..=i + len].iter().collect();
            gaps.push(Gap { text: std::mem::take(&mut gap) });
            words.push(Word { norm: normalize(&body), text: body, tag: true, fragment: false });
            i += len + 1;
            continue;
        }
        if !c.is_alphanumeric() {
            gap.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() {
            let c = chars[i];
            let inner_apostrophe = is_apostrophe(c)
                && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric())
                && i > start;
            if c.is_alphanumeric() || inner_apostrophe {
                i += 1;
            } else {
                break;
            }
        }
        let word: String = chars[start..i].iter().collect();
        let fragment = chars.get(i) == Some(&'-') && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        gaps.push(Gap { text: std::mem::take(&mut gap) });
        words.push(Word { norm: normalize(&word), text: word, tag: false, fragment });
    }
    gaps.push(Gap { text: gap });
    Tokens { words, gaps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norms(s: &str) -> Vec<String> {
        tokenize(s).words.into_iter().map(|w| w.norm).collect()
    }

    #[test]
    fn apostrophes_inside_words_only() {
        assert_eq!(norms("Didn’t he?"), ["didn't", "he"]);
        assert_eq!(norms("didn't he"), ["didn't", "he"]);
        assert_eq!(norms("‘Tis the dogs’ bowl"), ["tis", "the", "dogs", "bowl"]);
        assert_eq!(norms("'Tis the dogs' bowl"), ["tis", "the", "dogs", "bowl"]);
        assert_eq!(norms("rock'n'roll"), ["rock'n'roll"]);
    }

    #[test]
    fn dashes_and_hyphens_separate_words() {
        assert_eq!(norms("well-known"), ["well", "known"]);
        assert_eq!(norms("up—not"), ["up", "not"]);
        assert_eq!(norms("up – not"), ["up", "not"]);
        assert_eq!(norms("wait..."), ["wait"]);
    }

    #[test]
    fn quotes_and_breaks_live_in_gaps() {
        let t = tokenize("“Get down,” Tom hissed.\n\nShe ran.");
        assert_eq!(t.gaps.len(), t.words.len() + 1);
        assert_eq!(t.gaps[0].quotes(), 1);
        assert_eq!(t.gaps[2].quotes(), 1);
        assert_eq!(t.gaps[4].break_level(), 2);
        assert_eq!(t.gaps[4].text, ".\n\n");
    }

    #[test]
    fn tags_and_fragments() {
        let t = tokenize("the [cough] th- the end-");
        assert!(t.words[1].tag);
        assert_eq!(t.words[1].norm, "[cough]");
        assert!(t.words[2].fragment);
        assert!(!t.words[3].fragment);
        assert!(t.words[4].fragment);
        assert!(!tokenize("well-known").words[0].fragment);
    }

    #[test]
    fn empty_and_punctuation_only() {
        assert_eq!(tokenize("").gaps.len(), 1);
        let t = tokenize(" “…” ");
        assert!(t.words.is_empty());
        assert_eq!(t.gaps[0].quotes(), 2);
    }
}
