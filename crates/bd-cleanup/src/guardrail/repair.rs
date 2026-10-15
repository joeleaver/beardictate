//! Repairing a rejected output: the dictated words, the model's formatting.
//!
//! A small model often gets the hard part right (quoting dialog, fixing
//! sentence boundaries) and then miscopies a word ("is coming" → "is
//! came"). Rejecting the whole answer throws the good edits away with the
//! bad one. Where the model swapped words one for one, the dictated words
//! go back into the model's text, keeping its quote marks, punctuation,
//! breaks and capitals. Anything less clear-cut (added or dropped words,
//! reordering, a lost mark) is not repaired.
//!
//! The repaired text must then pass [`review_in_context`] like any other
//! output, so a repair can never let through more than the guardrail
//! allows.

use super::align::{self, Op};
use super::words::{Tokens, tokenize};
use super::review_in_context;

/// A rejected output with the model's word swaps undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    /// The output to use: the model's text with the dictated words restored.
    pub text: String,
    /// (the model's word, the dictated word put back), in text order.
    pub restored: Vec<(String, String)>,
}

/// Repairs `output` when its only problem is words swapped one for one.
/// `None` when there is nothing to repair or the repair would not pass the
/// guardrail.
pub fn repair(context: &str, input: &str, output: &str) -> Option<Repair> {
    let a = tokenize(input);
    let b = tokenize(output);
    let mut fix = Fix { a: &a, b: &b, words: b.words.iter().map(|w| w.text.clone()).collect(), restored: Vec::new() };
    let (mut drops, mut inserts) = (Vec::new(), Vec::new());
    for op in align::diff(&a, &b) {
        match op {
            Op::Same => fix.run(&mut drops, &mut inserts)?,
            Op::Drop(i) => drops.push(i),
            Op::Insert(j) => inserts.push(j),
        }
    }
    fix.run(&mut drops, &mut inserts)?;
    if fix.restored.is_empty() {
        return None;
    }
    let mut text = String::with_capacity(output.len());
    for (gap, word) in b.gaps.iter().zip(&fix.words) {
        text.push_str(&gap.text);
        text.push_str(word);
    }
    text.push_str(&b.gaps[fix.words.len()].text);
    review_in_context(context, input, &text).is_accepted().then_some(Repair { text, restored: fix.restored })
}

struct Fix<'t> {
    a: &'t Tokens,
    b: &'t Tokens,
    /// The output's words, with restored ones replaced.
    words: Vec<String>,
    restored: Vec<(String, String)>,
}

impl Fix<'_> {
    /// Handles one run of drops and inserts between unchanged words.
    /// Returns `None` when the run is not a clean one-for-one swap.
    fn run(&mut self, drops: &mut Vec<usize>, inserts: &mut Vec<usize>) -> Option<()> {
        if inserts.is_empty() {
            // Only removals: the final review decides whether they were
            // allowed.
            drops.clear();
            return Some(());
        }
        // Fillers and tags may go anyway; what is left must pair up.
        drops.retain(|&i| align::always_removable(self.a, i).is_none());
        if drops.len() != inserts.len() {
            return None;
        }
        for (&i, &j) in drops.iter().zip(inserts.iter()) {
            let (dictated, model) = (&self.a.words[i], &self.b.words[j]);
            if dictated.tag || model.tag {
                return None;
            }
            let word = recase(&dictated.text, &model.text);
            self.restored.push((model.text.clone(), word.clone()));
            self.words[j] = word;
        }
        drops.clear();
        inserts.clear();
        Some(())
    }
}

/// The dictated word, capitalized if the model capitalized its word there
/// (a sentence start). A dictated capital is kept: it may be a name.
fn recase(dictated: &str, model: &str) -> String {
    let model_upper = model.chars().next().is_some_and(char::is_uppercase);
    let mut chars = dictated.chars();
    match chars.next() {
        Some(first) if model_upper && first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => dictated.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(input: &str, output: &str) -> Option<String> {
        repair("", input, output).map(|r| r.text)
    }

    #[test]
    fn a_miscopied_word_is_put_back_and_the_quotes_kept() {
        let input = "one day the tax collector came. The small boy said, Oh yes, the tax collector is coming to visit.";
        let output = "One day the tax collector came. The small boy said, “Oh yes, the tax collector is came to visit.”";
        let r = repair("", input, output).unwrap();
        assert_eq!(
            r.text,
            "One day the tax collector came. The small boy said, “Oh yes, the tax collector is coming to visit.”"
        );
        assert_eq!(r.restored, [("came".to_string(), "coming".to_string())]);
    }

    #[test]
    fn casing_follows_the_model_except_dictated_capitals() {
        assert_eq!(fixed("going home, he said", "Went home, he said."), Some("Going home, he said.".into()));
        assert_eq!(fixed("ask Mara now", "Ask mary now."), Some("Ask Mara now.".into()));
    }

    #[test]
    fn allowed_removals_still_count() {
        assert_eq!(
            fixed("um, she was going home", "She was gone home."),
            Some("She was going home.".into())
        );
    }

    #[test]
    fn unclear_edits_are_not_repaired() {
        // Added word, dropped word, reordering, nothing to fix.
        assert_eq!(fixed("she ran home", "She ran all the way home."), None);
        assert_eq!(fixed("she ran home fast", "She ran home."), None);
        assert_eq!(fixed("home she ran", "She ran home."), None);
        assert_eq!(fixed("she ran home", "She ran home."), None);
        // Two words swapped for one.
        assert_eq!(fixed("she did not go", "She didn't go."), None);
    }

    #[test]
    fn shouting_is_not_repaired() {
        assert_eq!(fixed("Ain't nobody coming, Jessup said.", "“AIN'T NOWNEVER COMING,” JESSUP SAID."), None);
    }

    #[test]
    fn a_repair_must_still_keep_commanded_marks() {
        // The model dropped a commanded quote as well as miscopying a word.
        assert_eq!(fixed("He said, “I am coming.”", "He said, I am came."), None);
    }
}
