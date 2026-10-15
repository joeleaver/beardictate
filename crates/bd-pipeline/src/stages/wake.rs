//! "Bear, wake up" and "bear, go to sleep": the spoken switch for
//! always-listening mode ([`PipelineConfig::voice_wake`](crate::PipelineConfig)).
//!
//! Matched on the ASR text, word by word, ignoring case and punctuation. The
//! recognizer spells the name several ways, so all of them count. A wake
//! phrase counts anywhere in a segment (what follows it is dictation); a
//! sleep phrase only at the end of one, so a bear in the story told to go
//! to sleep mid-sentence doesn't stop dictation.

/// How the ASR writes "bear".
const NAMES: &[&str] = &["bear", "bare", "bair", "baer", "behr"];
/// What it sometimes hears instead ("Where? Go to sleep."). Counted only
/// when the phrase is the whole segment, where prose rarely says it; a
/// hotword boost doesn't fix these.
const SOUND_ALIKES: &[&str] = &["where", "wear", "ware", "bar", "beer", "bears", "bert"];
const WAKE: &[&[&str]] = &[&["wake", "up"], &["wakeup"], &["wake"]];
const SLEEP: &[&[&str]] = &[&["go", "to", "sleep"], &["sleep"]];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Switch {
    Wake,
    Sleep,
}

/// A switch phrase found in a segment, with the text on either side.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Heard<'a> {
    pub switch: Switch,
    pub before: &'a str,
    pub after: &'a str,
}

/// Words with their byte spans: letters, digits and apostrophes.
fn words(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        let in_word = c.is_alphanumeric() || c == '\'' || c == '\u{2019}';
        match (in_word, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, i, text[s..i].to_lowercase()));
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// Where `phrases` (after a name) match in `w`, as (first word, last word).
fn find(w: &[(usize, usize, String)], phrases: &[&[&str]]) -> Option<(usize, usize)> {
    (0..w.len()).find_map(|i| {
        let name = w[i].2.as_str();
        let alone = |end: usize| i == 0 && end + 1 == w.len();
        if !NAMES.contains(&name) && !SOUND_ALIKES.contains(&name) {
            return None;
        }
        let loose = !NAMES.contains(&name);
        phrases.iter().find_map(|p| {
            let end = i + p.len();
            let hit = end < w.len() && p.iter().zip(&w[i + 1..=end]).all(|(a, b)| *a == b.2);
            (hit && (!loose || alone(end))).then_some((i, end))
        })
    })
}

/// The first wake or sleep phrase in `text`.
pub(crate) fn heard(text: &str) -> Option<Heard<'_>> {
    let w = words(text);
    let trim = |s: &'static [char]| move |t: &str| -> usize { t.len() - t.trim_start_matches(s).len() };
    let skip = trim(&[' ', ',', '.', '!', '?', ';', ':', '\u{2026}', '-', '\u{2014}']);
    let cut = |first: usize, last: usize| {
        let before = text[..w[first].0].trim_end_matches([' ', ',', ';', ':', '-', '\u{2014}']);
        let rest = &text[w[last].1..];
        (before, &rest[skip(rest)..])
    };
    if let Some((first, last)) = find(&w, SLEEP)
        && last + 1 == w.len()
    {
        let (before, after) = cut(first, last);
        return Some(Heard { switch: Switch::Sleep, before, after });
    }
    let (first, last) = find(&w, WAKE)?;
    let (before, after) = cut(first, last);
    Some(Heard { switch: Switch::Wake, before, after })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(text: &str) -> Option<(Switch, &str, &str)> {
        heard(text).map(|h| (h.switch, h.before, h.after))
    }

    #[test]
    fn wake_and_sleep_in_the_ways_the_asr_writes_them() {
        assert_eq!(h("Bear, wake up."), Some((Switch::Wake, "", "")));
        assert_eq!(h("Bare wake up. It was late."), Some((Switch::Wake, "", "It was late.")));
        assert_eq!(h("Um, bear, wake up!"), Some((Switch::Wake, "Um", "")));
        assert_eq!(h("Bear, go to sleep."), Some((Switch::Sleep, "", "")));
        assert_eq!(h("He left. Bear, go to sleep."), Some((Switch::Sleep, "He left.", "")));
        assert_eq!(h("Bear sleep"), Some((Switch::Sleep, "", "")));
        // Misheard names count only when the phrase is all that was said.
        assert_eq!(h("Where? Go to sleep."), Some((Switch::Sleep, "", "")));
        assert_eq!(h("Wear, wake up."), Some((Switch::Wake, "", "")));
        assert_eq!(h("I asked her where go to sleep"), None);
        assert_eq!(h("Hey, wake up."), None);
    }

    #[test]
    fn story_bears_are_left_alone() {
        assert_eq!(h("The bear went to sleep."), None);
        assert_eq!(h("Bear, go to sleep, she whispered to the cub."), None);
        assert_eq!(h("A bear woke up."), None);
        assert_eq!(h("Wake up, bear."), None);
    }
}
