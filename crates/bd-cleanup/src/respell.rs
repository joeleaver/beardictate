//! Misheard special words the cleanup model may respell.
//!
//! The Bear fine-tunes fix a listed name the speech recognizer misheard
//! ("desk car" → Teskar) in the same call as the cleanup. The guardrail
//! rejects any changed word, so those fixes are allowed only where this code,
//! not the model, finds a span that sounds like a listed word: the same test
//! the training data was built with (`training/special_words.py::sounds_like`),
//! so every fix the model learned is one this allows.
//!
//! [`respell`] puts the allowed names into the dictated text where the
//! model's answer has them; the guardrail then checks the answer against
//! that text as usual.

use std::ops::Range;

/// A heard span that may become a listed word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Respelling {
    /// Byte range of the heard words in the dictated text (any `'s` after
    /// them stays outside).
    pub span: Range<usize>,
    pub heard: String,
    pub word: String,
}

/// Spans made only of these are never respelled: "at the" is not a
/// misheard Oduya.
const COMMON: &str = "a an the and or but if so as at by for from in into of off on onto out over to up down with \
about after again all also am any are around away back be been before being both came can come could \
day did do does done each even ever far few first get go going gone good got had has have he her here him \
his how i is it its just know last left like long look made make man many may me more most much must my \
never new no nor not now off oh old once one only other our over own put right said same saw say see seen \
she should since some still such take than that them then there these they thing think this those though \
three through time told too took two under until upon us very was way we well went were what when where \
which while who why will with would yes yet you your ah eh yeah okay ok";

fn vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// A crude sound key: voicing merged, vowel runs as `A`, doubles collapsed.
pub fn key(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut s = String::new();
    for w in lower.split(|c: char| !c.is_ascii_lowercase()).filter(|w| !w.is_empty()) {
        let b = w.as_bytes();
        let silent_e = w.len() > 3 && w.ends_with('e') && !vowel(b[b.len() - 2] as char);
        s.push_str(if silent_e { &w[..w.len() - 1] } else { w });
    }
    for (from, to) in [("kn", "n"), ("gn", "n"), ("wr", "r"), ("ps", "s"), ("x", "s"), ("wh", "w")] {
        if let Some(rest) = s.strip_prefix(from) {
            s = format!("{to}{rest}");
        }
    }
    for (from, to) in [("tch", "X"), ("sch", "sk"), ("ch", "X"), ("sh", "X"), ("th", "t"), ("ph", "f"), ("gh", "g"), ("ck", "k")] {
        s = s.replace(from, to);
    }
    // A soft c before e, i, y.
    let chars: Vec<char> = s.chars().collect();
    s = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| if c == 'c' && chars.get(i + 1).is_some_and(|n| matches!(n, 'e' | 'i' | 'y')) { 's' } else { c })
        .collect();
    for (from, to) in [
        ("c", "k"), ("q", "k"), ("x", "ks"), ("dg", "j"), ("z", "s"), ("b", "p"), ("d", "t"), ("g", "k"), ("v", "f"), ("j", "X"),
    ] {
        s = s.replace(from, to);
    }
    s = s.replace('h', "");
    let chars: Vec<char> = s.chars().collect();
    s = chars.iter().enumerate().filter(|&(i, &c)| c != 'w' || chars.get(i + 1).is_some_and(|&n| vowel(n))).map(|(_, &c)| c).collect();
    let mut out = String::new();
    for c in s.chars() {
        let c = if vowel(c) { 'A' } else { c };
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

fn lev(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            cur.push((prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + usize::from(ca != cb)));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Word spans: ASCII letters, joined across an apostrophe.
fn words(text: &str) -> Vec<Range<usize>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        loop {
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            // An apostrophe (' or ’) followed by a letter continues the word.
            let apos = if text[i..].starts_with('\'') { 1 } else if text[i..].starts_with('\u{2019}') { 3 } else { 0 };
            if apos > 0 && b.get(i + apos).is_some_and(u8::is_ascii_alphabetic) {
                i += apos;
            } else {
                break;
            }
        }
        out.push(start..i);
    }
    out
}

/// `text` without a possessive `'s` at the end of a word.
fn strip_possessive(text: &str) -> String {
    let mut out = text.to_string();
    for apos in ["'s", "\u{2019}s"] {
        while let Some(at) = out.find(apos).filter(|&at| !out[at + apos.len()..].starts_with(|c: char| c.is_alphanumeric())) {
            out.replace_range(at..at + apos.len(), "");
        }
    }
    out
}

/// May the heard `span` be restored to the listed `word`?
pub fn sounds_like(span: &str, word: &str) -> bool {
    let stripped = strip_possessive(span);
    let ws: Vec<String> = words(&stripped).into_iter().map(|r| stripped[r].to_lowercase()).collect();
    let (n, wn) = (ws.len(), word.split_whitespace().count());
    if ws.is_empty() || !(wn..=wn + 2).contains(&n) || n > 4 {
        return false;
    }
    let joined = ws.join(" ");
    if joined == word.to_lowercase() || ws.iter().all(|w| COMMON.split(' ').any(|c| c == w)) {
        return false;
    }
    let (ks, kw) = (key(&joined), key(word));
    let consonants = kw.chars().filter(|&c| c != 'A').count();
    let allow = match consonants {
        0..=3 => 0,
        4..=5 => 1,
        _ => 2,
    };
    ks.chars().next() == kw.chars().next() && lev(&ks, &kw) <= allow
}

/// Every span of 1–4 words in `text`, with only spaces between them, that
/// sounds like a listed word, closest first.
pub fn candidates(text: &str, names: &[String]) -> Vec<Respelling> {
    let w = words(text);
    let mut found: Vec<(usize, Respelling)> = Vec::new();
    for i in 0..w.len() {
        for j in i..w.len().min(i + 4) {
            if j > i && !text[w[j - 1].end..w[j].start].chars().all(|c| c == ' ') {
                break;
            }
            let span = w[i].start..w[j].end;
            let heard = &text[span.clone()];
            for name in names {
                if sounds_like(heard, name) {
                    let distance = lev(&key(heard), &key(name)) * 8 + (j - i);
                    let span = span.start..span.start + heard.len() - possessive_len(heard);
                    found.push((distance, Respelling { heard: text[span.clone()].to_string(), span, word: name.clone() }));
                }
            }
        }
    }
    found.sort_by_key(|(d, r)| (*d, r.span.start));
    found.into_iter().map(|(_, r)| r).collect()
}

fn possessive_len(heard: &str) -> usize {
    ["'s", "\u{2019}s"].iter().find(|p| heard.ends_with(*p)).map_or(0, |p| p.len())
}

/// How many times `word` appears in `text` as whole words, any case.
fn count(text: &str, word: &str) -> usize {
    let (t, w) = (text.to_lowercase(), word.to_lowercase());
    let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    t.match_indices(&w).filter(|(at, _)| !letter(t[..*at].chars().next_back()) && !letter(t[at + w.len()..].chars().next())).count()
}

/// The respellings `answer` made: listed words it has more of than the
/// dictated `text`, each at a sound-alike span that doesn't overlap
/// another. Returns them with `text` respelled to match.
pub fn respell(text: &str, answer: &str, names: &[String]) -> (String, Vec<Respelling>) {
    let wanted: Vec<&String> = names.iter().filter(|n| count(answer, n) > count(text, n)).collect();
    if wanted.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut chosen: Vec<Respelling> = Vec::new();
    for r in candidates(text, names) {
        let budget = count(answer, &r.word) - count(text, &r.word).min(count(answer, &r.word));
        let used = chosen.iter().filter(|c| c.word == r.word).count();
        let overlaps = chosen.iter().any(|c| c.span.start < r.span.end && r.span.start < c.span.end);
        if wanted.contains(&&r.word) && used < budget && !overlaps {
            chosen.push(r);
        }
    }
    chosen.sort_by_key(|r| r.span.start);
    let mut out = text.to_string();
    for r in chosen.iter().rev() {
        out.replace_range(r.span.clone(), &r.word);
    }
    (out, chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_matches_the_training_code() {
        // From training/special_words.py::key.
        for (text, want) in [
            ("desk car", "tAskAr"), ("Teskar", "tAskAr"), ("Tuscar", "tAskAr"), ("Hollen Reach", "AlAnrAX"),
            ("Knight", "nAkt"), ("phone", "fAn"), ("Wright", "rAkt"), ("Psyche", "sAX"), ("Xander", "sAntAr"),
            ("whistle", "wAstl"), ("church", "XArX"), ("school", "skAl"), ("thought", "tAkt"), ("Caoimhe", "kAm"),
            ("Kee va", "kAfA"), ("Neve", "nAf"), ("Niamh", "nAm"), ("Able", "Apl"), ("Elba", "AlpA"),
            ("Mira", "mArA"), ("judge", "XAX"), ("city", "sAtA"), ("Oyelaran", "AlArAn"), ("Ice old", "AsAlt"),
            ("Ysolde", "AsAlt"), ("Aether", "AtAr"), ("Aethra", "AtrA"), ("queue", "kA"),
            ("Wyrmstone", "wArmstAn"), ("worm stone", "wArmstAn"), ("Bartholomew", "pArtAlAmA"),
            ("Goethe", "kAt"), ("Lowe", "lA"), ("throw", "trA"),
        ] {
            assert_eq!(key(text), want, "{text}");
        }
    }

    #[test]
    fn sounds_like_matches_the_training_code() {
        for (span, word, want) in [
            ("desk car", "Teskar", true), ("Tuscar", "Teskar", true), ("Tescar", "Teskar", true),
            ("Hollen Reach", "Hollin Reach", true), ("at the", "Oduya", false), ("Teskar", "Teskar", false),
            ("Neve", "Niamh", false), ("Able", "Elba", false), ("Mira", "Mara", true), ("Ice old", "Ysolde", true),
            ("worm stone", "Wyrmstone", true), ("Tuscar's", "Teskar", true), ("desk car was", "Teskar", false),
            ("the desk car", "Teskar", false), ("Kee va", "Caoimhe", false), ("Aether", "Aethra", false),
        ] {
            assert_eq!(sounds_like(span, word), want, "{span} -> {word}");
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn respells_only_what_the_answer_fixed() {
        let n = names(&["Teskar", "Ysolde"]);
        let (text, done) = respell("Then desk car's ship left, and Ice old waved.", "Then Teskar's ship left, and Ice old waved.", &n);
        assert_eq!(text, "Then Teskar's ship left, and Ice old waved.");
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].heard.as_str(), done[0].word.as_str()), ("desk car", "Teskar"));
        // No name in the answer that the dictation lacked: nothing to do.
        assert_eq!(respell("The desk car.", "The desk car.", &n).1, []);
        // A span across punctuation is never one name.
        assert_eq!(respell("The desk, car.", "The Teskar.", &n).1, []);
    }
}
