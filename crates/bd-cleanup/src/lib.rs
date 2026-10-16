//! Cleanup-model backends and the word-diff guardrail.
//!
//! A backend ([`ChatBackend`]) only turns chat messages into a completion.
//! [`GuardedCleanup`] wraps any backend into a [`bd_core::Cleanup`]: it
//! renders the versioned [`prompt`] template, calls the backend, tidies the
//! answer (straight quotes become the style's marks, see [`quotes`]), and
//! runs the [`guardrail`]. If the model changed anything other
//! than fillers, repeats, casing and punctuation, the dictated words are put
//! back where it swapped them one for one ([`guardrail::repair`]);
//! otherwise the segment falls back to its input (spoken commands already
//! applied). The [`GuardrailVerdict`] says why. A misheard special word
//! the model respelled is allowed where [`respell`] finds a span that sounds
//! like it.
//!
//! Backends:
//! - [`LlamaChat`] (feature `llama`, on by default): llama.cpp in process,
//!   model resident on a worker thread. Feature `vulkan` adds the GPU
//!   backend and offloads every layer. [`LlamaCleanup`] is it behind the
//!   guardrail.
//! - [`HttpChat`]: any OpenAI-compatible `/v1/chat/completions`.
//!   [`HttpCleanup`] is it behind the guardrail.

pub mod guardrail;
mod http;
#[cfg(feature = "llama")]
mod llama;
pub mod prompt;
pub mod quotes;
pub mod respell;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bd_core::{BoxError, Cleanup, CleanupRequest};

pub use guardrail::{GuardrailVerdict, Rejection, Removal, RemovalReason, Repair};
pub use http::{HttpChat, HttpConfig};
#[cfg(feature = "llama")]
pub use llama::{GpuLayers, LlamaChat, LlamaConfig, ModelInfo, Sampling, shared_backend};
pub use prompt::{ChatMessage, PromptOptions, PromptTemplate, Role};

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("unknown prompt template {0:?}")]
    UnknownTemplate(String),
    #[error("model file not found: {0}")]
    ModelNotFound(PathBuf),
    #[error("llama.cpp: {0}")]
    Llama(String),
    #[error("prompt of {prompt} tokens plus {budget} to generate does not fit the {n_ctx}-token context")]
    ContextOverflow { prompt: usize, budget: usize, n_ctx: usize },
    #[error("the cleanup model's worker thread has stopped")]
    WorkerGone,
    #[error("request to {url} failed: {message}")]
    Http { url: String, message: String },
    #[error("unexpected response from {url}: {message}")]
    Protocol { url: String, message: String },
}

/// Counters from one generation, when the backend reports them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerationStats {
    pub prompt_tokens: usize,
    /// Prompt tokens whose KV cache was reused from the previous request
    /// (the shared system-prompt prefix).
    pub reused_prompt_tokens: usize,
    pub generated_tokens: usize,
    pub prefill: Duration,
    pub decode: Duration,
}

/// A backend's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub text: String,
    pub stats: Option<GenerationStats>,
}

/// Something that completes a chat: a local model or a server.
pub trait ChatBackend: Send {
    /// Generates the assistant's reply to `messages`, at most `max_tokens`
    /// tokens, stopping at end of turn. No thinking/reasoning output.
    fn complete(&mut self, messages: &[ChatMessage], max_tokens: usize) -> Result<Completion, CleanupError>;
}

/// In-process llama.cpp cleanup with the guardrail.
#[cfg(feature = "llama")]
pub type LlamaCleanup = GuardedCleanup<LlamaChat>;

/// OpenAI-compatible HTTP cleanup with the guardrail.
pub type HttpCleanup = GuardedCleanup<HttpChat>;

/// Everything known about one cleaned segment.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupOutcome {
    /// What to type: the model's text if accepted, the repaired text if it
    /// was rejected but [`guardrail::repair`] could fix it, otherwise the
    /// input.
    pub text: String,
    /// The model's tidied answer, before the guardrail. `None` when the model
    /// was not called (a segment with no words).
    pub model_output: Option<String>,
    /// `None` when the model was not called.
    pub verdict: Option<GuardrailVerdict>,
    /// When the verdict was a rejection: the repair that was used instead.
    pub repair: Option<Repair>,
    /// Misheard special words the model was allowed to respell.
    pub respelled: Vec<respell::Respelling>,
    pub stats: Option<GenerationStats>,
    pub elapsed: Duration,
}

impl CleanupOutcome {
    /// True when the guardrail rejected the model, no repair was possible,
    /// and the input was used.
    pub fn fell_back(&self) -> bool {
        matches!(self.verdict, Some(GuardrailVerdict::Rejected(_))) && self.repair.is_none()
    }
}

/// A [`ChatBackend`] behind the prompt template and the guardrail.
pub struct GuardedCleanup<B> {
    backend: B,
    template: &'static PromptTemplate,
    options: PromptOptions,
    last: Option<CleanupOutcome>,
}

impl<B: ChatBackend> GuardedCleanup<B> {
    /// Wraps `backend` with the template named `template_id`.
    pub fn new(backend: B, template_id: &str) -> Result<Self, CleanupError> {
        let template = prompt::template(template_id).ok_or_else(|| CleanupError::UnknownTemplate(template_id.into()))?;
        Ok(Self { backend, template, options: PromptOptions::default(), last: None })
    }

    pub fn with_options(mut self, options: PromptOptions) -> Self {
        self.options = options;
        self
    }

    /// Switches to another template, keeping the backend (and a loaded
    /// model) as is.
    pub fn set_template(&mut self, template_id: &str) -> Result<(), CleanupError> {
        self.template = prompt::template(template_id).ok_or_else(|| CleanupError::UnknownTemplate(template_id.into()))?;
        Ok(())
    }

    pub fn template(&self) -> &'static PromptTemplate {
        self.template
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// The outcome of the most recent [`Cleanup::clean`] call, so a caller
    /// holding the concrete type can report a guardrail fallback.
    pub fn last_outcome(&self) -> Option<&CleanupOutcome> {
        self.last.as_ref()
    }

    /// Cleans one segment and reports everything that happened.
    pub fn clean_detailed(&mut self, request: &CleanupRequest<'_>) -> Result<CleanupOutcome, CleanupError> {
        let started = Instant::now();
        if !request.raw.chars().any(char::is_alphanumeric) {
            return Ok(CleanupOutcome {
                text: request.raw.to_string(),
                model_output: None,
                verdict: None,
                repair: None,
                respelled: Vec::new(),
                stats: None,
                elapsed: started.elapsed(),
            });
        }
        let messages = self.template.render(request, &self.options);
        let completion = self.backend.complete(&messages, max_new_tokens(request.raw))?;
        let output = quotes::smarten(request.context, &finalize(request.raw, &completion.text), request.style);
        let output = quotes::straighten(&output, request.style);
        let output = guardrail::strip_context_echo(request.context, request.raw, &output).into_owned();
        // Checked against the dictation with the allowed name fixes in, when
        // that passes; otherwise against the dictation as it was.
        let (respelled_raw, mut respelled) = respell::respell(request.raw, &output, request.names);
        let mut verdict = guardrail::review_in_context(request.context, &respelled_raw, &output);
        let dictated = if respelled.is_empty() || verdict.is_accepted() {
            respelled_raw.as_str()
        } else {
            respelled.clear();
            verdict = guardrail::review_in_context(request.context, request.raw, &output);
            request.raw
        };
        for r in &respelled {
            tracing::info!(heard = %r.heard, word = %r.word, "cleanup respelled a special word");
        }
        let mut repair = None;
        let text = match &verdict {
            GuardrailVerdict::Accepted { .. } => output.clone(),
            GuardrailVerdict::Rejected(r) => match guardrail::repair(request.context, dictated, &output) {
                Some(fixed) => {
                    let swaps: Vec<String> = fixed.restored.iter().map(|(m, d)| format!("{m} → {d}")).collect();
                    tracing::info!(reason = %r, restored = %swaps.join(", "), "cleanup output repaired: dictated words put back");
                    let text = fixed.text.clone();
                    repair = Some(fixed);
                    text
                }
                None => {
                    tracing::info!(reason = %r, "cleanup output rejected; using the dictated text");
                    request.raw.to_string()
                }
            },
        };
        let text = if request.style.model_paragraphs() || text == request.raw {
            text
        } else {
            guardrail::keep_dictated_breaks(request.context, dictated, &text)
        };
        Ok(CleanupOutcome {
            text,
            model_output: Some(output),
            verdict: Some(verdict),
            repair,
            respelled,
            stats: completion.stats,
            elapsed: started.elapsed(),
        })
    }
}

impl<B: ChatBackend> Cleanup for GuardedCleanup<B> {
    fn clean(&mut self, request: &CleanupRequest<'_>) -> Result<String, BoxError> {
        let outcome = self.clean_detailed(request)?;
        let text = outcome.text.clone();
        self.last = Some(outcome);
        Ok(text)
    }
}

/// Generation budget for a segment: the answer is the input minus
/// hesitations plus marks, so about as many tokens as the input has. Twice
/// the character count over four, plus slack, leaves room without letting a
/// runaway model ramble.
pub fn max_new_tokens(raw: &str) -> usize {
    raw.chars().count() / 2 + 32
}

/// Labels a model may put before its answer, echoing the prompt format.
const ANSWER_LABELS: &[&str] = &["answer:", "cleaned dictation:", "cleaned text:", "output:"];

/// Tidies a raw model answer: drops `<think>` blocks, a wrapping code fence
/// and an echoed answer label, normalizes leading and trailing breaks, and
/// restores any break the input itself starts or ends with (a dictated
/// "new paragraph" at a segment edge).
pub fn finalize(raw: &str, answer: &str) -> String {
    let mut text = strip_think(answer);
    let trimmed = text.trim();
    if let Some(inner) = trimmed.strip_prefix("```").and_then(|t| t.strip_suffix("```")) {
        // Drop the fence's language tag line, if any.
        let inner = match inner.split_once('\n') {
            Some((tag, rest)) if tag.chars().all(char::is_alphanumeric) => rest,
            _ => inner,
        };
        text = inner.trim().to_string();
    }
    let body = text.trim_start_matches([' ', '\t']);
    let lower = body.to_lowercase();
    let body = ANSWER_LABELS
        .iter()
        .find(|l| lower.starts_with(*l))
        .map_or(body, |l| &body[l.len()..]);
    let lead_breaks = body.chars().take_while(|c| c.is_whitespace()).filter(|c| *c == '\n').count();
    let core = unwrap_echo_quotes(raw, body.trim());
    let lead_breaks = lead_breaks.max(edge_breaks(raw.chars()));
    let trail_breaks = edge_breaks(raw.chars().rev());
    let breaks = |n: usize| "\n".repeat(n.min(2));
    if core.is_empty() {
        return breaks(lead_breaks.max(trail_breaks));
    }
    format!("{}{}{}", breaks(lead_breaks), core, breaks(trail_breaks))
}

/// Drops a pair of straight double quotes wrapped around the whole answer
/// when the input had none: models echo their answer as a quoted string.
/// Real dialog is quoted with the style's marks, which this leaves alone.
fn unwrap_echo_quotes<'a>(raw: &str, core: &'a str) -> &'a str {
    if raw.contains('"') || core.matches('"').count() != 2 {
        return core;
    }
    core.strip_prefix('"').and_then(|c| c.strip_suffix('"')).map_or(core, str::trim)
}

/// Newlines in the whitespace at one edge of `text`.
fn edge_breaks(chars: impl Iterator<Item = char>) -> usize {
    chars.take_while(|c| c.is_whitespace()).filter(|c| *c == '\n').count()
}

/// Removes `<think>…</think>` blocks and the line break the chat template
/// puts after one, including an unterminated block.
fn strip_think(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            // Templates put "\n\n" after the block; that is not a paragraph.
            Some(end) => {
                let after = &rest[start + end + "</think>".len()..];
                rest = after.strip_prefix("\n\n").or_else(|| after.strip_prefix('\n')).unwrap_or(after);
            }
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
