//! Joining cleaned segments into one running text.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Marks after which the next segment follows without a space. Not the
/// straight `"`, which opens and closes alike.
const OPENERS: &[char] = &['\u{201C}', '\u{2018}', '\u{00AB}', '(', '['];

/// Marks a segment can start with that attach to the text before them: a
/// closing quote that continues an open quotation, or punctuation the ASR
/// put at the start of a segment.
const ATTACHED: &[char] = &[
    '\u{201D}', '\u{2019}', '\u{00BB}', ')', ']', ',', '.', ';', ':', '!', '?', '\u{2026}',
];

/// The marks a spoken punctuation command makes, or the recognizer puts
/// at a word's end.
const MARKS: &[char] = &[',', '.', ';', ':', '!', '?', '\u{2026}'];

/// Our own output for the current passage: what has been delivered, kept to
/// a bounded tail for the cleanup model's context. Never read back from the
/// target application.
#[derive(Debug, Clone)]
pub struct Composer {
    tail: String,
    max_chars: usize,
    /// When each delivered piece was appended, and the byte in `tail` it
    /// starts at, oldest first.
    stamps: VecDeque<(Instant, usize)>,
}

impl Composer {
    /// Keeps at most `max_chars` characters of context. The cleanup prompt
    /// trims further; this only bounds memory.
    pub fn new(max_chars: usize) -> Self {
        Self { tail: String::new(), max_chars, stamps: VecDeque::new() }
    }

    /// The delivered text of this passage, most recent last (bounded).
    pub fn context(&self) -> &str {
        &self.tail
    }

    /// The cleanup model's context: what was delivered in the last
    /// `window`, at most `max_chars` of it, starting at a sentence or
    /// paragraph where it's cut.
    pub fn recent(&self, window: Duration, max_chars: usize) -> &str {
        self.recent_at(Instant::now(), window, max_chars)
    }

    /// [`Composer::recent`] for the text before byte `end` of the context:
    /// the context of a piece already delivered, when it's cleaned later.
    pub(crate) fn recent_before(&self, end: usize, window: Duration, max_chars: usize) -> &str {
        self.recent_until(Instant::now(), end, window, max_chars)
    }

    fn recent_at(&self, now: Instant, window: Duration, max_chars: usize) -> &str {
        self.recent_until(now, self.tail.len(), window, max_chars)
    }

    fn recent_until(&self, now: Instant, end: usize, window: Duration, max_chars: usize) -> &str {
        let start = self
            .stamps
            .iter()
            .filter(|&&(_, at)| at < end)
            .find(|(t, _)| now.saturating_duration_since(*t) <= window)
            .map_or(end, |&(_, at)| at);
        bd_cleanup::prompt::trim_context(&self.tail[start..end], max_chars)
    }

    /// Replaces the delivered text from byte `from` on with `text`, as one
    /// piece from when the first piece it replaces was delivered: a
    /// sentence tidied in place, and what was typed after it.
    pub(crate) fn replace_from(&mut self, from: usize, text: &str) {
        let at = self.stamps.iter().find(|&&(_, start)| start >= from).map_or_else(Instant::now, |&(t, _)| t);
        self.tail.truncate(from);
        self.drop_stamps_after(from);
        if !text.is_empty() {
            self.stamps.push_back((at, from));
        }
        self.tail.push_str(text);
        self.trim();
    }

    /// Starts a new passage: no context, nothing to join onto.
    pub fn clear(&mut self) {
        self.tail.clear();
        self.stamps.clear();
    }

    /// Appends one cleaned segment and returns exactly what to deliver: how
    /// many characters to take back first (the trailing space before a break
    /// or a closing mark), then the segment with the space it needs after it.
    /// Empty when the segment has nothing in it.
    pub fn append(&mut self, segment: &str) -> (usize, String) {
        self.append_at(Instant::now(), segment)
    }

    fn append_at(&mut self, now: Instant, segment: &str) -> (usize, String) {
        let (take_back, delta) = join(&self.tail, segment);
        for _ in 0..take_back {
            self.tail.pop();
        }
        self.drop_stamps_after(self.tail.len());
        if !delta.is_empty() {
            self.stamps.push_back((now, self.tail.len()));
        }
        self.tail.push_str(&delta);
        self.trim();
        (take_back, delta)
    }

    /// Takes back `delivered`, the most recent [`Composer::append`]'s
    /// result, after it was erased. If it is no longer all in the context
    /// (trimmed away), the context is dropped rather than left wrong.
    pub fn retract(&mut self, delivered: &str) {
        if self.tail.ends_with(delivered) {
            self.tail.truncate(self.tail.len() - delivered.len());
            self.drop_stamps_after(self.tail.len());
        } else {
            self.clear();
        }
    }

    /// Forgets the pieces that started at or after byte `len` (taken back).
    fn drop_stamps_after(&mut self, len: usize) {
        while self.stamps.back().is_some_and(|&(_, at)| at >= len) {
            self.stamps.pop_back();
        }
    }

    fn trim(&mut self) {
        let len = self.tail.chars().count();
        if len <= self.max_chars {
            return;
        }
        let cut = self.tail.char_indices().nth(len - self.max_chars).map_or(0, |(i, _)| i);
        self.tail.drain(..cut);
        // A piece cut into keeps its time, from the new start.
        for stamp in &mut self.stamps {
            stamp.1 = stamp.1.saturating_sub(cut);
        }
        while self.stamps.len() > 1 && self.stamps[1].1 == 0 {
            self.stamps.pop_front();
        }
    }
}

/// `text` with each paragraph break (a run of two or more newlines) made a
/// single newline.
pub fn single_return_paragraphs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\n' && out.ends_with('\n') {
            continue;
        }
        out.push(c);
    }
    out
}

/// What to add after `before` so that `segment` reads on from it: how many
/// characters to take back first, and the text.
///
/// Each segment ends with a space rather than starting with one, so a
/// paragraph the user starts by pressing Enter themselves begins cleanly; a
/// trailing space is invisible. Before a break or a mark that attaches to
/// the word before it (a closing quote, a comma), that space is taken back.
pub fn join(before: &str, segment: &str) -> (usize, String) {
    let segment = segment.trim_start_matches([' ', '\t']).trim_end_matches([' ', '\t']);
    // A break the model starts with after a dictated one ("new paragraph",
    // then a model that also opens the next speaker's paragraph) is the same
    // break: never more than one blank line.
    let ending = before.trim_end_matches([' ', '\t']);
    let have = ending.len() - ending.trim_end_matches('\n').len();
    let lead = segment.len() - segment.trim_start_matches('\n').len();
    let segment = if have > 0 && lead > 0 && have + lead > 2 { &segment[lead.min(lead + have - 2)..] } else { segment };
    if segment.trim().is_empty() && !segment.contains('\n') {
        return (0, String::new());
    }
    let first = segment.chars().next().unwrap_or(' ');
    // A mark said on its own ("…so necessary" and then "exclamation
    // point"): it takes the place of the one the recognizer put there.
    if MARKS.contains(&first) {
        let ending = before.trim_end_matches(' ');
        if ending.ends_with(MARKS) {
            let gone = before.len() - ending.trim_end_matches(MARKS).len();
            let take_back = before[before.len() - gone..].chars().count();
            let (_, text) = join(&before[..before.len() - gone], segment);
            return (take_back, text);
        }
    }
    let attaches = first == '\n' || ATTACHED.contains(&first);
    let (take_back, lead) = match before.chars().last() {
        None => (0, ""),
        Some(' ') if attaches => (1, ""),
        Some(c) if attaches || c.is_whitespace() || OPENERS.contains(&c) => (0, ""),
        // Text that doesn't end in our space (it was taken back, or erased).
        Some(_) => (0, " "),
    };
    let ends_open = segment.ends_with('\n') || segment.ends_with(OPENERS);
    let trail = if ends_open { "" } else { " " };
    (take_back, format!("{lead}{segment}{trail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_end_with_a_space() {
        assert_eq!(join("It rained. ", "We stayed in."), (0, "We stayed in. ".into()));
        assert_eq!(join("", "We stayed in."), (0, "We stayed in. ".into()));
        // A context that lost its trailing space gets one in front instead.
        assert_eq!(join("It rained.", "We stayed in."), (0, " We stayed in. ".into()));
    }

    #[test]
    fn a_mark_said_alone_replaces_the_one_before_it() {
        assert_eq!(join("So necessary. ", "!"), (2, "! ".into()));
        assert_eq!(join("Is it working. ", "?"), (2, "? ".into()));
        assert_eq!(join("Wait, ", "\u{2026}"), (2, "\u{2026} ".into()));
        assert_eq!(join("So necessary ", ","), (1, ", ".into()));
        // Inside a closed quote, the mark stays outside it.
        assert_eq!(join("\u{201C}Go.\u{201D} ", "!"), (1, "! ".into()));
    }

    #[test]
    fn breaks_and_closing_marks_take_the_space_back() {
        assert_eq!(join("It rained. ", "\n\nNext day."), (1, "\n\nNext day. ".into()));
        assert_eq!(join("“Wait ", "” she said."), (1, "” she said. ".into()));
        assert_eq!(join("Done.\n", "Next."), (0, "Next. ".into()));
        assert_eq!(join("He said, “", "Go."), (0, "Go. ".into()));
        assert_eq!(join("Then: ", "\n"), (1, "\n".into()));
        assert_eq!(join("He said, ", "“Go"), (0, "“Go ".into()));
        assert_eq!(join("Open ", "“"), (0, "“".into()), "nothing after an opening mark");
    }

    #[test]
    fn a_model_break_after_a_dictated_one_is_one_break() {
        assert_eq!(join("Done.\n\n", "\n\nNext."), (0, "Next. ".into()));
        assert_eq!(join("Done.\n", "\n\nNext."), (0, "\nNext. ".into()));
        assert_eq!(join("Done.\n", "\nNext."), (0, "\nNext. ".into()), "two dictated new lines stay two");
        assert_eq!(join("Done. ", "\n\nNext."), (1, "\n\nNext. ".into()));
    }

    #[test]
    fn empty_segments_deliver_nothing_but_breaks_survive() {
        assert_eq!(join("Text. ", ""), (0, String::new()));
        assert_eq!(join("Text. ", "  "), (0, String::new()));
        assert_eq!(join("Text. ", "\n\n"), (1, "\n\n".into()));
    }

    #[test]
    fn composer_keeps_a_bounded_tail() {
        let mut c = Composer::new(10);
        assert_eq!(c.append("One two."), (0, "One two. ".into()));
        assert_eq!(c.append("Three."), (0, "Three. ".into()));
        assert_eq!(c.context(), "o. Three. ");
        assert!(c.context().chars().count() <= 10);
        c.clear();
        assert_eq!(c.append("Fresh."), (0, "Fresh. ".into()));
        let mut c = Composer::new(100);
        c.append("Kept.");
        let (_, d) = c.append("Gone.");
        c.retract(&d);
        assert_eq!(c.context(), "Kept. ");
        assert_eq!(c.append("Back."), (0, "Back. ".into()));
        // A break takes the trailing space back, in the context too.
        assert_eq!(c.append("\n\nNew."), (1, "\n\nNew. ".into()));
        assert_eq!(c.context(), "Kept. Back.\n\nNew. ");
    }

    #[test]
    fn a_tidied_sentence_replaces_what_was_typed() {
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let mut c = Composer::new(4000);
        c.append_at(at(0), "Before.");
        c.append_at(at(10), "um so we");
        c.append_at(at(12), "Next.");
        let from = "Before. ".len();
        // Its context is what came before it.
        assert_eq!(c.recent_until(at(13), from, Duration::from_secs(90), 600), "Before. ");
        c.replace_from(from, "So we. Next. ");
        assert_eq!(c.context(), "Before. So we. Next. ");
        // The replaced text keeps the time of the first piece it replaced.
        assert_eq!(c.recent_at(at(95), Duration::from_secs(90), 600), "So we. Next. ");
        assert_eq!(c.recent_at(at(101), Duration::from_secs(90), 600), "");
    }

    #[test]
    fn cleanup_sees_only_recent_text() {
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let window = Duration::from_secs(90);
        let mut c = Composer::new(4000);
        c.append_at(at(0), "An old line in another app.");
        c.append_at(at(100), "Now this.");
        c.append_at(at(110), "And this.");
        // Joining still sees everything; cleanup only the last 90 s.
        assert_eq!(c.context(), "An old line in another app. Now this. And this. ");
        assert_eq!(c.recent_at(at(120), window, 600), "Now this. And this. ");
        assert_eq!(c.recent_at(at(300), window, 600), "");
        // ...and only so much of it, from a sentence.
        assert_eq!(c.recent_at(at(120), window, 12), "And this. ");
        // Taking a piece back takes its time with it.
        let (_, d) = c.append_at(at(115), "Gone.");
        c.retract(&d);
        assert_eq!(c.recent_at(at(120), window, 600), "Now this. And this. ");
        // A trimmed tail keeps the time of the piece it now starts in.
        let mut c = Composer::new(12);
        c.append_at(at(0), "First part.");
        c.append_at(at(50), "Second.");
        assert_eq!(c.context(), "rt. Second. ");
        assert_eq!(c.recent_at(at(60), Duration::from_secs(30), 600), "Second. ");
        assert_eq!(c.recent_at(at(60), window, 600), "rt. Second. ");
    }
}
