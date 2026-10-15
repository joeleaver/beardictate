//! The word-level guardrail: did the model only make allowed edits?
//!
//! Allowed:
//! - removing fillers (um, uh, er, ah, hmm, …; "you know"; "like" set off by
//!   commas or next to another filler), noise tags (`[cough]`), cut-off
//!   fragments (`th-`), the first copy of an immediate repeat of up to four
//!   words (fillers in between are ignored), false starts of two to four
//!   words that the speaker restarted with the same first word(s), marked by
//!   a dash or ellipsis, or by a comma when the attempt is two words, a
//!   filler follows it or the restart repeats two or more words; the echoed
//!   opening of a restart ("I just, I needed" → "I just needed"); and, with
//!   [`review_in_context`], an opening that re-says the end of the context;
//! - any change of case, punctuation, quote marks or whitespace, including
//!   new paragraph breaks, except turning a word of more than three letters
//!   into ALL CAPS (a model "shouting"; `OK`, `TV` are fine).
//!
//! Everything else is rejected: an inserted word, a substituted word (also
//! `don't` → `do not`, `3` → `three`), a reordered word, or any other
//! dropped word. Marks the command pass inserted must also survive:
//! every quote mark and line or paragraph break in the input must still be
//! in the output, between the same two kept words.
//!
//! A rejected output may still be usable: [`repair`] puts the dictated
//! words back where the model swapped them one for one, keeping its
//! formatting, and the result is reviewed again.
//!
//! Words are compared after folding case and apostrophes (`’` = `'`), with
//! hyphens and dashes separating words, so `well-known` = `well known`.

mod align;
mod repair;
mod words;

#[cfg(test)]
mod tests;

use std::fmt;

pub use align::RemovalReason;
pub use repair::{Repair, repair};
pub use words::{Gap, Tokens, Word, normalize, tokenize};

use align::Op;

/// What the guardrail decided about one model output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardrailVerdict {
    /// The output only made allowed edits. `removed` lists the input words
    /// the model dropped, and why that was allowed.
    Accepted { removed: Vec<Removal> },
    Rejected(Rejection),
}

impl GuardrailVerdict {
    pub fn is_accepted(&self) -> bool {
        matches!(self, GuardrailVerdict::Accepted { .. })
    }
}

/// Input words the model was allowed to drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// The dropped words as they appeared in the input, joined by spaces.
    pub words: String,
    pub reason: RemovalReason,
}

/// Why an output was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The model changed the words. Each list is in text order.
    WordsChanged {
        /// Output words with no counterpart in the input.
        inserted: Vec<String>,
        /// Input words missing from the output that were not allowed to go.
        dropped: Vec<String>,
        /// (input word, output word) pairs where one replaced the other.
        substituted: Vec<(String, String)>,
        /// Words present in both but moved.
        reordered: Vec<String>,
    },
    /// The model removed a commanded line or paragraph break. `before` is
    /// the kept word that followed it (`None` at the end).
    BreakLost { before: Option<String> },
    /// The model removed a commanded quote mark.
    QuoteLost { before: Option<String> },
    /// The model put a word into capitals that the speaker didn't
    /// (`coming` → `COMING`).
    Shouted { word: String },
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let near = |w: &Option<String>| w.as_ref().map_or("the end".to_string(), |w| format!("\"{w}\""));
        match self {
            Rejection::WordsChanged { inserted, dropped, substituted, reordered } => {
                let mut parts = Vec::new();
                if !substituted.is_empty() {
                    let s: Vec<String> = substituted.iter().map(|(a, b)| format!("{a} → {b}")).collect();
                    parts.push(format!("changed {}", s.join(", ")));
                }
                if !inserted.is_empty() {
                    parts.push(format!("added {}", inserted.join(", ")));
                }
                if !dropped.is_empty() {
                    parts.push(format!("dropped {}", dropped.join(", ")));
                }
                if !reordered.is_empty() {
                    parts.push(format!("moved {}", reordered.join(", ")));
                }
                write!(f, "cleanup reworded the text ({})", parts.join("; "))
            }
            Rejection::BreakLost { before } => write!(f, "cleanup removed a dictated break before {}", near(before)),
            Rejection::QuoteLost { before } => write!(f, "cleanup removed a dictated quote mark before {}", near(before)),
            Rejection::Shouted { word } => write!(f, "cleanup put {word:?} in capitals"),
        }
    }
}

/// Checks a model output against its input.
pub fn review(input: &str, output: &str) -> GuardrailVerdict {
    review_in_context("", input, output)
}

/// [`review`], also allowing the segment to drop an opening that re-says
/// the last words of `context` (up to four words), as a speaker does after
/// a pause.
pub fn review_in_context(context: &str, input: &str, output: &str) -> GuardrailVerdict {
    let a = tokenize(input);
    let b = tokenize(output);
    let tail: Vec<String> = tokenize(context).words.into_iter().map(|w| w.norm).collect();
    let tail = &tail[tail.len().saturating_sub(4)..];
    let Some(alignment) = align::align(&a, &b, tail) else {
        return GuardrailVerdict::Rejected(explain(&a, &b));
    };
    if let Some(rejection) = check_marks(&a, &b, &alignment.kept) {
        return GuardrailVerdict::Rejected(rejection);
    }
    if let Some(rejection) = check_shouting(&a, &b, &alignment.kept) {
        return GuardrailVerdict::Rejected(rejection);
    }
    let removed = alignment
        .removed
        .into_iter()
        .map(|(range, reason)| Removal {
            words: a.words[range].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
            reason,
        })
        .collect();
    GuardrailVerdict::Accepted { removed }
}

/// Every quote mark and break in the input must reappear in the output gap
/// that sits between the same kept words. A gap in the output stands for
/// all input gaps between its two kept words, including those around words
/// the model removed.
fn check_marks(a: &Tokens, b: &Tokens, kept: &[usize]) -> Option<Rejection> {
    for g in 0..b.gaps.len() {
        let from = if g == 0 { 0 } else { kept[g - 1] + 1 };
        let to = if g == kept.len() { a.words.len() } else { kept[g] };
        let region = &a.gaps[from..=to];
        let out = &b.gaps[g];
        let before = b.words.get(g).map(|w| w.text.clone());
        let quotes: usize = region.iter().map(Gap::quotes).sum();
        // Brackets of removed tags are words, not gaps, so they never count.
        if out.quotes() < quotes {
            return Some(Rejection::QuoteLost { before });
        }
        let level = region.iter().map(Gap::break_level).max().unwrap_or(0);
        if out.break_level() < level {
            return Some(Rejection::BreakLost { before });
        }
    }
    None
}

/// A kept word may change case freely, except into ALL CAPS when it is
/// longer than three letters and the speaker's word wasn't already.
fn check_shouting(a: &Tokens, b: &Tokens, kept: &[usize]) -> Option<Rejection> {
    let all_caps = |w: &str| w.chars().any(char::is_alphabetic) && !w.chars().any(char::is_lowercase);
    b.words.iter().zip(kept).find_map(|(out, &i)| {
        let letters = out.text.chars().filter(|c| c.is_alphabetic()).count();
        (!out.tag && letters > 3 && all_caps(&out.text) && !all_caps(&a.words[i].text))
            .then(|| Rejection::Shouted { word: out.text.clone() })
    })
}

fn explain(a: &Tokens, b: &Tokens) -> Rejection {
    let ops = align::diff(a, b);
    let mut inserted: Vec<usize> = Vec::new();
    let mut dropped: Vec<usize> = Vec::new();
    let mut substituted = Vec::new();
    // Pair drops and inserts within each run between unchanged words.
    let mut run_drops: Vec<usize> = Vec::new();
    let mut run_inserts: Vec<usize> = Vec::new();
    let mut flush = |drops: &mut Vec<usize>, inserts: &mut Vec<usize>| {
        drops.retain(|&i| align::always_removable(a, i).is_none());
        let pairs = drops.len().min(inserts.len());
        for k in 0..pairs {
            substituted.push((drops[k], inserts[k]));
        }
        dropped.extend(drops.drain(..).skip(pairs));
        inserted.extend(inserts.drain(..).skip(pairs));
    };
    for op in ops {
        match op {
            Op::Same => flush(&mut run_drops, &mut run_inserts),
            Op::Drop(i) => run_drops.push(i),
            Op::Insert(j) => run_inserts.push(j),
        }
    }
    flush(&mut run_drops, &mut run_inserts);
    // A word dropped in one place and inserted in another was moved.
    let mut reordered = Vec::new();
    inserted.retain(|&j| {
        let pos = dropped.iter().position(|&i| a.words[i].norm == b.words[j].norm);
        if let Some(p) = pos {
            reordered.push(b.words[j].text.clone());
            dropped.remove(p);
            false
        } else {
            true
        }
    });
    let text_a = |i: &usize| a.words[*i].text.clone();
    let text_b = |j: &usize| b.words[*j].text.clone();
    let mut rejection = Rejection::WordsChanged {
        inserted: inserted.iter().map(text_b).collect(),
        dropped: dropped.iter().map(text_a).collect(),
        substituted: substituted.iter().map(|(i, j)| (text_a(i), text_b(j))).collect(),
        reordered,
    };
    // The diff found nothing beyond removable words, so the removals broke
    // a rule that needs context ("you" without "know", a "like" that is not
    // filler). Report the dropped words themselves.
    if let Rejection::WordsChanged { inserted, dropped, substituted, reordered } = &mut rejection
        && inserted.is_empty()
        && dropped.is_empty()
        && substituted.is_empty()
        && reordered.is_empty()
    {
        dropped.extend(
            align::diff(a, b)
                .into_iter()
                .filter_map(|op| if let Op::Drop(i) = op { Some(a.words[i].text.clone()) } else { None }),
        );
    }
    rejection
}
