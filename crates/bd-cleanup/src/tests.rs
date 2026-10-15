//! The guarded pipeline around a scripted backend.

use bd_core::{Cleanup, CleanupRequest, Style};

use super::*;

/// Replies with canned answers and records what it was asked.
struct Scripted {
    replies: Vec<String>,
    seen: Vec<Vec<ChatMessage>>,
}

impl ChatBackend for Scripted {
    fn complete(&mut self, messages: &[ChatMessage], _max_tokens: usize) -> Result<Completion, CleanupError> {
        self.seen.push(messages.to_vec());
        Ok(Completion { text: self.replies.remove(0), stats: None })
    }
}

fn guarded(replies: &[&str]) -> GuardedCleanup<Scripted> {
    let backend = Scripted { replies: replies.iter().map(|s| s.to_string()).collect(), seen: Vec::new() };
    GuardedCleanup::new(backend, "generic-v1").unwrap()
}

fn req<'a>(raw: &'a str, style: &'a Style) -> CleanupRequest<'a> {
    CleanupRequest { context: "", raw, style }
}

#[test]
fn accepted_output_is_used() {
    let style = Style::default();
    let mut c = guarded(&["<think>\n\n</think>\n\n“Get down,” Tom hissed.  "]);
    let out = c.clean(&req("get down um Tom hissed", &style)).unwrap();
    assert_eq!(out, "“Get down,” Tom hissed.");
    let last = c.last_outcome().unwrap();
    assert!(!last.fell_back());
    assert!(c.backend_mut().seen[0][1].content.contains("get down um Tom hissed"));
}

#[test]
fn rejected_output_falls_back_to_the_input() {
    let style = Style::default();
    let mut c = guarded(&["Tom whispered, “Get down.”"]);
    let raw = "“get down” Tom hissed";
    let outcome = c.clean_detailed(&req(raw, &style)).unwrap();
    assert_eq!(outcome.text, raw);
    assert!(outcome.fell_back());
    assert_eq!(outcome.model_output.as_deref(), Some("Tom whispered, “Get down.”"));
}

#[test]
fn segments_without_words_skip_the_model() {
    let style = Style::default();
    let mut c = guarded(&[]);
    for raw in ["", "\n\n", "“"] {
        let outcome = c.clean_detailed(&req(raw, &style)).unwrap();
        assert_eq!(outcome.text, raw);
        assert!(outcome.verdict.is_none());
    }
    assert!(c.backend_mut().seen.is_empty());
}

#[test]
fn unknown_template_is_an_error() {
    let backend = Scripted { replies: vec![], seen: vec![] };
    assert!(matches!(GuardedCleanup::new(backend, "nope"), Err(CleanupError::UnknownTemplate(_))));
}

#[test]
fn finalize_tidies_answers() {
    let cases: &[(&str, &str, &str)] = &[
        ("he left", "He left.\n", "He left."),
        ("he left", "  He left.", "He left."),
        ("he left", "\n\n\n“He left.”", "\n\n“He left.”"),
        ("he left", "\nHe left.", "\nHe left."),
        ("he left", "Answer: He left.", "He left."),
        ("he left", "Answer:\n\nHe left.", "\n\nHe left."),
        ("he left", "```\nHe left.\n```", "He left."),
        ("he left", "<think>hmm</think>He left.", "He left."),
        // A whole answer echoed in straight quotes is unwrapped, unless the
        // input had them or the quotes are dialog inside the answer.
        ("he left", "\"He left.\"", "He left."),
        ("\"he left\"", "\"He left.\"", "\"He left.\""),
        ("hi he said bye", "\"Hi,\" he said. \"Bye.\"", "\"Hi,\" he said. \"Bye.\""),
        ("he left", "<think>never closed", ""),
        // Breaks the command pass put at the edges survive the model.
        ("\n\nhe left", "He left.", "\n\nHe left."),
        ("he left\n\n", "He left.", "He left.\n\n"),
        ("\n\n", "", "\n\n"),
    ];
    for (raw, answer, expected) in cases {
        assert_eq!(finalize(raw, answer), *expected, "{raw:?} / {answer:?}");
    }
}

#[test]
fn budget_grows_with_input() {
    assert!(max_new_tokens("") >= 32);
    assert!(max_new_tokens(&"word ".repeat(40)) > 100);
}

#[test]
fn a_miscopied_word_is_repaired_not_thrown_away() {
    let style = Style::default();
    let mut c = guarded(&["He said, \"Oh yes, it is came.\""]);
    let outcome = c.clean_detailed(&req("he said, oh yes, it is coming.", &style)).unwrap();
    assert_eq!(outcome.text, "He said, \u{201C}Oh yes, it is coming.\u{201D}");
    assert!(!outcome.fell_back());
    assert!(!outcome.verdict.as_ref().unwrap().is_accepted());
    assert_eq!(outcome.repair.unwrap().restored, [("came".to_string(), "coming".to_string())]);
}
