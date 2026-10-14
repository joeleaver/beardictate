//! Aligning the model's words against the input's.
//!
//! The question is not "how similar are these?" but "can the output be
//! reached from the input using only allowed deletions?". That is a
//! subsequence check with a grammar of allowed removals, solved by dynamic
//! programming over (input position, output position).

use std::ops::Range;

use super::words::Tokens;

/// Why the guardrail allowed some input words to disappear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// um, uh, er, ah, hmm, "you know", or "like" used as filler.
    Filler,
    /// A bracketed noise tag such as `[cough]`.
    NoiseTag,
    /// A cut-off word fragment such as `th-`.
    Fragment,
    /// The first copy of an immediately repeated word or phrase.
    Repeat,
    /// An abandoned start the speaker restarted ("I went, I walked"), or the
    /// repeated opening of a restart ("I just, I needed").
    FalseStart,
}

/// Single words that may always be removed.
const FILLERS: &[&str] = &[
    "um", "umm", "uh", "uhh", "uhm", "er", "erm", "ah", "ahh", "hmm", "hm", "mm", "mmm", "eh",
];

/// The longest repeated phrase recognized, in words.
const MAX_REPEAT: usize = 4;
/// The longest false start recognized, in words, when a dash or ellipsis
/// marks the cut-off.
const MAX_FALSE_START: usize = 4;
/// The longest false start recognized when only a comma marks it.
const MAX_COMMA_FALSE_START: usize = 2;

/// A successful alignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alignment {
    /// For each output word, the input word it keeps.
    pub kept: Vec<usize>,
    /// Input word ranges that were removed, with the reason.
    pub removed: Vec<(Range<usize>, RemovalReason)>,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Keep,
    Remove { end: usize, reason: RemovalReason },
}

/// Finds a way to turn `input` into `output` by allowed removals alone.
/// `context_tail` holds the normalized last words of the previous text, for
/// a segment that opens by repeating them.
pub(crate) fn align(input: &Tokens, output: &Tokens, context_tail: &[String]) -> Option<Alignment> {
    let (n, m) = (input.words.len(), output.words.len());
    let removals: Vec<Vec<(usize, RemovalReason)>> =
        (0..n).map(|i| removals_at(input, i, context_tail)).collect();
    let width = m + 1;
    // step[i * width + j]: how to make progress from (i, j), if anything works.
    let mut step: Vec<Option<Step>> = vec![None; (n + 1) * width];
    let ok = |step: &[Option<Step>], i: usize, j: usize| (i == n && j == m) || (i < n && step[i * width + j].is_some());
    for i in (0..n).rev() {
        for j in (0..=m).rev() {
            let s = if j < m && input.words[i].norm == output.words[j].norm && ok(&step, i + 1, j + 1) {
                Some(Step::Keep)
            } else {
                removals[i]
                    .iter()
                    .find(|(end, _)| ok(&step, *end, j))
                    .map(|&(end, reason)| Step::Remove { end, reason })
            };
            step[i * width + j] = s;
        }
    }
    if !ok(&step, 0, 0) {
        return None;
    }
    let mut alignment = Alignment { kept: Vec::with_capacity(m), removed: Vec::new() };
    let (mut i, mut j) = (0, 0);
    while i < n {
        match step[i * width + j].expect("path exists") {
            Step::Keep => {
                alignment.kept.push(i);
                i += 1;
                j += 1;
            }
            Step::Remove { end, reason } => {
                alignment.removed.push((i..end, reason));
                i = end;
            }
        }
    }
    Some(alignment)
}

/// Whether a word can be dropped on its own, wherever it is.
pub(crate) fn always_removable(t: &Tokens, k: usize) -> Option<RemovalReason> {
    let w = &t.words[k];
    if w.tag {
        Some(RemovalReason::NoiseTag)
    } else if w.fragment {
        Some(RemovalReason::Fragment)
    } else if FILLERS.contains(&w.norm.as_str()) {
        Some(RemovalReason::Filler)
    } else {
        None
    }
}

/// Every allowed removal that starts at input word `i`, as (end, reason).
fn removals_at(t: &Tokens, i: usize, context_tail: &[String]) -> Vec<(usize, RemovalReason)> {
    let words = &t.words;
    let mut out = Vec::new();
    if let Some(reason) = always_removable(t, i) {
        out.push((i + 1, reason));
        return out;
    }
    let norm = |k: usize| words.get(k).map(|w| w.norm.as_str());
    if norm(i) == Some("you")
        && norm(i + 1) == Some("know")
        && t.gaps[i + 1].text.trim().is_empty()
        && set_off(t, i, i + 2)
    {
        out.push((i + 2, RemovalReason::Filler));
    }
    if norm(i) == Some("like") && set_off(t, i, i + 1) {
        out.push((i + 1, RemovalReason::Filler));
    }
    // Content words from i on, skipping words that are removable anyway, so
    // "the [cough] the" and "I, um, I" count as repeats.
    let content: Vec<usize> = (i..words.len())
        .filter(|&k| always_removable(t, k).is_none())
        .take(2 * MAX_REPEAT.max(MAX_FALSE_START) + 1)
        .collect();
    for len in 1..=MAX_REPEAT {
        if content.len() < 2 * len {
            break;
        }
        let same = (0..len).all(|k| words[content[k]].norm == words[content[len + k]].norm);
        if same {
            out.push((content[len], RemovalReason::Repeat));
        }
    }
    for len in 2..=MAX_FALSE_START {
        let Some(&restart) = content.get(len) else { break };
        let last = content[len - 1];
        let cut = &t.gaps[last + 1];
        let shared = shared_prefix(t, &content[..len], &content[len..]);
        // A filler or tag between the attempt and the restart: the speaker
        // hesitated, so the comma marks a real cut-off.
        let hesitated = restart > last + 1;
        let comma_ok = len <= MAX_COMMA_FALSE_START || hesitated || shared >= 2;
        let marked = cut.is_cut_off() || (cut.has_comma() && comma_ok);
        if marked && shared >= 1 {
            out.push((restart, RemovalReason::FalseStart));
        }
    }
    out.extend(restart_echo(t, i, &content));
    if i == 0 {
        out.extend(context_echo(t, &content, context_tail));
    }
    out
}

/// How many leading words `a` and `b` share.
fn shared_prefix(t: &Tokens, a: &[usize], b: &[usize]) -> usize {
    a.iter()
        .zip(b)
        .take_while(|(x, y)| t.words[**x].norm == t.words[**y].norm)
        .count()
}

/// The restart repeats the start of its own clause after a cut-off: "I
/// just, I needed" may lose the second "I" ("I just needed"). `content`
/// starts at `i`. Allowed when a comma or dash comes right before `i` and
/// the clause began at most [`MAX_FALSE_START`] words earlier with the same
/// word(s).
fn restart_echo(t: &Tokens, i: usize, content: &[usize]) -> Option<(usize, RemovalReason)> {
    let gap = &t.gaps[i];
    if i == 0 || !(gap.has_comma() || gap.is_cut_off()) {
        return None;
    }
    let before: Vec<usize> = (0..i)
        .rev()
        .filter(|&k| always_removable(t, k).is_none())
        .take(MAX_FALSE_START)
        .collect();
    // Walk back to the clause start: a word preceded by punctuation or the
    // start of the text.
    let start = before.iter().copied().find(|&k| k == 0 || is_clause_boundary(&t.gaps[k]))?;
    let clause: Vec<usize> = before.iter().copied().filter(|&k| k >= start).rev().collect();
    let shared = shared_prefix(t, &clause, content).min(clause.len());
    (shared >= 1).then(|| (content[shared - 1] + 1, RemovalReason::FalseStart))
}

fn is_clause_boundary(gap: &super::words::Gap) -> bool {
    gap.text.contains(['.', '!', '?', ',', ';', ':', '\n', '\u{2014}', '\u{2026}']) || gap.quotes() > 0
}

/// The segment opens by re-saying the last words of the previous text, as
/// a speaker does after a pause: context "…something that", input
/// "Something that moves" may lose "Something that".
fn context_echo(t: &Tokens, content: &[usize], context_tail: &[String]) -> Option<(usize, RemovalReason)> {
    (1..=MAX_REPEAT.min(context_tail.len()).min(content.len()))
        .rev()
        .find(|&len| {
            let tail = &context_tail[context_tail.len() - len..];
            tail.iter().zip(&content[..len]).all(|(c, &k)| *c == t.words[k].norm)
        })
        .map(|len| (content[len - 1] + 1, RemovalReason::Repeat))
}

/// Whether the phrase in words `start..end` ("like", "you know") is set off
/// as filler: by a comma on either side, by the start of the text, or by an
/// adjacent filler ("um like", "like, you know"). Elsewhere these are real
/// words ("she looked like a ghost", "you know nothing") and must stay.
fn set_off(t: &Tokens, start: usize, end: usize) -> bool {
    let filler_at = |k: usize| {
        k < t.words.len()
            && (always_removable(t, k) == Some(RemovalReason::Filler)
                || matches!(t.words[k].norm.as_str(), "like" | "you"))
    };
    start == 0
        || t.gaps[start].has_comma()
        || t.gaps[end].has_comma()
        || filler_at(start - 1)
        || filler_at(end)
}

/// One step of a word-level diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Same,
    /// Input word dropped.
    Drop(usize),
    /// Output word inserted.
    Insert(usize),
}

/// A word-level diff for explaining a rejection, from a longest common
/// subsequence of normalized words.
pub(crate) fn diff(input: &Tokens, output: &Tokens) -> Vec<Op> {
    let (a, b) = (&input.words, &output.words);
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] = if a[i].norm == b[j].norm {
                lcs[at(i + 1, j + 1)] + 1
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i].norm == b[j].norm {
            ops.push(Op::Same);
            i += 1;
            j += 1;
        } else if j == m || (i < n && lcs[at(i + 1, j)] >= lcs[at(i, j + 1)]) {
            ops.push(Op::Drop(i));
            i += 1;
        } else {
            ops.push(Op::Insert(j));
            j += 1;
        }
    }
    ops
}
