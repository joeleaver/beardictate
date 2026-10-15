//! Joining cleaned segments into one running text.

/// Marks after which the next segment follows without a space. Not the
/// straight `"`, which opens and closes alike.
const OPENERS: &[char] = &['\u{201C}', '\u{2018}', '\u{00AB}', '(', '['];

/// Marks a segment can start with that attach to the text before them: a
/// closing quote that continues an open quotation, or punctuation the ASR
/// put at the start of a segment.
const ATTACHED: &[char] = &[
    '\u{201D}', '\u{2019}', '\u{00BB}', ')', ']', ',', '.', ';', ':', '!', '?', '\u{2026}',
];

/// Our own output for the current passage: what has been delivered, kept to
/// a bounded tail for the cleanup model's context. Never read back from the
/// target application.
#[derive(Debug, Clone)]
pub struct Composer {
    tail: String,
    max_chars: usize,
}

impl Composer {
    /// Keeps at most `max_chars` characters of context. The cleanup prompt
    /// trims further; this only bounds memory.
    pub fn new(max_chars: usize) -> Self {
        Self { tail: String::new(), max_chars }
    }

    /// The delivered text of this passage, most recent last (bounded).
    pub fn context(&self) -> &str {
        &self.tail
    }

    /// Starts a new passage: no context, nothing to join onto.
    pub fn clear(&mut self) {
        self.tail.clear();
    }

    /// Appends one cleaned segment and returns exactly what to deliver: the
    /// segment with the separator it needs in front. Empty when the segment
    /// has nothing in it.
    pub fn append(&mut self, segment: &str) -> String {
        let delta = join(&self.tail, segment);
        self.tail.push_str(&delta);
        self.trim();
        delta
    }

    fn trim(&mut self) {
        let len = self.tail.chars().count();
        if len <= self.max_chars {
            return;
        }
        let cut = self.tail.char_indices().nth(len - self.max_chars).map_or(0, |(i, _)| i);
        self.tail.drain(..cut);
    }
}

/// The text to add after `before` so that `segment` reads on from it.
pub fn join(before: &str, segment: &str) -> String {
    let segment = segment.trim_start_matches([' ', '\t']).trim_end_matches([' ', '\t']);
    if segment.trim().is_empty() && !segment.contains('\n') {
        return String::new();
    }
    let Some(last) = before.chars().last() else {
        return segment.to_string();
    };
    let first = segment.chars().next().unwrap_or(' ');
    let tight = first == '\n' || last.is_whitespace() || OPENERS.contains(&last) || ATTACHED.contains(&first);
    if tight { segment.to_string() } else { format!(" {segment}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_are_joined_with_a_space() {
        assert_eq!(join("It rained.", "We stayed in."), " We stayed in.");
        assert_eq!(join("", "We stayed in."), "We stayed in.");
    }

    #[test]
    fn breaks_and_closing_marks_attach() {
        assert_eq!(join("It rained.", "\n\nNext day."), "\n\nNext day.");
        assert_eq!(join("“Wait", "” she said."), "” she said.");
        assert_eq!(join("Done.\n", "Next."), "Next.");
        assert_eq!(join("He said, “", "Go."), "Go.");
        assert_eq!(join("\"Done.\"", "\"Next.\""), " \"Next.\"");
    }

    #[test]
    fn empty_segments_deliver_nothing_but_breaks_survive() {
        assert_eq!(join("Text.", ""), "");
        assert_eq!(join("Text.", "  "), "");
        assert_eq!(join("Text.", "\n\n"), "\n\n");
    }

    #[test]
    fn composer_keeps_a_bounded_tail() {
        let mut c = Composer::new(10);
        assert_eq!(c.append("One two."), "One two.");
        assert_eq!(c.append("Three."), " Three.");
        assert_eq!(c.context(), "wo. Three.");
        assert!(c.context().chars().count() <= 10);
        c.clear();
        assert_eq!(c.append("Fresh."), "Fresh.");
    }
}
