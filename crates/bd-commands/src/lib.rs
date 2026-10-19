//! Deterministic spoken-command pass, run on ASR text before the cleanup
//! model.
//!
//! Commands are matched by rules, not left to the model, so they are always
//! applied the same way:
//!
//! | Spoken | Effect |
//! |---|---|
//! | `open quote`, `open quotes`, `begin quote` | opening mark from [`Style::quotes`] |
//! | `close quote(s)`, `end quote(s)`, `end of quote`, `unquote` | closing mark |
//! | `quote … end quote` | a bare `quote` opens when a closing command follows it |
//! | `new paragraph` / `new line` | `"\n\n"` / `"\n"` |
//! | `scratch that` | drops what came before it (see [`ActionKind`]) |
//! | `comma`, `period`, `question mark`, … | literal marks, only with [`CommandConfig::literal_punctuation`] |
//!
//! The recognizer punctuates and capitalizes what it hears, so a command often
//! arrives as `Open quote.` or `, new paragraph,`. The pass strips the marks
//! the ASR attached to command words. The one exception is a sentence end the
//! ASR put on `close quote` or `new paragraph`, which moves onto the preceding
//! word, where the sentence actually ended.
//!
//! The same words occur in prose ("the new paragraph began", "a new line of
//! work", "to scratch that itch"), so each command also has to pass context
//! checks: no article or possessive just before it, no noun continuation
//! after it, and, for `scratch that`, a clause of its own.

mod render;
mod rules;
mod token;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_prose;

use bd_core::Style;

pub use render::{Action, ActionKind};

/// Which optional commands are recognized.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CommandConfig {
    /// Obey spoken `new paragraph` and `new line`. On by default; off, they
    /// are typed as the words they are.
    pub break_commands: bool,
    /// Treat spoken `comma`, `period`, `question mark`, `exclamation point`,
    /// `colon`, `semicolon`, `em dash` and `ellipsis` as marks. Off by
    /// default: the ASR already punctuates, and these words are common in
    /// prose ("a period of calm", "a question mark hung over it").
    pub literal_punctuation: bool,
}

impl Default for CommandConfig {
    fn default() -> Self {
        Self { break_commands: true, literal_punctuation: false }
    }
}

/// How many of the last `words` (as the ASR wrote them) to hold back while
/// dictation streams in: the trailing run that could still turn out to be a
/// spoken command once the next word arrives. 0 when none could.
pub fn command_tail(words: &[&str], config: &CommandConfig) -> usize {
    let plain: Vec<String> = words
        .iter()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .collect();
    let plain: Vec<&str> = plain.iter().map(String::as_str).collect();
    (1..=plain.len().min(3))
        .rev()
        .find(|&n| rules::may_start_command(&plain[plain.len() - n..], config.literal_punctuation))
        .unwrap_or(0)
}

/// The result of running the pass over one segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// The segment with commands applied: quote marks inserted, breaks as
    /// `"\n\n"` / `"\n"`, command words removed. May begin or end with a
    /// break, and may be empty (for example a lone "Scratch that.").
    pub text: String,
    /// Every command applied, in order. Offsets are byte offsets into
    /// [`CommandOutput::text`].
    pub actions: Vec<Action>,
    /// Whether a commanded quotation is still open at the end of the segment.
    /// Feed it back as `quote_open` for the next segment.
    pub quote_open: bool,
}

impl CommandOutput {
    /// How many earlier, not-yet-typed segments the caller should discard:
    /// one for each `scratch that` that opened this segment.
    pub fn scratch_previous(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| a.kind == ActionKind::ScratchPreviousSegment)
            .count()
    }

    /// True when the text has at least one word in it, so the cleanup model
    /// has something to do.
    pub fn has_words(&self) -> bool {
        self.text.chars().any(char::is_alphanumeric)
    }
}

/// The command pass, configured once per session.
#[derive(Debug, Clone, Default)]
pub struct CommandPass {
    pub config: CommandConfig,
    pub style: Style,
}

impl CommandPass {
    pub fn new(config: CommandConfig, style: Style) -> Self {
        Self { config, style }
    }

    /// Applies spoken commands to one segment of ASR text.
    ///
    /// `quote_open` says whether a commanded quotation was left open by the
    /// previous segment. It decides whether a closing command here can pair
    /// with a bare `quote` in this segment. After a
    /// [`ActionKind::ScratchPreviousSegment`], pass the state from before the
    /// discarded segment.
    pub fn apply(&self, asr_text: &str, quote_open: bool) -> CommandOutput {
        let toks = token::tokenize(asr_text, rules::is_command_phrase);
        let mut items: Vec<render::Item> = Vec::new();
        let mut leading: Vec<Action> = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let after_command = matches!(items.last(), Some(render::Item::Cmd { .. }));
            let matched = rules::match_at(&toks, i, self.config.literal_punctuation, after_command)
                .filter(|m| self.config.break_commands || !matches!(m.cmd, rules::Cmd::NewParagraph | rules::Cmd::NewLine));
            if let Some(m) = matched {
                let last = &toks[i + m.len - 1];
                if m.cmd == rules::Cmd::Scratch {
                    leading.push(self.scratch(&mut items));
                } else {
                    items.push(render::Item::Cmd { cmd: m.cmd, trail: last.trail.clone() });
                }
                i += m.len;
                continue;
            }
            items.push(render::Item::Word(toks[i].clone()));
            i += 1;
        }
        resolve_bare_quotes(&mut items, quote_open);
        let (text, mut actions) = render::render(&items, &self.style, quote_open);
        let mut open = quote_open;
        for a in &actions {
            match a.kind {
                ActionKind::OpenQuote => open = true,
                ActionKind::CloseQuote => open = false,
                _ => {}
            }
        }
        leading.append(&mut actions);
        CommandOutput { text, actions: leading, quote_open: open }
    }

    /// Handles one `scratch that`: drops this segment's words so far, or,
    /// when there are none, asks the caller to drop the previous segment.
    fn scratch(&self, items: &mut Vec<render::Item>) -> Action {
        if items.is_empty() {
            return Action { kind: ActionKind::ScratchPreviousSegment, offset: 0 };
        }
        let (discarded, _) = render::render(items, &self.style, false);
        items.clear();
        Action { kind: ActionKind::ScratchWithinSegment { discarded }, offset: 0 }
    }
}

/// Turns a bare `quote` into an opening command when a closing command
/// follows it with no opening command in between: `quote get down end
/// quote`. The nearest eligible `quote` before the closer wins, so "to quote
/// my father, quote, never again, end quote" opens at the second one.
fn resolve_bare_quotes(items: &mut [render::Item], quote_open: bool) {
    use render::Item;
    use rules::Cmd;
    let mut window_start = 0;
    let mut open = quote_open;
    for k in 0..items.len() {
        match &items[k] {
            Item::Cmd { cmd: Cmd::OpenQuote, .. } => open = true,
            Item::Cmd { cmd: Cmd::CloseQuote, .. } => {
                if !open && let Some(q) = nearest_bare_quote(items, window_start, k) {
                    items[q] = Item::Cmd { cmd: Cmd::OpenQuote, trail: String::new() };
                }
                open = false;
                window_start = k + 1;
            }
            _ => {}
        }
    }
}

fn nearest_bare_quote(items: &[render::Item], from: usize, to: usize) -> Option<usize> {
    use render::Item;
    (from..to).rev().find(|&q| {
        let Item::Word(tok) = &items[q] else { return false };
        let prev = q.checked_sub(1).and_then(|p| match &items[p] {
            Item::Word(t) => Some(t),
            Item::Cmd { .. } => None,
        });
        let next_is_unquote = matches!(items.get(q + 1), Some(Item::Word(t)) if t.lower == "unquote");
        rules::can_open_bare_quote(tok, prev) && !next_is_unquote
    })
}
