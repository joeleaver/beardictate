//! The worker thread's side: one model, one context, prefix-cached
//! generation.

use std::num::NonZeroU32;
use std::sync::mpsc;
use std::time::Instant;

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend as Backend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LlamaStateSeqFlags, SeqState};

use super::turns::Turns;
use super::{GpuLayers, Job, LlamaConfig, ModelInfo, Sampling, default_threads};
use crate::{ChatMessage, CleanupError, Completion, GenerationStats};

/// What Qwen3's own template emits for `enable_thinking=false`: an empty
/// think block pre-filled into the assistant turn. llama.cpp's built-in
/// template engine has no kwargs, so we append it ourselves.
const EMPTY_THINK: &str = "<think>\n\n</think>\n\n";

fn llama_err(context: &str) -> impl Fn(&dyn std::fmt::Display) -> CleanupError + '_ {
    move |e| CleanupError::Llama(format!("{context}: {e}"))
}

/// Why [`run`] stopped before the model was ready.
enum LoadError {
    /// Loading the weights or creating the context failed: on the GPU, often
    /// for want of memory, so worth a retry on the CPU.
    Placement(CleanupError),
    /// Anything else (the chat template); the CPU would fail the same way.
    Other(CleanupError),
}

/// Loads the model, reports readiness, then answers jobs until the channel
/// closes. Runs on the worker thread; everything is freed on return. A model
/// that doesn't fit on the GPU loads again on the CPU.
pub(super) fn serve(
    backend: &'static Backend,
    config: &LlamaConfig,
    ready: &mpsc::Sender<Result<ModelInfo, CleanupError>>,
    jobs: &mpsc::Receiver<Job>,
) {
    let gpu_layers = match config.gpu_layers {
        GpuLayers::Auto if backend.supports_gpu_offload() => u32::MAX,
        GpuLayers::Auto | GpuLayers::None => 0,
        GpuLayers::Count(n) => n,
    };
    let result = match run(backend, config, gpu_layers, ready, jobs) {
        Err(LoadError::Placement(e)) if gpu_layers > 0 => {
            tracing::warn!("{e}; loading the cleanup model on the CPU instead");
            run(backend, config, 0, ready, jobs)
        }
        other => other,
    };
    if let Err(LoadError::Placement(e) | LoadError::Other(e)) = result {
        let _ = ready.send(Err(e));
    }
}

/// [`serve`] with a fixed GPU layer count. Returns an error only before
/// readiness was reported.
fn run(
    backend: &'static Backend,
    config: &LlamaConfig,
    gpu_layers: u32,
    ready: &mpsc::Sender<Result<ModelInfo, CleanupError>>,
    jobs: &mpsc::Receiver<Job>,
) -> Result<(), LoadError> {
    let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    let model = LlamaModel::load_from_file(backend, &config.model_path, &params)
        .map_err(|e| LoadError::Placement(llama_err("loading model")(&e)))?;
    let threads = config.threads.unwrap_or_else(default_threads);
    let threads_i32 = i32::try_from(threads).unwrap_or(4);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(config.n_ctx))
        .with_n_batch(config.n_batch)
        .with_n_ubatch(config.n_batch)
        .with_n_threads(threads_i32)
        .with_n_threads_batch(threads_i32);
    let ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| LoadError::Placement(llama_err("creating context")(&e)))?;
    let template_source = model
        .chat_template(None)
        .ok()
        .and_then(|t| t.to_string().ok());
    let fallback_template = template_source.is_none();
    let turns = template_source.as_deref().and_then(Turns::detect);
    let template = model
        .chat_template(None)
        .or_else(|_| LlamaChatTemplate::new("chatml"))
        .map_err(|e| llama_err("chat template")(&e));
    let template = template.map_err(LoadError::Other)?;
    // Qwen-style templates think unless told not to, with `<think>` blocks.
    // Others (Gemma 4: `<|channel>thought`, off by default) need nothing.
    let suppress_thinking =
        template_source.as_deref().is_some_and(|s| s.contains("enable_thinking") && s.contains("<think>"));
    let info = ModelInfo {
        path: config.model_path.clone(),
        n_params: model.n_params(),
        size_bytes: model.size(),
        n_layer: model.n_layer(),
        gpu_layers: gpu_layers.min(model.n_layer()),
        n_ctx: ctx.n_ctx(),
        threads,
        thinking_suppressed: suppress_thinking,
        fallback_template,
    };
    let mut engine = Engine {
        model: &model,
        ctx,
        template,
        suppress_thinking,
        turns,
        n_batch: config.n_batch as usize,
        sampling: config.sampling,
        cached: Vec::new(),
        recurrent: model.is_recurrent() || model.is_hybrid(),
        checkpoint: None,
    };
    if ready.send(Ok(info)).is_err() {
        return Ok(());
    }
    while let Ok(job) = jobs.recv() {
        let result = engine.complete(&job.messages, job.max_tokens);
        let _ = job.reply.send(result);
    }
    Ok(())
}

struct Engine<'m> {
    model: &'m LlamaModel,
    ctx: LlamaContext<'m>,
    template: LlamaChatTemplate,
    suppress_thinking: bool,
    /// Rendered here: llama.cpp can't apply this model's template.
    turns: Option<Turns>,
    n_batch: usize,
    sampling: Sampling,
    /// Tokens whose keys and values are in the KV cache, in order.
    cached: Vec<LlamaToken>,
    /// Recurrent layers (Qwen3.5's gated delta net) keep one running state,
    /// not one entry per token, so the cache can't be cut back to a prefix.
    recurrent: bool,
    /// For recurrent models: that state just after `cached[..len]`, taken
    /// before the last message, so the next request starts from there.
    checkpoint: Option<Checkpoint>,
}

struct Checkpoint {
    len: usize,
    state: SeqState,
}

impl Engine<'_> {
    fn complete(&mut self, messages: &[ChatMessage], max_tokens: usize) -> Result<Completion, CleanupError> {
        let prompt = self.render(messages)?;
        let vocab = self.model.vocab();
        let bos_text = String::from_utf8_lossy(&vocab.token_to_piece(vocab.bos(), true, None)).into_owned();
        let add_bos = vocab.should_add_bos() && (bos_text.is_empty() || !prompt.starts_with(&bos_text));
        let tokens = vocab.tokenize(prompt.as_bytes(), add_bos, true);
        let n_ctx = self.ctx.n_ctx() as usize;
        if tokens.is_empty() || tokens.len() + max_tokens > n_ctx {
            return Err(CleanupError::ContextOverflow { prompt: tokens.len(), budget: max_tokens, n_ctx });
        }
        let split = if self.recurrent { self.last_message_start(&prompt, messages, &tokens, add_bos) } else { None };
        let result = self.generate(&tokens, split, max_tokens);
        if result.is_err() {
            // The cache may hold a half-decoded batch; start clean next time.
            self.ctx.clear_kv_cache();
            self.cached.clear();
            self.checkpoint = None;
        }
        result
    }

    fn render(&self, messages: &[ChatMessage]) -> Result<String, CleanupError> {
        if let Some(turns) = self.turns {
            let prompt = turns.render(messages);
            tracing::trace!(%prompt, "rendered prompt");
            return Ok(prompt);
        }
        let chat = messages
            .iter()
            .map(|m| LlamaChatMessage::new(m.role.as_str().to_string(), m.content.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| llama_err("chat message")(&e))?;
        let mut prompt = self
            .model
            .apply_chat_template(&self.template, &chat, true)
            .map_err(|e| llama_err("applying chat template")(&e))?;
        if self.suppress_thinking {
            prompt.push_str(EMPTY_THINK);
        }
        tracing::trace!(%prompt, "rendered prompt");
        Ok(prompt)
    }

    /// How many of `tokens` come before the last message's text: the part
    /// the next request will share, unless the system prompt changes.
    fn last_message_start(
        &self,
        prompt: &str,
        messages: &[ChatMessage],
        tokens: &[LlamaToken],
        add_bos: bool,
    ) -> Option<usize> {
        let last = messages.last().filter(|m| !m.content.is_empty())?;
        let start = prompt.rfind(&last.content)?;
        let head = self.model.vocab().tokenize(&prompt.as_bytes()[..start], add_bos, true);
        // The last token of the head may merge with the text after it.
        let shared = head.iter().zip(tokens).take_while(|(a, b)| a == b).count();
        (shared > 0).then_some(shared)
    }

    /// Reuses the KV cache for the prompt prefix shared with the previous
    /// request (the system prompt, usually), decodes the rest, then samples
    /// until end of turn or `max_tokens`. For a recurrent model, saves the
    /// state at `split` on the way.
    fn generate(
        &mut self,
        tokens: &[LlamaToken],
        split: Option<usize>,
        max_tokens: usize,
    ) -> Result<Completion, CleanupError> {
        let started = Instant::now();
        // At least one prompt token must be decoded to get fresh logits.
        let mut common = self
            .cached
            .iter()
            .zip(tokens)
            .take_while(|(a, b)| a == b)
            .count()
            .min(tokens.len() - 1);
        if common < self.cached.len() {
            common = self.rewind(common);
        }
        let save_at = split.filter(|&s| s > common && s < tokens.len());
        let mut batch = LlamaBatch::new(self.n_batch.max(1), 1);
        let mut last_index = 0;
        let mut from = common;
        for to in save_at.into_iter().chain([tokens.len()]) {
            last_index = self.prefill(&mut batch, tokens, from, to)?;
            if to < tokens.len() {
                self.save_checkpoint(to);
            }
            from = to;
        }
        self.cached.truncate(common);
        self.cached.extend_from_slice(&tokens[common..]);
        let prefill = started.elapsed();

        let decode_started = Instant::now();
        let vocab = self.model.vocab();
        let mut sampler = if self.sampling.temperature > 0.0 {
            LlamaSampler::chain_simple([
                LlamaSampler::temp(self.sampling.temperature),
                LlamaSampler::dist(self.sampling.seed),
            ])
        } else {
            LlamaSampler::greedy()
        };
        let mut bytes = Vec::new();
        let mut generated = 0;
        let mut logits_index = last_index;
        while generated < max_tokens {
            let token = sampler.sample(&self.ctx, logits_index);
            if vocab.is_eog(token) {
                break;
            }
            bytes.extend(vocab.token_to_piece(token, false, None));
            generated += 1;
            batch.clear();
            let pos = self.cached.len();
            batch
                .add(token, pos_i32(pos), &[0], true)
                .map_err(|e| llama_err("building batch")(&e))?;
            self.ctx.decode(&mut batch).map_err(|e| llama_err("decoding")(&e))?;
            self.cached.push(token);
            logits_index = 0;
        }
        let stats = GenerationStats {
            prompt_tokens: tokens.len(),
            reused_prompt_tokens: common,
            generated_tokens: generated,
            prefill,
            decode: decode_started.elapsed(),
        };
        Ok(Completion { text: String::from_utf8_lossy(&bytes).into_owned(), stats: Some(stats) })
    }
}

impl Engine<'_> {
    /// Cuts the cache back to `common` tokens, or as near as it can get:
    /// a recurrent model goes back to its checkpoint, or to nothing.
    fn rewind(&mut self, common: usize) -> usize {
        let from = u32::try_from(common).unwrap_or(u32::MAX);
        if matches!(self.ctx.clear_kv_cache_seq(Some(0), Some(from), None), Ok(true)) {
            self.cached.truncate(common);
            return common;
        }
        if self.checkpoint.as_ref().is_some_and(|c| c.len > common) {
            self.checkpoint = None;
        }
        if let Some(checkpoint) = &self.checkpoint {
            let len = checkpoint.len;
            let at = u32::try_from(len).unwrap_or(u32::MAX);
            // Restores the recurrent state only; the attention layers' cache
            // still holds the prefix, and loses what came after it.
            if self.ctx.state_seq_set(&checkpoint.state, 0).is_ok()
                && matches!(self.ctx.clear_kv_cache_seq(Some(0), Some(at), None), Ok(true))
            {
                self.cached.truncate(len);
                return len;
            }
            self.checkpoint = None;
        }
        tracing::debug!(common, cached = self.cached.len(), "can't cut the cache back; decoding the whole prompt");
        self.ctx.clear_kv_cache();
        self.cached.clear();
        0
    }

    /// Decodes `tokens[from..to]`, asking for logits only at the prompt's
    /// last token. Returns the batch index of the last token decoded.
    fn prefill(&mut self, batch: &mut LlamaBatch, tokens: &[LlamaToken], from: usize, to: usize) -> Result<i32, CleanupError> {
        let mut last_index = 0;
        for (chunk_no, chunk) in tokens[from..to].chunks(self.n_batch.max(1)).enumerate() {
            batch.clear();
            let base = from + chunk_no * self.n_batch.max(1);
            for (k, &token) in chunk.iter().enumerate() {
                let pos = base + k;
                let is_last = pos == tokens.len() - 1;
                batch
                    .add(token, pos_i32(pos), &[0], is_last)
                    .map_err(|e| llama_err("building batch")(&e))?;
            }
            self.ctx.decode(batch).map_err(|e| llama_err("decoding prompt")(&e))?;
            last_index = batch.n_tokens() - 1;
        }
        Ok(last_index)
    }

    /// Saves the recurrent state after the first `len` prompt tokens.
    fn save_checkpoint(&mut self, len: usize) {
        match self.ctx.state_seq_get(0, LlamaStateSeqFlags::PARTIAL_ONLY) {
            Ok(state) => {
                tracing::debug!(len, bytes = state.byte_len(), "saved recurrent state");
                self.checkpoint = Some(Checkpoint { len, state });
            }
            Err(e) => {
                tracing::debug!(%e, "couldn't save recurrent state");
                self.checkpoint = None;
            }
        }
    }
}

fn pos_i32(pos: usize) -> i32 {
    i32::try_from(pos).unwrap_or(i32::MAX)
}
