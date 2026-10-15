//! Special words: names and invented words, in groups (a book, a series)
//! that can be switched off together, each word switchable on its own.

use serde::{Deserialize, Serialize};

fn on() -> bool {
    true
}

/// A named set of special words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordGroup {
    pub name: String,
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default)]
    pub words: Vec<SpecialWord>,
}

impl WordGroup {
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), enabled: true, words: Vec::new() }
    }
}

/// One special word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialWord {
    /// The spelling to produce.
    pub word: String,
    /// What the recognizer writes instead, any casing.
    #[serde(default)]
    pub heard: Vec<String>,
    #[serde(default = "on")]
    pub enabled: bool,
}

impl SpecialWord {
    pub fn new(word: &str, heard: &[&str]) -> Self {
        Self { word: word.into(), heard: heard.iter().map(|h| h.to_string()).collect(), enabled: true }
    }
}

/// The name of the group words go in when there's nothing else.
pub const FIRST_GROUP: &str = "My words";

/// The list a new user starts with: a real entry, which also shows how.
pub fn starter_groups() -> Vec<WordGroup> {
    vec![WordGroup {
        words: vec![SpecialWord::new("beardictate", &["bear dictate", "bare dictate"])],
        ..WordGroup::new(FIRST_GROUP)
    }]
}

/// The words in effect: switched on, in a group that is switched on, and
/// not blank.
pub fn active_words(groups: &[WordGroup]) -> Vec<SpecialWord> {
    groups
        .iter()
        .filter(|g| g.enabled)
        .flat_map(|g| &g.words)
        .filter(|w| w.enabled && !w.word.trim().is_empty())
        .cloned()
        .collect()
}

/// The pipeline's vocabulary for `words`.
pub fn vocabulary(words: &[SpecialWord]) -> bd_vocab::Vocabulary {
    bd_vocab::Vocabulary::new(
        words
            .iter()
            .map(|w| bd_vocab::Word {
                text: w.word.trim().to_string(),
                heard: w.heard.iter().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_switched_on_words_in_switched_on_groups_count() {
        let mut groups = starter_groups();
        groups.push(WordGroup {
            words: vec![SpecialWord::new("Ysolde", &["Ussel"]), SpecialWord { enabled: false, ..SpecialWord::new("Thrane", &[]) }],
            ..WordGroup::new("Book two")
        });
        let names = |g: &[WordGroup]| active_words(g).into_iter().map(|w| w.word).collect::<Vec<_>>();
        assert_eq!(names(&groups), ["beardictate", "Ysolde"]);
        groups[1].enabled = false;
        assert_eq!(names(&groups), ["beardictate"]);
        let v = vocabulary(&active_words(&groups));
        assert_eq!(v.apply("I use bare dictate daily."), "I use beardictate daily.");
        assert_eq!(v.hotwords(), ["beardictate"]);
    }
}
