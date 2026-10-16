//! Bear Pad: dictation collects in a pad instead of being typed, and goes to
//! the focused application in one piece when the user says "bear, send it".
//!
//! Each segment lands in the pad as soon as it is heard, with spoken
//! commands applied: raw. The cleanup model works on the pad, not on each
//! segment: when the speaker pauses, it cleans the paragraph being dictated
//! (the clean part again, with the raw sentences after it), with the two
//! paragraphs before as context. So a later sentence can still fix an earlier
//! one in the same paragraph, and the model never sees more than a few
//! paragraphs at once.
//!
//! The pad is a list of spans: a raw span is one segment; a clean span is
//! the cleaned text of the segments it was made from, which it keeps, so
//! "scratch that" can take one segment out of it (the rest goes back to raw
//! and is cleaned again).

use crate::compose::join;

/// A clean pass takes at most about this many characters of the pad.
const BATCH_CHARS: usize = 900;
/// Paragraphs before the cleaned text that the model gets as context.
const CONTEXT_PARAGRAPHS: usize = 2;

/// What the user can do to the pad besides dictating into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PadAction {
    /// Type the pad at the cursor in the focused application, and empty it.
    Send,
    /// Empty it ("scratch that" brings it back).
    Clear,
    /// Take back the last thing said or done, like "scratch that".
    Undo,
    /// Replace the last `from` in the pad with `to`.
    Change { from: String, to: String },
    /// Take the last `word` (or words) out of the pad.
    Delete(String),
}

/// What the pad shows: its text in order, each part clean or still raw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PadView {
    pub parts: Vec<PadPart>,
    /// The cleanup model is working on the pad.
    pub cleaning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadPart {
    pub text: String,
    /// Cleaned (or there is no cleanup model): as it will be sent.
    pub clean: bool,
}

impl PadView {
    pub fn text(&self) -> String {
        self.parts.iter().map(|p| p.text.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.text.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seg {
    id: u64,
    /// After the command pass.
    text: String,
    /// Whether a commanded quotation was open before it.
    quote_before: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    segs: Vec<Seg>,
    /// The cleaned text of `segs`; `None` while raw (then it has one seg).
    cleaned: Option<String>,
}

impl Span {
    fn raw(seg: Seg) -> Span {
        Span { segs: vec![seg], cleaned: None }
    }

    fn text(&self) -> &str {
        self.cleaned.as_deref().unwrap_or(&self.segs[0].text)
    }

    fn text_mut(&mut self) -> &mut String {
        match &mut self.cleaned {
            Some(t) => t,
            None => &mut self.segs[0].text,
        }
    }
}

#[derive(Debug, Clone)]
enum Change {
    Segment(u64),
    /// An edit or a clear, with the pad as it was before.
    Edit(Vec<Span>),
}

/// Part of the pad to clean: spans `start..end`, joined into `raw`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Batch {
    start: usize,
    spans: Vec<Span>,
    pub raw: String,
    pub context: String,
}

#[derive(Debug, Default)]
pub(crate) struct Pad {
    spans: Vec<Span>,
    history: Vec<Change>,
}

impl Pad {
    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text().trim().is_empty())
    }

    pub fn has_raw(&self) -> bool {
        self.spans.iter().any(|s| s.cleaned.is_none())
    }

    /// Adds a segment, raw. `text` is after the command pass.
    pub fn push(&mut self, id: u64, text: String, quote_before: bool) {
        if text.trim().is_empty() && !text.contains('\n') {
            return;
        }
        self.spans.push(Span::raw(Seg { id, text, quote_before }));
        self.history.push(Change::Segment(id));
    }

    /// Takes back the last segment or edit. Returns the commanded-quotation
    /// state to carry on with (`quote_now` unless a segment went), or `None`
    /// when there was nothing to take back.
    pub fn undo(&mut self, quote_now: bool) -> Option<bool> {
        match self.history.pop()? {
            Change::Edit(before) => {
                self.spans = before;
                Some(quote_now)
            }
            Change::Segment(id) => {
                let Some(i) = self.spans.iter().position(|s| s.segs.iter().any(|g| g.id == id)) else {
                    return Some(quote_now);
                };
                let span = self.spans.remove(i);
                let mut quote = quote_now;
                let mut rest = Vec::new();
                for seg in span.segs {
                    if seg.id == id {
                        quote = seg.quote_before;
                    } else {
                        rest.push(Span::raw(seg));
                    }
                }
                self.spans.splice(i..i, rest);
                Some(quote)
            }
        }
    }

    /// Empties the pad, which "scratch that" can undo. False if it was empty.
    pub fn clear(&mut self) -> bool {
        if self.spans.is_empty() {
            return false;
        }
        self.history.push(Change::Edit(std::mem::take(&mut self.spans)));
        true
    }

    /// Replaces the last whole-word `from` (any case) with `to`, which takes
    /// the capital of the word it replaces. False if `from` isn't there.
    pub fn change(&mut self, from: &str, to: &str) -> bool {
        let from = from.trim();
        if from.is_empty() {
            return false;
        }
        let before = self.spans.clone();
        for span in self.spans.iter_mut().rev() {
            let text = span.text_mut();
            if let Some((s, e)) = find_last(text, from) {
                let to = match_case(&text[s..e], to.trim());
                text.replace_range(s..e, &to);
                if to.is_empty() {
                    tidy_cut(text, s);
                }
                self.history.push(Change::Edit(before));
                return true;
            }
        }
        false
    }

    /// Everything in the pad, ready to send; the pad is left empty, with
    /// nothing to undo (the text has gone to another application).
    pub fn take(&mut self) -> String {
        let text: String = self.view(true).text();
        self.spans.clear();
        self.history.clear();
        text.trim_end_matches([' ', '\t']).to_string()
    }

    /// The pad's text. With `no_model`, raw text shows as clean (nothing
    /// will clean it).
    pub fn view(&self, no_model: bool) -> PadView {
        let mut acc = String::new();
        let mut parts: Vec<PadPart> = Vec::new();
        for span in &self.spans {
            let (back, delta) = join(&acc, span.text());
            for _ in 0..back {
                acc.pop();
                if let Some(last) = parts.last_mut() {
                    last.text.pop();
                    if last.text.is_empty() {
                        parts.pop();
                    }
                }
            }
            acc.push_str(&delta);
            let clean = no_model || span.cleaned.is_some();
            match parts.last_mut() {
                Some(last) if last.clean == clean => last.text.push_str(&delta),
                _ if delta.is_empty() => {}
                _ => parts.push(PadPart { text: delta, clean }),
            }
        }
        PadView { parts, cleaning: false }
    }

    /// The next part to clean, if any: the paragraph the first raw span is
    /// in, from its start, through its last raw sentence that's finished.
    /// With `all`, through the last raw span, finished or not.
    pub fn next_batch(&self, all: bool) -> Option<Batch> {
        let first_raw = self.spans.iter().position(|s| s.cleaned.is_none())?;
        // Back to the start of the paragraph, within the size limit.
        let mut start = first_raw;
        let mut size = self.spans[first_raw].text().chars().count();
        while start > 0 {
            let prev = self.spans[start - 1].text();
            if starts_paragraph(self.spans[start].text()) || ends_paragraph(prev) {
                break;
            }
            size += prev.chars().count();
            if size > BATCH_CHARS {
                break;
            }
            start -= 1;
        }
        let mut raw = String::new();
        let mut finished = None;
        let mut end = start;
        while end < self.spans.len() {
            // One paragraph a pass.
            if end > first_raw && (starts_paragraph(self.spans[end].text()) || ends_paragraph(self.spans[end - 1].text())) {
                break;
            }
            let (back, delta) = join(&raw, self.spans[end].text());
            for _ in 0..back {
                raw.pop();
            }
            raw.push_str(&delta);
            end += 1;
            let raw_part = end > first_raw;
            if raw_part && ends_sentence(&raw) {
                finished = Some((end, raw.clone()));
            }
            if raw_part && raw.chars().count() >= BATCH_CHARS {
                break;
            }
        }
        let full = raw.chars().count() >= BATCH_CHARS;
        let (end, raw) = if all || (full && finished.is_none()) { (end, raw) } else { finished? };
        let before = self.view(true).text();
        let before = &before[..self.prefix_len(start).min(before.len())];
        Some(Batch {
            start,
            spans: self.spans[start..end].to_vec(),
            raw: raw.trim_end_matches([' ', '\t']).to_string(),
            context: last_paragraphs(before, CONTEXT_PARAGRAPHS).to_string(),
        })
    }

    /// Puts a batch's cleaned text in, if that part of the pad hasn't
    /// changed since. False when it had (the result is dropped).
    pub fn apply(&mut self, batch: Batch, cleaned: &str) -> bool {
        let end = batch.start + batch.spans.len();
        if self.spans.get(batch.start..end) != Some(&batch.spans[..]) {
            return false;
        }
        let segs = batch.spans.into_iter().flat_map(|s| s.segs).collect();
        let cleaned = cleaned.trim_end_matches([' ', '\t']).to_string();
        self.spans.splice(batch.start..end, [Span { segs, cleaned: Some(cleaned) }]);
        true
    }

    /// Bytes of the joined text before span `i`.
    fn prefix_len(&self, i: usize) -> usize {
        let mut acc = String::new();
        for span in &self.spans[..i] {
            let (back, delta) = join(&acc, span.text());
            for _ in 0..back {
                acc.pop();
            }
            acc.push_str(&delta);
        }
        acc.len()
    }
}

fn starts_paragraph(text: &str) -> bool {
    text.trim_start_matches([' ', '\t']).starts_with('\n')
}

fn ends_paragraph(text: &str) -> bool {
    text.trim_end_matches([' ', '\t']).ends_with('\n')
}

/// Whether `text` ends a sentence (or a line), so it can be cleaned whole.
fn ends_sentence(text: &str) -> bool {
    let text = text.trim_end_matches([' ', '\t']);
    if text.ends_with('\n') {
        return true;
    }
    let text = text.trim_end_matches(['\u{201D}', '\u{2019}', '"', '\'', ')', ']', '\u{00BB}']);
    text.ends_with(['.', '!', '?', '\u{2026}'])
}

/// The last `n` paragraphs (or lines) of `text`.
fn last_paragraphs(text: &str, n: usize) -> &str {
    let text = text.trim_end();
    let mut seen = 0;
    let mut prev_newline = false;
    for (i, c) in text.char_indices().rev() {
        if c == '\n' {
            if !prev_newline {
                seen += 1;
                if seen == n {
                    return text[i + 1..].trim_start();
                }
            }
            prev_newline = true;
        } else {
            prev_newline = false;
        }
    }
    text
}

/// The byte range of the last whole-word, ASCII-case-insensitive `needle`.
fn find_last(hay: &str, needle: &str) -> Option<(usize, usize)> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.len() > h.len() {
        return None;
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '\'' || c == '\u{2019}');
    (0..=h.len() - n.len()).rev().find_map(|s| {
        let e = s + n.len();
        let fits = hay.is_char_boundary(s) && hay.is_char_boundary(e) && h[s..e].eq_ignore_ascii_case(n);
        (fits && !word(hay[..s].chars().next_back()) && !word(hay[e..].chars().next())).then_some((s, e))
    })
}

/// `to` with the first letter capitalized when `found` started with one.
fn match_case(found: &str, to: &str) -> String {
    let capital = found.chars().next().is_some_and(char::is_uppercase);
    let mut chars = to.chars();
    match chars.next() {
        Some(c) if capital && c.is_lowercase() => c.to_uppercase().chain(chars).collect(),
        _ => to.to_string(),
    }
}

/// After a word was cut out at `at`: no doubled space, no space before a
/// mark, none at the start.
fn tidy_cut(text: &mut String, at: usize) {
    let next = text[at..].chars().next();
    let prev = text[..at].chars().next_back();
    let mark = |c: Option<char>| c.is_none_or(|c| c == ' ' || c == '\n' || ",.;:!?\u{201D}\u{2019})".contains(c));
    if prev == Some(' ') && mark(next) {
        text.remove(at - 1);
    } else if prev.is_none_or(|c| c == '\n') && next == Some(' ') {
        text.remove(at);
    }
}

#[cfg(test)]
#[path = "pad_tests.rs"]
mod tests;
