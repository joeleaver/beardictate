//! Versioned prompt templates for the cleanup model.
//!
//! Each template is a text file under `prompts/`, compiled in, with this
//! format (shared with `training/`, so a fine-tune trains on exactly the
//! runtime prompt):
//!
//! ```text
//! === SYSTEM ===
//! …system text…
//! === USER ===
//! …user text with {context} and {raw}…
//! ```
//!
//! Few-shot templates may put worked examples as `=== USER ===` /
//! `=== ASSISTANT ===` pairs before the final `=== USER ===`; blank lines
//! at the start of an example answer are part of it (a new paragraph). Placeholders,
//! usable anywhere: `{style_rule}` (quote style and punctuation placement),
//! `{open}` and `{close}` (the style's quote marks), `{context}` (becomes
//! [`NO_CONTEXT`] when there is no previous text) and `{raw}`. Bare-tag
//! templates for the Bear fine-tunes use `{style}` (`us` or `uk`),
//! `{context_raw}` (empty when there is none) and `{names}` (the special
//! words, comma-separated), and may have no system section. The
//! model manifest names a template by its id, so a model ships with the
//! prompt it was tuned or trained for.

use bd_core::{CleanupRequest, PunctuationPlacement, QuoteStyle, Style};

/// What `{context}` becomes at the start of a passage.
pub const NO_CONTEXT: &str = "(none: this is the start of the passage)";

/// The template used when nothing names one.
pub const DEFAULT_TEMPLATE: &str = "generic-v1";

/// A prompt template, identified by a versioned id. Templates are never
/// edited in place once published; a change gets a new id.
#[derive(Debug, PartialEq, Eq)]
pub struct PromptTemplate {
    pub id: &'static str,
    pub description: &'static str,
    source: &'static str,
}

/// Every built-in template.
pub const TEMPLATES: &[PromptTemplate] = &[
    PromptTemplate {
        id: "generic-v1",
        description: "Zero-shot prompt for stock instruct models: hesitations, punctuation, unmarked dialog.",
        source: include_str!("../prompts/generic-v1.txt"),
    },
    PromptTemplate {
        id: "generic-fewshot-v1",
        description: "generic-v1 plus four worked examples in the system prompt.",
        source: include_str!("../prompts/generic-fewshot-v1.txt"),
    },
    PromptTemplate {
        id: "generic-turns-v1",
        description: "generic-v1 plus five worked examples as prior chat turns.",
        source: include_str!("../prompts/generic-turns-v1.txt"),
    },
    PromptTemplate {
        id: "short-v1",
        description: "A few plain sentences, no numbered rules: Qwen3.5-4B's best (51% exact vs 39% with generic-v1, 2026-10-15 survey).",
        source: include_str!("../prompts/short-v1.txt"),
    },
    PromptTemplate {
        id: "short-turns-v1",
        description: "short-v1 plus five worked examples as prior chat turns: Gemma 4 E2B's best (47% exact, 71% quotes ok, 2026-10-15 survey).",
        source: include_str!("../prompts/short-turns-v1.txt"),
    },
    PromptTemplate {
        id: "narration-turns-v1",
        description: "short-turns-v1 told that most dictation is narration, with narration examples (questions, first person) left unquoted. For Gemma 4 E2B, which quoted narration.",
        source: include_str!("../prompts/narration-turns-v1.txt"),
    },
    PromptTemplate {
        id: "short-v2",
        description: "short-v1 rewritten so most dictation is narration: quote only what the story so far shows is spoken aloud; the narrator's questions, thoughts and asides stay unquoted.",
        source: include_str!("../prompts/short-v2.txt"),
    },
    PromptTemplate {
        id: "short-turns-v2",
        description: "short-v2's narration-first instructions with short-turns-v1's worked examples: Gemma 4 E2B's best (49% exact, 72% quotes ok, words changed in 21 cases, 2026-10-15 survey).",
        source: include_str!("../prompts/short-turns-v2.txt"),
    },
    PromptTemplate {
        id: "bear-tags-v1",
        description: "Bare tags, one user turn, no instructions: the format the Bear fine-tunes were trained on (2026-10-16), with the special words.",
        source: include_str!("../prompts/bear-tags-v1.txt"),
    },
    PromptTemplate {
        id: "eval-v0",
        description: "The 2026-10-14 seed-bench prompt, kept as a baseline for comparison.",
        source: include_str!("../prompts/eval-v0.txt"),
    },
];

/// Looks up a built-in template by id.
pub fn template(id: &str) -> Option<&'static PromptTemplate> {
    TEMPLATES.iter().find(|t| t.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// How much previous text the prompt carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptOptions {
    /// The context is cut to at most this many characters from its end, at a
    /// paragraph or sentence boundary. Roughly the last two paragraphs.
    pub max_context_chars: usize,
}

impl Default for PromptOptions {
    fn default() -> Self {
        Self { max_context_chars: 1200 }
    }
}

impl PromptTemplate {
    /// The template's messages before substitution, in order, trimmed of
    /// blank lines (except leading ones in an example answer).
    pub fn sections(&self) -> Vec<(Role, &'static str)> {
        let mut out = Vec::new();
        let mut current: Option<(Role, usize)> = None;
        let mut offset = 0;
        for line in self.source.split_inclusive('\n') {
            if let Some(role) = marker_role(line.trim_end()) {
                if let Some((r, start)) = current {
                    out.push((r, section_text(r, &self.source[start..offset])));
                }
                current = Some((role, offset + line.len()));
            }
            offset += line.len();
        }
        if let Some((r, start)) = current {
            out.push((r, section_text(r, &self.source[start..])));
        }
        out
    }

    /// Builds the chat messages for one request.
    pub fn render(&self, request: &CleanupRequest<'_>, options: &PromptOptions) -> Vec<ChatMessage> {
        let context = trim_context(request.context, options.max_context_chars);
        let context_raw = context.trim();
        let context = if context_raw.is_empty() { NO_CONTEXT } else { context };
        let names = request.names.join(", ");
        let rule = style_rule(request.style);
        let (open, close) = request.style.quotes.marks();
        let (open, close) = (open.to_string(), close.to_string());
        let values = [
            ("style_rule", rule.as_str()),
            ("context", context),
            ("raw", request.raw),
            ("open", open.as_str()),
            ("close", close.as_str()),
            ("style", style_tag(request.style)),
            ("context_raw", context_raw),
            ("names", names.as_str()),
        ];
        self.sections()
            .into_iter()
            .map(|(role, text)| ChatMessage { role, content: substitute(text, &values) })
            .collect()
    }
}

/// Trailing newlines never count. Leading ones do in an example answer,
/// where they mean "this answer starts a new paragraph".
fn section_text(role: Role, text: &str) -> &str {
    let text = text.trim_end_matches('\n');
    if role == Role::Assistant { text } else { text.trim_start_matches('\n') }
}

fn marker_role(line: &str) -> Option<Role> {
    match line {
        "=== SYSTEM ===" => Some(Role::System),
        "=== USER ===" => Some(Role::User),
        "=== ASSISTANT ===" => Some(Role::Assistant),
        _ => None,
    }
}

/// The style for `{style}`: `uk` (single marks, the Bear fine-tunes'
/// British style) or `us` (double).
pub fn style_tag(style: &Style) -> &'static str {
    match style.quotes {
        QuoteStyle::CurlySingle | QuoteStyle::StraightSingle => "uk",
        QuoteStyle::CurlyDouble | QuoteStyle::Straight => "us",
    }
}

/// The style sentence for `{style_rule}`.
pub fn style_rule(style: &Style) -> String {
    let (open, close) = style.quotes.marks();
    let kind = match style.quotes {
        QuoteStyle::CurlyDouble => "curly double quotation marks",
        QuoteStyle::CurlySingle => "curly single quotation marks",
        QuoteStyle::Straight => "straight double quotation marks",
        QuoteStyle::StraightSingle => "straight single quotation marks",
    };
    let placement = match style.punctuation {
        PunctuationPlacement::Inside => format!(
            "A comma or period that ends a quoted line always goes inside the closing mark: {open}To the river,{close} he said."
        ),
        PunctuationPlacement::Logical => format!(
            "A comma or period goes inside the closing mark only when it belongs to the speech, as with a dialog tag: {open}To the river,{close} he said. A quoted fragment keeps it outside: she called it {open}the end{close}."
        ),
    };
    let topic = if style.topic_paragraphs {
        " Also start a new paragraph where the narration moves on, as a novelist would: a new topic, place, time or focus."
    } else {
        ""
    };
    // Only the non-default choice adds words, so the usual prompt stays the
    // one the eval measured.
    let run_on = if style.close_continued_speech {
        format!(" When one character's speech runs on into a new paragraph, close it with {close} at the end of each paragraph and open the next with {open}.")
    } else {
        String::new()
    };
    format!(
        "Quote dialog with {kind}: {open} to open and {close} to close. Write apostrophes as plain ' (don't, Mara's). {placement}{topic}{run_on}"
    )
}

/// The tail of `context`, at most `max_chars` characters, starting at a
/// paragraph or sentence boundary when it had to be cut.
pub fn trim_context(context: &str, max_chars: usize) -> &str {
    let total = context.chars().count();
    if total <= max_chars {
        return context;
    }
    let start = context
        .char_indices()
        .nth(total - max_chars)
        .map_or(context.len(), |(i, _)| i);
    let tail = &context[start..];
    let cut = tail
        .find("\n\n")
        .map(|i| i + 2)
        .or_else(|| tail.find(". ").map(|i| i + 2))
        .unwrap_or(0);
    tail[cut..].trim_start()
}

/// Replaces `{name}` placeholders in one pass, so a value containing
/// `{raw}` is never expanded again.
fn substitute(text: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let hit = values.iter().find_map(|(name, value)| {
            after
                .strip_prefix(name)
                .and_then(|r| r.strip_prefix('}'))
                .map(|r| (value, r))
        });
        match hit {
            Some((value, r)) => {
                out.push_str(value);
                rest = r;
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(context: &'a str, raw: &'a str, style: &'a Style) -> CleanupRequest<'a> {
        CleanupRequest { context, raw, style, names: &[] }
    }

    #[test]
    fn every_template_parses_and_uses_its_placeholders() {
        for t in TEMPLATES {
            let sections = t.sections();
            let (last_role, last) = sections.last().copied().unwrap();
            assert_eq!(last_role, Role::User, "{}", t.id);
            // A bare-tag template is one user turn with the raw context.
            let bare = sections.len() == 1;
            if bare {
                assert!(last.contains("{raw}") && last.contains("{context_raw}"), "{}", t.id);
            } else {
                assert_eq!(sections.first().map(|s| s.0), Some(Role::System), "{}", t.id);
                assert!(last.contains("{raw}") && last.contains("{context}"), "{}", t.id);
            }
            for (k, (role, text)) in sections.iter().enumerate() {
                assert!(!text.is_empty() && !text.contains("=== "), "{} section {k}", t.id);
                if k > 0 && k + 1 < sections.len() {
                    // Example turns alternate user, assistant.
                    let want = if k % 2 == 1 { Role::User } else { Role::Assistant };
                    assert_eq!(*role, want, "{} section {k}", t.id);
                    assert!(!text.contains("{raw}"), "{} section {k}", t.id);
                }
            }
            let msgs = t.render(&request("", "RAW", &Style::default()), &PromptOptions::default());
            for m in &msgs {
                for p in ["{raw}", "{style_rule}", "{context}", "{open}", "{close}", "{style}", "{context_raw}", "{names}"] {
                    assert!(!m.content.contains(p), "{} leaves {p}", t.id);
                }
            }
            let last = msgs.last().unwrap();
            assert!(last.content.contains("RAW"));
            assert_eq!(last.content.contains(NO_CONTEXT), !bare, "{}", t.id);
        }
        assert_eq!(template(DEFAULT_TEMPLATE).map(|t| t.id), Some(DEFAULT_TEMPLATE));
    }

    #[test]
    fn bear_tags_render_exactly_as_trained() {
        let t = template("bear-tags-v1").unwrap();
        let names = ["Teskar".to_string(), "Port Varrow".to_string()];
        let req = CleanupRequest { context: "", raw: "um hello", style: &Style::default(), names: &names };
        let msgs = t.render(&req, &PromptOptions::default());
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(
            msgs[0].content,
            "<style>us</style>\n<context>\n\n</context>\n<names>Teskar, Port Varrow</names>\n<dictation>\num hello\n</dictation>"
        );
        let uk = Style { quotes: QuoteStyle::CurlySingle, ..Style::default() };
        let req = CleanupRequest { context: "Before. ", raw: "x", style: &uk, names: &[] };
        let msgs = t.render(&req, &PromptOptions::default());
        assert!(msgs[0].content.starts_with("<style>uk</style>\n<context>\nBefore.\n</context>\n<names></names>"));
    }

    #[test]
    fn topic_paragraphs_add_a_sentence_only_when_on() {
        assert!(!style_rule(&Style::default()).contains("narration moves on"));
        assert!(!style_rule(&Style::default()).contains("runs on"));
        let closed = Style { close_continued_speech: true, ..Style::default() };
        assert!(style_rule(&closed).ends_with("open the next with \u{201C}."));
        let topic = Style { topic_paragraphs: true, ..Style::default() };
        assert!(style_rule(&topic).ends_with("a new topic, place, time or focus."));
    }

    #[test]
    fn style_rule_follows_style() {
        let uk = Style { quotes: QuoteStyle::CurlySingle, punctuation: PunctuationPlacement::Logical, ..Style::default() };
        let msgs = template("generic-v1").unwrap().render(&request("x", "y", &uk), &PromptOptions::default());
        assert!(msgs[0].content.contains("curly single") && msgs[0].content.contains('‘'));
        assert!(!msgs[0].content.contains('“'), "US marks leak into a UK prompt");
        assert!(style_rule(&Style::default()).contains("“To the river,” he said."));
    }

    #[test]
    fn substitution_is_single_pass() {
        let msgs = template("generic-v1")
            .unwrap()
            .render(&request("{raw}", "a {context} b", &Style::default()), &PromptOptions::default());
        let last = &msgs.last().unwrap().content;
        assert!(last.contains("{raw}\n"));
        assert!(last.ends_with("a {context} b"));
    }

    #[test]
    fn context_is_trimmed_at_a_boundary() {
        assert_eq!(trim_context("short", 100), "short");
        let ctx = "First para here.\n\nSecond para. Third sentence.";
        assert_eq!(trim_context(ctx, 35), "Second para. Third sentence.");
        assert_eq!(trim_context(ctx, 20), "Third sentence.");
        assert_eq!(trim_context("ééééé", 2), "éé");
    }
}
