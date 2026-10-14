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

use super::{GpuLayers, Job, LlamaConfig, ModelInfo, Sampling, default_threads};
use crate::{ChatMessage, CleanupError, Completion, GenerationStats};

/// What Qwen3's own template emits for `enable_thinking=false`: an empty
/// think block pre-filled into the assistant turn. llama.cpp's built-in
/// template engine has no kwargs, so we append it ourselves.
const EMPTY_THINK: &str = "<think>\n\n</think>\n\n";

fn llama_err(context: &str) -> impl Fn(&dyn std::fmt::Display) -> CleanupError + '_ {
    move |e| CleanupError::Llama(format!("{context}: {e}"))
}

/// Loads the model, reports readiness, then answers jobs until the channel
/// closes. Runs on the worker thread; everything is freed on return.
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
    let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    let model = match LlamaModel::load_from_file(backend, &config.model_path, &params) {
        Ok(model) => model,
        Err(e) => {
            let _ = ready.send(Err(llama_err("loading model")(&e)));
            return;
        }
    };
    let threads = config.threads.unwrap_or_else(default_threads);
    let threads_i32 = i32::try_from(threads).unwrap_or(4);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(config.n_ctx))
        .with_n_batch(config.n_batch)
        .with_n_ubatch(config.n_batch)
        .with_n_threads(threads_i32)
        .with_n_threads_batch(threads_i32);
    let ctx = match model.new_context(backend, ctx_params) {
        Ok(ctx) => ctx,
        Err(e) => {
            let _ = ready.send(Err(llama_err("creating context")(&e)));
            return;
        }
    };
    let template_source = model
        .chat_template(None)
        .ok()
        .and_then(|t| t.to_string().ok());
    let fallback_template = template_source.is_none();
    let template = model
        .chat_template(None)
        .or_else(|_| LlamaChatTemplate::new("chatml"))
        .map_err(|e| llama_err("chat template")(&e));
    let template = match template {
        Ok(t) => t,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let suppress_thinking = template_source.as_deref().is_some_and(|s| s.contains("enable_thinking"));
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
        n_batch: config.n_batch as usize,
        sampling: config.sampling,
        cached: Vec::new(),
    };
    if ready.send(Ok(info)).is_err() {
        return;
    }
    while let Ok(job) = jobs.recv() {
        let result = engine.complete(&job.messages, job.max_tokens);
        let _ = job.reply.send(result);
    }
}

struct Engine<'m> {
    model: &'m LlamaModel,
    ctx: LlamaContext<'m>,
    template: LlamaChatTemplate,
    suppress_thinking: bool,
    n_batch: usize,
    sampling: Sampling,
    /// Tokens whose keys and values are in the KV cache, in order.
    cached: Vec<LlamaToken>,
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
        let result = self.generate(&tokens, max_tokens);
        if result.is_err() {
            // The cache may hold a half-decoded batch; start clean next time.
            self.ctx.clear_kv_cache();
            self.cached.clear();
        }
        result
    }

    fn render(&self, messages: &[ChatMessage]) -> Result<String, CleanupError> {
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

    /// Reuses the KV cache for the prompt prefix shared with the previous
    /// request (the system prompt, usually), decodes the rest, then samples
    /// until end of turn or `max_tokens`.
    fn generate(&mut self, tokens: &[LlamaToken], max_tokens: usize) -> Result<Completion, CleanupError> {
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
            let from = u32::try_from(common).unwrap_or(u32::MAX);
            if !matches!(self.ctx.clear_kv_cache_seq(Some(0), Some(from), None), Ok(true)) {
                self.ctx.clear_kv_cache();
                common = 0;
            }
            self.cached.truncate(common);
        }
        let mut batch = LlamaBatch::new(self.n_batch.max(1), 1);
        let mut last_index = 0;
        for (chunk_no, chunk) in tokens[common..].chunks(self.n_batch.max(1)).enumerate() {
            batch.clear();
            let base = common + chunk_no * self.n_batch.max(1);
            for (k, &token) in chunk.iter().enumerate() {
                let pos = base + k;
                let is_last = pos == tokens.len() - 1;
                batch
                    .add(token, pos_i32(pos), &[0], is_last)
                    .map_err(|e| llama_err("building batch")(&e))?;
            }
            self.ctx.decode(&mut batch).map_err(|e| llama_err("decoding prompt")(&e))?;
            last_index = batch.n_tokens() - 1;
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

fn pos_i32(pos: usize) -> i32 {
    i32::try_from(pos).unwrap_or(i32::MAX)
}
