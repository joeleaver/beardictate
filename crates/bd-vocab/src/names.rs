//! Words known as names: people, places, months, days, peoples (about
//! 9,000, from SCOWL; `data/make_names.py`). For keeping a capital the
//! dictation would otherwise lower: "…, Sarah" stays, "…, Then" doesn't.

use std::sync::OnceLock;

const LIST: &str = include_str!("../data/names.txt");

fn names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| LIST.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect())
}

/// Whether `word`, as written ("Sarah", not "sarah"), is a known name. A
/// possessive counts ("Sarah's").
pub fn is_name(word: &str) -> bool {
    let bare = word.strip_suffix("'s").or_else(|| word.strip_suffix("\u{2019}s")).unwrap_or(word);
    names().binary_search(&bare).is_ok()
}

#[cfg(test)]
mod tests {
    use super::is_name;

    #[test]
    fn names_but_not_words() {
        for name in ["Sarah", "Monday", "January", "Glasgow", "English", "Mr", "Will", "May", "Sarah's", "Christmas"] {
            assert!(is_name(name), "{name}");
        }
        for word in ["Then", "Begin", "Rather", "Er", "Co", "sarah", "How", "Hello", "Okay", "Yeah", "A", "Reading"] {
            assert!(!is_name(word), "{word}");
        }
    }
}
