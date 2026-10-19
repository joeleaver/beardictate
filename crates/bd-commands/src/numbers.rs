//! Spoken numbers: reading them from the words the recognizer wrote, and
//! writing them as digits, for "numeral …" and the style-guide setting
//! ([`NumberStyle`]).
//!
//! The recognizer writes numbers as words ("twenty five", "the twenty
//! first", "ten thirty"). Left alone they stay that way, which is right for
//! most prose. "numeral" before one asks for digits. With
//! [`NumberStyle::StyleGuide`], the usual book rules apply by themselves
//! (as in the Chicago Manual of Style): words for zero to one hundred and
//! round numbers, hyphenated ("twenty-five"); digits for the rest; and
//! digits for money ("$300"), percentages ("12 percent"), times with a.m.
//! or p.m. ("4:30 p.m.") and dates after a month ("May 21"). "Four
//! o'clock" stays as words.

use serde::{Deserialize, Serialize};

/// How numbers said as words are written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NumberStyle {
    /// As said (words), except after "numeral".
    #[default]
    AsSaid,
    /// The style guide's rules, above.
    StyleGuide,
}

const UNITS: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
];
const UNIT_ORDINALS: [&str; 20] = [
    "zeroth", "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth",
    "eleventh", "twelfth", "thirteenth", "fourteenth", "fifteenth", "sixteenth", "seventeenth", "eighteenth",
    "nineteenth",
];
const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
const TEN_ORDINALS: [&str; 10] =
    ["", "", "twentieth", "thirtieth", "fortieth", "fiftieth", "sixtieth", "seventieth", "eightieth", "ninetieth"];
const SCALES: [(&str, &str, u64); 4] = [
    ("hundred", "hundredth", 100),
    ("thousand", "thousandth", 1_000),
    ("million", "millionth", 1_000_000),
    ("billion", "billionth", 1_000_000_000),
];
pub(crate) const MONTHS: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november",
    "december",
];

/// A number read from the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Int { n: u64, ordinal: bool },
    /// Digits before and after the point.
    Decimal(u64, String),
    /// A time of day; `meridiem` is "a.m." or "p.m." when said.
    Time { hour: u64, minute: u64, meridiem: Option<&'static str>, oclock: bool },
}

/// A number and how many words it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spoken {
    pub value: Value,
    pub len: usize,
}

/// One word's value: a unit or tens (with its ordinal flag), a scale, or a
/// number already in digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    Small(u64, bool),
    Scale(u64, bool),
    Digits(u64, bool),
}

fn word(w: &str) -> Option<Word> {
    if let Some(i) = UNITS.iter().position(|u| *u == w) {
        return Some(Word::Small(i as u64, false));
    }
    if let Some(i) = UNIT_ORDINALS.iter().position(|u| *u == w) {
        return Some(Word::Small(i as u64, true));
    }
    if let Some(i) = TENS.iter().position(|u| !u.is_empty() && *u == w) {
        return Some(Word::Small(i as u64 * 10, false));
    }
    if let Some(i) = TEN_ORDINALS.iter().position(|u| !u.is_empty() && *u == w) {
        return Some(Word::Small(i as u64 * 10, true));
    }
    if let Some((_, _, n)) = SCALES.iter().find(|(s, _, _)| *s == w) {
        return Some(Word::Scale(*n, false));
    }
    if let Some((_, _, n)) = SCALES.iter().find(|(_, o, _)| *o == w) {
        return Some(Word::Scale(*n, true));
    }
    // Digits as the recognizer wrote them: "25", "21st".
    let digits: String = w.chars().take_while(char::is_ascii_digit).collect();
    let rest = &w[digits.len()..];
    if !digits.is_empty() && matches!(rest, "" | "st" | "nd" | "rd" | "th") {
        return digits.parse().ok().map(|n| Word::Digits(n, !rest.is_empty()));
    }
    None
}

/// The words of `w`, split at hyphens ("twenty-five").
fn parts(w: &str) -> Vec<&str> {
    w.split('-').filter(|p| !p.is_empty()).collect()
}

/// Reads a whole number from the start of `words` (lowercase, no marks;
/// `stops[i]` when punctuation follows word `i`, which ends the number
/// there). Returns its value, whether it was an ordinal, and the words used.
fn integer(words: &[&str], stops: &[bool]) -> Option<(u64, bool, usize)> {
    let (mut total, mut chunk) = (0u64, 0u64);
    let (mut used, mut ordinal, mut any) = (0usize, false, false);
    // What the last number word was, so "twenty five" joins but "five five" doesn't.
    let mut last: Option<u64> = None;
    'words: for (i, w) in words.iter().enumerate() {
        if ordinal {
            break;
        }
        // "two hundred and five": "and" only between a scale and more.
        if *w == "and" {
            let after_scale = matches!(last, Some(n) if n >= 100);
            let more = words.get(i + 1).and_then(|n| parts(n).first().and_then(|p| word(p)));
            if after_scale && !stops[i] && matches!(more, Some(Word::Small(..))) {
                used = i + 1;
                continue;
            }
            break;
        }
        let ps = parts(w);
        if ps.is_empty() {
            break;
        }
        for p in ps {
            match word(p) {
                Some(Word::Small(n, ord)) => {
                    // Units after tens join ("twenty five"); anything else
                    // after a small number is a second number.
                    let joins = match last {
                        None => true,
                        Some(l) if l >= 100 => true,
                        Some(l) => l >= 20 && l % 10 == 0 && n < 10 && n > 0,
                    };
                    if !joins {
                        break 'words;
                    }
                    chunk += n;
                    last = Some(n);
                    ordinal = ord;
                }
                Some(Word::Scale(s, ord)) => {
                    if !any {
                        break 'words;
                    }
                    if s == 100 {
                        chunk = chunk.max(1) * 100;
                    } else {
                        total += chunk.max(1) * s;
                        chunk = 0;
                    }
                    last = Some(s);
                    ordinal = ord;
                }
                Some(Word::Digits(n, ord)) if !any => {
                    return Some((n, ord, i + 1));
                }
                _ => break 'words,
            }
            any = true;
        }
        used = i + 1;
        if stops[i] {
            break;
        }
    }
    any.then_some((total + chunk, ordinal, used))
}

/// Reads a number from the start of `words`: a time ("four thirty", "ten
/// o'clock", "four thirty p.m."), a decimal ("three point five"), or a
/// whole number, cardinal or ordinal.
pub(crate) fn read(words: &[&str], stops: &[bool]) -> Option<Spoken> {
    let (n, ordinal, used) = integer(words, stops)?;
    let rest = &words[used..];
    let stopped = used > 0 && stops[used - 1];
    if !ordinal && !stopped {
        // A time: an hour, then minutes or "o'clock", then perhaps a.m./p.m.
        if (1..=12).contains(&n) && used == 1 {
            if let Some((minute, mlen, oclock)) = minutes(rest, &stops[used..]) {
                let after = used + mlen;
                let (meridiem, mer_len) = meridiem(&words[after..]);
                return Some(Spoken { value: Value::Time { hour: n, minute, meridiem, oclock }, len: after + mer_len });
            }
            // "seven a.m.": the hour alone.
            if let (Some(m), mer_len) = meridiem(rest) {
                return Some(Spoken {
                    value: Value::Time { hour: n, minute: 0, meridiem: Some(m), oclock: false },
                    len: used + mer_len,
                });
            }
        }
        // A decimal: "point" and digit words.
        if rest.first() == Some(&"point") {
            let digits: String = rest[1..]
                .iter()
                .map_while(|w| match word(w) {
                    Some(Word::Small(d, false)) if d < 10 => char::from_digit(d as u32, 10),
                    _ => None,
                })
                .collect();
            if !digits.is_empty() {
                return Some(Spoken { value: Value::Decimal(n, digits.clone()), len: used + 1 + digits.len() });
            }
        }
    }
    Some(Spoken { value: Value::Int { n, ordinal }, len: used })
}

/// Minutes after an hour: "thirty", "oh five", "fifteen", "o'clock".
fn minutes(words: &[&str], stops: &[bool]) -> Option<(u64, usize, bool)> {
    match *words.first()? {
        "o'clock" | "oclock" => Some((0, 1, true)),
        "oh" | "o" => {
            let (m, ord, used) = integer(&words[1..], &stops[1..])?;
            (!ord && m < 10).then_some((m, used + 1, false))
        }
        _ => {
            let (m, ord, used) = integer(words, stops)?;
            (!ord && (10..60).contains(&m)).then_some((m, used, false))
        }
    }
}

/// "a.m." / "p.m." as the recognizer writes them ("am", "a.m.", "a m").
fn meridiem(words: &[&str]) -> (Option<&'static str>, usize) {
    let w = |i: usize| words.get(i).map(|w| w.replace('.', ""));
    match (w(0).as_deref(), w(1).as_deref()) {
        (Some("am"), _) => (Some("a.m."), 1),
        (Some("pm"), _) => (Some("p.m."), 1),
        (Some("a"), Some("m")) => (Some("a.m."), 2),
        (Some("p"), Some("m")) => (Some("p.m."), 2),
        _ => (None, 0),
    }
}

fn suffix(n: u64) -> &'static str {
    match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// Digits, with commas from 10,000 (and for 1,000 to 9,999 too, except
/// where it may be a year: 1000 to 2100).
fn grouped(n: u64) -> String {
    let s = n.to_string();
    if n < 1000 || (1000..=2100).contains(&n) {
        return s;
    }
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The number in digits: "25", "21st", "4:30", "4:30 p.m.", "3.5".
pub(crate) fn digits(value: &Value) -> String {
    match value {
        Value::Int { n, ordinal: false } => grouped(*n),
        Value::Int { n, ordinal: true } => format!("{n}{}", suffix(*n)),
        Value::Decimal(n, d) => format!("{n}.{d}"),
        Value::Time { hour, minute: 0, meridiem: Some(m), oclock: false } => format!("{hour} {m}"),
        Value::Time { hour, minute, meridiem, .. } => match meridiem {
            Some(m) => format!("{hour}:{minute:02} {m}"),
            None => format!("{hour}:{minute:02}"),
        },
    }
}

/// A number up to 100 in words, hyphenated: "twenty-five", "twenty-first".
fn small_words(n: u64, ordinal: bool) -> String {
    let (units, tens) = if ordinal { (&UNIT_ORDINALS, &TEN_ORDINALS) } else { (&UNITS, &TENS) };
    match n {
        0..=19 => units[n as usize].to_string(),
        100 if ordinal => "one hundredth".into(),
        100 => "one hundred".into(),
        _ if n.is_multiple_of(10) => tens[(n / 10) as usize].to_string(),
        _ => format!("{}-{}", TENS[(n / 10) as usize], units[(n % 10) as usize]),
    }
}

/// What the style guide writes for a number, given the word after it and
/// the one before (lowercase): `None` to leave the words as said, or the
/// text and whether it took the word after (the unit it absorbed).
pub(crate) fn styled(value: &Value, before: Option<&str>, after: Option<&str>) -> Option<(String, bool)> {
    let after_month = before.is_some_and(|b| MONTHS.contains(&b));
    match value {
        Value::Time { oclock: true, meridiem: None, .. } => None,
        Value::Time { .. } => Some((digits(value), false)),
        Value::Decimal(..) => Some((digits(value), false)),
        Value::Int { n, ordinal } => {
            match after {
                Some("dollars" | "dollar") if !ordinal => return Some((format!("${}", grouped(*n)), true)),
                Some("percent") if !ordinal => return Some((format!("{} percent", grouped(*n)), true)),
                _ => {}
            }
            if after_month && (1..=31).contains(n) {
                return Some((n.to_string(), false));
            }
            let round = *n >= 100 && n % 100 == 0 && *n < 1_000_000 && (n % 1000 == 0 || *n < 1000);
            if *n <= 100 || round {
                return (*n <= 100).then(|| (small_words(*n, *ordinal), false));
            }
            Some((digits(value), false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(s: &str) -> Option<Spoken> {
        let words: Vec<String> = s.split_whitespace().map(str::to_string).collect();
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        read(&w, &vec![false; w.len()])
    }

    fn d(s: &str) -> String {
        digits(&read_all(s).unwrap().value)
    }

    #[test]
    fn reads_whole_numbers() {
        assert_eq!(d("twenty five"), "25");
        assert_eq!(d("twenty-five"), "25");
        assert_eq!(d("three hundred"), "300");
        assert_eq!(d("two hundred and thirty four"), "234");
        assert_eq!(d("one thousand two hundred"), "1200");
        assert_eq!(d("twelve thousand five hundred"), "12,500");
        assert_eq!(d("two million"), "2,000,000");
        assert_eq!(d("25"), "25");
    }

    #[test]
    fn reads_ordinals() {
        assert_eq!(d("first"), "1st");
        assert_eq!(d("twenty first"), "21st");
        assert_eq!(d("twenty-second"), "22nd");
        assert_eq!(d("eleventh"), "11th");
        assert_eq!(d("one hundredth"), "100th");
        assert_eq!(d("21st"), "21st");
    }

    #[test]
    fn reads_times_and_decimals() {
        assert_eq!(d("four thirty"), "4:30");
        assert_eq!(d("ten oh five"), "10:05");
        assert_eq!(d("four o'clock"), "4:00");
        assert_eq!(d("four thirty p.m."), "4:30 p.m.");
        assert_eq!(d("seven a m"), "7 a.m.");
        assert_eq!(d("seven am"), "7 a.m.");
        assert_eq!(d("three point five"), "3.5");
    }

    #[test]
    fn stops_where_the_number_does() {
        assert_eq!(read_all("twenty five people").unwrap().len, 2);
        assert_eq!(read_all("one two three").unwrap().len, 1, "a count, not one number");
        assert_eq!(read_all("five and six").unwrap().len, 1);
        assert!(read_all("people").is_none());
    }

    #[test]
    fn the_style_guide_keeps_small_numbers_as_words() {
        let v = |s: &str| read_all(s).unwrap().value;
        assert_eq!(styled(&v("twenty five"), None, Some("people")), Some(("twenty-five".into(), false)));
        assert_eq!(styled(&v("twenty first"), None, None), Some(("twenty-first".into(), false)));
        assert_eq!(styled(&v("three hundred"), None, None), None, "round numbers stay as said");
        assert_eq!(styled(&v("two hundred and thirty four"), None, None), Some(("234".into(), false)));
        assert_eq!(styled(&v("three hundred"), None, Some("dollars")), Some(("$300".into(), true)));
        assert_eq!(styled(&v("twelve"), None, Some("percent")), Some(("12 percent".into(), true)));
        assert_eq!(styled(&v("twenty first"), Some("may"), None), Some(("21".into(), false)));
        assert_eq!(styled(&v("four o'clock"), None, None), None);
        assert_eq!(styled(&v("four thirty p.m."), None, None), Some(("4:30 p.m.".into(), false)));
    }
}
