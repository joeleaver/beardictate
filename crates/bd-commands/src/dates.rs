//! Spoken dates: years ("nineteen eighty four", "twenty twenty six") and
//! the dates the style guide writes in digits ("May 21, 2026", "the 21st of
//! May", "October 2025").
//!
//! A year is read only where it can't be a count: right after a month
//! ("October twenty twenty five"), after a month and day ("May twenty
//! first, twenty twenty six") and after "numeral". Elsewhere a pair like
//! "nineteen twenty three" may be a chapter and verse ("John nineteen
//! twenty three"), so it is left as said.

use crate::numbers::{MONTHS, integer};
use crate::token::Token;

/// Whether the token is a month as the recognizer writes one: capitalized,
/// so the verbs "may" and "march" aren't.
pub(crate) fn is_month(tok: &Token) -> bool {
    tok.core.starts_with(|c: char| c.is_uppercase()) && MONTHS.contains(&tok.lower.as_str())
}

/// A year said in two parts at the start of `words` ("nineteen eighty four",
/// "twenty twenty six", "nineteen oh five"), with the words it took. The
/// first part is thirteen to twenty, so it is never a clock time ("ten
/// twenty") nor a day before a year.
pub(crate) fn year_pair(words: &[&str], stops: &[bool]) -> Option<(u64, usize)> {
    let (century, ord, used) = integer(words, stops)?;
    if ord || used != 1 || stops[0] || !(13..=20).contains(&century) || words[0].chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = &words[1..];
    let (decade, len) = match rest.first()? {
        &("oh" | "o") if !stops[1] => {
            let (n, ord, used) = integer(&rest[1..], &stops[2..])?;
            (!ord && (1..10).contains(&n) && used == 1).then_some((n, used + 1))?
        }
        _ => {
            let (n, ord, used) = integer(rest, &stops[1..])?;
            (!ord && (10..100).contains(&n) && !rest[0].chars().any(|c| c.is_ascii_digit())).then_some((n, used))?
        }
    };
    Some((century * 100 + decade, 1 + len))
}

/// A year at the start of `words`: in two parts ([`year_pair`]), or whole
/// ("two thousand and five", "nineteen hundred").
pub(crate) fn year(words: &[&str], stops: &[bool]) -> Option<(u64, usize)> {
    if let Some(y) = year_pair(words, stops) {
        return Some(y);
    }
    let (n, ord, used) = integer(words, stops)?;
    let spoken = words[..used].iter().any(|w| matches!(*w, "hundred" | "thousand"));
    (!ord && spoken && (1000..=2099).contains(&n)).then_some((n, used))
}

/// What the style guide writes for the words after a month: a year
/// ("October 2025"), or a day ("May 21") and perhaps a year after it, with
/// or without the recognizer's comma ("May 21, 2026"). `commas[k]` is true
/// when word `k` ends in a lone comma (a stop a year may follow). The text
/// and the words it took.
pub(crate) fn after_month(words: &[&str], stops: &[bool], commas: &[bool]) -> Option<(String, usize)> {
    if let Some((y, used)) = year(words, stops) {
        return Some((y.to_string(), used));
    }
    let (day, _, used) = integer(words, stops)?;
    if !(1..=31).contains(&day) {
        return None;
    }
    if (!stops[used - 1] || commas[used - 1])
        && let Some((y, len)) = year(&words[used..], &stops[used..])
    {
        return Some((format!("{day}, {y}"), used + len));
    }
    Some((day.to_string(), used))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> (Vec<&str>, Vec<bool>, Vec<bool>) {
        let raw: Vec<&str> = s.split_whitespace().collect();
        let words = raw.iter().map(|w| w.trim_end_matches(',')).collect();
        let commas: Vec<bool> = raw.iter().map(|w| w.ends_with(',')).collect();
        (words, commas.clone(), commas)
    }

    fn y(s: &str) -> Option<(u64, usize)> {
        let (w, stops, _) = split(s);
        year(&w, &stops)
    }

    fn m(s: &str) -> Option<(String, usize)> {
        let (w, stops, commas) = split(s);
        after_month(&w, &stops, &commas)
    }

    #[test]
    fn reads_years() {
        assert_eq!(y("nineteen eighty four"), Some((1984, 3)));
        assert_eq!(y("twenty twenty six"), Some((2026, 3)));
        assert_eq!(y("twenty twenty"), Some((2020, 2)));
        assert_eq!(y("nineteen oh five"), Some((1905, 3)));
        assert_eq!(y("nineteen hundred"), Some((1900, 2)));
        assert_eq!(y("two thousand and five"), Some((2005, 4)));
        assert_eq!(y("two thousand twenty six"), Some((2026, 4)));
    }

    #[test]
    fn what_is_not_a_year() {
        assert_eq!(y("twenty five"), None);
        assert_eq!(y("ten twenty"), None, "a clock time");
        assert_eq!(y("twenty first"), None);
        assert_eq!(y("nineteen fifties"), None);
        assert_eq!(y("nineteen, twenty"), None, "a mark between");
        assert_eq!(y("two thousand people"), Some((2000, 2)), "only read where a year can be");
        assert_eq!(y("five hundred"), None);
    }

    #[test]
    fn after_a_month() {
        assert_eq!(m("twenty first"), Some(("21".into(), 2)));
        assert_eq!(m("twenty sixth, twenty twenty six"), Some(("26, 2026".into(), 5)));
        assert_eq!(m("twenty first twenty twenty six"), Some(("21, 2026".into(), 5)));
        assert_eq!(m("twenty twenty five"), Some(("2025".into(), 3)));
        assert_eq!(m("seven"), Some(("7".into(), 1)));
        assert_eq!(m("forty"), None);
    }
}
